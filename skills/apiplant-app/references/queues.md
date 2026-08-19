# Queues

Work that happens *after* the response, with Postgres as the whole of the
broker. There is nothing new to run: no Redis, no RabbitMQ, no Kafka, no
sidecar. If your app has a database, it has a queue.

```toml
# main.toml
[queues.subscribe]
"order.paid" = "fulfilOrder"
```

```ts
queue.publish("order.paid", { orderId: order.id });
```

The request returns. `fulfilOrder` runs a few milliseconds later, on its own,
with retries.

## Why you want this

The usual shape of a slow endpoint is a handler doing four things, three of
which the caller does not care about:

```ts
// The version without a queue.
db.execute(sql`UPDATE ... SET status = 'paid' ...`);
sendReceiptEmail(order);        // 400ms, and the provider is sometimes down
tellTheWarehouse(order);        // 900ms, and their API 502s on Mondays
syncToAnalytics(order);         // 200ms, and nobody notices when it breaks
```

That endpoint takes 1.5 seconds instead of 20 milliseconds, and — worse — the
sale fails when the *analytics* vendor has an outage. The four lines have been
welded together for no reason other than that they were written next to each
other.

```ts
// The version with one.
db.execute(sql`UPDATE ... SET status = 'paid' ...`);
queue.publish("order.paid", { orderId: order.id });
```

Now the sale succeeds or fails on its own, and the other three are somebody
else's problem — retried on a backoff, and visible in a table when they run out
of retries.

## How it works

Two things happen when you publish, and it is worth knowing which does what,
because most home-made queues build only one of them:

| | What it is | What it buys |
|---|---|---|
| A row in `queue_message` | the message | survives a restart, records failures, can be retried, can be looked at |
| A Postgres `NOTIFY` | a tap on the shoulder | delivery in milliseconds instead of on the next poll |

A subscriber `LISTEN`s on one channel. When a notification arrives it claims
messages with `FOR UPDATE SKIP LOCKED` — so several replicas *share* the work
rather than each doing all of it — runs the handler, and marks the row.

Losing the notification costs latency and nothing else: every subscriber also
sweeps on `poll_secs`, so an unclaimed row is always found. That is the reason
the row exists as well as the notification. A design with only the `NOTIFY`
drops every message published while nothing happened to be listening.

## Publishing

### From a function

The normal way. No endpoint, no credential, no round trip.

```ts
import { queue } from "apiplant";

const published = queue.publish("order.paid", { orderId: order.id });
if (published.delivered === 0) {
  log.warn("nothing subscribes to order.paid");
}
```

```rust
// Rust
ctx.publish("order.paid", &order)?;
```

```c
/* C, Zig, Go — the same call over the C ABI */
char *reply = host->publish(host->ctx,
    "{\"op\":\"publish\",\"topic\":\"order.paid\",\"message\":{}}");
```

`publish` returns once the message is **committed**, not once it has been
handled. That is the entire point, and the difference from calling the other
function directly.

### From a resource, with no function at all

The shortest path from "a row changed" to "something happens about it". The row
*is* the message.

```toml
# resources/order.toml
[publish]
after_create = "order.placed"
after_update = "order.changed"
after_delete = "order.cancelled"
```

A subscriber gets the order exactly as the API would have returned it. Only the
three `after_*` writes can be announced — a `before_*` topic would announce
something that has not happened and may still be rejected.

Publishing here can never fail the write. By the time it runs the row is
committed and the response is decided; a queue failure is logged, not turned
into a 500 that tells the caller their write did not happen when it did.

### Over HTTP

Off by default. Switch it on when the publisher is something that cannot call a
function — a webhook from a service with no SDK, a script, another system.

```toml
[queues]
publish = "role:admin"     # default: "private", meaning the endpoint 404s
```

```bash
curl -X POST http://localhost:8080/api/queues/order.paid \
  -H "authorization: Bearer $TOKEN" \
  -d '{"order_id": "..."}'
# 202 Accepted
# {"id":"...","topic":"order.paid","delivered":2}
```

`202`, not `200`: the message is written down, and that is the whole promise.

## Subscribing

A subscriber is an ordinary function. Nothing marks it as one — the
subscription lives in `main.toml`, not in the code:

```toml
[queues.subscribe]
"order.paid"      = ["fulfilOrder", "notifyOps"]
"order.cancelled" = "releaseStock"
```

```ts
fulfilOrder: {
  handler(input: { orderId: string }) {
    // The message body is the ordinary input. This function can also be
    // POSTed to by hand, which is how you test it.
    const message = delivery();   // { topic, messageId, attempts, ... }
    ...
  },
},
```

Because the wiring is config, adding a second handler to a topic touches
neither the publisher nor the first handler. That indirection is what a topic
is *for*.

**Each subscriber gets its own row and its own retries.** A failing `notifyOps`
never re-runs `fulfilOrder`.

## The one rule: handlers must be safe to run twice

Delivery is **at-least-once**. A handler that succeeds and then dies before its
row can be marked runs again when the lease expires.

This is not a rough edge to be fixed in a later version. It is the only honest
guarantee a queue can give without the handler taking part, because "did my
side effect actually happen?" is a question only the handler can answer.
Anything claiming exactly-once either has a distributed transaction with your
side effects, or is lying.

The fix is not to detect the retry. It is to write the work so that doing it
twice is doing it once:

```ts
// Not this:
db.execute(sql`UPDATE apiplant_order SET fulfilled_at = now() WHERE id = ${id}::uuid`);
sendEmail(...);   // sends twice on a retry

// This:
const claimed = db.execute(
  sql`UPDATE apiplant_order SET fulfilled_at = now()
       WHERE id = ${id}::uuid AND fulfilled_at IS NULL`,
);
if (claimed > 0) sendEmail(...);   // only the attempt that won sends
```

`delivery().messageId` is stable across retries, which makes it a natural
idempotency key when there is no column to guard on.

## When something fails

A handler that returns an error, throws, panics, or is not loaded at all leaves
the row `pending` with the reason on it, scheduled for
`retry_backoff_secs * 2^(n-1)` seconds' time — 10s, 20s, 40s, 80s by default.

After `max_attempts` the row is left `failed`. It is **not** deleted, and the
retention sweep never touches it: a dead-letter that empties itself overnight
takes the evidence with it.

None of this reaches anybody's request. There is no request — it ended before
the handler ever ran.

```sql
-- What is stuck, and why.
SELECT topic, subscriber, attempts, error, updated_at
  FROM apiplant_queue_message
 WHERE status = 'failed'
 ORDER BY updated_at DESC;

-- Try one again after fixing the cause.
UPDATE apiplant_queue_message
   SET status = 'pending', attempts = 0, available_at = now()
 WHERE id = '...';
```

### In the dashboard

`queue_message` is in the admin dashboard under **Settings → Background tasks**
— down with the other screens every app has, not among the app's own resources
— listing
topic, subscriber, status, attempts and timings, with `?search=` covering the
topic, the subscriber and the error text — because the way somebody arrives here
is usually with half a message from a log line.

It is `role:admin` to read and `private` to write: the columns are a state
machine the subscriber owns, and a message payload is arbitrary app data that
routinely carries personal information, so the rest of the organisation does not
get to read the ledger. Retrying by hand is the `UPDATE` above, run deliberately.

### A subscriber that dies mid-handler

Its messages sit in `running` until `lease_secs` passes, then go back to
`pending` for another replica. Set the lease comfortably above your slowest
handler: expiring it early is what turns at-least-once into "twice,
concurrently".

The attempt is *not* refunded. A handler that reliably kills its process runs
out of attempts and lands in the dead-letter, instead of retrying forever while
somebody watches the restart count climb.

## Configuration

```toml
[queues]
enabled            = true      # pause handling; publishing still records rows
prefix             = "apiplant"  # NOTIFY channel prefix, for two apps in one database
poll_secs          = 30        # sweep interval; the safety net under the NOTIFY
batch              = 10        # messages claimed at once
max_attempts       = 5         # then the dead-letter
retry_backoff_secs = 10        # doubling: 10s, 20s, 40s, 80s
lease_secs         = 300       # before a claimed message is offered to somebody else
retain_hours       = 24        # delete handled messages after this; 0 keeps them
publish            = "private" # who may POST <base>/queues/{topic}

[queues.subscribe]
"order.paid" = ["fulfilOrder", "notifyOps"]
```

A retry due before the next sweep pulls the sweep forward, so the backoff you
configure is the backoff you get rather than being rounded up to `poll_secs`.

## Limits

* **Ordering.** Messages are claimed oldest-first, but completion order across
  replicas is not guaranteed, and a retry moves a message behind its
  successors. A topic that needs strict ordering wants one subscriber and
  `batch = 1`.
* **Throughput.** Every message is a row and two `UPDATE`s: thousands per
  second, not millions. At millions you want a log.
* **Scheduling.** There is no scheduler; a cron job calling
  `apiplant call <function>` covers fixed times.
* **Request/response.** Publishing confirms the message was recorded, not the
  handler's decision. For a return value, call a function.

## A topic nobody subscribes to

Not an error. The message is still recorded — with an empty `subscriber` and
status `done` — and a warning is logged.

That is deliberate. The alternative is publishing into silence, where the only
evidence a message was ever sent is the absence of its effect, which is the
hardest kind of bug to be handed. `delivered: 0` in the reply is the same fact,
available to the publisher.

## See also

* [`examples/23-queues`](../examples/23-queues) — a runnable app with a
  publisher, three subscribers and a resource that announces its own deletes.
* [Lifecycle hooks](hooks.md) — for work that must happen *before* the response,
  and is allowed to reject it.
* [Functions](functions.md) — writing the handlers.
