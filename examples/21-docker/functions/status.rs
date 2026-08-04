//! A compiled function, for the half of the deployment story TypeScript skips.
//!
//! This is an ordinary Rust function — nothing about it knows it is in a
//! container. What the container has to do for it is compile it: `.rs` needs
//! cargo, which the runtime image deliberately does not carry, so the Dockerfile
//! builds it in a `rust` stage and copies `libstatus.so` into the final image.
//! The result is a runtime image with no toolchain in it, which is the point.

use apiplant_function::prelude::*;
use serde::{Deserialize, Serialize};

/// From `functions/status.toml`, whose values are expanded from the
/// environment — so one image reports the release it was built from.
#[derive(Deserialize, Default)]
struct Config {
    #[serde(default)]
    release: String,
    #[serde(default)]
    env: String,
}

/// Takes nothing: an empty body arrives as `{}`, which this deserializes from.
#[derive(Deserialize, JsonSchema)]
struct NoInput {}

#[derive(Serialize, JsonSchema)]
struct Output {
    /// What this container is running.
    release: String,
    /// Which deployment it thinks it is.
    env: String,
    /// Rows visible right now — a query, so a green answer means the database
    /// is genuinely reachable rather than merely configured.
    notes: i64,
}

/// Cheap enough to be a readiness probe: one round trip to Postgres.
fn status(ctx: &Context<Config>, _input: NoInput) -> Result<Output, String> {
    let notes = ctx
        .query_one("SELECT count(*)::int AS n FROM apiplant_note", &[])?
        .and_then(|row| row.get("n").and_then(|n| n.as_i64()))
        .unwrap_or(0);

    let config = ctx.config();

    Ok(Output {
        release: if config.release.is_empty() {
            "dev".to_string()
        } else {
            config.release.clone()
        },
        env: if config.env.is_empty() {
            "local".to_string()
        } else {
            config.env.clone()
        },
        notes,
    })
}

apiplant_function::function! {
    name: "status",
    description: "Reports the running release and proves the database answers.",
    method: Get,
    visibility: Public,
    handler: status,
}
