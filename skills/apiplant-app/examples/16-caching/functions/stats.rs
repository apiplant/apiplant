//! Two things a cache is genuinely good for, and one thing it is not.
//!
//! * `report` memoises an expensive aggregate. The function author knows what
//!   invalidates it, which is exactly why the framework doesn't try to guess.
//! * `quota` counts requests per caller in a rolling window. The counter is
//!   incremented on the Redis server, so it stays correct across every worker.
//!
//! What neither does is cache authorisation or CRUD reads — see the README.

use apiplant_function::prelude::*;
use serde::{Deserialize, Serialize};
use serde_json::json;

/// Config from `functions/report.toml`.
#[derive(Deserialize)]
#[serde(default)]
struct ReportSettings {
    /// How long a computed report stays fresh.
    ttl_secs: u64,
}

impl Default for ReportSettings {
    fn default() -> Self {
        ReportSettings { ttl_secs: 60 }
    }
}

#[derive(Deserialize, JsonSchema)]
struct ReportInput {
    /// Which sensor to summarise. Part of the cache key, so two sensors don't
    /// share an answer.
    sensor: String,
}

#[derive(Serialize, Deserialize, JsonSchema)]
struct Report {
    sensor: String,
    readings: i64,
    average: f64,
    /// Whether this answer came from the cache. Present so the example can
    /// *show* the cache working; a real endpoint wouldn't advertise it.
    cached: bool,
}

/// `POST /api/functions/report` — an aggregate, computed at most once per TTL.
///
/// The shape is the whole pattern: look, return on a hit, compute, store.
fn report(ctx: &Context<ReportSettings>, input: ReportInput) -> Result<Report, String> {
    // Version the key (`v1`). When the shape of `Report` changes, the new build
    // reads new keys and the old entries expire on their own — which beats
    // reasoning about what the previous release wrote.
    let key = format!("report:v1:{}", input.sensor);

    // `.ok().flatten()` rather than `?`: a cache that is down should cost this
    // endpoint a recomputation, not an error. The data is reconstructible —
    // that is what makes it cacheable in the first place.
    if let Some(mut hit) = ctx.cache_get_as::<Report>(&key).ok().flatten() {
        ctx.info(&format!("report for {} served from cache", input.sensor));
        hit.cached = true;
        return Ok(hit);
    }

    let row = ctx
        .query_one(
            "SELECT count(*)::bigint AS readings, \
                    coalesce(avg(value), 0)::double precision AS average \
             FROM apiplant_reading WHERE sensor = $1",
            &[json!(input.sensor)],
        )?
        .ok_or("aggregate returned no row")?;

    let report = Report {
        sensor: input.sensor,
        readings: row["readings"].as_i64().unwrap_or(0),
        average: row["average"].as_f64().unwrap_or(0.0),
        cached: false,
    };

    // Also best-effort: failing to *store* an answer is not a reason to withhold
    // one that was just computed correctly.
    if let Err(error) = ctx.cache_set(&key, &report, Some(ctx.config().ttl_secs)) {
        ctx.warn(&format!("could not cache the report: {error}"));
    }

    Ok(report)
}

/// Config from `functions/quota.toml`.
#[derive(Deserialize)]
#[serde(default)]
struct QuotaSettings {
    /// Requests allowed per window.
    limit: i64,
    /// Length of the window, in seconds.
    window_secs: u64,
}

impl Default for QuotaSettings {
    fn default() -> Self {
        QuotaSettings {
            limit: 5,
            window_secs: 60,
        }
    }
}

#[derive(Serialize, JsonSchema)]
struct QuotaOutput {
    used: i64,
    limit: i64,
    /// Seconds until the window resets.
    resets_in: i64,
}

/// `POST /api/functions/quota` — a rolling per-caller limit.
///
/// `cache_incr` is atomic on the Redis server, which is what makes this correct
/// with several workers (or several hosts) answering at once; a `cache_get`
/// followed by a `cache_set` would lose increments under concurrency.
///
/// The TTL is applied only when the counter is created, so a window that
/// started 50 seconds ago doesn't get a fresh minute on every request.
fn quota(ctx: &Context<QuotaSettings>, _input: serde_json::Value) -> Result<QuotaOutput, String> {
    let settings = ctx.config();
    let key = format!("quota:{}", ctx.principal_id());

    // `?`, not `.ok()`: with no cache there is no counter, and an endpoint that
    // silently stops limiting when Redis blinks is worse than one that fails.
    let used = ctx.cache_incr(&key, 1, Some(settings.window_secs))?;
    if used > settings.limit {
        let wait = ctx.cache_ttl(&key)?.unwrap_or(settings.window_secs as i64);
        return Err(format!("rate limit reached; try again in {wait}s"));
    }

    Ok(QuotaOutput {
        used,
        limit: settings.limit,
        resets_in: ctx.cache_ttl(&key)?.unwrap_or(0),
    })
}

// ---------------------------------------------------------------------------
// A read-through cache on the resource itself.
//
// `report` above caches something the framework could never have cached for it.
// This pair does the other thing a cache is for: it serves `GET /api/reading/{id}`
// out of Redis, skipping Postgres entirely. The framework still won't do it for
// you — but a `before_read` hook that returns `reply::replace(row)` *is* the
// response, so the hook author can, having decided what makes the row stale.
// ---------------------------------------------------------------------------

/// The key a single reading is cached under. Versioned like `report`'s, so a
/// change to what a row looks like doesn't have to reason about old entries.
///
/// `reading` is a global, publicly readable resource, so the id is the whole
/// key. On a scoped resource it would not be: a `before_read` short-circuit
/// skips the row-level filters the query would have applied, so the key has to
/// carry whatever those filters would have checked — the tenant, the owner.
fn row_key(id: &str) -> String {
    format!("reading:v1:{id}")
}

/// `before_read` on `reading` — answer from Redis, or fall through to Postgres.
///
/// A hit returns the row and the database is never touched; a miss (or a Redis
/// that is down) returns `proceed()` and the request runs exactly as it would
/// have with no cache configured. That is the whole reason the miss path stays
/// cheap to keep correct: it is the only path when the cache isn't there.
fn reading_before_read(
    ctx: &Context<()>,
    _input: serde_json::Value,
) -> Result<serde_json::Value, String> {
    let Some(hook) = ctx.hook() else {
        return Ok(reply::proceed());
    };
    let Some(id) = hook.record_id.clone() else {
        return Ok(reply::proceed());
    };

    // `.ok().flatten()`, as in `report`: a cache failure is a miss, not an
    // error — the row is in Postgres either way.
    match ctx.cache_get_as::<serde_json::Value>(&row_key(&id)).ok().flatten() {
        Some(row) => {
            ctx.info(&format!("reading {id} served from cache"));
            Ok(reply::replace(row))
        }
        None => Ok(reply::proceed()),
    }
}

/// `after_read` on `reading` — populate the cache with what the query returned.
///
/// Only reached on a miss: a hit never gets this far, because the `before` hook
/// already answered. The TTL is the backstop for an invalidation that didn't
/// happen — a delete that raced a concurrent read, a Redis that blinked — so
/// this is deliberately short.
fn reading_after_read(
    ctx: &Context<()>,
    input: serde_json::Value,
) -> Result<serde_json::Value, String> {
    if let Some(id) = input["id"].as_str() {
        if let Err(error) = ctx.cache_set(&row_key(id), &input, Some(60)) {
            ctx.warn(&format!("could not cache the reading: {error}"));
        }
    }
    // Observational: the response body is the row the query produced.
    Ok(reply::proceed())
}

/// Drops one reading's cached row. Best-effort, like every invalidation here.
fn drop_row(ctx: &Context<()>, id: &str) {
    if id.is_empty() {
        return;
    }
    match ctx.cache_delete(&row_key(id)) {
        Ok(true) => ctx.info(&format!("row cache for reading {id} invalidated")),
        Ok(false) => {}
        Err(error) => ctx.warn(&format!("could not invalidate the row cache: {error}")),
    }
}

/// Deletes the memoised report for one sensor. Best-effort by design: a cache
/// that is down has nothing stale in it to invalidate.
fn drop_report(ctx: &Context<()>, sensor: &str) {
    if sensor.is_empty() {
        return;
    }
    // Same key the `report` function writes — including the `v1` version.
    let key = format!("report:v1:{sensor}");
    match ctx.cache_delete(&key) {
        Ok(true) => ctx.info(&format!("report cache for {sensor} invalidated")),
        Ok(false) => {}
        Err(error) => ctx.warn(&format!("could not invalidate the report cache: {error}")),
    }
}

/// `after_create` / `after_update` / `after_delete` on `reading`.
///
/// This is the half of caching the framework can't do for you: only the author
/// of `report` knows that a row in `apiplant_reading` is what makes its answer
/// stale. The hook receives the row that was written or removed, so the sensor
/// to invalidate is right there.
///
/// Observational — it returns `proceed()` and never changes the response.
fn reading_changed(ctx: &Context<()>, _input: serde_json::Value) -> Result<serde_json::Value, String> {
    let Some(hook) = ctx.hook() else {
        return Ok(reply::proceed());
    };
    let row = hook.row();
    drop_report(ctx, row["sensor"].as_str().unwrap_or_default());
    // The same event makes the row's own cached copy stale. `after_create` has
    // nothing to drop, and deleting a key that isn't there is not an error.
    drop_row(ctx, row["id"].as_str().unwrap_or_default());
    Ok(reply::proceed())
}

/// `before_update` on `reading`.
///
/// An update can move a reading from one sensor to another, and by the time
/// `after_update` runs the old name is gone. `before_update` still has the id,
/// so it reads the current sensor and invalidates that key too; the `after`
/// hook then takes care of the new one.
fn reading_before_update(
    ctx: &Context<()>,
    _input: serde_json::Value,
) -> Result<serde_json::Value, String> {
    let Some(hook) = ctx.hook() else {
        return Ok(reply::proceed());
    };
    if let Some(id) = hook.record_id.clone() {
        if let Ok(Some(row)) = ctx.query_one(
            "SELECT sensor FROM apiplant_reading WHERE id = $1",
            &[json!(id)],
        ) {
            drop_report(ctx, row["sensor"].as_str().unwrap_or_default());
        }
    }
    Ok(reply::proceed())
}

apiplant_function::functions! {
    {
        name: "report",
        description: "Summarises a sensor's readings, memoised in Redis.",
        method: Post,
        permission: "public",
        handler: report,
    },
    {
        name: "quota",
        description: "A per-caller rate limit backed by an atomic Redis counter.",
        method: Post,
        permission: "authenticated",   // the counter is keyed by principal
        handler: quota,
    },
    {
        name: "reading_changed",
        description: "Invalidates a sensor's memoised report when its readings change.",
        method: Post,
        visibility: Private,           // a hook needs no HTTP endpoint
        handler: reading_changed,
    },
    {
        name: "reading_before_read",
        description: "Serves a single reading from Redis, skipping the database.",
        method: Post,
        visibility: Private,
        handler: reading_before_read,
    },
    {
        name: "reading_after_read",
        description: "Caches the reading a query just returned.",
        method: Post,
        visibility: Private,
        handler: reading_after_read,
    },
    {
        name: "reading_before_update",
        description: "Invalidates the report of the sensor a reading is moving away from.",
        method: Post,
        visibility: Private,
        handler: reading_before_update,
    },
}
