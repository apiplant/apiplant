# 16 · Caching (optional Redis, for functions)

An app points at a Redis server; functions get somewhere to put data they know
how to invalidate. Nothing else changes — the CRUD endpoints on `reading` go to
Postgres on every single request, exactly as they would with no cache at all.

```
16-caching/
├── main.toml                # [cache] url, prefix, default TTL
├── models/
│   └── reading.toml         # ordinary, deliberately uncached data
└── functions/
    ├── stats.rs             # report (memoised aggregate) + quota (rate limit)
    ├── report.toml          # …its TTL
    └── quota.toml           # …the limit and the window
```

## Run it

```bash
redis-server --port 6379        # or: docker run -p 6379:6379 redis
createdb -h 127.0.0.1 -p 55432 -U postgres apiplant_caching
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
a counter — has an invalidation rule, and only the function author knows it.

So the cache is reachable from exactly one place: a function's `Context`.

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

Add a fourth reading and the answer stays stale until the TTL runs out — which
is the honest trade a cache makes, stated in `functions/report.toml` as
`ttl_secs = 60`. Shorten it, restart, and the window shrinks; no rebuild.

Two things in `stats.rs` are worth copying:

* **The key is versioned** — `report:v1:{sensor}`. When the shape of the
  response changes, the new build reads new keys and the old entries expire on
  their own. (`cache_get_as` also treats a value it can't deserialize as a miss,
  which covers the version you forget to bump.)
* **A cache failure is a miss.** The lookup uses `.ok().flatten()`, so an
  unreachable Redis costs a recomputation rather than an error. The data is
  reconstructible — that's what made it cacheable.

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
