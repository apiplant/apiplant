# 12 · Functions with dependencies

Examples 07–11 wrote each function as a single source file. That is enough until
a function needs a third-party crate, a second source file, or a linked library.
For that, an entry in `functions/` can be a **directory** instead of a file — a
self-contained project in the language's own native form, which is exactly where
its dependency machinery already lives.

This app has one directory function per language, and each one reaches for
something a single file couldn't:

```
12-function-dependencies/
├── main.toml
└── functions/
    ├── token/            # Rust crate — depends on the `uuid` crate
    │   ├── Cargo.toml    #   your manifest: any dependencies, any modules
    │   └── src/lib.rs
    ├── token.toml        #   config still lives here, keyed by the dir name
    ├── checksum/         # C — split across two .c files + a header
    │   ├── hello.c
    │   ├── checksum.c
    │   └── checksum.h
    ├── reverse/          # Go module — own go.mod, code in two files
    │   ├── go.mod
    │   ├── main.go
    │   └── strutil.go
    ├── answer/           # Zig — a root file that @imports a helper
    │   ├── answer.zig
    │   └── mathutil.zig
    └── lib*.so           # ← one per directory, produced by `apiplant build`
```

A directory named `token/` compiles to `libtoken.so` beside it, loaded exactly
like a single-file function. **How apiplant reads a directory:**

| A directory containing… | is built as | how |
|-------------------------|-------------|-----|
| `Cargo.toml`            | Rust | `cargo build` on your crate — any crates.io dependency |
| `go.mod`                | Go   | `go build -buildmode=c-shared` on your module |
| `.c` files              | C    | `cc` over every `.c`, with the directory on the include path |
| `.zig` files            | Zig  | `zig build-lib` from `answer/answer.zig` (named for the directory) |

| Endpoint | Language | What it shows |
|---|---|---|
| `POST /api/functions/token` | Rust | depending on the third-party `uuid` crate |
| `POST /api/functions/checksum` | C | one function split across two `.c` files |
| `POST /api/functions/reverse` | Go | a module with its own `go.mod` and a helper file |
| `POST /api/functions/answer` | Zig | a root `.zig` that `@import`s a sibling |

For Rust and Go the project is yours: apiplant runs your `Cargo.toml` / `go.mod`
unchanged and copies out the library it produces, so you own the dependencies
*and* the build profiles. (`token/Cargo.toml` sets the same size-reducing
profiles `apiplant build` injects for single-file functions — see
[the Functions guide](../../docs/functions.md#library-size).)

## Running it

Building needs all four toolchains on `PATH` — `cargo`, `cc`, `go` and `zig`.
Build only the languages you have by removing the other directories.

```bash
createdb -h 127.0.0.1 -p 55432 -U postgres apiplant_function_deps

cargo run -p apiplant -- build --release examples/12-function-dependencies
cargo run -p apiplant -- examples/12-function-dependencies
```

```bash
# Rust, using the uuid crate:
curl -sX POST localhost:8099/api/functions/token \
  -H 'content-type: application/json' -d '{"count":2}'
# {"tokens":["9aed2248-…","92d54a13-…"]}

# C, hashing the body with a helper from a second file:
curl -sX POST localhost:8099/api/functions/checksum \
  -H 'content-type: application/json' -d '{"hello":"world"}'
# {"checksum":3050092219}

# Go, reversing a string via a helper file:
curl -sX POST localhost:8099/api/functions/reverse \
  -H 'content-type: application/json' -d '{"text":"apiplant"}'
# {"reversed":"tnalpipa"}

# Zig, 10! computed in an @import'ed module:
curl -sX POST localhost:8099/api/functions/answer \
  -H 'content-type: application/json' -d '{}'
# {"answer":3628800}
```

Nothing else in the app knows a function came from a directory: each one is
mounted, access-controlled and documented exactly like a single-file function,
and can serve as a [lifecycle hook](../08-hooks) too.

## Single file vs. directory

Reach for a directory only when a single file can't express what you need:

* **a dependency** — a crate, a Go module (Rust and Go);
* **more than one source file** — splitting the implementation up (all four);
* **your own build setup** — profiles, build flags, linked libraries.

Otherwise a single `.rs` / `.c` / `.zig` / `.go` file (examples 07–11) is less to
carry. The two forms live side by side in the same `functions/` directory.
