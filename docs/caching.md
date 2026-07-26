# Caching with Redis

An app can point at a Redis server, and functions get somewhere to put
short-lived data:

```toml
[cache]
url    = "redis://127.0.0.1:6379"
prefix = "my-app:"
```

```rust
if let Some(rates) = ctx.cache_get("rates:eur")? {
    return Ok(rates);
}
let rates = fetch_rates()?;               // slow, or metered, or both
ctx.cache_set("rates:eur", &rates, Some(900))?;
```

The cache is **off by default**: no `url`, no cache, no connection.

## Nothing is cached for you

Resources, permissions, hooks, the OpenAPI document and the admin manifest all
behave exactly the same whether a cache is configured or not. CRUD reads go to
Postgres every time.

That is deliberate. A cache in front of generic CRUD would have to guess when a
row becomes stale — and every wrong guess serves data an API never warned you
might be old. The work that genuinely benefits from caching is the work a
*function* does: a third-party response worth memoising, a rate-limit counter, a
one-time token with a natural expiry. Those have an invalidation rule, and only
the function author knows it.

So the cache is reachable from exactly one place: a function's `Context`.

## Configuration

```toml
[cache]
enabled          = true                      # off switch that keeps the settings
url              = "redis://127.0.0.1:6379"  # empty (default) = no cache
prefix           = "my-app:"                 # prepended to every key
default_ttl_secs = 0                         # 0 = keys persist until deleted
timeout_secs     = 5                         # per operation
```

`url` accepts everything the Redis URL scheme does, including `rediss://` for
TLS, a password (`redis://:pass@host:6379`) and a database index
(`redis://host:6379/2`). It holds a credential, so name the variable rather than
the value — `url = "$REDIS_URL"`, or
`url = "redis://:$REDIS_PASSWORD@$REDIS_HOST:6379"` — see
[Configuration → Environment variables](configuration.md#environment-variables).

`prefix` is what lets several apps share one Redis without colliding. Keys are
stored as `<prefix><key>`; functions never see the prefix.

A `url` the server can't reach stops the boot. An app whose functions were
written against a cache and silently given none is a bug that only shows up as
a load spike.

## Using it from a function

```rust
// Read. `None` is a miss.
let hit: Option<serde_json::Value> = ctx.cache_get("rates:eur")?;

// Read into your own type. A value of the wrong shape counts as a miss, so a
// deployment that changes the type doesn't break on its own old entries.
let hit: Option<Rates> = ctx.cache_get_as::<Rates>("rates:eur")?;

// Write. `Some(secs)` expires; `None` uses default_ttl_secs; `Some(0)` persists.
ctx.cache_set("rates:eur", &rates, Some(900))?;

// Remove. `true` when the key was there.
ctx.cache_delete("rates:eur")?;

// Count, atomically on the server, creating the key at zero first.
let hits = ctx.cache_incr("rate:{user}", 1, Some(60))?;

// Seconds left, or `None` when absent or set to persist.
let ttl = ctx.cache_ttl("rates:eur")?;
```

Values are stored as JSON, so anything `serde` can serialize can go in.

### Rate limiting

`cache_incr` is atomic on the Redis server, which is what makes it correct
across every worker and every host — a `get` then `set` pair is not. The TTL is
applied only when the counter is created, so a window doesn't extend itself on
every request:

```rust
let key = format!("send:{}", ctx.principal_id());
if ctx.cache_incr(&key, 1, Some(3600))? > 100 {
    return Err("hourly limit reached".to_string());
}
```

### Failing soft

Every cache call returns a `Result`, and an unreachable Redis is an `Err` — not
a panic and not a 500 unless you make it one. Since a cache holds only what can
be recomputed, treating an error like a miss is usually right, and keeps the
endpoint working while Redis restarts:

```rust
if let Some(hit) = ctx.cache_get("rates:eur").ok().flatten() {
    return Ok(hit);
}
```

Use `?` where a miss genuinely can't be recovered from — a one-time token that
only exists in the cache, say — and accept that the endpoint fails when Redis
does.

## Choosing keys and TTLs

* **Namespace by hand.** `prefix` separates apps; `user:{id}:quota` style keys
  separate concerns within one. Nothing else namespaces for you.
* **Every key should expire.** Set a TTL per write, or a `default_ttl_secs` for
  the whole app. A cache whose entries never expire is a database with no
  schema and no backups.
* **Version keys that outlive a deploy.** `rates:v2:eur` invalidates by
  construction when the shape changes — cleaner than reasoning about what the
  previous release wrote. (`cache_get_as` also treats an unreadable value as a
  miss, which covers the case you forget.)
* **Don't cache authorisation.** Memberships and roles are read fresh on every
  request precisely so a revoked role takes effect immediately; caching them in
  a function gives that back.

## From C, Zig or Go

The C ABI carries the same operations as JSON through one callback:

```c
char *reply = host->cache(host->ctx, "{\"op\":\"get\",\"key\":\"hits\"}");
/* {"hit":true,"value":42}, or {"error":"…"} */
host->free_string(host->ctx, reply);
```

| Request | Reply |
|---------|-------|
| `{"op":"get","key":"k"}` | `{"hit":true,"value":…}` |
| `{"op":"set","key":"k","value":…,"ttl":60}` | `{"ok":true}` |
| `{"op":"delete","key":"k"}` | `{"deleted":true}` |
| `{"op":"exists","key":"k"}` | `{"exists":true}` |
| `{"op":"incr","key":"k","by":1,"ttl":60}` | `{"value":3}` |
| `{"op":"ttl","key":"k"}` | `{"ttl":42}` |

Omitting `ttl` means "use `default_ttl_secs`"; `"ttl": 0` means "never expire".

## See also

* [Example 16 · caching](../examples/16-caching) — a working app that memoises
  a slow computation and rate-limits an endpoint.
* [Configuration](configuration.md) — the full `main.toml` reference.
* [Functions](functions.md) — writing the code that caches.
* [Sending email](email.md) — the other optional service a function can reach.
