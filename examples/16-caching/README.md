# 16 · Caching (optional Redis, for functions)

An app points at a Redis server; functions get somewhere to put data they know
how to invalidate. Nothing is cached on your behalf — but a hook is a function,
so even a plain `GET /reading/{id}` can be served from Redis once you've said
what makes it stale.

```
16-caching/
├── main.toml                # [cache] url, prefix, default TTL
├── resources/
│   └── reading.toml         # read-through cached rows + invalidation hooks
└── functions/
    ├── stats.rs             # report + quota, the cached read, and the hooks
    ├── report.toml          # …its TTL
    └── quota.toml           # …the limit and the window
```

## Run it

```bash
redis-server --port 6379        # or: docker run -p 6379:6379 redis
createdb -h 127.0.0.1 -p 5432 -U postgres apiplant_caching
cargo run -p apiplant -- build examples/16-caching   # needs cargo on PATH
cargo run -p apiplant -- run examples/16-caching
```

```
INFO apiplant_server:   cache -> redis (prefix "example-16:")
INFO apiplant_server:   fn report -> /api/functions/report
INFO apiplant_server:   fn quota -> /api/functions/quota
```

The connection is made at **boot**. A `url` the server can't reach stops it
there — an app whose functions were written against a cache and silently given
none is a bug that only shows up later, as a load spike.

## Nothing is cached for you

Worth stating plainly, because it's the design decision this example exists to
show: apiplant caches nothing on your behalf. Resources, permissions, hooks,
the OpenAPI document and the admin manifest behave identically whether `[cache]`
is configured or not.

A cache in front of generic CRUD would have to guess when a row goes stale, and
every wrong guess serves data through an API that never said it might be old.
The work that genuinely benefits — a slow aggregate, a metered third-party call,
a counter, a hot row — has an invalidation rule, and only the author knows it.

So the cache is reachable from exactly one place: a function's `Context`. That
includes a function bound to a [hook](../../docs/hooks.md), which is how the
last section here caches CRUD reads *with* an invalidation rule rather than a
guess.

## A memoised aggregate

```bash
for v in 21.5 22.1 20.9; do
  curl -s -X POST http://127.0.0.1:8099/api/reading \
    -H 'content-type: application/json' \
    -d "{\"sensor\":\"roof\",\"value\":$v}" > /dev/null
done

curl -s -X POST http://127.0.0.1:8099/api/functions/report \
  -H 'content-type: application/json' -d '{"sensor":"roof"}'
```

```json
{ "sensor": "roof", "readings": 3, "average": 21.5, "cached": false }
```

Run it again within the minute and the aggregate doesn't happen:

```json
{ "sensor": "roof", "readings": 3, "average": 21.5, "cached": true }
```

The TTL — `ttl_secs = 60` in `functions/report.toml` — is the ceiling on how
stale an answer can get. Shorten it, restart, and the window shrinks; no
rebuild.

## Invalidating it on write

A TTL alone means a fourth reading doesn't show up for up to a minute. The
framework won't guess when that happened, but it will *tell* you: `reading` puts
[lifecycle hooks](../../docs/hooks.md) on every write, and they delete the key
`report` wrote.

```toml
# resources/reading.toml
[hooks]
after_create  = "reading_changed"
before_update = "reading_before_update"
after_update  = "reading_changed"
after_delete  = "reading_changed"
```

`reading_changed` reads `hook.row()["sensor"]` — the row that was written or
removed — and calls `ctx.cache_delete("report:v1:{sensor}")`. So:

```bash
curl -s -X POST http://127.0.0.1:8099/api/reading \
  -H 'content-type: application/json' -d '{"sensor":"roof","value":30.0}'

curl -s -X POST http://127.0.0.1:8099/api/functions/report \
  -H 'content-type: application/json' -d '{"sensor":"roof"}'
```

```json
{ "sensor": "roof", "readings": 4, "average": 23.625, "cached": false }
```

`cached` is `false` again straight after the write, without waiting out the TTL.

Two details:

* **`before_update` exists for the rename.** Moving a reading from `roof` to
  `shed` makes *two* reports stale, and by the time `after_update` runs the old
  sensor name is gone. The `before` hook still has `record_id`, so it reads the
  current sensor and drops that key; the `after` hook drops the new one.
* **Invalidation is best-effort**, like the write. A Redis that is down holds
  nothing stale to invalidate, so the hook logs a warning and returns
  `reply::proceed()` rather than failing the write. Note also that hooks run
  *outside* the operation's transaction — the TTL stays the backstop for a
  delete that races a concurrent recompute.

Two more things in `stats.rs` are worth copying:

* **The key is versioned** — `report:v1:{sensor}`. When the shape of the
  response changes, the new build reads new keys and the old entries expire on
  their own. (`cache_get_as` also treats a value it can't deserialize as a miss,
  which covers the version you forget to bump.)
* **A cache failure is a miss.** The lookup uses `.ok().flatten()`, so an
  unreachable Redis costs a recomputation rather than an error. The data is
  reconstructible — that's what made it cacheable.

## Reading a row without touching Postgres

The two functions above cache what only they could have cached. A resource's own
rows are the other half: `reading` serves `GET /api/reading/{id}` out of Redis,
and the framework still isn't the one deciding when that goes stale — three
hooks are.

```toml
# resources/reading.toml
[hooks]
before_read  = "reading_before_read"   # a hit *is* the response
after_read   = "reading_after_read"    # only runs on a miss
after_update = "reading_changed"       # …which also evicts the row
after_delete = "reading_changed"
```

A `before_read` that returns `reply::replace(row)` answers the request: the
`SELECT` never runs, `?expand=` is not applied, and `after_read` doesn't fire.
On a miss it returns `proceed()` and the request behaves exactly as it would
with no cache at all — then `after_read` stores what the query returned.

```bash
ID=$(curl -s -X POST http://127.0.0.1:8099/api/reading \
  -H 'content-type: application/json' -d '{"sensor":"shed","value":10.0}' \
  | python3 -c 'import sys,json; print(json.load(sys.stdin)["id"])')

curl -s http://127.0.0.1:8099/api/reading/$ID          # miss: queries, then caches
redis-cli get "example-16:reading:v1:$ID"              # the row is in Redis
```

To *prove* the second read skips the database, doctor the cached copy into
something Postgres does not contain — only a short-circuited read can return it:

```bash
redis-cli set "example-16:reading:v1:$ID" \
  "{\"id\":\"$ID\",\"sensor\":\"shed\",\"value\":999.0}" EX 60

curl -s http://127.0.0.1:8099/api/reading/$ID
```

```json
{ "id": "…", "sensor": "shed", "value": 999.0 }
```

The server logs `reading … served from cache`. A write puts it back in touch
with reality:

```bash
curl -s -X PATCH http://127.0.0.1:8099/api/reading/$ID \
  -H 'content-type: application/json' -d '{"value":30.0}'
redis-cli exists "example-16:reading:v1:$ID"   # 0 — the after hook dropped it
curl -s http://127.0.0.1:8099/api/reading/$ID  # 30.0, straight from Postgres
```

Three things this pattern depends on, and one it doesn't:

* **The permission check has already run** when `before_read` fires, so a cached
  answer can't widen access. What it *does* skip is the row-level scoping the
  query would have applied — `reading` is global and publicly readable, so the
  id is the whole key. On a tenanted or owner-scoped resource the key must carry
  whatever those filters would have checked, or one tenant's read serves
  another's row.
* **The TTL is still the backstop.** Hooks run outside the operation's
  transaction, so a read that races a write can re-populate the key with the old
  row. Sixty seconds is the ceiling on how long that survives.
* **A miss is the only path when Redis is gone**, which is why it stays the
  plain one: `.ok().flatten()` on the lookup, a warning on a failed store.
* It does *not* depend on the framework knowing anything about `reading`. Delete
  the three hook lines and the resource is ordinary CRUD again.

## A rate limit

`quota` counts per caller, in a rolling window:

```bash
for i in $(seq 1 6); do
  curl -s -X POST http://127.0.0.1:8099/api/functions/quota \
    -H "authorization: Bearer $TOKEN" -H 'content-type: application/json' -d '{}'
  echo
done
```

```json
{"used":1,"limit":5,"resets_in":60}
…
{"used":5,"limit":5,"resets_in":56}
{"error":"rate limit reached; try again in 56s"}
```

`ctx.cache_incr` increments **on the Redis server**, which is what makes this
correct when several workers — or several hosts — answer at once. A `cache_get`
followed by a `cache_set` would lose increments under concurrency and quietly
let everyone through.

The TTL is applied only when the counter is created, so `resets_in` counts down
rather than restarting on every request.

Note the opposite error handling from `report`: `quota` uses `?`. With no
counter there is no limit, and an endpoint that silently stops limiting when
Redis blinks is worse than one that fails.

## Turning it off

Delete `url` from `[cache]` and restart. `report` still works — the miss path is
the whole function — and logs a warning that it couldn't store its answer.
`quota` returns an error, because a rate limit with no counter isn't one.

That difference is the point: *you* choose which of your endpoints can live
without the cache.

## What to read next

* [Caching](../../docs/caching.md) — every operation, key and TTL advice, and
  the C-ABI form.
* [Example 15 · email](../15-email) — the other optional service a function can
  reach.
