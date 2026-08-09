//! A function that answers slowly, on purpose.
//!
//! It emits a line, waits two seconds, emits the next, and finishes with a
//! summary — which is the shape of every genuinely slow endpoint: a report
//! being assembled, a batch being processed, a third party being polled. The
//! sleeps stand in for the work.
//!
//! The point is what the two endpoints do with the same code:
//!
//! * `POST /api/functions/rehearse` waits for all of it and answers with the
//!   return value, as JSON. Several seconds of silence, then a document.
//! * `POST /api/functions/rehearse/stream` sends each line the moment it is
//!   produced, then the return value as the final `done` event.
//!
//! The handler does not know which one it is serving, and does not need to.
//! [`ctx.emit`] answers *"keep going?"* rather than *"did that arrive?"*: it is
//! `true` on a plain invocation, whose caller is still waiting for the return
//! value, and `false` only once a streaming caller has hung up — which is the
//! one case where continuing would be work for nobody.
//!
//! ## Why sleeping here is fine
//!
//! Functions run on a blocking worker precisely so that they may block. It is
//! the same reason `ctx.query` can be synchronous, and it is why a function
//! author never writes `async`.
//!
//! [`ctx.emit`]: apiplant_function::Context::emit

use std::thread::sleep;
use std::time::{Duration, Instant};

use apiplant_function::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Deserialize, Default)]
struct Config {
    /// How long to pause between lines. Two seconds by default: long enough
    /// that a buffering proxy is obvious rather than subtle.
    #[serde(default = "default_pause")]
    pause_secs: u64,
}

fn default_pause() -> u64 {
    2
}

#[derive(Deserialize, JsonSchema)]
struct Input {
    /// What to say, one line per chunk. Defaults to a short rehearsal.
    #[serde(default)]
    lines: Vec<String>,
}

#[derive(Serialize, JsonSchema)]
struct Output {
    /// How many lines were produced before the function stopped.
    lines: usize,
    /// How many were left unsaid because the caller stopped listening. Zero
    /// unless somebody closed a stream halfway through.
    abandoned: usize,
    /// How long the whole thing took. Roughly `pause_secs × (lines - 1)`,
    /// which is the number the streaming endpoint spreads out and the plain
    /// one makes you wait for in one go.
    elapsed_ms: u128,
}

fn rehearse(ctx: &Context<Config>, input: Input) -> Result<Output, String> {
    let lines = match input.lines.is_empty() {
        true => vec![
            "First, the thing that is quick to know.".to_string(),
            "Then the part that took a moment to work out.".to_string(),
            "And finally the bit that needed everything above it.".to_string(),
        ],
        false => input.lines,
    };

    let pause = Duration::from_secs(ctx.config().pause_secs);
    let started = Instant::now();
    let mut produced = 0;

    for (index, line) in lines.iter().enumerate() {
        // The work. Nothing about `emit` requires a delay — this is what makes
        // the example legible from a terminal.
        if index > 0 {
            sleep(pause);
        }

        // Each chunk becomes a `delta` event on the streaming endpoint, sent
        // long before this function knows what it will eventually return.
        if !ctx.emit(&format!("{line}\n")) {
            // Only a streaming caller who closed the connection gets here.
            // Carrying on would spend two seconds a line producing text for
            // nobody.
            ctx.info(&format!("nobody listening after {produced} lines"));
            return Ok(Output {
                lines: produced,
                abandoned: lines.len() - produced,
                elapsed_ms: started.elapsed().as_millis(),
            });
        }
        produced += 1;
    }

    Ok(Output {
        lines: produced,
        abandoned: 0,
        elapsed_ms: started.elapsed().as_millis(),
    })
}

apiplant_function::function! {
    name: "rehearse",
    description: "Says three things, two seconds apart — streamed if anybody is listening.",
    method: Post,
    visibility: Public,
    handler: rehearse,
}
