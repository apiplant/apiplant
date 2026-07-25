//! A Rust function that depends on a third-party crate.
//!
//! The point of this file is the `use uuid::Uuid` below: it is a dependency the
//! host never provides, available only because this function lives in a
//! directory with its own `Cargo.toml`. Everything else is an ordinary
//! single-function library, identical to `examples/07-functions`.

use apiplant_function::prelude::*;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Deserialize, JsonSchema)]
struct Input {
    /// How many tokens to mint (default 1).
    #[serde(default = "one")]
    count: usize,
}
fn one() -> usize {
    1
}

#[derive(Serialize, JsonSchema)]
struct Output {
    /// Freshly generated v4 UUIDs, courtesy of the `uuid` crate.
    tokens: Vec<String>,
}

fn token(_ctx: &Context<()>, input: Input) -> Result<Output, String> {
    let tokens = (0..input.count.clamp(1, 100))
        .map(|_| Uuid::new_v4().to_string())
        .collect();
    Ok(Output { tokens })
}

apiplant_function::function! {
    name: "token",
    description: "Mints UUID tokens using the third-party `uuid` crate.",
    method: Post,
    visibility: Public,
    handler: token,
}
