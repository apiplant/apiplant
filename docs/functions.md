# Functions (compiled plugins)

A **function** is custom logic compiled into a shared library
(`.so`/`.dylib`/`.dll`) and dropped into the app's `functions/` directory. The
framework loads it at boot and mounts it as an HTTP endpoint. Functions talk to
the host across a stable C ABI (via [`abi_stable`]), so they can be written in
any language, shipped independently, and never require recompiling the server.

```
my-app/functions/
├── libgreet.so       # the compiled library
└── greet.toml        # optional per-deployment config for the `greet` function
```

## What a function looks like

Both the host and every function depend only on the tiny `apiplant-abi` crate.
A function exports one root module whose constructor returns a `Function`:

```rust
use abi_stable::{export_root_module, prefix_type::PrefixTypeTrait, sabi_extern_fn,
    sabi_trait::TD_Opaque, std_types::{RResult, RStr, RString}};
use apiplant_abi::*;

#[export_root_module]
fn init() -> FunctionMod_Ref {
    FunctionMod { new }.leak_into_prefix()
}

#[sabi_extern_fn]
fn new() -> BoxedFunction { Function_TO::from_value(Greet, TD_Opaque) }

struct Greet;

impl Function for Greet {
    fn manifest(&self) -> FunctionManifest {
        FunctionManifest {
            name: "greet".into(),
            version: env!("CARGO_PKG_VERSION").into(),
            description: "Greets a person.".into(),
            visibility: Visibility::Public,
            role: RString::new(),          // required role when RoleGated
            method: HttpMethod::Post,
            config_schema: RString::new(), // optional JSON-Schema for config
        }
    }

    fn invoke(&self, host: HostApi_TO<'_, abi_stable::std_types::RBox<()>>, input: RStr<'_>)
        -> RResult<RString, RString>
    {
        let cfg = host.config();                      // functions/greet.toml as JSON
        // host.query(r#"{"sql":"…","params":[…]}"#)  // borrow the host database
        // host.log(LogLevel::Info, "…".into());
        // host.principal_id()                         // caller's user id, or ""
        RResult::ROk(r#"{"message":"hi"}"#.into())
    }
}
```

`crate-type = ["cdylib"]` in the function crate's `Cargo.toml`. See a complete,
working example in [`examples/function-greet`](../examples/function-greet).

## The manifest

Read once at load time; it decides how the endpoint is mounted.

| Field | Meaning |
|-------|---------|
| `name` | URL segment ⇒ `<base>/functions/<name>`. |
| `version` | The function's own semver (independent of the ABI version). |
| `description` | Shown in the generated OpenAPI docs. |
| `visibility` | Access policy for the endpoint (below). |
| `role` | Required role name when `visibility = RoleGated`. |
| `method` | HTTP method the endpoint answers (`Get`/`Post`/`Put`/`Delete`). |
| `config_schema` | Optional JSON-Schema describing the config object. |

### Visibility

| Value | Who can call it |
|-------|-----------------|
| `Public` | anyone |
| `Authenticated` | any authenticated caller |
| `RoleGated` | caller holding `manifest.role` |
| `Private` | not exposed over HTTP (omitted from routing and docs) |

Mirrors the resource [permission model](permissions.md).

## The host API

During `invoke`, the function is handed a `HostApi` giving it exactly what it
needs — nothing more:

| Method | Does |
|--------|------|
| `query(request)` | Run SQL against the host DB. `request` is `{"sql": "...", "params": [...]}`; a `SELECT`/`WITH` returns a JSON array of rows, anything else returns `{"rows_affected": n}`. |
| `log(level, msg)` | Emit through the host's `tracing` subscriber. |
| `config()` | The function's resolved config as JSON (see below). |
| `principal_id()` | The authenticated caller's user id, or `""` when anonymous. |

Everything crosses the boundary as JSON strings or small `#[repr(C)]` enums — the
host never shares a sea-orm, ntex or tokio type with the plugin, which is what
keeps the ABI stable across compiler and allocator versions.

Functions run on a blocking worker, so `host.query(...)` is a normal synchronous
call — you never touch `async`.

## Configuration

A function named `greet` reads `functions/greet.toml` (if present), which the
framework converts to JSON and returns from `host.config()`:

```toml
# functions/greet.toml
greeting = "Bonjour"
```

```rust
#[derive(serde::Deserialize)]
struct Config { greeting: String }
let cfg: Config = serde_json::from_str(host.config().as_str())?;
```

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
# → {"message":"Bonjour, World!"}
```

* The request body is passed to `invoke` as the `input` string (empty ⇒ `{}`).
* The manifest's `method` is enforced (`405` on a mismatch).
* Visibility is enforced (`401`/`403`/`404` as appropriate).
* The returned JSON is sent back verbatim; an `RErr` becomes a `400`.

[`abi_stable`]: https://docs.rs/abi_stable
