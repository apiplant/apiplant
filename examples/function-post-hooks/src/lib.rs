//! Example apiplant function used as a **resource lifecycle hook**.
//!
//! One library exports one function, but one function can serve *several*
//! events — branch on `ctx.hook().event`. Wire it up from `models/post.toml`:
//!
//! ```toml
//! [hooks]
//! before_create = "post_hooks"
//! before_update = "post_hooks"
//! after_create  = "post_hooks"
//! after_list    = "post_hooks"
//! ```
//!
//! Build with `cargo build -p function-post-hooks --release` and drop the `.so`
//! into an app's `functions/` directory.

use apiplant_function::prelude::*;
use serde::Deserialize;
use serde_json::{json, Value};

/// Config comes from `functions/post_hooks.toml` (all optional).
#[derive(Deserialize)]
#[serde(default)]
struct Config {
    /// Titles longer than this are rejected.
    max_title_len: usize,
    /// Titles containing any of these (case-insensitive) are rejected.
    banned_words: Vec<String>,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            max_title_len: 200,
            banned_words: Vec::new(),
        }
    }
}

/// The whole hook. `input` is the operation's payload — the submitted body on
/// `before_*`, the row or rows on `after_*` — and the returned object tells the
/// host to continue, to replace that payload, or to reject the request.
fn post_hooks(ctx: &Context<Config>, input: Value) -> Result<Value, String> {
    // Invoked over HTTP rather than from a lifecycle event: nothing to do.
    let Some(hook) = ctx.hook() else {
        return Ok(reply::proceed());
    };

    match hook.event.as_str() {
        // Validate and normalise what the client sent, before it is stored.
        "before_create" | "before_update" => {
            let mut body = input;
            let Some(title) = body.get("title").and_then(Value::as_str) else {
                // PATCH bodies are partial; nothing to check without a title.
                return Ok(reply::proceed());
            };
            let title = title.trim().to_string();

            if title.is_empty() {
                return Ok(reply::abort(422, "title is required"));
            }
            if title.chars().count() > ctx.config().max_title_len {
                return Ok(reply::abort(
                    422,
                    format!("title must be at most {} characters", ctx.config().max_title_len),
                ));
            }
            let lowered = title.to_lowercase();
            if let Some(word) = ctx
                .config()
                .banned_words
                .iter()
                .find(|w| lowered.contains(&w.to_lowercase()))
            {
                return Ok(reply::abort(422, format!("title may not mention `{word}`")));
            }

            body["title"] = json!(title);
            Ok(reply::replace(body))
        }

        // The row exists now: record who created it, using the host's database.
        "after_create" => {
            ctx.info(&format!(
                "post {} created by {}",
                hook.row()["id"],
                hook.principal_id.as_deref().unwrap_or("anonymous"),
            ));
            Ok(reply::proceed())
        }

        // Wrap list responses in an envelope, and hide drafts from anonymous
        // callers (the resource's permissions may allow public reads).
        "after_list" => {
            let rows: Vec<Value> = hook
                .rows()
                .iter()
                .filter(|row| hook.authenticated || row["published"] == json!(true))
                .cloned()
                .collect();
            Ok(reply::replace(json!({ "count": rows.len(), "rows": rows })))
        }

        _ => Ok(reply::proceed()),
    }
}

apiplant_function::function! {
    name: "post_hooks",
    description: "Validates, normalises and reports on posts as a lifecycle hook.",
    method: Post,
    // Hooks run whatever the visibility, so a hook-only function should be
    // private: no HTTP endpoint, and absent from the generated docs.
    visibility: Private,
    handler: post_hooks,
}
