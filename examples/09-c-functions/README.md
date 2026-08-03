# 09 · Functions in C

Example 07 wrote functions in Rust. A function is really just a shared library
the server dlopens, so it does not have to be Rust at all — this app's only
function is a single `.c` file.

```
09-c-functions/
├── main.toml
├── models/
│   └── note.toml       # something for the function to count
└── functions/
    ├── hello.c         # both endpoints below, one translation unit
    ├── hello.toml      #   …config for `hello`
    └── libhello.so     # ← produced by `apiplant build`
```

| Endpoint | Visibility | What it shows |
|---|---|---|
| `POST /api/functions/hello` | public | typed config, logging, JSON in and out |
| `GET /api/functions/notes` | authenticated | querying Postgres, reading the caller id |

Nothing else in the app knows the difference. Both endpoints are mounted,
access-controlled and typed in the OpenAPI spec exactly like Rust ones, and a C
function can serve as a [lifecycle hook](../08-hooks) too.

## Running it

```bash
createdb -h 127.0.0.1 -p 5432 -U postgres apiplant_c_functions

cargo run -p apiplant -- build examples/09-c-functions   # compiles functions/*.c with cc
cargo run -p apiplant -- run examples/09-c-functions
```

```bash
# public, and picks up `greeting` from hello.toml
curl -X POST localhost:8099/api/functions/hello \
  -H 'content-type: application/json' -d '{"name":"Federico"}'
# {"message":"Ciao, Federico!","compiled_by":"gcc"}

# a bad request gets a 400 with the function's own message
curl -X POST localhost:8099/api/functions/hello \
  -H 'content-type: application/json' -d '{}'
# {"error":"`name` is required and must be a string"}

# authenticated: register, then count the notes
TOKEN=$(curl -sX POST localhost:8099/api/auth/register \
  -H 'content-type: application/json' \
  -d '{"email":"c@example.com","password":"hunter2hunter2"}' | jq -r .token)

curl -X POST localhost:8099/api/note \
  -H 'content-type: application/json' -d '{"title":"alpha"}'

curl localhost:8099/api/functions/notes -H "Authorization: Bearer $TOKEN"
# {"notes":1,"caller":"1d45a522-…"}
```

Then open <http://127.0.0.1:8099/api/docs> — both functions are there, with the
schemas from the manifest.

## The four symbols

A C library implements these and nothing else. The full contract, including the
memory rules, is in [`apiplant.h`](../../crates/apiplant-abi/include/apiplant.h)
and [the docs](../../docs/functions.md#writing-a-function-in-c-zig-or-go).

```c
uint32_t     apiplant_abi_version(void);  /* return APIPLANT_ABI_VERSION */
const char  *apiplant_manifest(void);     /* static JSON array of manifests */
int32_t      apiplant_invoke(const char *name, const char *input_json,
                             const ApiplantHost *host, char **out);
void         apiplant_free(char *string); /* free what apiplant_invoke produced */
```

`apiplant_manifest` is where a C function declares what Rust gets from the
`function!` macro — name, visibility, method, and the JSON Schemas for the docs.
Only `name` is required; `visibility` defaults to `"private"`, so a typo hides an
endpoint rather than publishing one.

`apiplant_invoke` returns `APIPLANT_OK`, `APIPLANT_ERR_REQUEST` (the message goes
to the caller as a `400`) or `APIPLANT_ERR_INTERNAL` (the message goes to the log
and the caller gets a generic `500`).

Two ownership rules, because the host and the library do not share an allocator:

* what you write to `*out` comes back to your `apiplant_free`;
* what the host hands you — `config`, `query`, `principal_id`, `hook` — goes back
  to `host->free_string`.

## Notes

`hello.c` parses JSON by hand so the example stays a single file with no
dependencies. That code is the least interesting part of it — link cJSON,
jansson or yyjson for anything real. The ABI is strings in and strings out, so
the choice is yours and does not affect the contract.

The same four symbols are reachable from anything with a C ABI: Zig via
`export fn`, Go via cgo and `-buildmode=c-shared`, or plain assembly if you
insist. `apiplant build` only compiles `.c` automatically; for other languages,
build the shared library yourself and drop it in `functions/`.

## Size

For contrast, on the same machine:

| Function | `--release` |
|---|---|
| `hello.c` (this example) | ~21 KB |
| `greet.rs` (example 07) | ~600 KB |

The Rust one statically links its own `std`, `serde_json` and `schemars` — that
self-containment is what keeps the Rust ABI stable across compiler versions. The
C one links nothing but libc, which is already loaded.
