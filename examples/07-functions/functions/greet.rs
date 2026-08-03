//! Example apiplant function — the whole thing, written with the `function!`
//! macro. Compare with the raw ABI: no root module, no `extern` shims, no
//! manual JSON or `RString` juggling. Build with
//! `cargo build -p function-greet --release` and drop the `.so` into an app's
//! `functions/` directory.

use apiplant_function::prelude::*;
use serde::{Deserialize, Serialize};

/// Config comes from `functions/greet.toml` (typed, with a default).
#[derive(Deserialize, Default)]
struct Config {
    #[serde(default = "default_greeting")]
    greeting: String,
}

fn default_greeting() -> String {
    "Hello".to_string()
}

#[derive(Deserialize, JsonSchema)]
struct Input {
    /// Who to greet.
    name: String,
}

#[derive(Serialize, JsonSchema)]
struct Output {
    /// The composed greeting.
    message: String,
    /// How many users are registered.
    registered_users: i64,
}

/// The entire function: a plain typed Rust fn. `ctx` gives you the typed config,
/// the caller id, logging, and database access.
fn greet(ctx: &Context<Config>, input: Input) -> Result<Output, String> {
    let registered_users = ctx
        .query_one("SELECT count(*)::int AS n FROM apiplant_user", &[])?
        .and_then(|row| row.get("n").and_then(|n| n.as_i64()))
        .unwrap_or(0);

    ctx.info("greet invoked");

    Ok(Output {
        message: format!("{}, {}!", ctx.config().greeting, input.name),
        registered_users,
    })
}

apiplant_function::function! {
    name: "greet",
    description: "Greets a person and counts total registered users.",
    method: Post,
    visibility: Public,
    handler: greet,
}
