# Functions (compiled plugins)

A **function** is custom logic in the app's `functions/` directory that the
framework loads at boot and mounts as an HTTP endpoint. Usually that means a
shared library (`.so`/`.dylib`/`.dll`) talking to the host across a stable C ABI
(via [`abi_stable`]), so it can be shipped independently and never requires
recompiling the server. A [TypeScript function](#writing-a-function-in-typescript)
is the exception: there is nothing to link, so it runs in a V8 isolate instead —
and everything around it (manifest, config, permissions, docs, hooks) is
identical.

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

## A function can be a file or a directory

The `greet.rs` above is a single file. That's enough until a function needs a
third-party crate, a second source file, or a linked library — for which an entry
in `functions/` can be a **directory** instead, a self-contained project in the
language's own native form:

```
my-app/functions/
├── greet/            # a directory is one function library…
│   ├── Cargo.toml    #   …here a real crate, with any dependencies you like
│   └── src/lib.rs
├── greet.toml        # config still sits beside it, keyed by the directory name
└── libgreet.so       # ← produced next to the directory, loaded the same way
```

apiplant reads the language from what the directory holds, and builds it its
native way:

| A directory containing… | is built as | with |
|-------------------------|-------------|-------|
| `Cargo.toml`            | Rust | `cargo build` on *your* crate — any dependencies, any modules |
| `go.mod`                | Go   | `go build -buildmode=c-shared` on your module |
| `.c` files              | C    | `cc` over every `.c` in the directory, with it on the include path |
| `.zig` files            | Zig  | `zig build-lib` from a root `.zig` (named for the directory) that may `@import` the rest |
| `package.json`          | TypeScript | your `build` script — npm dependencies, your own bundler |

A directory named `greet/` compiles to `libgreet.so` beside it, so the host loads
it exactly like a single-file function. For **Rust and Go the project is yours**:
apiplant runs your `Cargo.toml` / `go.mod` unchanged and copies out the library it
produces, so you own the dependencies *and* the build profiles (a directory does
not get the [size-reducing profiles](#library-size) injected — set them yourself).
For **C and Zig** a directory just widens the single-file build to every source it
holds, so its own headers and `@import`ed modules resolve.

A `greet/` directory and a `greet.rs` file both want `libgreet.so`, so apiplant
rejects that collision — pick one form per name. See
[`examples/12-function-dependencies`](../examples/12-function-dependencies) for one
directory per language, each pulling in something a single file couldn't.

## Building

```bash
apiplant build my-app          # compile every source in functions/
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

C, Zig and Go sources need their own toolchain on `PATH` instead — see
[writing a function in C, Zig or Go](#writing-a-function-in-c-zig-or-go).
TypeScript needs nothing: `apiplant build` transpiles it in-process — see
[writing a function in TypeScript](#writing-a-function-in-typescript).

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

A C function links nothing but libc and comes out around **16 KB**; Zig ~288 KB
and Go ~2.2 MB. See [sizes](#sizes).

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
    permission: "public",
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
| `handler` | yes | The function above. |
| `permission` | no* | Access policy: `"public"`, `"authenticated"`, `"member"`, `"role:<name>"`, `"private"`. |
| `visibility` | no* | The older form: `Public` \| `Authenticated` \| `RoleGated` \| `Private`. |
| `role` | no | Required role name when `visibility: RoleGated`. |
| `admin` | no | How the [dashboard](admin.md#actions) presents it. |
| `version` | no | Defaults to the crate's `CARGO_PKG_VERSION`. |

\* Give `permission` **or** `visibility`, not both. Omit both and the function
is private — the safe direction, so a forgotten line hides an endpoint rather
than publishing one.

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
| `send_email(email)` | `Result<Sent, String>` | Send mail through the configured [`[email]` provider](email.md). |
| `cache_get(key)` | `Result<Option<Value>, String>` | Read from the [`[cache]` Redis](caching.md); `None` is a miss. |
| `cache_get_as::<T>(key)` | `Result<Option<T>, String>` | The same, deserialized; a value of the wrong shape is a miss. |
| `cache_set(key, &value, ttl)` | `Result<(), String>` | Write. `None` ttl uses `default_ttl_secs`; `Some(0)` persists. |
| `cache_delete(key)` | `Result<bool, String>` | Remove; `true` when it was there. |
| `cache_incr(key, by, ttl)` | `Result<i64, String>` | Atomic counter — correct across workers and hosts. |
| `cache_ttl(key)` | `Result<Option<i64>, String>` | Seconds left, or `None`. |
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

`send_email` and the `cache_*` methods error when the app configured no
provider or no cache, so a function that uses them only works in an app whose
`main.toml` says so. Both are covered in full in [Sending email](email.md) and
[Caching](caching.md).

## Permissions

A function's access uses the same string grammar as a resource's
[`[permissions]`](permissions.md), so an app has one access vocabulary rather
than two. The framework enforces it before your handler runs:

| Value | Who can call it | On mismatch |
|-------|-----------------|-------------|
| `"public"` | anyone | — |
| `"authenticated"` | any authenticated caller | `401` |
| `"member"` | any member of the caller's active organisation | `401` / `403` |
| `"role:<name>"` | a member holding that role in the active organisation | `401` / `403` |
| `"private"` | nobody over HTTP (hidden from routing & docs) | `404` |

`owner` is the one level with no meaning here: a function call has no row to
own.

```rust
apiplant_function::function! {
    name: "sales_summary",
    description: "Orders and revenue over a recent window.",
    method: Post,
    permission: "member",
    handler: sales_summary,
}
```

A `private` function answers `404` rather than `403` — the same answer an
unknown name gets, so probing cannot enumerate what is there.

This governs the **HTTP endpoint** only. A function attached to a resource's
lifecycle still runs whatever its permission — which is why hook functions are
usually `"private"`.

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
`member` — "anyone in the active organisation", the level most operator-facing
actions actually want, and the one `Visibility` has no variant for. A library
compiled before `permission` existed keeps exactly the access its `visibility`
gave it.

An unreadable `permission` string collapses to `private`, matching how the rest
of apiplant treats a typo in an access string: a mistake hides an endpoint
instead of exposing one.

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

## Appearing in the dashboard

A function whose permission is not `private` becomes a runnable **action** in
the generated [admin dashboard](admin.md), with a form derived from its `Input`
type. An optional `admin` block says how it is presented — its label, its
grouping, and whether running it asks for confirmation first:

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

Functions bound to a resource's lifecycle never appear: they are machinery, not
something a person triggers. Full reference:
[Admin dashboard § Actions](admin.md#actions).

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

`apiplant build` compiles all four languages, so nothing about the workflow
changes:

| Source | Built with | Scaffolding |
|---|---|---|
| `.rs` | `cargo build` | a generated cdylib crate |
| `.c` | `cc -shared -fPIC` | none — one translation unit |
| `.zig` | `zig build-lib -dynamic -lc` | none |
| `.go` | `go build -buildmode=c-shared` | a generated `go.mod` |
| `.ts` | transpiled in-process, no toolchain | none — output is `greet.js`, not a library |
| `<dir>/package.json` | your `build` script, via npm/pnpm/yarn/bun | the project is yours |

```bash
apiplant build my-app            # every .rs, .c, .zig, .go and .ts in functions/
apiplant build my-app --release
```

Each toolchain is overridable — `CARGO`, `CC`, `ZIG`, `GO` pick the executable,
and `CFLAGS`, `ZIGFLAGS` and `CGO_CFLAGS` are passed through for extra includes
or libraries. Anything else with a C ABI works too; build the shared library
yourself and drop it in `functions/`.

Worked examples: [C](../examples/09-c-functions),
[Zig](../examples/10-zig-functions), [Go](../examples/11-go-functions). All three
serve the same two endpoints, so they read as a direct comparison.

### Language notes

**Zig** reaches the ABI with `@cImport`, so the header is the binding and there is
no second declaration to keep in sync. `defer` is what makes the `free_string`
pairing hard to get wrong. `apiplant build` keeps safety checks on in both
profiles (`Debug` and `ReleaseSafe`) — turning a bounds check into undefined
behaviour inside the host process is not worth the few KB.

**Go** needs three cgo accommodations, all in the file's preamble: define
`APIPLANT_NO_PROTOTYPES` before including the header (cgo emits its own
prototypes, which disagree about `const`); wrap each host callback in a
`static` one-liner (cgo cannot call a C function pointer); and `recover()` your
panics (see below). In exchange you get `encoding/json` and the rest of the
standard library. A `dlopen`ed Go runtime does work — it is covered by the
tests — but every library embeds the runtime, so Go artifacts start around 2 MB.

### Faults in a non-Rust function

`apiplant-function` catches panics for Rust handlers because Rust can unwind.
**C, Zig and Go cannot be rescued from outside**, so each has to contain its own
faults:

* **C** — return `APIPLANT_ERR_INTERNAL`. There is nothing to catch; avoid
  undefined behaviour.
* **Zig** — a failed safety check calls the panic handler and aborts the host,
  with no unwinding to intercept. Handle errors and return the status code.
* **Go** — `recover()` in `apiplant_invoke`. A panic escaping an exported cgo
  function crashes the process; nine lines of `defer` turn it into a `500`.

### Sizes

Same two endpoints, `--release`, same machine:

| Language | Size |
|---|---|
| C | ~16 KB |
| Zig | ~288 KB |
| Rust | ~600 KB |
| Go | ~2.2 MB |

Rust carries its own `std`, `serde_json` and `schemars` — the self-containment
that keeps the Rust ABI stable across compilers. Go carries its runtime and GC. C
and Zig carry almost nothing.

### The manifest

`apiplant_manifest` returns a static JSON array — one object per function, and
what the `function!` macro generates on the Rust side:

```json
[{ "name": "hello", "version": "1.0.0", "description": "Greets someone.",
   "permission": "public", "method": "POST",
   "admin": { "label": "Say hello", "group": "Demo" },
   "input_schema": { "type": "object" }, "output_schema": { "type": "object" } }]
```

Only `name` is required. `permission` takes the same strings as a resource's
permissions (`"public"`, `"authenticated"`, `"member"`, `"role:admin"`,
`"private"`) and defaults to `"private"` — the safe direction, so a typo hides
an endpoint instead of exposing it. `visibility` is accepted as the older name
for the same thing; `permission` wins if both appear. `method` defaults to
`"POST"`. `admin` is the [dashboard](admin.md#actions) block, as an object or a
pre-serialised string. The schema fields are optional, may likewise be an object
or a string, and only feed the generated docs.

### Status codes and memory

| Return | Meaning | Response |
|---|---|---|
| `APIPLANT_OK` | `*out` is the JSON response body | `200` |
| `APIPLANT_ERR_REQUEST` | `*out` is a message for the caller | `400` |
| `APIPLANT_ERR_INTERNAL` | `*out` is a message for the log | `500`, withheld |

Each side frees what it allocated, since the two do not share an allocator:

* the string you write to `*out` comes back to your `apiplant_free`;
* strings the host hands you — `config`, `query`, `principal_id`, `hook`,
  `send_email`, `cache` — go back to `host->free_string`.

`host->query` takes the same `{"sql": …, "params": […]}` request as the Rust
side and answers with a JSON array of rows, `{"rows_affected": n}`, or
`{"error": …}` when the query failed.

`host->send_email` and `host->cache` follow the same convention — JSON in, JSON
out, `{"error": …}` on failure:

```c
char *receipt = host->send_email(host->ctx,
    "{\"to\":\"ann@example.com\",\"subject\":\"Hi\",\"text\":\"Hello\"}");
char *hits = host->cache(host->ctx, "{\"op\":\"incr\",\"key\":\"hits\"}");
host->free_string(host->ctx, receipt);
host->free_string(host->ctx, hits);
```

Callbacks are only ever *appended* to `ApiplantHost`, and the host is what
allocates it — so a library built against an older `apiplant.h` keeps working
and `APIPLANT_ABI_VERSION` does not move when one is added.

See [`examples/09-c-functions`](../examples/09-c-functions) for a working app —
two endpoints in one `.c` file, one of them querying Postgres.

## Writing a function in TypeScript

A `.ts` file in `functions/` is a function like any other. It is the only
language that does not produce a shared library, because there is nothing to
link — so it works in two stages instead:

| | |
|---|---|
| `apiplant build` | parses `greet.ts`, strips the type annotations, writes `greet.js` beside it |
| `apiplant run` | loads `greet.js` into a pool of V8 isolates and calls into it per request |

Nothing is installed: the transpiler is built into `apiplant build`, so there is
no node, no deno, no bun, no `package.json` and no `node_modules`. Nothing is
type-*checked* either — the same choice `deno run --no-check` and `bun` make.
What `apiplant build` writes into `functions/` is what makes an editor (and
`tsc --noEmit -p functions`, if you want it in CI) do the checking:

| | |
|---|---|
| `apiplant.d.ts` | the types: the `apiplant` module, `ctx`, the manifest. Rewritten every build |
| `tsconfig.json` | strict, and `lib: ["ES2022"]` — an isolate has no DOM and no Node. Created once, then yours |

### The `apiplant` module

A function imports one module, and it is the only one it can import:

```ts
import { defineFunctions, db, cache, email, config, log, s, sql } from "apiplant";
```

Nothing installs it. It is compiled into the server and handed to the isolate on
demand, so it can never be a version out of step with the host running it; the
`apiplant.d.ts` beside your source is what your editor reads. The source lives in
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

`defineFunctions` is what removes the two-lists problem: each entry *is* its
manifest entry, and the key is the endpoint's name, so a name is written once.
The default export is what the host reads.

**Schemas.** `s.object({...})` is declared once and used three times: it becomes
the `input_schema` in the generated OpenAPI, the check that runs *before* the
handler (a bad body is a `400` naming the field, and your handler never sees it),
and the type of `input` — inferred, not annotated. Fields are required unless
wrapped in `s.optional`. It covers objects, arrays, strings, numbers, booleans
and enums; for anything richer, pass `input` a plain JSON Schema object, which
reaches the docs untouched and leaves the checking to you.

**Postgres.** `db.query` returns rows, `db.first` a row or `null`, `db.one`
insists on exactly one, `db.value` unwraps a single column (the `count(*)` case),
and `db.execute` returns `rows_affected` for statements that change things. Using
`db.query` for one of those is an error rather than an empty array. The `` sql`…` ``
template numbers `$1`, `$2`, … and passes the values separately, so nothing a
caller sends can become SQL.

**Cache and email.** `cache.get`/`set`/`has`/`increment`/`ttl`, plus
`cache.remember(key, ttl, compute)` for read-through; and `email.send({ to,
subject, text })`, which fills `from` and `reply_to` in from `[email]`. Both
throw "no cache configured" / "no email provider configured" when the app has
not configured one, rather than quietly doing nothing.

**The request.** `config<T>()`, `principalId()`, `hook<T>()`, `log`. All
synchronous: the isolate blocks while the host does the work on another thread.
A handler writes `async` only when it wants to for its own reasons; it is awaited
either way. `console.log` goes to the server's log, and the standard timers work.

**Errors** follow the ABI's split. `throw new BadRequest("…")` is a `400` with
that message — the JavaScript spelling of `APIPLANT_ERR_REQUEST` — and
`new HttpError(status, message)` picks the status. Every other throw is a `500`
whose message goes to the log rather than to the caller. A failed host call
(`db.query` against a bad table) arrives as a thrown `Error`, so ignoring it
cannot turn a failure into data.

### The import-free form

A **single `.ts` file** is not bundled, so it is self-contained: relative imports
and npm packages are refused at build time, with the line number. Reach for a
[directory](#a-typescript-directory-npm-dependencies-and-your-own-bundler) when
you need either. A module that would rather import nothing at all can still
declare itself the long way, which is the same JSON a C library returns from
`apiplant_manifest` plus one export per entry:

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

`ctx` is the same host API `apiplant.h` declares — `query`, `config`,
`principalId`, `hook`, `sendEmail`, `cache`, `log` — and the types for it, and
for `BadRequest`, are global, so this form needs no import and no schema.

Worked example: [17-typescript-functions](../examples/17-typescript-functions),
the same two endpoints as the C, Zig and Go ones.

### A TypeScript directory: npm dependencies and your own bundler

A single file covers a lot, but not a package from npm or a second source file.
For that a TypeScript function is a **directory**, and it follows the rule every
other language's directory follows: it is a project in the language's own native
form, built by the language's own toolchain.

```text
functions/
├── slug/
│   ├── package.json      # yours: dependencies, and the `build` script apiplant runs
│   ├── tsconfig.json     # yours: for your editor
│   ├── src/index.ts      # imports npm packages, siblings, and `apiplant`
│   └── src/reserved.ts
├── slug.toml             # config, still keyed by the directory name
└── slug.js               # ← the bundle, copied out by `apiplant build`
```

`apiplant build` runs `<pm> install` (only when `node_modules` is missing, so the
network is touched once) and then `<pm> run build`, and copies what it produced
to `functions/slug.js`. The package manager is read from the lockfile the project
already has — `pnpm-lock.yaml`, `package-lock.json`, `yarn.lock`, `bun.lockb` —
defaulting to pnpm, and `NODE_PACKAGE_MANAGER` overrides it the way `CARGO`,
`CC`, `ZIG` and `GO` override the others.

Two requirements on what your build produces, both checked at build time rather
than left to fail at boot:

* **ESM.** The isolate loads a module. CommonJS output declares no exports and
  is rejected with that explanation.
* **`apiplant` left external.** The host provides that module; a bundler that
  inlines it, or that leaves some *other* import unresolved, is told which
  specifier is the problem.

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

Any bundler works — rollup, tsup, bun build, esbuild — because apiplant runs
*your* script and never generates a config. It looks for the bundle at the
package's `module` field, then `main`, then `dist/<name>.js`.

So the toolchain requirement follows the layout, which is the point: a single
`.ts` file needs nothing on `PATH`, and a directory needs the node toolchain the
`package.json` already implies. Worked example:
[12-function-dependencies](../examples/12-function-dependencies), which has one
directory function per language.

### Isolates, concurrency and runaway code

A module is loaded into a small pool of isolates that share one queue, so calls
run concurrently up to the pool size and queue beyond it. **Isolates share
nothing**: module-level state exists once per worker, not once per app, and is
lost on restart. Anything shared or durable belongs in the database or the
[cache](caching.md).

| Variable | Default | What it does |
|---|---|---|
| `APIPLANT_JS_WORKERS` | `2` | isolates per module — how many calls run at once |
| `APIPLANT_JS_TIMEOUT_MS` | `30000` | how long one call may run before its isolate is terminated |

That timeout is load-bearing. JavaScript is not preemptible, so `while (true) {}`
would hold a worker until the process died; when the watchdog fires, that one
request fails with a `500` and the worker carries on serving.

## Without the macro (the raw ABI)

The macro is optional sugar over the [`apiplant-abi`](../crates/apiplant-abi)
contract. If you need full control — or you're writing a function in another
language — you can implement the `Function` trait and export the root module
yourself. The macro expands to exactly that: an `#[export_root_module]`
constructor whose `new_functions` returns one `Function` trait object per
exported function, each delegating to `apiplant_function::invoke_handler`. See
the crate docs for the trait definitions.

[`abi_stable`]: https://docs.rs/abi_stable
