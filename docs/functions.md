# Functions (compiled plugins)

A **function** is custom logic in the app's `functions/` directory that the
framework loads at boot and mounts as an HTTP endpoint. Usually that means a
shared library (`.so`/`.dylib`/`.dll`) talking to the host across a stable C ABI
(via [`abi_stable`]), so it can be shipped independently and never requires
recompiling the server. [TypeScript functions](#writing-a-function-in-typescript)
are the exception: there is nothing to link, so they run in a V8 isolate
instead. Everything around them (manifest, config, permissions, docs and hooks)
is identical.

You write the **source**, and `apiplant build` produces the library:

```
my-app/functions/
├── greet.rs          # source: one file, written as if it were a lib.rs
├── greet.toml        # optional per-deployment config for the `greet` function
├── libgreet.so       # produced by `apiplant build`
└── libhooks.so       # another library; every .so here is loaded
```

A directory can hold any number of libraries, and each library can export any
number of functions.

## A function can be a file or a directory

The `greet.rs` above is a single file, which suffices until a function needs a
third-party crate, a second source file, or a linked library. For those cases an
entry in `functions/` may be a **directory**: a self-contained project in the
language's own native form.

```
my-app/functions/
├── greet/            # a directory is one function library
│   ├── Cargo.toml    #   here a full crate, with any dependencies
│   └── src/lib.rs
├── greet.toml        # config sits beside it, keyed by the directory name
└── libgreet.so       # produced next to the directory, loaded the same way
```

apiplant reads the language from what the directory holds, and builds it its
native way:

| A directory containing… | is built as | with |
|-------------------------|-------------|-------|
| `Cargo.toml`            | Rust | `cargo build` on *your* crate, with any dependencies and modules |
| `go.mod`                | Go   | `go build -buildmode=c-shared` on your module |
| `.c` files              | C    | `cc` over every `.c` in the directory, with it on the include path |
| `.zig` files            | Zig  | `zig build-lib` from a root `.zig` (named for the directory) that may `@import` the rest |
| `package.json`          | TypeScript | your `build` script — npm dependencies, your own bundler |

A directory named `greet/` compiles to `libgreet.so` beside it, so the host loads
it exactly like a single-file function. For **Rust and Go the project is yours**:
apiplant runs your `Cargo.toml` or `go.mod` unchanged and copies out the library
it produces, so you control both the dependencies and the build profiles. A
directory does not receive the injected [size-reducing profiles](#library-size),
so set them yourself. For **C and Zig**, a directory extends the single-file
build to every source it contains, so its own headers and `@import`ed modules
resolve.

A `greet/` directory and a `greet.rs` file would both produce `libgreet.so`, so
apiplant rejects that collision; use one form per name. See
[`examples/12-function-dependencies`](../examples/12-function-dependencies) for
one directory per language, each using something a single file cannot.

## Building

```bash
apiplant build my-app          # recompile every source in functions/
apiplant build my-app --release
apiplant run my-app --build    # build what is out of date, then serve
```

`apiplant build` always rebuilds, since an explicit build request implies fresh
libraries, and timestamp comparison misses changes made by a build script or a
dependency. `--force` is still accepted and has no effect. Only the implicit
build behind `run --build` skips sources whose library is newer than the
source.

Each Rust source is wrapped in a generated cdylib crate under
`my-app/.apiplant-build/`, compiled with cargo, and the resulting library copied
next to the source. Only sources newer than their library are rebuilt, and one
shared `target/` keeps dependencies compiled once. Serving an app whose source is
newer than its library logs a warning.

`cargo` on `PATH` is the only requirement for Rust functions. Every generated
crate depends on `apiplant-function`, `abi_stable`, `serde`, `serde_json` and
`schemars`. `apiplant-function` comes from `APIPLANT_FUNCTION_CRATE` if you set
it, else from the checkout the binary was built from, else from crates.io at the
binary's own version, so an installed `apiplant` builds without a checkout
present.

C, Zig and Go sources require their own toolchain on `PATH`; see
[writing a function in C, Zig or Go](#writing-a-function-in-c-zig-or-go).
TypeScript requires nothing, as `apiplant build` transpiles it in-process; see
[writing a function in TypeScript](#writing-a-function-in-typescript).

### Library size

Each library statically links its own copy of `std`, `serde_json` and
`schemars`. That self-containment is what keeps the ABI stable across compilers,
and libraries cannot share the host's copies. Debug info and dead code are
avoidable, so the generated manifest carries its own profiles: `dev` drops DWARF
while keeping the symbol table for legible backtraces, and `--release` strips
fully and applies fat LTO over a single codegen unit so the linker can discard
everything the function does not reach.

A small function comes out at roughly **2.3 MB** in `dev` and **600 KB** with
`--release`, of which about 1 KB is the function's own code. Ship `--release`
libraries.

A C function links only libc and comes out at roughly **16 KB**; Zig is around
288 KB and Go around 2.2 MB. See [sizes](#sizes).

You can manage the crate yourself instead; see [Cargo setup](#cargo-setup)
below. `apiplant build` automates it.

## Writing a function

Use the [`apiplant-function`](../crates/apiplant-function) crate. You write **one
plain typed Rust function** plus a short `function!` block: no ABI traits, no
root-module export, and no manual JSON or `RString` handling. Types are inferred
from the handler's signature, so they are never named twice.

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
    permission: "public",
    handler: greet,
}
```

That is the complete library. The macro generates the root module, reads and
writes JSON, resolves the typed config and input, and converts any `Err(_)` into
a `400`.

A complete, runnable version is in
[`examples/07-functions`](../examples/07-functions).

### Several functions in one library

A library is not limited to one function. `functions!` takes a braced entry per
function, each with its own name, manifest and handler, and its own inferred
types and config file:

```rust
apiplant_function::functions! {
    {
        name: "post_before_create",
        description: "Validates a post before it is stored.",
        method: Post,
        permission: "private",
        handler: post_before_create,
    },
    {
        name: "post_after_create",
        description: "Records a newly created post.",
        method: Post,
        permission: "private",
        handler: post_after_create,
    },
}
```

`function!` is this macro with a single entry, so use whichever is clearer. Names
must be unique within a library; the host rejects a library exporting two
functions with the same name, and a name colliding across libraries resolves to
the last loaded.

This is the usual way to ship a resource's [lifecycle hooks](hooks.md): one
handler per event, with no dispatcher. See
[`examples/08-hooks`](../examples/08-hooks).

### The handler signature

```rust
fn my_fn(ctx: &Context<Config>, input: Input) -> Result<Output, Error>
```

| Part | Requirement | Notes |
|------|-------------|-------|
| `Config` | `Deserialize + Default` | Parsed from `functions/<name>.toml`; `Default` is used when the file is absent or invalid. Use `Context<()>` when there is no config. |
| `Input` | `Deserialize` (+ `JsonSchema`) | The request body. Derive `JsonSchema` to type it in the OpenAPI docs. |
| `Output` | `Serialize` (+ `JsonSchema`) | The response body. Derive `JsonSchema` to type it in the OpenAPI docs. |
| `Error` | `Display` | Any displayable error. It becomes a `400` with its message; `String` is sufficient. |

### Typed OpenAPI

Deriving `JsonSchema`, re-exported by the prelude, on `Input` and `Output` makes
the `function!` macro emit their JSON Schemas into the manifest. The framework
registers them as components (`Fn<Name>Input` and `Fn<Name>Output`) and
references them from the function's path, so **Swagger UI renders a typed form
and typed response**, with doc comments on fields becoming schema descriptions.
This requires the `schema` feature, enabled by default, and a `schemars`
dependency. Without them, or without the derives, function bodies fall back to
an untyped `object`.

### The `function!` block

| Field | Required | Meaning |
|-------|----------|---------|
| `name` | yes | URL segment, giving `<base>/functions/<name>`. |
| `description` | yes | Shown in the generated OpenAPI docs. |
| `method` | yes | `Get` \| `Post` \| `Put` \| `Delete`. |
| `handler` | yes | The function above. |
| `permission` | no* | Access policy: `"public"`, `"authenticated"`, `"member"`, `"role:<name>"`, `"private"`. |
| `visibility` | no* | The older form: `Public` \| `Authenticated` \| `RoleGated` \| `Private`. |
| `role` | no | Required role name when `visibility: RoleGated`. |
| `admin` | no | How the [dashboard](admin.md#actions) presents it. |
| `version` | no | Defaults to the crate's `CARGO_PKG_VERSION`. |

\* Give `permission` **or** `visibility`, not both. Omitting both makes the
function private, which is the safe default: an omitted line hides an endpoint
rather than publishing one.

### Cargo setup

This is only needed when building the library yourself instead of using
[`apiplant build`](#building), which generates exactly this:

```toml
[lib]
crate-type = ["cdylib"]

[dependencies]
apiplant-function = { path = "…/crates/apiplant-function" }  # or version
abi_stable = "0.11"   # used only by the generated glue; never referenced directly
serde = { version = "1", features = ["derive"] }
serde_json = "1"
schemars = "0.8"      # for typed OpenAPI (#[derive(JsonSchema)]); optional
```

To drop `schemars`, disable the crate's default features
(`apiplant-function = { …, default-features = false }`) and remove the
`JsonSchema` derives; function bodies then appear as untyped objects.

## The `Context` API

During a call the handler receives a `&Context<Config>`:

| Method | Returns | Purpose |
|--------|---------|---------|
| `config()` | `&Config` | The typed configuration. |
| `principal_id()` | `&str` | The caller's user id, or `""` when anonymous. |
| `hook()` | `Option<&Hook>` | The lifecycle context when called as a [hook](hooks.md); `None` over HTTP. |
| `query(sql, params)` | `Result<Vec<Value>, String>` | Run a `SELECT`/`WITH`; rows as JSON objects. |
| `query_one(sql, params)` | `Result<Option<Value>, String>` | First row, if any. |
| `execute(sql, params)` | `Result<u64, String>` | Run a write; rows affected. |
| `send_email(email)` | `Result<Sent, String>` | Send mail through the configured [`[email]` provider](email.md). |
| `cache_get(key)` | `Result<Option<Value>, String>` | Read from the [`[cache]` Redis](caching.md); `None` is a miss. |
| `cache_get_as::<T>(key)` | `Result<Option<T>, String>` | The same, deserialized; a value of the wrong shape is a miss. |
| `cache_set(key, &value, ttl)` | `Result<(), String>` | Write. `None` ttl uses `default_ttl_secs`; `Some(0)` persists. |
| `cache_delete(key)` | `Result<bool, String>` | Remove; `true` when it was there. |
| `cache_incr(key, by, ttl)` | `Result<i64, String>` | Atomic counter, correct across workers and hosts. |
| `cache_ttl(key)` | `Result<Option<i64>, String>` | Seconds left, or `None`. |
| `checkout(price, recurring, org)` | `Result<String, String>` | Start a purchase through the [`[payments]` provider](payments.md); returns the URL to send the buyer to. |
| `billing_portal(customer)` | `Result<String, String>` | A link to the provider's self-service billing screens. |
| `subscription(id)` | `Result<Value, String>` | Ask the *provider* about a subscription. |
| `cancel_subscription(id, at_period_end)` | `Result<Value, String>` | End one, immediately or at the end of the paid period. |
| `payments(request)` | `Result<Value, String>` | Any operation the four above do not cover. |
| `chat(request)` | `Result<ChatReply, String>` | Call the [`[ai]` assistant](ai.md) and wait for the complete answer. |
| `ask(prompt)` | `Result<String, String>` | The same, returning only the answer text. |
| `chat_streaming(request)` | `Result<ChatReply, String>` | The same, forwarding every token to your own caller as it arrives. |
| `emit(chunk)` | `bool` | Send a chunk of the response immediately. `false` means the caller disconnected and work should stop. |
| `info` / `warn` / `error` / `debug(msg)` | `()` | Log through the host's `tracing`. |
| `log(level, msg)` | `()` | Log at an explicit level. |

`params` is `&[serde_json::Value]`, bound as `$1, $2, …` in the SQL. Because a
function runs on a blocking worker, these are ordinary synchronous calls with no
`async` involved.

```rust
let rows = ctx.query(
    "SELECT id, title FROM apiplant_post WHERE owner_id = $1",
    &[serde_json::json!(ctx.principal_id())],
)?;
```

`send_email`, the `cache_*` methods, the billing methods and `chat` return errors
when the app has configured no email provider, no cache, no payments provider
and no assistant respectively, so a function using one requires an app whose
`main.toml` configures it. All four are covered in full in
[Sending email](email.md), [Caching](caching.md), [Payments](payments.md) and
[AI](ai.md).

The billing methods call the *provider* over the network. Checking whether an
organisation is subscribed is an ordinary `query` against
`billing_subscription`, which the webhook keeps current and which costs no round
trip. Use the billing methods to perform an action.

## Permissions

A function's access uses the same string grammar as a resource's
[`[permissions]`](permissions.md), so an app has one access vocabulary rather
than two. The framework enforces it before your handler runs:

| Value | Who can call it | On mismatch |
|-------|-----------------|-------------|
| `"public"` | anyone | none |
| `"authenticated"` | any authenticated caller | `401` |
| `"member"` | any member of the caller's active organisation | `401` / `403` |
| `"role:<name>"` | a member holding that role in the active organisation | `401` / `403` |
| `"private"` | nobody over HTTP; hidden from routing and docs | `404` |

`owner` is the one level with no meaning here, since a function call has no row
to own.

```rust
apiplant_function::function! {
    name: "sales_summary",
    description: "Orders and revenue over a recent window.",
    method: Post,
    permission: "member",
    handler: sales_summary,
}
```

A `private` function returns `404` rather than `403`, the same response an
unknown name receives, so probing cannot enumerate what exists.

This governs the **HTTP endpoint** only. A function attached to a resource's
lifecycle still runs regardless of its permission, which is why hook functions
are usually `"private"`.

### The older `visibility` field

`permission` supersedes the original `visibility` + `role` pair, which still
works and still means what it always did:

| `visibility` | equivalent `permission` |
|---|---|
| `Public` | `"public"` |
| `Authenticated` | `"authenticated"` |
| `RoleGated` + `role: "admin"` | `"role:admin"` |
| `Private` | `"private"` |

Give one form or the other, not both. The reason to prefer `permission` is
`member`, meaning anyone in the active organisation, which is the level most
operator-facing actions require and which `Visibility` has no variant for. A
library compiled before `permission` existed retains exactly the access its
`visibility` granted.

An unrecognised `permission` string falls back to `private`, matching how the
rest of apiplant treats an invalid access string: a mistake hides an endpoint
rather than exposing one.

## Running as a lifecycle hook

The same function can be wired into a resource's CRUD lifecycle from
`models/<name>.toml`:

```toml
[hooks]
before_create = "post_before_create"
after_create  = "post_after_create"
```

When invoked that way, `ctx.hook()` carries the operation's context: the event,
the row created or fetched, the rows a list returned, the request URL and the
caller's auth status. The handler's return value tells the host what to do next:
continue, replace the payload, answer a read without querying, or reject the
request.

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

## Appearing in the dashboard

A function whose permission is not `private` becomes a runnable **action** in
the generated [admin dashboard](admin.md), with a form derived from its `Input`
type. An optional `admin` block controls how it is presented: its label, its
grouping, and whether running it requires confirmation.

```rust
apiplant_function::function! {
    name: "reindex_catalogue",
    description: "Rebuilds the product search index.",
    method: Post,
    permission: "role:admin",
    admin: {
        label: "Rebuild search index",
        group: "Maintenance",
        confirm: "Rebuild the index for every product?",
        run_label: "Rebuild index",
    },
    handler: reindex,
}
```

Functions bound to a resource's lifecycle never appear, since they are invoked by
the framework rather than by an operator. Full reference:
[Admin dashboard § Actions](admin.md#actions).

## Configuration

A function named `greet` reads `functions/greet.toml` when present. The framework
converts it to JSON and the macro deserializes it into the `Config` type:

```toml
# functions/greet.toml
greeting = "Bonjour"
```

`ctx.config().greeting` is then `"Bonjour"`. A missing file yields
`Config::default()`.

Config is per **function**, not per library: a library exporting
`post_before_create` and `post_before_update` reads
`functions/post_before_create.toml` and `functions/post_before_update.toml`, so
the two can be configured independently even when they share a `Config` type.

## Deploying

```bash
apiplant build my-app --release     # or build the cdylib yourself and copy it in
```

On boot the server logs every function each library provides, and its route:

```
INFO apiplant_server:   fn greet -> /api/functions/greet
```

A library that fails to load is logged and skipped rather than stopping the
server. This includes a library exporting no functions, or two with the same
name.

## Calling a function

```bash
curl -XPOST http://localhost:8099/api/functions/greet \
  -H 'content-type: application/json' \
  -d '{"name":"World"}'
# → {"message":"Bonjour, World!","registered_users":1}
```

* The request body becomes `Input`; an empty body is treated as `{}`.
* The manifest's `method` is enforced, returning `405` on a mismatch.
* Visibility is enforced, returning `401`, `403` or `404`.
* `Ok(output)` is serialized to JSON; `Err(e)` becomes a `400` with `e`'s text.

### Streaming what it produces

Every function has a second endpoint, `<base>/functions/<name>/stream`, which
responds with server-sent events: one `delta` event per chunk the function
`emit`s, sent as soon as it is produced, followed by a `done` event carrying the
return value as `result`.

```bash
curl -N -XPOST http://localhost:8099/api/functions/report/stream -d '{}'
```
```
event: delta
data: {"text":"Sales are up.\n"}

event: done
data: {"result":{"sections":3}}
```

```rust
for section in sections {
    if !ctx.emit(&section) {
        break;              // no reader left; stop.
    }
}
```

The same handler serves both endpoints without needing to know which:

* On a non-streaming call, whether the plain endpoint or a lifecycle hook, the
  chunk is discarded and the call costs nothing.
* `emit` reports whether to **continue**, not whether the chunk was delivered.
  It is `false` only once a streaming caller has closed the connection, leaving
  no recipient for further work. A plain invocation always returns `true`, since
  its caller is still waiting for the return value.

Method and access are the function's own, and `/stream` grants no additional
access. Because the status code is determined before the function starts, a
failure partway through arrives as an `error` event rather than a `500`.

This makes a function usable in front of slow work: a report being assembled, a
batch being processed, or a model generating tokens. See
[`chat_streaming`](ai.md#from-a-function), which emits an assistant's answer
through this same channel while still returning it in full. The built-in admin
dashboard also uses this `/stream` endpoint for actions, so emitted output is
visible immediately and the return value still arrives as the final result.

## When a function panics

A panic in a handler fails **that request only**, returning `500 internal
function error`, and the server continues serving. The panic message and
backtrace go to the host's log; the caller never sees them, since they often
expose internals.

This is worth stating explicitly, because it is not the default behaviour at an
FFI boundary. `Function::invoke` is reached through an `extern "C"` function
pointer, and a panic escaping one aborts the entire process, including every
in-flight request on every connection. `apiplant-function` therefore catches
unwinding panics while still on the function's side of the boundary and converts
them into an error the host reports as a `500`.

Two consequences:

* **Do not set `panic = "abort"`** in a function's profile. It would replace the
  containment above with a server that terminates on `unwrap()`. `apiplant
  build` leaves the panic strategy unchanged for this reason.
* **A function written against the raw ABI must do the same**, catching its own
  faults and returning them rather than allowing them to unwind or `longjmp`
  out. In C that means returning `APIPLANT_ERR_INTERNAL`; the marker the host
  looks for is `apiplant_abi::INTERNAL_ERROR_PREFIX`.

An error a handler *returns* is different: `Err(e)` produces a `400` with `e`'s
text, because it describes what was wrong with the request.

## Writing a function in C, Zig or Go

A function is a shared library, so it need not be written in Rust. The
`abi_stable` contract cannot reasonably be written by hand outside Rust, so a
library may instead export four plain C symbols; the host tries the Rust root
module first and falls back to these. Either form becomes the same `Function`
object internally, so a C function is mounted, access-controlled, documented and
usable as a lifecycle hook exactly like a Rust one.

Include [`apiplant.h`](../crates/apiplant-abi/include/apiplant.h) and implement:

```c
uint32_t     apiplant_abi_version(void);  /* return APIPLANT_ABI_VERSION */
const char  *apiplant_manifest(void);     /* static JSON array of manifests */
int32_t      apiplant_invoke(const char *name, const char *input_json,
                             const ApiplantHost *host, char **out);
void         apiplant_free(char *string); /* free what apiplant_invoke produced */
```

`apiplant build` compiles all four languages, so nothing about the workflow
changes:

| Source | Built with | Scaffolding |
|---|---|---|
| `.rs` | `cargo build` | a generated cdylib crate |
| `.c` | `cc -shared -fPIC` | none; one translation unit |
| `.zig` | `zig build-lib -dynamic -lc` | none |
| `.go` | `go build -buildmode=c-shared` | a generated `go.mod` |
| `.ts` | transpiled in-process, no toolchain | none; the output is `greet.js`, not a library |
| `<dir>/package.json` | your `build` script, via npm/pnpm/yarn/bun | the project is yours |

```bash
apiplant build my-app            # every .rs, .c, .zig, .go and .ts in functions/
apiplant build my-app --release
```

Each toolchain is overridable: `CARGO`, `CC`, `ZIG` and `GO` select the
executable, and `CFLAGS`, `ZIGFLAGS` and `CGO_CFLAGS` are passed through for
additional includes or libraries. Any other language with a C ABI works as well;
build the shared library yourself and place it in `functions/`.

Worked examples: [C](../examples/09-c-functions),
[Zig](../examples/10-zig-functions) and [Go](../examples/11-go-functions). All
three serve the same two endpoints, so they can be compared directly.

### Language notes

**Zig** reaches the ABI with `@cImport`, so the header is the binding and there
is no second declaration to keep in sync. `defer` makes the `free_string`
pairing straightforward. `apiplant build` keeps safety checks enabled in both
profiles (`Debug` and `ReleaseSafe`), since turning a bounds check into
undefined behaviour inside the host process is not worth the few kilobytes
saved.

**Go** requires three cgo accommodations, all in the file's preamble: define
`APIPLANT_NO_PROTOTYPES` before including the header, since cgo emits its own
prototypes that disagree about `const`; wrap each host callback in a `static`
one-liner, since cgo cannot call a C function pointer; and `recover()` panics
(see below). In return you get `encoding/json` and the rest of the standard
library. A `dlopen`ed Go runtime works and is covered by the tests, but every
library embeds the runtime, so Go artifacts start at around 2 MB.

### Faults in a non-Rust function

`apiplant-function` catches panics for Rust handlers because Rust can unwind.
**C, Zig and Go cannot be recovered from outside**, so each must contain its own
faults:

* **C**: return `APIPLANT_ERR_INTERNAL`. There is nothing to catch, so avoid
  undefined behaviour.
* **Zig**: a failed safety check calls the panic handler and aborts the host,
  with no unwinding to intercept. Handle errors and return the status code.
* **Go**: use `recover()` in `apiplant_invoke`. A panic escaping an exported cgo
  function crashes the process; a short `defer` block converts it into a `500`.

### Sizes

Same two endpoints, `--release`, same machine:

| Language | Size |
|---|---|
| C | ~16 KB |
| Zig | ~288 KB |
| Rust | ~600 KB |
| Go | ~2.2 MB |

Rust carries its own `std`, `serde_json` and `schemars`, the self-containment
that keeps the Rust ABI stable across compilers. Go carries its runtime and
garbage collector. C and Zig carry almost nothing.

### The manifest

`apiplant_manifest` returns a static JSON array with one object per function,
which is what the `function!` macro generates on the Rust side:

```json
[{ "name": "hello", "version": "1.0.0", "description": "Greets someone.",
   "permission": "public", "method": "POST",
   "admin": { "label": "Say hello", "group": "Demo" },
   "input_schema": { "type": "object" }, "output_schema": { "type": "object" } }]
```

Only `name` is required. `permission` takes the same strings as a resource's
permissions (`"public"`, `"authenticated"`, `"member"`, `"role:admin"` and
`"private"`) and defaults to `"private"`, so a typo hides an endpoint rather
than exposing it. `visibility` is accepted as the older name for the same
setting, and `permission` takes precedence when both appear. `method` defaults
to `"POST"`. `admin` is the [dashboard](admin.md#actions) block, given as an
object or a pre-serialised string. The schema fields are optional, may likewise
be an object or a string, and affect only the generated docs.

### Status codes and memory

| Return | Meaning | Response |
|---|---|---|
| `APIPLANT_OK` | `*out` is the JSON response body | `200` |
| `APIPLANT_ERR_REQUEST` | `*out` is a message for the caller | `400` |
| `APIPLANT_ERR_INTERNAL` | `*out` is a message for the log | `500`, message withheld |

Each side frees what it allocated, since the two do not share an allocator:

* the string written to `*out` is returned to your `apiplant_free`;
* strings the host provides (`config`, `query`, `principal_id`, `hook`,
  `send_email`, `cache`, `payments` and `ai`) are returned to
  `host->free_string`.

`host->query` takes the same `{"sql": …, "params": […]}` request as the Rust
side and returns a JSON array of rows, `{"rows_affected": n}`, or `{"error": …}`
when the query failed.

`host->send_email`, `host->cache`, `host->payments` and `host->ai` follow the
same convention: JSON in, JSON out, and `{"error": …}` on failure.

```c
char *receipt = host->send_email(host->ctx,
    "{\"to\":\"ann@example.com\",\"subject\":\"Hi\",\"text\":\"Hello\"}");
char *hits = host->cache(host->ctx, "{\"op\":\"incr\",\"key\":\"hits\"}");
char *answer = host->ai(host->ctx,
    "{\"messages\":[{\"role\":\"user\",\"content\":\"Summarise this.\"}]}");
host->free_string(host->ctx, receipt);
host->free_string(host->ctx, hits);
host->free_string(host->ctx, answer);
```

`host->emit` allocates nothing: it pushes a chunk of the response to the caller
before the call returns, and reports whether more output is still worth
producing (non-zero) or the caller has disconnected (zero). See
[streaming](#streaming-what-it-produces).

```c
if (!host->emit(host->ctx, "the first paragraph\n")) return APIPLANT_OK;
```

Callbacks are only ever *appended* to `ApiplantHost`, and the host allocates it,
so a library built against an older `apiplant.h` keeps working and
`APIPLANT_ABI_VERSION` does not change when one is added.

See [`examples/09-c-functions`](../examples/09-c-functions) for a working app:
two endpoints in one `.c` file, one of which queries Postgres.

## Writing a function in TypeScript

A `.ts` file in `functions/` is a function like any other. It is the only
language that does not produce a shared library, since there is nothing to link,
so it works in two stages:

| | |
|---|---|
| `apiplant build` | parses `greet.ts`, strips the type annotations, writes `greet.js` beside it |
| `apiplant run` | loads `greet.js` into a pool of V8 isolates and calls into it per request |

Nothing is installed: the transpiler is built into `apiplant build`, so there is
no node, deno, bun, `package.json` or `node_modules`. Nothing is
type-*checked* either, which matches the behaviour of `deno run --no-check` and
`bun`. What `apiplant build` writes into `functions/` is what allows an editor,
or `tsc --noEmit -p functions` in CI, to perform the checking:

| | |
|---|---|
| `apiplant.d.ts` | the types for the `apiplant` module, `ctx` and the manifest. Rewritten on every build |
| `tsconfig.json` | strict, with `lib: ["ES2022"]`, since an isolate has neither DOM nor Node APIs. Created once, then yours to maintain |

### The `apiplant` module

A function imports one module, which is the only import available to it:

```ts
import { defineFunctions, db, cache, email, config, log, s, sql } from "apiplant";
```

There is nothing to install. It is compiled into the server and provided to the
isolate on demand, so it can never be out of step with the host running it. The
`apiplant.d.ts` beside your source is what an editor reads. The source lives in
[`typescript/`](../typescript) at the root of this repository.

```ts
const NewNote = s.object({
  title: s.string({ minLength: 1, description: "What the note says." }),
  tags: s.optional(s.array(s.string())),
});

export default defineFunctions({
  createNote: {
    permission: "authenticated",
    description: "Files a note.",
    input: NewNote,
    output: s.object({ id: s.string() }),

    handler(input) {
      const owner = principalId();
      const row = db.one(
        sql`INSERT INTO apiplant_note (title, owner) VALUES (${input.title}, ${owner}) RETURNING id`,
      );
      cache.delete(`notes:${owner}`);
      log.info(`note ${row.id} filed`);
      return { id: row.id };
    },
  },
});
```

`defineFunctions` avoids maintaining two parallel lists: each entry *is* its
manifest entry, and the key is the endpoint's name, so a name is written once.
The host reads the default export.

**Schemas.** `s.object({...})` is declared once and serves three purposes: it
becomes the `input_schema` in the generated OpenAPI, the validation that runs
*before* the handler (an invalid body returns a `400` naming the field, and the
handler never runs), and the inferred type of `input`. Fields are required
unless wrapped in `s.optional`. It covers objects, arrays, strings, numbers,
booleans and enums; for anything more complex, pass `input` a plain JSON Schema
object, which reaches the docs unchanged and leaves validation to you.

**Postgres.** `db.query` returns rows, `db.first` returns a row or `null`,
`db.one` requires exactly one, `db.value` unwraps a single column, as for
`count(*)`, and `db.execute` returns `rows_affected` for statements that modify
data. Using `db.query` for a modifying statement raises an error rather than
returning an empty array. The `` sql`…` `` template numbers `$1`, `$2`, … and
passes the values separately, so caller-supplied data can never become SQL.

**Cache and email.** `cache.get`/`set`/`has`/`increment`/`ttl`, plus
`cache.remember(key, ttl, compute)` for read-through caching; and
`email.send({ to, subject, text })`, which fills in `from` and `reply_to` from
`[email]`. Both throw ("no cache configured" and "no email provider configured")
when the app has not configured one, rather than silently doing nothing.

**The assistant.** `ai.chat(messagesOrPrompt)` returns the complete answer,
`ai.ask(prompt)` returns only its text, and `ai.chatStreaming(...)` forwards
every token to your own caller. `emit(chunk)` does the same for any other output
a function produces incrementally. See [AI](ai.md).

**The request.** `config<T>()`, `principalId()`, `hook<T>()` and `log`. All are
synchronous: the isolate blocks while the host performs the work on another
thread. A handler may be declared `async` for its own reasons and is awaited
either way. `console.log` goes to the server's log, and the standard timers are
available.

**Errors** follow the ABI's split. `throw new BadRequest("…")` produces a `400`
with that message, the JavaScript equivalent of `APIPLANT_ERR_REQUEST`, and
`new HttpError(status, message)` selects the status. Every other throw produces
a `500` whose message goes to the log rather than to the caller. A failed host
call, such as `db.query` against a missing table, arrives as a thrown `Error`,
so ignoring it cannot turn a failure into data.

### The import-free form

A **single `.ts` file** is not bundled, so it must be self-contained: relative
imports and npm packages are rejected at build time, with the line number. Use a
[directory](#a-typescript-directory-npm-dependencies-and-your-own-bundler) when
either is needed. A module that prefers to import nothing can declare itself
explicitly, using the same JSON a C library returns from `apiplant_manifest`
plus one export per entry:

```ts
export const manifest: FunctionManifest[] = [
  { name: "greet", permission: "public", description: "Greets someone." },
];

export function greet(input: { name?: string }, ctx: Ctx) {
  if (!input?.name) throw new BadRequest("`name` is required");

  const { greeting = "Hello" } = ctx.config<{ greeting?: string }>();
  ctx.log.info(`greeting ${input.name}`);
  return { message: `${greeting}, ${input.name}!` };
}
```

`ctx` is the same host API `apiplant.h` declares (`query`, `config`,
`principalId`, `hook`, `sendEmail`, `cache`, `chat`, `emit` and `log`). Its
types, and those for `BadRequest`, are global, so this form needs no import and
no schema.

Worked example: [17-typescript-functions](../examples/17-typescript-functions),
the same two endpoints as the C, Zig and Go ones.

### A TypeScript directory: npm dependencies and your own bundler

A single file covers most cases, but not an npm package or a second source file.
For those, a TypeScript function is a **directory**, following the same rule as
every other language: a project in the language's own native form, built by its
own toolchain.

```text
functions/
├── slug/
│   ├── package.json      # yours: dependencies and the `build` script apiplant runs
│   ├── tsconfig.json     # yours: for your editor
│   ├── src/index.ts      # imports npm packages, sibling modules and `apiplant`
│   └── src/reserved.ts
├── slug.toml             # config, still keyed by the directory name
└── slug.js               # the bundle, copied out by `apiplant build`
```

`apiplant build` runs `<pm> install`, only when `node_modules` is missing so the
network is used once, then `<pm> run build`, and copies the output to
`functions/slug.js`. The package manager is determined from the project's
lockfile (`pnpm-lock.yaml`, `package-lock.json`, `yarn.lock` or `bun.lockb`),
defaulting to pnpm; `NODE_PACKAGE_MANAGER` overrides it as `CARGO`, `CC`, `ZIG`
and `GO` do for the others.

Two requirements apply to the build output, both checked at build time rather
than failing at boot:

* **ESM.** The isolate loads a module. CommonJS output declares no exports and
  is rejected with that explanation.
* **`apiplant` left external.** The host provides that module. A bundler that
  inlines it, or that leaves any *other* import unresolved, is told which
  specifier is at fault.

A `build` script that satisfies both, with esbuild:

```json
{
  "type": "module",
  "module": "dist/slug.js",
  "scripts": {
    "build": "esbuild src/index.ts --bundle --format=esm --platform=neutral --main-fields=module,main --external:apiplant --outfile=dist/slug.js"
  },
  "devDependencies": { "esbuild": "^0.25.0" }
}
```

Any bundler works (rollup, tsup, bun build, esbuild), because apiplant runs
*your* script and never generates a config. It looks for the bundle at the
package's `module` field, then `main`, then `dist/<name>.js`.

The toolchain requirement therefore follows the layout: a single `.ts` file
needs nothing on `PATH`, while a directory needs the node toolchain its
`package.json` already implies. Worked example:
[12-function-dependencies](../examples/12-function-dependencies), which includes
one directory function per language.

### Isolates, concurrency and runaway code

A module is loaded into a small pool of isolates sharing one queue, so calls run
concurrently up to the pool size and queue beyond it. **Isolates share nothing**:
module-level state exists once per worker rather than once per app, and is lost
on restart. Anything shared or durable belongs in the database or the
[cache](caching.md).

| Variable | Default | Effect |
|---|---|---|
| `APIPLANT_JS_WORKERS` | `2` | isolates per module, and therefore how many calls run at once |
| `APIPLANT_JS_TIMEOUT_MS` | `30000` | how long one call may run before its isolate is terminated |

That timeout is essential. JavaScript is not preemptible, so `while (true) {}`
would hold a worker until the process exited. When the watchdog fires, that
single request fails with a `500` and the worker continues serving.

## Without the macro (the raw ABI)

The macro is optional convenience over the
[`apiplant-abi`](../crates/apiplant-abi) contract. For full control, or when
writing a function in another language, implement the `Function` trait and
export the root module directly. The macro expands to exactly that: an
`#[export_root_module]` constructor whose `new_functions` returns one `Function`
trait object per exported function, each delegating to
`apiplant_function::invoke_handler`. See the crate docs for the trait
definitions.

[`abi_stable`]: https://docs.rs/abi_stable
