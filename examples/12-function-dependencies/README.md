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
├── models/
│   └── article.toml      # something for `slug` to check names against
└── functions/
    ├── token/            # Rust crate — depends on the `uuid` crate
    │   ├── Cargo.toml    #   your manifest: any dependencies, any modules
    │   └── src/lib.rs
    ├── token.toml        #   config still lives here, keyed by the dir name
    ├── slug/             # npm project — depends on the `slugify` package
    │   ├── package.json  #   your dependencies, and the build script apiplant runs
    │   ├── tsconfig.json
    │   └── src/index.ts  #   plus src/reserved.ts beside it
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
    ├── lib*.so           # ← one per compiled directory, from `apiplant build`
    └── slug.js           # ← and the bundle, for the TypeScript one
```

A directory named `token/` compiles to `libtoken.so` beside it, loaded exactly
like a single-file function. **How apiplant reads a directory:**

| A directory containing… | is built as | how |
|-------------------------|-------------|-----|
| `Cargo.toml`            | Rust | `cargo build` on your crate — any crates.io dependency |
| `go.mod`                | Go   | `go build -buildmode=c-shared` on your module |
| `package.json`          | TypeScript | `pnpm install` once, then your own `build` script |
| `.c` files              | C    | `cc` over every `.c`, with the directory on the include path |
| `.zig` files            | Zig  | `zig build-lib` from `answer/answer.zig` (named for the directory) |

| Endpoint | Language | What it shows |
|---|---|---|
| `POST /api/functions/token` | Rust | depending on the third-party `uuid` crate |
| `POST /api/functions/slug` | TypeScript | an npm package, a sibling module, and a query |
| `POST /api/functions/checksum` | C | one function split across two `.c` files |
| `POST /api/functions/reverse` | Go | a module with its own `go.mod` and a helper file |
| `POST /api/functions/answer` | Zig | a root `.zig` that `@import`s a sibling |

For Rust, Go and TypeScript the project is yours: apiplant runs your
`Cargo.toml` / `go.mod` / `package.json` unchanged and copies out what it
produced, so you own the dependencies *and* the build. (`token/Cargo.toml` sets the same size-reducing
profiles `apiplant build` injects for single-file functions — see
[the Functions guide](../../docs/functions.md#library-size).)

## Running it

Building needs every toolchain on `PATH` — `cargo`, `cc`, `go`, `zig`, and for
the TypeScript one a package manager (`pnpm` by default) plus node. Build only
the languages you have by removing the other directories.

The TypeScript directory installs from npm the first time, which is the only step
here that needs the network. It is skipped on later builds: `node_modules` being
there is what says the dependencies are installed.

```bash
createdb -h 127.0.0.1 -p 5432 -U postgres apiplant_function_deps

cargo run -p apiplant -- build --release examples/12-function-dependencies
cargo run -p apiplant -- run examples/12-function-dependencies
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

# TypeScript, slugifying with the `slugify` package from npm:
curl -sX POST localhost:8099/api/functions/slug \
  -H 'content-type: application/json' -d '{"title":"Héllo, Wörld! A Post"}'
# {"slug":"hello-world-a-post","reserved":false,"taken":false}

# …and its sibling module, which knows which slugs are spoken for:
curl -sX POST localhost:8099/api/functions/slug \
  -H 'content-type: application/json' -d '{"title":"Admin"}'
# {"slug":"admin-page","reserved":true,"taken":false}
```

Nothing else in the app knows a function came from a directory: each one is
mounted, access-controlled and documented exactly like a single-file function,
and can serve as a [lifecycle hook](../08-hooks) too.

## Single file vs. directory

Reach for a directory only when a single file can't express what you need:

* **a dependency** — a crate, a Go module, an npm package;
* **more than one source file** — splitting the implementation up (all five);
* **your own build setup** — profiles, build flags, linked libraries, a bundler.

Otherwise a single `.rs` / `.ts` / `.c` / `.zig` / `.go` file (examples 07–11 and
17) is less to carry — and for TypeScript it is also the form that needs no
toolchain at all, since `apiplant build` transpiles a lone `.ts` itself. The two
forms live side by side in the same `functions/` directory.

The TypeScript directory is where that trade is clearest. `slug/src/index.ts`
opens with three imports a single file could not have written:

```ts
import slugify from "slugify";              // an npm package
import { defineFunctions, db, s } from "apiplant";   // the host, left external
import { isReserved } from "./reserved.ts"; // a sibling module
```

`apiplant` is the one thing the bundler must *not* inline — the host provides it
to the isolate — which is what `--external:apiplant` in the build script is for.
Everything else is bundled in, so what the server loads is still one
self-contained file.

**Next:** [13 · A real-world app](../13-real-world) — every idea so far in one
20-resource domain model.
