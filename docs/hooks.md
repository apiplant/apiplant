# Lifecycle hooks

A **hook** runs one of your [functions](functions.md) at a specific point in a
resource's request lifecycle. It's how you add custom behaviour — validation,
normalisation, audit trails, notifications, response shaping — to the generated
CRUD endpoints without writing any server code.

Declare them in the resource's `[hooks]` section, one function name per event:

```toml
# models/post.toml
[resource]
name = "post"

[hooks]
before_create = "post_guard"     # validate & normalise what the client sent
after_create  = "post_audit"     # record it, notify, enrich the response
after_list    = "post_redact"

[fields.title]
type     = "string"
required = true
```

The names refer to the `name` in a function's `function!` block — the same
functions that can be mounted at `/functions/<name>`. Hooks ignore a function's
`visibility`, so declaring a hook function `Private` is the usual choice: it's
callable from the lifecycle but invisible over HTTP and absent from the docs.

## The events

Ten events, a `before` and an `after` for each action:

| Event | Fires | Payload the hook receives | A returned `data` |
|-------|-------|---------------------------|-------------------|
| `before_list` | before the query | `{}` | ignored |
| `after_list` | after rows are fetched and `?expand=`ed | the array of rows | replaces the response body |
| `before_read` | before the query | `{}` | ignored |
| `after_read` | after the row is fetched | the row | replaces the response body |
| `before_create` | before the insert | the submitted body | replaces the body to insert |
| `after_create` | after the insert | the created row | replaces the `201` body |
| `before_update` | before the update | the submitted body | replaces the body to write |
| `after_update` | after the update | the updated row | replaces the response body |
| `before_delete` | before the delete | the row about to be deleted | ignored |
| `after_delete` | after the delete | the row that was deleted | replaces the `204` with a `200` + body |

Ordering guarantees:

* `before_*` runs **after** the permission check and after multitenancy filters
  are resolved — an unauthorised request never reaches your hook.
* `before_create`/`before_update` run **before** `organization_id` and the owner
  column are stamped, so a hook can't spoof the tenant or the owner.
* A `before_*` hook that aborts stops the operation, so its `after_*` twin never
  runs and nothing is written.
* `after_*` runs only when the operation succeeded. A `404` skips it.
* `GET /parent/{id}/child` runs the **child's** list hooks — the rows returned
  are the child's.

`before_delete` and `after_delete` need the row, so the framework fetches it
first (respecting the same permission filters). That extra read only happens
when one of those hooks is declared.

## Writing a hook

A hook is an ordinary function; nothing in the `function!` block changes. What's
new is `ctx.hook()`, which is `Some` when the call came from a lifecycle event
and `None` when the function was called directly over HTTP:

```rust
use apiplant_function::prelude::*;
use serde_json::{json, Value};

/// before_create on `post`: require a title, and normalise it.
fn post_guard(ctx: &Context<()>, mut input: Value) -> Result<Value, String> {
    let Some(hook) = ctx.hook() else {
        return Ok(reply::proceed()); // called over HTTP, not as a hook
    };

    let title = input["title"].as_str().unwrap_or_default().trim().to_string();
    if title.is_empty() {
        return Ok(reply::abort(422, "title is required"));
    }
    ctx.info(&format!("{} on {} by {:?}", hook.event, hook.resource, hook.principal_id));

    input["title"] = json!(title);
    Ok(reply::replace(input))
}

apiplant_function::function! {
    name: "post_guard",
    description: "Validates and normalises posts before they are stored.",
    method: Post,
    visibility: Private,   // hooks don't need an HTTP endpoint
    handler: post_guard,
}
```

A complete, runnable version — one function serving four events — is in
[`examples/function-post-hooks`](../examples/function-post-hooks); build it and
drop the `.so` into `functions/` like any other function:

```bash
cargo build -p function-post-hooks --release
cp target/release/libfunction_post_hooks.so my-app/functions/
```

The handler's `Input` is the payload from the table above, so
`serde_json::Value` is the flexible choice — but a typed struct works whenever
the payload's shape is known (`Input` for `before_create`, your row type for
`after_read`, `Vec<Row>` for `after_list`).

### What a hook returns

The `Ok` value is a small instruction object. Build it with the `reply` helpers:

| Helper | JSON | Effect |
|--------|------|--------|
| `reply::proceed()` | `{}` | Continue unchanged. |
| `reply::replace(value)` | `{"data": …}` | Replace the payload or the response body (see the table above). |
| `reply::abort(status, msg)` | `{"error": {"status": …, "message": …}}` | Stop the request with that status and a `{"error": msg}` body. |

Anything else — `null`, a bare string, an object without those keys — also means
"continue unchanged", so an observational hook can return whatever is convenient.
Returning `Err(e)` from the handler aborts with a **400** and `e`'s text.
Statuses outside `400..=599` are clamped to `400`.

A `before_create`/`before_update` replacement must be a JSON **object** (it
becomes the row's columns); replacements for `after_*` hooks can be any JSON.

## The hook context

`ctx.hook()` returns a `Hook` describing the operation:

| Field | Type | Meaning |
|-------|------|---------|
| `event` | `String` | `"before_create"`, `"after_list"`, … |
| `action` | `String` | `"list"` \| `"read"` \| `"create"` \| `"update"` \| `"delete"` |
| `phase` | `String` | `"before"` \| `"after"` (also `is_before()` / `is_after()`) |
| `resource` | `String` | The resource the hook is attached to. |
| `url` | `String` | Path + query string of the request. |
| `method` | `String` | `GET`, `POST`, `PATCH`, … |
| `query` | `BTreeMap<String, String>` | Parsed query parameters. |
| `authenticated` | `bool` | Whether the caller is authenticated. |
| `principal_id` | `Option<String>` | The caller's user id. |
| `organization_id` | `Option<String>` | The caller's active organisation. |
| `role` | `Option<String>` | The caller's role in that organisation. |
| `record_id` | `Option<String>` | The id in the URL, for read/update/delete. |
| `data` | `Option<Value>` | The submitted body, on `before_create`/`before_update`. |
| `row` | `Option<Value>` | The row created, fetched, updated or deleted. |
| `rows` | `Option<Vec<Value>>` | The rows a list returned, on `after_list`. |

Accessors keep the common cases short: `hook.data()`, `hook.row()` and
`hook.rows()` return the value or an empty default, and `hook.field("title")`
reads from whichever subject the event carries.

The payload is passed *both* as the handler's `input` and inside the context, so
either style works:

```rust
fn audit(ctx: &Context<()>, _input: Value) -> Result<Value, String> {
    let Some(hook) = ctx.hook() else { return Ok(reply::proceed()) };
    ctx.execute(
        "INSERT INTO apiplant_audit (event, actor, detail) VALUES ($1, $2, $3)",
        &[
            json!(hook.event),
            json!(hook.principal_id),
            json!(hook.row()["title"]),
        ],
    )?;
    Ok(reply::proceed())
}
```

Everything else on `Context` — `query`, `execute`, `config()`, the loggers —
works exactly as it does for an HTTP function. Hooks run on a blocking worker,
so these stay ordinary synchronous calls.

## One function, several events

A library exports exactly one function, so a function that serves more than one
event branches on `hook.event`:

```toml
[hooks]
before_create = "post_hooks"
before_update = "post_hooks"
after_create  = "post_hooks"
```

```rust
match hook.event.as_str() {
    "before_create" | "before_update" => validate(input),
    "after_create" => { audit(hook); Ok(reply::proceed()) }
    _ => Ok(reply::proceed()),
}
```

Nothing stops you from splitting them across libraries instead — hooks are
resolved by name, one function per event.

## Recipes

**Reject invalid input with a precise status**

```rust
if input["price"].as_f64().unwrap_or(-1.0) < 0.0 {
    return Ok(reply::abort(422, "price must be positive"));
}
```

**Stamp a derived column on write**

```rust
input["slug"] = json!(slugify(input["title"].as_str().unwrap_or_default()));
Ok(reply::replace(input))
```

**Hide a field from anonymous callers**

```rust
let hook = ctx.hook().ok_or("hook only")?;
let mut row = input;
if !hook.authenticated {
    row["internal_notes"] = Value::Null;
}
Ok(reply::replace(row))
```

**Paginate-style envelope on list responses**

```rust
let rows = input.as_array().cloned().unwrap_or_default();
Ok(reply::replace(json!({ "count": rows.len(), "rows": rows })))
```

**Protect rows from deletion** (`before_delete` receives the row)

```rust
if input["locked"] == json!(true) {
    return Ok(reply::abort(409, "record is locked"));
}
Ok(reply::proceed())
```

## Failure modes

| Situation | Result |
|-----------|--------|
| Hook names a function that isn't loaded | **500**, and the operation does not run. Hooks fail **closed** — silently skipping one would bypass validation. The server also logs this loudly at boot. |
| Handler returns `Err(msg)` | **400** with `msg`. |
| Hook returns invalid JSON | **500**. |
| Hook panics | **500**. |
| Unknown key in `[hooks]` | The resource fails to load, so a typo like `befor_create` is caught at boot instead of silently never firing. |

Boot logs one line per resolved hook:

```
INFO apiplant_server:   hook post.before_create -> post_guard
```

## Performance notes

* A resource with no `[hooks]` section costs nothing — no extra work happens on
  its endpoints.
* Each hook is one blocking call. Two hooks on one request means two.
* `before_delete`/`after_delete` add one `SELECT` to fetch the doomed row.
* Hooks run **outside** the operation's transaction. An `after_create` hook that
  fails turns the response into an error, but the row stays written.

## See also

* [Functions](functions.md) — writing, building and configuring the functions
  hooks point at.
* [Permissions](permissions.md) — the checks that run before any hook.
* [Resources](resources.md) — the rest of `models/<name>.toml`.
