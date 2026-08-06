# 23 · Queues

Background work with no broker to run. `publish` writes a row and fires a
Postgres `NOTIFY`; a subscriber wakes on the notification, claims the row and
runs a function — a few milliseconds after the request that published it has
already returned.

```
23-queues/
├── main.toml               # [queues.subscribe]: topic → function
├── models/
│   └── order.toml          # [publish]: announce a delete, with no function at all
└── functions/
    ├── orders.ts           # one publisher, three subscribers
    └── orders.js           # ← written by `apiplant build`
```

| | What it shows |
|---|---|
| `POST /api/functions/checkout` | publishing from a function, and returning without waiting |
| `fulfilOrder` | a subscriber, `delivery()`, and an idempotent update |
| `notifyOps` | a second subscriber on the same topic, with its own retries |
| `releaseStock` | handling a topic a *model* announced |
| `POST /api/queues/{topic}` | publishing from outside the app |

## Running it

```bash
createdb -h 127.0.0.1 -p 5432 -U postgres apiplant_queues

cargo run -p apiplant -- build examples/23-queues   # orders.ts → orders.js
cargo run -p apiplant -- run examples/23-queues
```

The boot log tells you what is wired to what:

```
topic order.cancelled -> releaseStock
topic order.paid -> fulfilOrder, notifyOps
queue subscriber started worker=... channel="apiplant_queue" topics=["order.cancelled", "order.paid"]
```

## The round trip

```bash
BASE=http://127.0.0.1:8105/api
TOKEN=$(curl -s -X POST $BASE/auth/register -H 'content-type: application/json' \
  -d '{"email":"ann@example.com","password":"pw"}' | jq -r .token)

curl -s -X POST $BASE/order -H "authorization: Bearer $TOKEN" \
  -H 'content-type: application/json' \
  -d '{"reference":"ORD-1","total_cents":4200}'

# Returns immediately. Nothing slow happened inside this request.
curl -s -X POST $BASE/functions/checkout -H "authorization: Bearer $TOKEN" \
  -H 'content-type: application/json' -d '{"reference":"ORD-1"}'
# {"reference":"ORD-1","queued":2}
```

A moment later, without anybody asking:

```bash
curl -s "$BASE/order?reference=ORD-1" -H "authorization: Bearer $TOKEN"
# ... "status": "fulfilled", "fulfilled_at": "2026-..."
```

`fulfilOrder` did that from the queue. The log shows both subscribers running:

```
INFO apiplant::function: fulfilled ORD-1 (1 row(s) changed)
INFO apiplant::function: ops: order ORD-1 is paid
```

## A model that announces its own writes

`models/order.toml` has three lines and no function:

```toml
[publish]
after_delete = "order.cancelled"
```

```bash
curl -s -X DELETE "$BASE/order/$ID" -H "authorization: Bearer $TOKEN" -o /dev/null -w '%{http_code}\n'
# 204
```

The `204` did not wait for `releaseStock`, but it ran anyway — with the deleted
row as its message, which is the last chance anything has to see it.

## Publishing from outside

`publish = "authenticated"` in `main.toml` mounts one endpoint. Most apps should
leave this alone and publish from a function; it is here so the example can be
driven with `curl`.

```bash
curl -s -X POST "$BASE/queues/order.cancelled" -H "authorization: Bearer $TOKEN" \
  -H 'content-type: application/json' -d '{"reference":"ORD-99"}'
# 202 Accepted
# {"id":"...","topic":"order.cancelled","delivered":1}
```

Without a token it is a `401`; with `publish` left at its default of `private`
the endpoint answers `404`, because a topic is an internal name wired to real
work.

## Watching the ledger

The queue is a table, so the state of every message is a query away — and in the
dashboard at <http://127.0.0.1:8105/admin/> under **Operations → Queue**.

```bash
psql -h 127.0.0.1 -U postgres -d apiplant_queues \
  -c "SELECT topic, subscriber, status, attempts, error FROM apiplant_queue_message ORDER BY created_at"
```

```
      topic      |  subscriber  | status | attempts | error
-----------------+--------------+--------+----------+-------
 order.paid      | fulfilOrder  | done   |        1 |
 order.paid      | notifyOps    | done   |        1 |
 order.cancelled | releaseStock | done   |        1 |
```

## Seeing it survive a restart, and seeing it fail

Stop the server, queue something by hand, and start it again — the message is a
committed row, so it is handled on boot:

```bash
psql -h 127.0.0.1 -U postgres -d apiplant_queues -c \
"INSERT INTO apiplant_queue_message
   (id, topic, subscriber, status, payload, attempts, available_at, published_by, created_at, updated_at)
 VALUES (gen_random_uuid(), 'order.cancelled', 'releaseStock', 'pending',
         '{\"reference\":\"WHILE-DOWN\"}', 0, now(), '', now(), now())"
```

And the failure path — a subscriber that does not exist. With this example's
`max_attempts = 3` and `retry_backoff_secs = 2` it retries at 2s and 4s, then
dead-letters:

```bash
psql -h 127.0.0.1 -U postgres -d apiplant_queues -c \
"INSERT INTO apiplant_queue_message
   (id, topic, subscriber, status, payload, attempts, available_at, published_by, created_at, updated_at)
 VALUES (gen_random_uuid(), 'order.paid', 'noSuchHandler', 'pending',
         '{\"reference\":\"BROKEN\"}', 0, now(), '', now(), now());
 SELECT pg_notify('apiplant_queue', 'order.paid')"
```

```
 status | attempts |                          error
--------+----------+---------------------------------------------------------
 failed |        3 | `noSuchHandler` is subscribed to `order.paid` but no ...
```

The row is kept, with the reason. Nothing deletes a dead letter.

## Next

[`docs/queues.md`](../../docs/queues.md) — the delivery guarantee and what it
means for your handlers, the full `[queues]` reference, and what this is
deliberately not.

**Next:** [24 · Nested resources](../24-nested-resources) takes the nested
collections of example 03 across the scope boundary of example 04.
