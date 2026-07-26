# 07 · Functions

Everything so far was declarative. A **function** is where you write actual code:
one plain `.rs` file that becomes an endpoint.

```
07-functions/
├── main.toml
├── models/
│   └── note.toml       # something for the functions to query
└── functions/
    ├── greet.rs        # POST /api/functions/greet — public, typed, configurable
    ├── greet.toml      #   …its config
    └── stats.rs        # GET  /api/functions/stats — authenticated, no config
```

Functions are compiled to shared libraries and loaded across a stable C ABI, so
they ship independently and never require recompiling the server.

## Run it

```bash
createdb -h 127.0.0.1 -p 55432 -U postgres apiplant_functions
cargo run -p apiplant -- build examples/07-functions   # needs cargo on PATH
cargo run -p apiplant -- run examples/07-functions
```

`apiplant build` wraps each `.rs` file in a generated cdylib crate, hands it to
cargo, and drops the resulting library next to the source:

```
INFO apiplant::compile: compiling function=greet
INFO apiplant::compile: built function=greet library=…/functions/libgreet.so
```

Only changed sources recompile. `apiplant run --build` does both steps at once,
and running without building warns if a source is newer than its library.

## Call them

```bash
curl -s -XPOST localhost:8099/api/functions/greet \
  -H 'content-type: application/json' -d '{"name":"World"}'
# → {"message":"Buongiorno, World!","registered_users":0}

TOKEN=$(curl -s -XPOST localhost:8099/api/auth/register -H 'content-type: application/json' \
  -d '{"email":"ana@example.com","password":"pw"}' | jq -r .token)

curl -s localhost:8099/api/functions/stats -H "authorization: Bearer $TOKEN"
# → {"notes":0,"users":1,"asked_by":"…"}
```

## What each file shows

**`greet.rs`** — the full shape of a function:

```rust
fn greet(ctx: &Context<Config>, input: Input) -> Result<Output, String>
```

`Config`, `Input` and `Output` are inferred from that signature; you never name
them twice. `ctx` gives you the database (`query`, `query_one`, `execute`), the
caller's id, logging, and your typed config. The `function!` block declares the
name, HTTP method and visibility.

**`stats.rs`** — a `Get` endpoint with `Authenticated` visibility and
`Context<()>` (no config at all).

## Things to try

```bash
# visibility is enforced before your code runs
curl -s -i localhost:8099/api/functions/stats                 # → 401

# so is the manifest's method
curl -s -i -XPOST localhost:8099/api/functions/stats          # → 405

# Err(_) from a handler becomes a 400 with its message
curl -s -i -XPOST localhost:8099/api/functions/greet \
  -H 'content-type: application/json' -d '{}'                 # → 400, missing `name`
```

**Configuration.** `greet` reads `functions/greet.toml`; change `greeting` to
`"Hello"`, restart, and the response changes — no rebuild, config is read at
boot. A function with no config file gets `Config::default()`.

**Typed docs.** Both functions derive `JsonSchema` on their input and output, so
<http://127.0.0.1:8099/api/docs> renders a typed form and a typed response for
each. `stats` is `Authenticated`, so use the Authorize button first.

## Visibility

| Value | Who can call it | On mismatch |
|-------|-----------------|-------------|
| `Public` | anyone | — |
| `Authenticated` | any signed-in caller | `401` |
| `RoleGated` | a caller holding `role` in their active org — or holding `admin`, which holds every role | `403` |
| `Private` | nobody over HTTP — hidden from routing and docs | `404` |

`Private` is not a dead end: those functions are exactly what the next example
attaches to the resource lifecycle.

Details in [Functions](../../docs/functions.md).

**Next:** [08 · Hooks](../08-hooks) runs functions around every CRUD operation.
