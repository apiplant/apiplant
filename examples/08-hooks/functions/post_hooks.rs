//! Example apiplant functions used as **resource lifecycle hooks**, wired up in
//! [`examples/demo-app`](../../demo-app).
//!
//! One function per event — no dispatcher, no matching on the event name. The
//! library exports all five through `functions!`, and `models/post.toml` points
//! each event at the one it wants:
//!
//! ```toml
//! [hooks]
//! before_create = "post_before_create"
//! before_update = "post_before_update"
//! after_create  = "post_after_create"
//! after_list    = "post_after_list"
//! before_delete = "post_before_delete"
//! ```
//!
//! Build with `cargo build -p function-post-hooks` and drop the `.so` into the
//! app's `functions/` directory.

use apiplant_function::prelude::*;
use serde::Deserialize;
use serde_json::{json, Value};

/// Config for the validating hooks, read from `functions/<name>.toml`. Each
/// function reads its own file, so create and update can be tuned separately.
#[derive(Deserialize)]
#[serde(default)]
struct Rules {
    /// Titles longer than this are rejected.
    max_title_len: usize,
    /// Titles containing any of these (case-insensitive) are rejected.
    banned_words: Vec<String>,
}

impl Default for Rules {
    fn default() -> Self {
        Rules {
            max_title_len: 200,
            banned_words: Vec::new(),
        }
    }
}

/// `before_create` — the submitted body arrives as `input`. Returning
/// `replace` rewrites what gets stored; `abort` rejects the request outright.
fn post_before_create(ctx: &Context<Rules>, input: Value) -> Result<Value, String> {
    let title = input
        .get("title")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim()
        .to_string();

    if title.is_empty() {
        return Ok(reply::abort(422, "title is required"));
    }
    if let Some(problem) = check(ctx.config(), &title) {
        return Ok(reply::abort(422, problem));
    }

    let mut body = input;
    body["title"] = json!(title);
    Ok(reply::replace(body))
}

/// `before_update` — the same rules, but `PATCH` bodies are partial: a body
/// that doesn't touch the title has nothing to validate.
fn post_before_update(ctx: &Context<Rules>, input: Value) -> Result<Value, String> {
    let Some(raw) = input.get("title").and_then(Value::as_str) else {
        return Ok(reply::proceed());
    };
    let title = raw.trim().to_string();

    if title.is_empty() {
        return Ok(reply::abort(422, "title cannot be blanked"));
    }
    if let Some(problem) = check(ctx.config(), &title) {
        return Ok(reply::abort(422, problem));
    }

    let mut body = input;
    body["title"] = json!(title);
    Ok(reply::replace(body))
}

fn check(rules: &Rules, title: &str) -> Option<String> {
    if title.chars().count() > rules.max_title_len {
        return Some(format!(
            "title must be at most {} characters",
            rules.max_title_len
        ));
    }
    let lowered = title.to_lowercase();
    rules
        .banned_words
        .iter()
        .find(|word| lowered.contains(&word.to_lowercase()))
        .map(|word| format!("title may not mention `{word}`"))
}

/// `after_create` — the stored row arrives as `input`, and the hook context
/// says who caused it. `proceed()` leaves the `201` response untouched.
fn post_after_create(ctx: &Context<()>, input: Value) -> Result<Value, String> {
    let hook = ctx.hook().ok_or("post_after_create is a lifecycle hook")?;
    ctx.info(&format!(
        "post {} created in org {} by {}",
        input["id"],
        hook.organization_id.as_deref().unwrap_or("-"),
        hook.principal_id.as_deref().unwrap_or("anonymous"),
    ));
    Ok(reply::proceed())
}

/// `after_list` — the rows arrive as `input`. Members see published posts plus
/// their own drafts; an organisation `admin` sees everything.
fn post_after_list(ctx: &Context<()>, input: Vec<Value>) -> Result<Value, String> {
    let hook = ctx.hook().ok_or("post_after_list is a lifecycle hook")?;
    // `roles`, not `role`: a member holds a set of roles, and `role` is only
    // the primary one — so an admin who was given that role alongside another
    // would slip past a check written against it.
    if hook.roles.iter().any(|role| role == "admin") {
        return Ok(reply::proceed());
    }

    let me = hook.principal_id.as_deref().unwrap_or_default();
    let visible: Vec<Value> = input
        .into_iter()
        .filter(|row| row["published"] == json!(true) || row["owner_id"] == json!(me))
        .collect();
    Ok(reply::replace(json!(visible)))
}

/// `before_delete` — the row about to be deleted arrives as `input` (the host
/// fetches it for you). Published posts are protected unless `?force=1`.
fn post_before_delete(ctx: &Context<()>, input: Value) -> Result<Value, String> {
    let hook = ctx.hook().ok_or("post_before_delete is a lifecycle hook")?;
    let forced = hook.query.get("force").map(String::as_str) == Some("1");

    if input["published"] == json!(true) && !forced {
        return Ok(reply::abort(
            409,
            "published posts are protected; retry with ?force=1",
        ));
    }
    Ok(reply::proceed())
}

// Five independent functions from one library. Hooks run whatever the
// visibility, so hook-only functions are `Private`: no HTTP endpoint, and
// absent from the generated docs.
apiplant_function::functions! {
    {
        name: "post_before_create",
        description: "Validates and normalises a post before it is stored.",
        method: Post,
        visibility: Private,
        handler: post_before_create,
    },
    {
        name: "post_before_update",
        description: "Validates and normalises a post before it is updated.",
        method: Post,
        visibility: Private,
        handler: post_before_update,
    },
    {
        name: "post_after_create",
        description: "Reports a newly created post.",
        method: Post,
        visibility: Private,
        handler: post_after_create,
    },
    {
        name: "post_after_list",
        description: "Hides other members' drafts from post listings.",
        method: Post,
        visibility: Private,
        handler: post_after_list,
    },
    {
        name: "post_before_delete",
        description: "Protects published posts from deletion.",
        method: Post,
        visibility: Private,
        handler: post_before_delete,
    },
}
