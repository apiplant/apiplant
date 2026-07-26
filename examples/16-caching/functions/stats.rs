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
}
