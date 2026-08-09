# Lifecycle hooks

A **hook** runs one of your [functions](functions.md) at a specific point in a
resource's request lifecycle. It is the mechanism for adding custom behaviour
(validation, normalisation, audit trails, notifications and response shaping) to
the generated CRUD endpoints without writing server code.

Declare them in the resource's `[hooks]` section, one function name per event:

```toml
# resources/post.toml
[resource]
name = "post"

[hooks]
before_create = "post_before_create"   # validate and normalise the submitted body
after_create  = "post_after_create"    # record, notify, or enrich the response
after_list    = "post_after_list"

[fields.title]
type     = "string"
required = true
```

Each event names **one function**, and each function serves one event, so a
handler never has to determine why it was called. The names refer to the `name`
in a function's `function!` or `functions!` block, the same names that can be
mounted at `/functions/<name>`. Hooks ignore a function's `visibility`, so
declaring a hook function `Private` is the usual choice: it remains callable
from the lifecycle while being unreachable over HTTP and absent from the docs.

## The events

Ten events, a `before` and an `after` for each action:

| Event | Fires | Payload the hook receives | A returned `data` |
|-------|-------|---------------------------|-------------------|
| `before_list` | before the query | `{}` | **answers the request**; the query never runs |
| `after_list` | after rows are fetched and `?expand=`ed | the array of rows | replaces the response body |
| `before_read` | before the query | `{}` | **answers the request**; the query never runs |
| `after_read` | after the row is fetched | the row | replaces the response body |
| `before_create` | before the insert | the submitted body | replaces the body to insert |
| `after_create` | after the insert | the created row | replaces the `201` body |
| `before_update` | before the update | the submitted body | replaces the body to write |
| `after_update` | after the update | the updated row | replaces the response body |
| `before_delete` | before the delete | the row about to be deleted | ignored |
| `after_delete` | after the delete | the row that was deleted | replaces the `204` with a `200` + body |

Ordering guarantees:

* `before_*` runs **after** the permission check and after multitenancy filters
  are resolved, so an unauthorised request never reaches a hook.
* `before_create` and `before_update` run **before** `organization_id` and the
  owner column are stamped, and any body they return is subject to the same
  [server-owned column](api-reference.md#server-owned-columns) stripping applied
  to a client's, so a hook cannot spoof the tenant, the owner or a password
  hash.
* A `before_*` hook that aborts stops the operation, so its `after_*` twin never
  runs and nothing is written.
* `after_*` runs only when the operation succeeded. A `404` skips it.
* `GET /parent/{id}/child` runs the **child's** list hooks, since the rows
  returned are the child's.
* A `before_read` or `before_list` that returns `data` short-circuits the
  operation: no query runs, `?expand=` is not applied, and the corresponding
  `after_read`/`after_list` hook does not fire. The hook's value becomes the
  response body exactly as returned.

### Answering a read without a query

That short-circuit is what makes a read-through cache possible: `before_read`
looks in the cache and returns any hit, and the write hooks delete the key.

```rust
fn reading_cached_read(ctx: &Context<()>, _in: Value) -> Result<Value, String> {
    let Some(hook) = ctx.hook() else { return Ok(reply::proceed()) };
    let Some(id) = hook.record_id.clone() else { return Ok(reply::proceed()) };
    match ctx.cache_get_as::<Value>(&format!("row:v1:{id}")) {
        Ok(Some(row)) => Ok(reply::replace(row)),   // the database is not queried
        _ => Ok(reply::proceed()),                  // a miss, or no cache configured
    }
}
```

The permission check has already run when the hook fires, so a cached answer
cannot widen access. It does, however, bypass the row-level scoping the query
would have applied, so key the entry by everything that scopes it (the tenant,
the owner) unless nothing does, as on a global public resource. Always set a
TTL: invalidation is best-effort, and the TTL is the backstop.

[Example 16](../examples/16-caching) shows the complete pattern (read hook,
write hooks and the miss path) running against Redis.

`before_delete` and `after_delete` require the row, so the framework fetches it
first, respecting the same permission filters. That extra read happens only when
one of those hooks is declared.

### Registration fires the `user` create hooks

`POST <base>/auth/register` writes a row to the `user` table, so it is a
`create` on the `user` resource: its `before_create` and `after_create` hooks
run there exactly as they do on `POST <base>/user`, so one function covers both
routes into the same table. Two details follow from the nature of registration:

* the plaintext `password` has already been swapped for the hashed
  `password_field` before `before_create` sees the body, so a hook never handles
  the secret;
* the caller is anonymous: `authenticated` is `false` and `principal_id` is
  null. The new account is the row `after_create` receives, and its id is also
  in `record_id`.

A replacement returned from `after_create` replaces the `user` object in the
register response, leaving the issued `token` alone. Aborting from
`before_create` fails the registration and writes nothing.

[`examples/14-email-domains`](../examples/14-email-domains) uses this to place a
new account directly into the organisation that owns its email domain.

### Auth hooks

The create hooks above fire on *both* routes into the `user` table, which is
their purpose, and also why they are unsuitable for logic specific to signup, or
for events with no `create` at all, such as a login. The `user` resource's
`[hooks]` section carries six further events for that, alongside its CRUD
ones:

```toml
# resources/users.toml
[hooks]
after_create = "index_user"     # the table's own lifecycle
before_login = "check_lockout"  # and the endpoints in front of it
after_login  = "record_attempt"
```

They are meaningful only on `user`, the resource the auth endpoints belong to;
the same key on any other resource fails to load, since nothing would fire it. The
protocol is as above: a returned `{"error": …}` aborts and a returned
`{"data": …}` replaces.

| Event | Fires | Payload the hook receives | A returned `data` |
|-------|-------|---------------------------|-------------------|
| `before_register` | before `before_create` | the submitted body, password already hashed | replaces the body to insert |
| `after_register` | after `after_create` | the created account | replaces the response's `user` |
| `before_login` | before the credential lookup | `{ "<identity_field>": … }` | replaces the credentials looked up |
| `after_login` | after **every** attempt | the outcome (below) | merged into the response beside `token` |
| `before_api_key` | before the key row is written | the key's fields | replaces what is written |
| `after_api_key` | after the key row is written | the created row | merged into the response beside `api_key` |

The password never reaches a hook on any of these events, so a hook rejecting an
attempt does so on the identity alone. `after_login` and `after_api_key` cannot
overwrite the secret they run alongside: a `token` or `api_key` key in their
replacement is dropped rather than applied.

`after_login` also runs on failed attempts, which is what makes a lockout
possible without a separate event. It receives:

```json
{ "success": false, "user_id": null, "identity": "ana@acme.test", "reason": "bad_password" }
```

`reason` is `null` on success, and `"unknown_identity"` or `"bad_password"` on
failure. That distinction is never exposed to the caller, since both return
`401 invalid credentials`. Returning an error from a failed attempt is how a
lockout returns `429` instead; on a failure no other returned value is used,
since there is no response body to modify.

In the hook context, `action` is `"register"`, `"login"` or `"api_key"`. The
payload lands in `row` for `after_register` and `after_api_key`, which hand back
an actual record, and in `data` everywhere else. Register and login run
anonymously (`authenticated` is `false`); key issuance runs as the caller.

## Writing a hook

A hook is an ordinary function, and nothing in the `function!` block changes.
The addition is `ctx.hook()`, which is `Some` when the call came from a
lifecycle event and `None` when the function was called directly over HTTP:

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
    visibility: Private,   // hooks do not need an HTTP endpoint
    handler: post_before_create,
}
```

`ctx.hook()` is `None` when the function is called over HTTP instead. Handle
that case if a function serves both roles; it can be ignored for a `Private`
hook-only function, which is unreachable any other way.

A complete, runnable version, with five per-event hooks in one library wired
into a resource, is [`examples/08-hooks`](../examples/08-hooks):

```bash
cargo run -p apiplant -- build examples/08-hooks
cargo run -p apiplant -- run examples/08-hooks
```

It logs all five at boot and enforces them on `/api/post`: titles are trimmed
and validated, other members' drafts are hidden from listings, and published
posts cannot be deleted without `?force=1`.

The handler's `Input` is the payload from the table above, so
`serde_json::Value` is the most flexible choice. A typed struct works whenever
the payload's shape is known: `Input` for `before_create`, your row type for
`after_read`, or `Vec<Row>` for `after_list`.

### What a hook returns

The `Ok` value is a small instruction object. Build it with the `reply` helpers:

| Helper | JSON | Effect |
|--------|------|--------|
| `reply::proceed()` | `{}` | Continue unchanged. |
| `reply::replace(value)` | `{"data": …}` | Replace the payload, answer a read without querying, or replace the response body (see the table above). |
| `reply::abort(status, msg)` | `{"error": {"status": …, "message": …}}` | Stop the request with that status and a `{"error": msg}` body. |

Any other value (`null`, a bare string, or an object without those keys) also
means "continue unchanged", so an observational hook can return whatever is
convenient. Returning `Err(e)` from the handler aborts with a **400** and `e`'s
text. Statuses outside `400..=599` are clamped to `400`.

A `before_create` or `before_update` replacement must be a JSON **object**,
since it becomes the row's columns; replacements for `after_*` hooks may be any
JSON.

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
| `role` | `Option<String>` | The caller's *primary* role in that organisation. |
| `roles` | `Vec<String>` | Every role they hold there, which is what a `role:` permission is checked against. |
| `record_id` | `Option<String>` | The id in the URL, for read/update/delete. |
| `data` | `Option<Value>` | The submitted body, on `before_create` and `before_update`. |
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

Everything else on `Context` (`query`, `execute`, `config()` and the loggers)
works exactly as it does for an HTTP function. Hooks run on a blocking worker,
so these remain ordinary synchronous calls.

## Several hooks in one library

Each event gets its **own** function; there is no dispatcher and no matching on
the event name inside a handler. One library can export any number of them with
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
`Config`, `Input` and `Output` types, including its own `functions/<name>.toml`,
so `post_before_create` and `post_before_update` can be configured separately.
Names must be unique within a library.

Hooks are resolved by function name, so it makes no difference whether they come
from one library or several. If two events require identical behaviour, point
them at the same function and read `hook.event` to distinguish them.

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
compiled into the server and registered in the same function registry as the
app's `functions/` libraries, using the same host API, reply protocol and
`[hooks]` wiring. The differences are that it is always present, always
`private` with no HTTP endpoint, and named within the reserved **`apiplant_`**
namespace, so an application function cannot collide with one.

| Function | Wired to | What it does |
|----------|----------|--------------|
| `apiplant_organization_join` | `membership.before_create` | Resolves the user being added, either by `user_id` or by the identity field they registered with (`email` by default). Returns `422` if the body names neither, `404` if no account matches the identity, and `409` if they already belong to the organisation. |

Built-ins exist for work that must happen *behind* the API rather than in front
of it. `apiplant_organization_join` is the clearest case: `user` is read at
`member` level, so an admin adding a newcomer cannot resolve that address to an
id themselves. The server does it on their behalf, for that single purpose, and
returns nothing else about the account.

Declaring your own function with a built-in's exact name replaces it, which is
the way to keep the hook while changing its implementation. Since doing so
requires typing the reserved prefix, it cannot happen by accident, and the
server logs a warning when it does.

## Failure modes

| Situation | Result |
|-----------|--------|
| Hook names a function that is not loaded | **500**, and the operation does not run. Hooks fail **closed**, since silently skipping one would bypass validation. The server also logs this at boot. |
| Handler returns `Err(msg)` | **400** with `msg`. |
| Hook returns invalid JSON | **500**. |
| Hook panics | **500**, and the request is aborted, but the server keeps running. The panic message and backtrace are logged; the caller receives a generic message, since a panic often exposes internals. See [when a function panics](functions.md#when-a-function-panics). |
| Unknown key in `[hooks]` | The resource fails to load, so a typo like `befor_create` is caught at boot instead of silently never firing. |

Boot logs one line per resolved hook:

```
INFO apiplant_server:   hook post.before_create -> post_before_create
```

## Performance notes

* A resource with no `[hooks]` section costs nothing; no extra work happens on
  its endpoints.
* Each hook is one blocking call, so two hooks on one request means two calls.
* `before_delete` and `after_delete` add one `SELECT` to fetch the target row.
* Hooks run **outside** the operation's transaction. An `after_create` hook that
  fails turns the response into an error, but the row stays written.

## Hooks or queues?

A hook runs *inside* the request. That is exactly what you want when the work
must happen before the response, or is allowed to reject it — validation,
rewriting a body, resolving an id. It is exactly what you do not want for work
the caller has no stake in: an `after_create` hook that emails a receipt makes
the receipt provider's outage into a failed signup.

For those, publish a message instead. A resource can even do it with no function
at all:

```toml
[publish]
after_create = "user.signed_up"
```

The caller's response goes out as soon as the row is committed; the handler runs
afterwards, retries on its own, and cannot fail the write. See
[Queues](queues.md).

## See also

* [Queues](queues.md): the same "and then…", but after the response and with
  retries.
* [Functions](functions.md): writing, building and configuring the functions
  hooks point at.
* [Permissions](permissions.md): the checks that run before any hook.
* [Resources](resources.md): the rest of `resources/<name>.toml`.
