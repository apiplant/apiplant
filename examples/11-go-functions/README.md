# 11 · Functions in Go

The same two endpoints as [example 09](../09-c-functions), written in Go. Go
reaches the C ABI through cgo, and `apiplant build` wraps the file in a generated
module and runs `go build -buildmode=c-shared`.

```
11-go-functions/
├── main.toml
├── models/
│   └── note.toml       # something for the function to count
└── functions/
    ├── hello.go        # both endpoints below, one file
    ├── hello.toml      #   …config for `hello`
    └── libhello.so     # ← produced by `apiplant build`
```

| Endpoint | Visibility | What it shows |
|---|---|---|
| `POST /api/functions/hello` | public | config, logging, JSON in and out |
| `GET /api/functions/notes` | authenticated | querying Postgres, reading the caller id |

## Running it

```bash
createdb -h 127.0.0.1 -p 5432 -U postgres apiplant_go_functions

cargo run -p apiplant -- build examples/11-go-functions   # go build -buildmode=c-shared
cargo run -p apiplant -- run examples/11-go-functions
```

```bash
curl -X POST localhost:8099/api/functions/hello \
  -H 'content-type: application/json' -d '{"name":"Federico"}'
# {"compiled_by":"go1.26.5","message":"Ciao, Federico!"}

TOKEN=$(curl -sX POST localhost:8099/api/auth/register \
  -H 'content-type: application/json' \
  -d '{"email":"g@example.com","password":"hunter2hunter2"}' | jq -r .token)

curl -X POST localhost:8099/api/note \
  -H 'content-type: application/json' -d '{"title":"alpha"}'

curl localhost:8099/api/functions/notes -H "Authorization: Bearer $TOKEN"
# {"caller":"4fefe2d1-…","notes":1}
```

Unlike the C and Zig examples, this one uses a real JSON library, because Go has
one in its standard library. `encoding/json` handles the input, the config and the
response; there is no hand-rolled parser to apologise for.

## Three cgo details

These are the only Go-specific wrinkles, and all three are in the preamble of
`hello.go`.

**1. Suppress the header's prototypes.** cgo generates its own declarations for
the symbols it exports, and they disagree with `apiplant.h` about `const`. Define
`APIPLANT_NO_PROTOTYPES` to take the types and constants without the four
declarations:

```c
#define APIPLANT_NO_PROTOTYPES
#include <apiplant.h>
```

**2. Wrap the host's callbacks.** cgo cannot call a C function pointer, so each
one needs a one-line shim. Keeping them `static` is what lets them sit in the
preamble of a file that also uses `//export`:

```c
static char *ap_config(const ApiplantHost *h) { return h->config(h->ctx); }
```

**3. Recover your panics.** A Go panic unwinding out of an exported cgo function
crashes the process, so `apiplant_invoke` wraps its dispatch in
`defer func() { recover() }` and reports the panic as `APIPLANT_ERR_INTERNAL` —
which the host turns into a `500` while the server keeps running. This is the
same firewall `apiplant-function` puts around every Rust handler; in Go you write
it yourself, and it is nine lines.

`C.CString` allocates with `malloc`, which is exactly what `apiplant_free`
releases, so the ownership rules work out without any extra care.

## Does a dlopen'd Go runtime actually work?

Yes, and it is tested rather than assumed — `crates/apiplant-server/tests/c_abi.rs`
builds a Go fixture, loads it the way the server does, and calls it repeatedly.
Beyond that, this example has been driven with 400 concurrent requests across
both endpoints from the host's blocking thread pool with no runtime faults.

Two caveats worth knowing. Go installs its own signal handlers when its runtime
starts, which is generally invisible but is a real difference from a C or Zig
library. And a `c-shared` library cannot be unloaded — apiplant never unloads
function libraries anyway, so this costs nothing here.

## Size

| Profile | Size |
|---|---|
| `apiplant build` | ~3.1 MB |
| `apiplant build --release` (`-ldflags "-s -w"`) | **~2.2 MB** |

Go is the heaviest of the four languages because every library embeds the Go
runtime, scheduler and garbage collector. Stripping helps at the margin; the floor
is the runtime. For comparison: C ~16 KB, Zig ~288 KB, Rust ~600 KB. Pick Go for
its libraries and its concurrency, not for small artifacts.
