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
let rates = fetch_rates()?;               // slow and metered
ctx.cache_set("rates:eur", &rates, Some(900))?;
```

The cache is **off by default**: with no `url` there is no cache and no
connection.

## Nothing is cached automatically

Resources, permissions, hooks, the OpenAPI document and the admin manifest
behave identically whether or not a cache is configured. CRUD reads always go to
Postgres, unless one of your own hooks intervenes (see below).

This is intentional. A cache in front of generic CRUD would have to infer when a
row becomes stale, and any incorrect inference serves data the API never
indicated might be out of date. The work that benefits from caching is the work
a *function* performs: a third-party response worth memoising, a rate-limit
counter, or a one-time token with a natural expiry. Each of these has an
invalidation rule known only to the function's author.

The cache is therefore reachable from one place: a function's `Context`. This
includes a function bound to a hook, which is the supported way to cache CRUD
reads.

## Caching a CRUD read

A [`before_read` or `before_list` hook](hooks.md#answering-a-read-without-a-query)
that returns `reply::replace(value)` *becomes* the response, and the query never
runs. Pair it with an `after_read` that stores what the query returned, and
`after_update` and `after_delete` hooks that delete the key.

```rust
fn cached_read(ctx: &Context<()>, _in: Value) -> Result<Value, String> {
    let Some(id) = ctx.hook().and_then(|h| h.record_id.clone()) else {
        return Ok(reply::proceed());
    };
    match ctx.cache_get_as::<Value>(&format!("reading:v1:{id}")) {
        Ok(Some(row)) => Ok(reply::replace(row)),  // Postgres is not queried
        _ => Ok(reply::proceed()),                 // a miss, or no cache configured
    }
}
```

This differs from a framework-level cache in that the invalidation rule is
yours, written alongside the code it invalidates. Three constraints apply:

* The permission check has already run, so a cached answer cannot widen access.
  However, the short-circuit skips the **row-level scoping** the query would
  have applied. Key the entry by whatever those filters would have checked (the
  organisation, the owner), or do not cache reads on a scoped resource.
* Hooks run **outside** the operation's transaction, so a read can race a write
  and repopulate a key that was just deleted. Always set a TTL; it bounds how
  long that state can persist.
* The short-circuit skips `?expand=` and the corresponding
  `after_read`/`after_list` hook, so the hook's return value is used as the body
  exactly as given.

[`examples/16-caching`](../examples/16-caching) contains a working version.

## Configuration

```toml
[cache]
enabled          = true                      # disable without removing settings
url              = "redis://127.0.0.1:6379"  # empty (the default) means no cache
prefix           = "my-app:"                 # prepended to every key
default_ttl_secs = 0                         # 0 means keys persist until deleted
timeout_secs     = 5                         # per operation
```

`url` accepts everything the Redis URL scheme does, including `rediss://` for
TLS, a password (`redis://:pass@host:6379`) and a database index
(`redis://host:6379/2`). It holds a credential, so reference an environment
variable rather than the value itself, for example `url = "$REDIS_URL"` or
`url = "redis://:$REDIS_PASSWORD@$REDIS_HOST:6379"`. See
[Configuration → Environment variables](configuration.md#environment-variables).

`prefix` allows several apps to share one Redis without key collisions. Keys are
stored as `<prefix><key>`, and functions never see the prefix.

A `url` the server cannot reach stops the boot. Silently running without a cache
that the app's functions were written against is a fault that typically only
surfaces as a load spike.

## Using it from a function

```rust
// Read. `None` is a miss.
let hit: Option<serde_json::Value> = ctx.cache_get("rates:eur")?;

// Read into your own type. A value of the wrong shape counts as a miss, so a
// deployment that changes the type is not broken by its own older entries.
let hit: Option<Rates> = ctx.cache_get_as::<Rates>("rates:eur")?;

// Write. `Some(secs)` sets a TTL, `None` uses default_ttl_secs, `Some(0)` persists.
ctx.cache_set("rates:eur", &rates, Some(900))?;

// Remove. Returns `true` when the key existed.
ctx.cache_delete("rates:eur")?;

// Count, atomically on the server, creating the key at zero first.
let hits = ctx.cache_incr("rate:{user}", 1, Some(60))?;

// Seconds left, or `None` when absent or set to persist.
let ttl = ctx.cache_ttl("rates:eur")?;
```

Values are stored as JSON, so any type `serde` can serialize may be cached.

### Rate limiting

`cache_incr` is atomic on the Redis server, which makes it correct across every
worker and every host; a `get` followed by a `set` is not. The TTL is applied
only when the counter is created, so the window does not extend itself on every
request:

```rust
let key = format!("send:{}", ctx.principal_id());
if ctx.cache_incr(&key, 1, Some(3600))? > 100 {
    return Err("hourly limit reached".to_string());
}
```

### Failing soft

Every cache call returns a `Result`, and an unreachable Redis produces an `Err`
rather than a panic or an automatic 500. Since a cache holds only recomputable
data, treating an error as a miss is usually correct and keeps the endpoint
working while Redis restarts:

```rust
if let Some(hit) = ctx.cache_get("rates:eur").ok().flatten() {
    return Ok(hit);
}
```

Use `?` where a miss cannot be recovered from, such as a one-time token that
exists only in the cache, accepting that the endpoint then fails when Redis
does.

## Choosing keys and TTLs

* **Namespace by hand.** `prefix` separates apps; `user:{id}:quota` style keys
  separate concerns within one. Nothing else namespaces for you.
* **Every key should expire.** Set a TTL per write, or a `default_ttl_secs` for
  the whole app. A cache whose entries never expire becomes a database with no
  schema and no backups.
* **Version keys that outlive a deploy.** `rates:v2:eur` invalidates by
  construction when the shape changes, which is simpler than reasoning about
  what the previous release wrote. `cache_get_as` also treats an unreadable
  value as a miss, covering the case where a version is not bumped.
* **Do not cache authorisation.** Memberships and roles are read fresh on every
  request specifically so a revoked role takes effect immediately; caching them
  in a function defeats that.

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

Omitting `ttl` uses `default_ttl_secs`; `"ttl": 0` means the key never
expires.

## See also

* [Example 16 · caching](../examples/16-caching): a working app that memoises a
  slow computation and rate-limits an endpoint.
* [Configuration](configuration.md): the full `main.toml` reference.
* [Functions](functions.md): writing the code that caches.
* [Sending email](email.md): the other optional service a function can reach.
