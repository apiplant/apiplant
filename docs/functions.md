# Functions (compiled plugins)

A **function** is custom logic compiled into a shared library
(`.so`/`.dylib`/`.dll`) in the app's `functions/` directory. The framework loads
it at boot and mounts it as an HTTP endpoint. Functions talk to the host across a
stable C ABI (via [`abi_stable`]), so they can be shipped independently and never
require recompiling the server.

You write the **source**; `apiplant build` produces the library:

```
my-app/functions/
├── greet.rs          # source: one file, written as if it were a lib.rs
├── greet.toml        # optional per-deployment config for the `greet` function
├── libgreet.so       # ← produced by `apiplant build`
└── libhooks.so       # another library; every .so here is loaded
```

A directory can hold any number of libraries, and each library can export any
number of functions.

## Building

```bash
apiplant build my-app          # compile every functions/*.rs and functions/*.c
apiplant build my-app --release
apiplant build my-app --force  # ignore up-to-date libraries
apiplant run my-app --build    # build, then serve
```

Each Rust source is wrapped in a generated cdylib crate under
`my-app/.apiplant-build/`, compiled with cargo, and the resulting library copied
next to the source. Only sources newer than their library are rebuilt, and one
shared `target/` keeps dependencies compiled once. Serving an app whose source is
newer than its library logs a warning.

`cargo` on `PATH` is the only requirement for Rust functions. Every generated
crate depends on `apiplant-function`, `abi_stable`, `serde`, `serde_json` and
`schemars`; the `apiplant-function` path is taken from the checkout the binary was
built from, or from `APIPLANT_FUNCTION_CRATE` if you set it.

C sources need a `cc` on `PATH` instead and are compiled straight to a shared
library, with no scaffolding — see
[writing a function in C](#writing-a-function-in-c-zig-or-go).

### Library size

Each library statically links its own copy of `std`, `serde_json` and
`schemars` — that self-containment is what makes the ABI stable across
compilers, so it is a feature rather than bloat, and the libraries cannot share
the host's copies. What *is* avoidable is debug info and dead code, so the
generated manifest carries its own profiles: `dev` drops DWARF but keeps the
symbol table for legible backtraces, and `--release` strips fully and applies
fat LTO over a single codegen unit so the linker can discard everything the
function never reaches.

A one-page function comes out around **2.3 MB** in `dev` and **600 KB** with
`--release`, of which ~1 KB is the function's own code. Ship `--release`
libraries.

A C function links nothing but libc and comes out around **21 KB**.

If you'd rather manage the crate yourself, you can — see
[Cargo setup](#cargo-setup) below. `apiplant build` just automates it.

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
[`examples/07-functions`](../examples/07-functions).

### Several functions in one library

A library isn't limited to one function. `functions!` takes a braced entry per
function — each with its own name, manifest and handler, and its own inferred
types and config file:

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

`function!` is exactly this macro with a single entry, so use whichever reads
better. Names must be unique within a library; the host rejects a library that
exports two functions with the same name, and a name that collides across
libraries is resolved last-loaded-wins.

This is the usual way to ship a resource's [lifecycle hooks](hooks.md): one
handler per event, no dispatcher. See
[`examples/08-hooks`](../examples/08-hooks).

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

Only needed if you build the library yourself instead of using
[`apiplant build`](#building) — which generates exactly this:

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
| `hook()` | `Option<&Hook>` | The lifecycle context when called as a [hook](hooks.md); `None` over HTTP. |
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

Visibility governs the **HTTP endpoint** only. A function attached to a
resource's lifecycle still runs whatever its visibility — which is why hook
functions are usually `Private`.

## Running as a lifecycle hook

The same function can be wired into a resource's CRUD lifecycle from
`models/<name>.toml`:

```toml
[hooks]
before_create = "post_before_create"
after_create  = "post_after_create"
```

When invoked that way, `ctx.hook()` carries the operation's context — the event,
the row created or fetched, the rows a list returned, the request URL, the
caller's auth status — and the handler's return value tells the host what to do
next (continue, replace the payload, or reject the request):

```rust
fn post_before_create(ctx: &Context<()>, mut input: serde_json::Value) -> Result<serde_json::Value, String> {
    let Some(hook) = ctx.hook() else { return Ok(reply::proceed()) };
    ctx.info(&format!("{} on {}", hook.event, hook.resource));
    if input["title"].as_str().unwrap_or_default().is_empty() {
        return Ok(reply::abort(422, "title is required"));
    }
    input["title"] = serde_json::json!(input["title"].as_str().unwrap().to_uppercase());
    Ok(reply::replace(input))
}
```

See [Lifecycle hooks](hooks.md) for the events, the payload each one receives,
and the full reply protocol.

## Configuration

A function named `greet` reads `functions/greet.toml` if present; the framework
converts it to JSON and the macro deserializes it into your `Config`:

```toml
# functions/greet.toml
greeting = "Bonjour"
```

Then `ctx.config().greeting` is `"Bonjour"`. Missing file ⇒ `Config::default()`.

Config is per **function**, not per library: a library exporting
`post_before_create` and `post_before_update` reads
`functions/post_before_create.toml` and `functions/post_before_update.toml`, so
the two can be tuned independently even when they share a `Config` type.

## Deploying

```bash
apiplant build my-app --release     # or build the cdylib yourself and copy it in
```

On boot the server logs every function each library provides, and its route:

```
INFO apiplant_server:   fn greet -> /api/functions/greet
```

A library that fails to load is logged and skipped — it never stops the server.
That includes a library exporting no functions at all, or two with the same name.

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

## When a function panics

A panic in your handler fails **that request only** — `500 internal function
error` — and the server keeps serving. The panic message and backtrace go to the
host's log; the caller never sees them, since they tend to name internals.

This is worth stating explicitly because it is not what an FFI boundary does by
default. `Function::invoke` is reached through an `extern "C"` function pointer,
and a panic escaping one of those aborts the entire process — every in-flight
request on every connection. So `apiplant-function` catches unwinding panics
while still on the function's side of the boundary and converts them into an
error the host reports as a `500`.

Two consequences:

* **Don't set `panic = "abort"`** in a function's profile. It would trade the
  containment above for a server that dies on `unwrap()`. `apiplant build` leaves
  the panic strategy alone for this reason.
* **A function written against the raw ABI must do the same** — catch its own
  faults and return them, rather than letting them unwind or `longjmp` out. In C
  that means returning `APIPLANT_ERR_INTERNAL`; the marker the host looks for is
  `apiplant_abi::INTERNAL_ERROR_PREFIX`.

An error your handler *returns* is a different thing: `Err(e)` is a `400` with
`e`'s text, because it describes what was wrong with the request.

## Writing a function in C, Zig or Go

A function is just a shared library, so it need not be Rust. The `abi_stable`
contract cannot reasonably be hand-written outside Rust, so a library may instead
export four plain C symbols; the host tries the Rust root module first and falls
back to these. Either way it becomes the same `Function` object internally, so a
C function is mounted, access-controlled, documented and usable as a lifecycle
hook exactly like a Rust one.

Include [`apiplant.h`](../crates/apiplant-abi/include/apiplant.h) and implement:

```c
uint32_t     apiplant_abi_version(void);  /* return APIPLANT_ABI_VERSION */
const char  *apiplant_manifest(void);     /* static JSON array of manifests */
int32_t      apiplant_invoke(const char *name, const char *input_json,
                             const ApiplantHost *host, char **out);
void         apiplant_free(char *string); /* free what apiplant_invoke produced */
```

`apiplant build` compiles every `.c` file in `functions/` with `cc`, so C
functions use the same workflow as Rust ones:

```bash
apiplant build my-app            # .rs via cargo, .c via cc
apiplant build my-app --release
```

`CC` and `CFLAGS` are honoured if you need a different compiler, extra includes
or a JSON library. For Zig or Go, build the shared library yourself — `zig
build-lib -dynamic`, or `go build -buildmode=c-shared` — and drop it in
`functions/`; only `.c` is compiled automatically.

### The manifest

`apiplant_manifest` returns a static JSON array — one object per function, and
what the `function!` macro generates on the Rust side:

```json
[{ "name": "hello", "version": "1.0.0", "description": "Greets someone.",
   "visibility": "public", "method": "POST",
   "input_schema": { "type": "object" }, "output_schema": { "type": "object" } }]
```

Only `name` is required. `visibility` takes the same strings as a resource's
permissions (`"public"`, `"authenticated"`, `"role:admin"`, `"private"`) and
defaults to `"private"` — the safe direction, so a typo hides an endpoint instead
of exposing it. `method` defaults to `"POST"`. The schema fields are optional,
may be an object or a pre-serialised string, and only feed the generated docs.

### Status codes and memory

| Return | Meaning | Response |
|---|---|---|
| `APIPLANT_OK` | `*out` is the JSON response body | `200` |
| `APIPLANT_ERR_REQUEST` | `*out` is a message for the caller | `400` |
| `APIPLANT_ERR_INTERNAL` | `*out` is a message for the log | `500`, withheld |

Each side frees what it allocated, since the two do not share an allocator:

* the string you write to `*out` comes back to your `apiplant_free`;
* strings the host hands you — `config`, `query`, `principal_id`, `hook` — go
  back to `host->free_string`.

`host->query` takes the same `{"sql": …, "params": […]}` request as the Rust
side and answers with a JSON array of rows, `{"rows_affected": n}`, or
`{"error": …}` when the query failed.

See [`examples/09-c-functions`](../examples/09-c-functions) for a working app —
two endpoints in one `.c` file, one of them querying Postgres. It compiles to
about 21 KB, against 600 KB for the Rust equivalent.

## Without the macro (the raw ABI)

The macro is optional sugar over the [`apiplant-abi`](../crates/apiplant-abi)
contract. If you need full control — or you're writing a function in another
language — you can implement the `Function` trait and export the root module
yourself. The macro expands to exactly that: an `#[export_root_module]`
constructor whose `new_functions` returns one `Function` trait object per
exported function, each delegating to `apiplant_function::invoke_handler`. See
the crate docs for the trait definitions.

[`abi_stable`]: https://docs.rs/abi_stable
