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
before_create = "post_before_create"   # validate & normalise what the client sent
after_create  = "post_after_create"    # record it, notify, enrich the response
after_list    = "post_after_list"

[fields.title]
type     = "string"
required = true
```

Each event names **one function**, and each function serves one event — the
handler never has to work out why it was called. The names refer to the `name`
in a function's `function!` / `functions!` block, the same names that can be
mounted at `/functions/<name>`. Hooks ignore a function's `visibility`, so
declaring a hook function `Private` is the usual choice: it's callable from the
lifecycle but invisible over HTTP and absent from the docs.

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
  column are stamped, and a body they return goes through the same
  [server-owned column](api-reference.md#server-owned-columns) stripping a
  client's does — so a hook can't spoof the tenant, the owner or a password
  hash either.
* A `before_*` hook that aborts stops the operation, so its `after_*` twin never
  runs and nothing is written.
* `after_*` runs only when the operation succeeded. A `404` skips it.
* `GET /parent/{id}/child` runs the **child's** list hooks — the rows returned
  are the child's.

`before_delete` and `after_delete` need the row, so the framework fetches it
first (respecting the same permission filters). That extra read only happens
when one of those hooks is declared.

### Registration fires the `user` create hooks

`POST <base>/auth/register` writes a row to the `user` table, so it is a
`create` on the `user` resource: its `before_create` and `after_create` hooks
run there exactly as they do on `POST <base>/user`. One function covers both
doors into the same table. Two details follow from what registration is:

* the plaintext `password` has already been swapped for the hashed
  `password_field` before `before_create` sees the body, so a hook never handles
  the secret;
* the caller is anonymous — `authenticated` is `false` and `principal_id` is
  null. The new account is the row `after_create` receives, and its id is also
  in `record_id`.

A replacement returned from `after_create` replaces the `user` object in the
register response, leaving the issued `token` alone. Aborting from
`before_create` fails the registration and writes nothing.

[`examples/14-email-domains`](../examples/14-email-domains) uses this to put a
new account straight into the organisation that owns its email domain.

## Writing a hook

A hook is an ordinary function; nothing in the `function!` block changes. What's
new is `ctx.hook()`, which is `Some` when the call came from a lifecycle event
and `None` when the function was called directly over HTTP:

```rust
use apiplant_function::prelude::*;
use serde_json::{json, Value};

/// before_create on `post`: require a title, and normalise it.
fn post_before_create(ctx: &Context<()>, mut input: Value) -> Result<Value, String> {
    let title = input["title"].as_str().unwrap_or_default().trim().to_string();
    if title.is_empty() {
        return Ok(reply::abort(422, "title is required"));
    }
    if let Some(hook) = ctx.hook() {
        ctx.info(&format!("{} by {:?}", hook.resource, hook.principal_id));
    }

    input["title"] = json!(title);
    Ok(reply::replace(input))
}

apiplant_function::function! {
    name: "post_before_create",
    description: "Validates and normalises posts before they are stored.",
    method: Post,
    visibility: Private,   // hooks don't need an HTTP endpoint
    handler: post_before_create,
}
```

`ctx.hook()` is `None` when the function is called over HTTP instead — worth
handling if a function serves both roles, and safe to ignore for a `Private`
hook-only function that can never be reached any other way.

A complete, runnable version — five per-event hooks in one library, wired into a
resource — is [`examples/08-hooks`](../examples/08-hooks):

```bash
cargo run -p apiplant -- build examples/08-hooks
cargo run -p apiplant -- run examples/08-hooks
```

It shows all five in the boot log and enforces them on `/api/post`: titles are
trimmed and validated, other members' drafts are hidden from listings, and
published posts refuse to be deleted without `?force=1`.

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

## A whole set of hooks in one library

Each event gets its **own** function — no dispatcher, no matching on the event
name inside a handler. One library can export as many as you like with
`functions!`:

```rust
apiplant_function::functions! {
    {
        name: "post_before_create",
        description: "Validates a post before it is stored.",
        method: Post,
        visibility: Private,
        handler: post_before_create,
    },
    {
        name: "post_after_create",
        description: "Records a newly created post.",
        method: Post,
        visibility: Private,
        handler: post_after_create,
    },
}
```

```toml
[hooks]
before_create = "post_before_create"
after_create  = "post_after_create"
```

Every entry takes the same fields as `function!` and keeps its own inferred
`Config`/`Input`/`Output` types — including its own `functions/<name>.toml`, so
`post_before_create` and `post_before_update` can be configured separately.
Names must be unique within a library.

Hooks are resolved by function name, so it makes no difference whether they come
from one library or several. If two events genuinely want identical behaviour,
point them at the same function and read `hook.event` to tell them apart.

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

## Built-in functions

Some hooks ship with the framework. A **built-in** is an ordinary Rust function
compiled into the server and registered in the same function registry your
`functions/` libraries land in — same host API, same reply protocol, same
`[hooks]` wiring. The only differences: it is always present, it is always
`private` (no HTTP endpoint), and its name lives in the reserved **`apiplant_`**
namespace, so a function of your own can never collide with one.

| Function | Wired to | What it does |
|----------|----------|--------------|
| `apiplant_organization_join` | `membership.before_create` | Resolves the person being added — by `user_id`, or by the identity field they registered with (`email` by default). `422` if the body names neither, `404` if nobody is registered with that identity, `409` if they already belong to the organisation. |

They exist for work that has to happen *behind* the API rather than in front of
it. `apiplant_organization_join` is the case in point: `user` reads as `member`,
so the admin adding a newcomer cannot resolve that newcomer's email to an id
themselves — the server does it for them, for that one purpose, and returns
nothing else about the account.

Declaring a function of your own with a built-in's exact name replaces it — the
escape hatch when you want the hook but not this version of it. Because you have
to type the reserved prefix to do so, it can't happen by accident; the server
logs a warning when it does.

## Failure modes

| Situation | Result |
|-----------|--------|
| Hook names a function that isn't loaded | **500**, and the operation does not run. Hooks fail **closed** — silently skipping one would bypass validation. The server also logs this loudly at boot. |
| Handler returns `Err(msg)` | **400** with `msg`. |
| Hook returns invalid JSON | **500**. |
| Hook panics | **500**, and the request is aborted — but the server keeps running. The panic message and backtrace go to the log; the caller gets a generic message, since a panic tends to name internals. See [when a function panics](functions.md#when-a-function-panics). |
| Unknown key in `[hooks]` | The resource fails to load, so a typo like `befor_create` is caught at boot instead of silently never firing. |

Boot logs one line per resolved hook:

```
INFO apiplant_server:   hook post.before_create -> post_before_create
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
