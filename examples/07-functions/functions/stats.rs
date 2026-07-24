//! A second function in the same app, showing the parts `greet.rs` doesn't:
//! a `Get` endpoint, `Authenticated` visibility, and no configuration at all.
//!
//! Mounted at `GET /api/functions/stats` — calling it anonymously returns 401,
//! and calling it with the wrong method returns 405.

use apiplant_function::prelude::*;
use serde::{Deserialize, Serialize};

/// This function takes no input. An empty request body arrives as `{}`, so an
/// empty struct deserializes cleanly.
#[derive(Deserialize, JsonSchema)]
struct NoInput {}

#[derive(Serialize, JsonSchema)]
struct Stats {
    /// How many notes exist.
    notes: i64,
    /// How many accounts are registered.
    users: i64,
    /// The id of whoever asked.
    asked_by: String,
}

/// `Context<()>` means "no config" — there is no `functions/stats.toml`.
fn stats(ctx: &Context<()>, _input: NoInput) -> Result<Stats, String> {
    let count = |sql: &str| -> Result<i64, String> {
        Ok(ctx
            .query_one(sql, &[])?
            .and_then(|row| row.get("n").and_then(|n| n.as_i64()))
            .unwrap_or(0))
    };

    Ok(Stats {
        notes: count("SELECT count(*)::int AS n FROM apiplant_note")?,
        users: count("SELECT count(*)::int AS n FROM apiplant_user")?,
        // Visibility is Authenticated, so this is never empty here.
        asked_by: ctx.principal_id().to_string(),
    })
}

apiplant_function::function! {
    name: "stats",
    description: "Counts the rows in this app. Requires authentication.",
    method: Get,
    visibility: Authenticated,
    handler: stats,
}
