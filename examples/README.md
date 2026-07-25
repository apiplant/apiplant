# Examples

Twelve self-contained apps, each adding one idea to the last. Every directory is
a complete app: point the binary at it and it runs.

| # | Example | Adds |
|---|---------|------|
| 01 | [hello-world](01-hello-world) | just `main.toml` — the smallest app that boots |
| 02 | [resources](02-resources) | your first table: fields, types, options, migrations |
| 03 | [relationships](03-relationships) | foreign keys, `?expand=`, nested collections, `on_delete` |
| 04 | [multitenancy](04-multitenancy) | organisation-scoped resources and automatic isolation |
| 05 | [auth](05-auth) | replacing the `user` model, sessions, API keys |
| 06 | [permissions](06-permissions) | per-action policies: public, owner, member, roles |
| 07 | [functions](07-functions) | writing custom code: compiled functions as endpoints |
| 08 | [hooks](08-hooks) | running those functions around every CRUD operation |
| 09 | [c-functions](09-c-functions) | the same idea in C: a function is just a shared library |
| 10 | [zig-functions](10-zig-functions) | and in Zig, reaching the ABI through `@cImport` |
| 11 | [go-functions](11-go-functions) | and in Go, via cgo and the standard library |
| 12 | [function-dependencies](12-function-dependencies) | function *directories*: crates, modules, multi-file — with dependencies |

## Running one

Each example owns its database, so create that once, then run:

```bash
createdb -h 127.0.0.1 -p 55432 -U postgres apiplant_resources
cargo run -p apiplant -- examples/02-resources
```

Examples 07–12 have functions, so build them first. Rust functions need `cargo`
on PATH; the others need their own toolchain (`cc`, `zig`, `go`), and example 12
needs all four:

```bash
createdb -h 127.0.0.1 -p 55432 -U postgres apiplant_hooks
cargo run -p apiplant -- build examples/08-hooks
cargo run -p apiplant -- examples/08-hooks
```

A function can be a single source file or a whole directory (a crate, a module, a
set of C or Zig files) — example 12 shows the directory form, which is how a
function pulls in dependencies.

Each example serves on `127.0.0.1:8099` under `/api`, with Swagger UI at
<http://127.0.0.1:8099/api/docs>. Run one at a time.

## The databases

| Example | Database |
|---------|----------|
| 01-hello-world | `apiplant_hello` |
| 02-resources | `apiplant_resources` |
| 03-relationships | `apiplant_relationships` |
| 04-multitenancy | `apiplant_tenancy` |
| 05-auth | `apiplant_auth` |
| 06-permissions | `apiplant_permissions` |
| 07-functions | `apiplant_functions` |
| 08-hooks | `apiplant_hooks` |
| 09-c-functions | `apiplant_c_functions` |
| 10-zig-functions | `apiplant_zig_functions` |
| 11-go-functions | `apiplant_go_functions` |
| 12-function-dependencies | `apiplant_function_deps` |

Separate databases keep the examples genuinely independent: example 05 redefines
`user` to log in by username, which cannot share a table with the built-in
email-based one. Create them all at once:

```bash
for db in hello resources relationships tenancy auth permissions functions hooks; do
  createdb -h 127.0.0.1 -p 55432 -U postgres "apiplant_$db"
done
```

Tables appear on first boot — `auto_migrate` reconciles the database with the
models. To reset an example, drop and recreate its database.

## Reading them

Each example's `README.md` explains the idea and gives curl commands to try; the
TOML files are commented too. They're meant to be read in order — later examples
assume the earlier ones.
