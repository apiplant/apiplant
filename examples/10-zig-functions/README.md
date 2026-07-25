# 10 · Functions in Zig

The same two endpoints as [example 09](../09-c-functions), written in Zig instead
of C. They speak the identical C ABI — Zig reaches it by `@cImport`ing the same
`apiplant.h` — so this example is really about what the contract looks like with
slices, `defer` and error unions instead of raw pointers and manual frees.

```
10-zig-functions/
├── main.toml
├── models/
│   └── note.toml       # something for the function to count
└── functions/
    ├── hello.zig       # both endpoints below, one file
    ├── hello.toml      #   …config for `hello`
    └── libhello.so     # ← produced by `apiplant build`
```

| Endpoint | Visibility | What it shows |
|---|---|---|
| `POST /api/functions/hello` | public | config, logging, JSON in and out |
| `GET /api/functions/notes` | authenticated | querying Postgres, reading the caller id |

## Running it

```bash
createdb -h 127.0.0.1 -p 55432 -U postgres apiplant_zig_functions

cargo run -p apiplant -- build examples/10-zig-functions   # zig build-lib -dynamic
cargo run -p apiplant -- run examples/10-zig-functions
```

```bash
curl -X POST localhost:8099/api/functions/hello \
  -H 'content-type: application/json' -d '{"name":"Federico"}'
# {"message":"Ciao, Federico!","compiled_by":"zig 0.16.0"}

TOKEN=$(curl -sX POST localhost:8099/api/auth/register \
  -H 'content-type: application/json' \
  -d '{"email":"z@example.com","password":"hunter2hunter2"}' | jq -r .token)

curl -X POST localhost:8099/api/note \
  -H 'content-type: application/json' -d '{"title":"alpha"}'

curl localhost:8099/api/functions/notes -H "Authorization: Bearer $TOKEN"
# {"notes":1,"caller":"51fa5c2d-…"}
```

## What Zig adds over the C version

`defer` is the real win. Every string the host hands over has to go back through
`free_string`, and in C that means remembering it on every return path:

```zig
const raw = host.query.?(host.ctx, request);
if (raw == null) { … }
defer host.free_string.?(host.ctx, raw);   // paired once, honoured everywhere
```

`@cImport` means there is no second declaration of the ABI to keep in sync — the
header *is* the binding. Function pointers arrive as optionals, hence the `.?`.

## Two things to know

**Zig panics abort the host.** A failed safety check — integer overflow, an
out-of-bounds slice — calls Zig's panic handler, and there is no unwinding to
catch it, so it takes the whole server down. This is the same constraint the C ABI
states as "must not unwind or longjmp": handle your errors and return
`APIPLANT_ERR_INTERNAL`. Rust functions get a firewall for free because they can
unwind; C and Zig cannot.

Because of that, `apiplant build` keeps safety checks **on** in both profiles
(`Debug` and `ReleaseSafe`). `ReleaseFast`/`ReleaseSmall` would save a few KB and
turn those checks into undefined behaviour inside your server process, which is a
bad trade at any size.

**`std.json` exists — use it.** `hello.zig` parses JSON by hand only to stay
comparable to the C version line for line. Real code should not.

## Size

| Profile | Size |
|---|---|
| `apiplant build` (Debug) | ~7.9 MB |
| `apiplant build --release` (ReleaseSafe, stripped) | **~288 KB** |

Zig's `Debug` mode is bulky because it compiles in the safety and stack-trace
machinery, not because of debug info — so unlike the Rust profile, stripping it
would cost the traces and save comparatively little. Ship `--release`.
