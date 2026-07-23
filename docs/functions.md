# Functions (compiled plugins)

A **function** is custom logic compiled into a shared library
(`.so`/`.dylib`/`.dll`) and dropped into the app's `functions/` directory. The
framework loads it at boot and mounts it as an HTTP endpoint. Functions talk to
the host across a stable C ABI (via [`abi_stable`]), so they can be shipped
independently and never require recompiling the server.

```
my-app/functions/
├── libgreet.so       # the compiled library
└── greet.toml        # optional per-deployment config for the `greet` function
```

## Writing a function

Use the [`apiplant-function`](../crates/apiplant-function) crate. You write **one
plain typed Rust function** plus a short `function!` block — no ABI traits, no
root-module export, no manual JSON or `RString` handling. Types are inferred from
your handler's signature, so you never name them twice.

```rust
use apiplant_function::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Deserialize, Default)]
struct Config {
    #[serde(default = "default_greeting")]
    greeting: String,
}
fn default_greeting() -> String { "Hello".into() }

#[derive(Deserialize, JsonSchema)]
struct Input { name: String }

#[derive(Serialize, JsonSchema)]
struct Output { message: String, registered_users: i64 }

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
```

That's the whole library. The macro generates the root module, reads/writes JSON,
resolves your typed config and input, and turns any `Err(_)` into a `400`.

A complete, runnable version is in
[`examples/function-greet`](../examples/function-greet).

### The handler signature

```rust
fn my_fn(ctx: &Context<Config>, input: Input) -> Result<Output, Error>
```

| Part | Requirement | Notes |
|------|-------------|-------|
| `Config` | `Deserialize + Default` | Parsed from `functions/<name>.toml`; `Default` used when absent/invalid. Use `Context<()>` if you have no config. |
| `Input` | `Deserialize` (+ `JsonSchema`) | The request body. Derive `JsonSchema` to type it in the OpenAPI docs. |
| `Output` | `Serialize` (+ `JsonSchema`) | The response body. Derive `JsonSchema` to type it in the OpenAPI docs. |
| `Error` | `Display` | Any displayable error; becomes a `400` with its message. `String` works. |

### Typed OpenAPI

Deriving `JsonSchema` (re-exported by the prelude) on `Input`/`Output` makes the
`function!` macro emit their JSON Schemas into the manifest. The framework
registers them as components (`Fn<Name>Input` / `Fn<Name>Output`) and references
them from the function's path, so **Swagger UI renders a typed form and typed
response** — doc comments on fields become schema descriptions. This needs the
`schema` feature (on by default) and a `schemars` dependency. Without them (or if
you skip the derives), function bodies fall back to an untyped `object`.

### The `function!` block

| Field | Required | Meaning |
|-------|----------|---------|
| `name` | yes | URL segment ⇒ `<base>/functions/<name>`. |
| `description` | yes | Shown in the generated OpenAPI docs. |
| `method` | yes | `Get` \| `Post` \| `Put` \| `Delete`. |
| `visibility` | yes | `Public` \| `Authenticated` \| `RoleGated` \| `Private`. |
| `handler` | yes | The function above. |
| `version` | no | Defaults to the crate's `CARGO_PKG_VERSION`. |
| `role` | no | Required role name when `visibility: RoleGated`. |

### Cargo setup

```toml
[lib]
crate-type = ["cdylib"]

[dependencies]
apiplant-function = { path = "…/crates/apiplant-function" }  # or version
abi_stable = "0.11"   # only for the exported glue; you never reference it
serde = { version = "1", features = ["derive"] }
serde_json = "1"
schemars = "0.8"      # for typed OpenAPI (#[derive(JsonSchema)]); optional
```

To drop `schemars`, disable the crate's default features
(`apiplant-function = { …, default-features = false }`) and remove the
`JsonSchema` derives; function bodies then show as untyped objects.

## The `Context` API

During a call your handler gets a `&Context<Config>`:

| Method | Returns | Purpose |
|--------|---------|---------|
| `config()` | `&Config` | Your typed configuration. |
| `principal_id()` | `&str` | The caller's user id, or `""` when anonymous. |
| `query(sql, params)` | `Result<Vec<Value>, String>` | Run a `SELECT`/`WITH`; rows as JSON objects. |
| `query_one(sql, params)` | `Result<Option<Value>, String>` | First row, if any. |
| `execute(sql, params)` | `Result<u64, String>` | Run a write; rows affected. |
| `info` / `warn` / `error` / `debug(msg)` | `()` | Log through the host's `tracing`. |
| `log(level, msg)` | `()` | Log at an explicit level. |

`params` is `&[serde_json::Value]`, bound as `$1, $2, …` in the SQL. Because a
function runs on a blocking worker, these are ordinary synchronous calls — you
never touch `async`.

```rust
let rows = ctx.query(
    "SELECT id, title FROM apiplant_post WHERE owner_id = $1",
    &[serde_json::json!(ctx.principal_id())],
)?;
```

## Visibility

Mirrors the resource [permission model](permissions.md). The framework enforces
it before your handler runs:

| Value | Who can call it | On mismatch |
|-------|-----------------|-------------|
| `Public` | anyone | — |
| `Authenticated` | any authenticated caller | `401` |
| `RoleGated` | caller holding `role` | `403` |
| `Private` | nobody over HTTP (hidden from routing & docs) | `404` |

## Configuration

A function named `greet` reads `functions/greet.toml` if present; the framework
converts it to JSON and the macro deserializes it into your `Config`:

```toml
# functions/greet.toml
greeting = "Bonjour"
```

Then `ctx.config().greeting` is `"Bonjour"`. Missing file ⇒ `Config::default()`.

## Building & deploying

```bash
cargo build -p my-function --release
cp target/release/libmy_function.so my-app/functions/
```

On boot the server logs each loaded function and its route:

```
INFO apiplant_server:   fn greet -> /api/functions/greet
```

A library that fails to load is logged and skipped — it never stops the server.

## Calling a function

```bash
curl -XPOST http://localhost:8099/api/functions/greet \
  -H 'content-type: application/json' \
  -d '{"name":"World"}'
# → {"message":"Bonjour, World!","registered_users":1}
```

* The request body becomes `Input` (an empty body is treated as `{}`).
* The manifest's `method` is enforced (`405` on a mismatch).
* Visibility is enforced (`401`/`403`/`404`).
* `Ok(output)` is serialized to JSON; `Err(e)` becomes a `400` with `e`'s text.

## Without the macro (the raw ABI)

The macro is optional sugar over the [`apiplant-abi`](../crates/apiplant-abi)
contract. If you need full control — or you're writing a function in another
language — you can implement the `Function` trait and export the root module
yourself. The macro expands to exactly that: a `Function` impl whose `invoke`
calls `apiplant_function::invoke_handler`, plus an `#[export_root_module]`
constructor. See the crate docs for the trait definitions.

[`abi_stable`]: https://docs.rs/abi_stable
