# Examples

Twenty self-contained apps, each adding one idea to the last. Every directory
is a complete app: point the binary at it and it runs.

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
| 12 | [function-dependencies](12-function-dependencies) | function *directories*: crates, modules, npm projects, multi-file — with dependencies |
| 13 | [real-world](13-real-world) | all of it at once: 20 resources, self-references, join rows, ledgers |
| 14 | [email-domains](14-email-domains) | a hook on registration: joining the org that owns your email domain |
| 15 | [email](15-email) | sending mail: one `[email]` provider, named in config |
| 16 | [caching](16-caching) | the optional `[cache]` Redis, reached from a function |
| 17 | [typescript-functions](17-typescript-functions) | and in TypeScript: the `apiplant` module, transpiled at build time and run in a V8 isolate |
| 18 | [payments](18-payments) | taking money: `[payments]`, a catalogue as resources, a paywall as a hook |
| 19 | [ai](19-ai) | an assistant: one `[ai]` provider, configured `agents/`, a chat endpoint that streams, a function in front of the model, live tokens in the admin action |
| 20 | [streaming](20-streaming) | any function's second endpoint: `/stream`, sending the answer as it is produced and into the admin action view |

## Running one

Each example owns its database, so create that once, then run:

```bash
createdb -h 127.0.0.1 -p 5432 -U postgres apiplant_resources
cargo run -p apiplant -- run --seed examples/02-resources
```

`--seed` loads the example's `seed/` directory after migrating — see **Signing
in** below. Leave it off and the app runs against an empty database, which is
what you want the second time.

Examples 07–12, 14–17 and 19–20 have functions, so build them first. Rust functions need
`cargo` on PATH; C, Zig and Go need their own toolchain (`cc`, `zig`, `go`). A
single `.ts` file needs nothing — `apiplant build` transpiles it itself — but the
TypeScript *directory* in example 12 is an npm project, so that one needs node
and `pnpm`. Example 12 needs the lot:

```bash
createdb -h 127.0.0.1 -p 5432 -U postgres apiplant_hooks
cargo run -p apiplant -- build examples/08-hooks
cargo run -p apiplant -- run --seed examples/08-hooks
```

A function can be a single source file or a whole directory (a crate, a Go
module, an npm project, a set of C or Zig files) — example 12 shows the directory
form, which is how a function pulls in dependencies.

Each example serves on `127.0.0.1:8099` under `/api`, with Swagger UI at
<http://127.0.0.1:8099/api/docs>. Run one at a time.

Three of them want a service alongside Postgres: example 15 sends mail to a
local catcher on port 1025 (Mailpit, MailHog, `python3 -m aiosmtpd -n -l :1025`),
example 16 needs a Redis on 6379, and example 19 talks to a model on 8080 —
llama.cpp, vLLM or LM Studio, or `AI_ENDPOINT=http://localhost:11434` for
Ollama. Example 19 runs against OpenAI or Anthropic instead by editing three
lines. Each README says so, and each app boots with
a clear error rather than a mystery if the service isn't there.

## Signing in

Every example carries the same fixture in `seed/`, so whichever one you start
there is already somebody to be — at the dashboard on
<http://127.0.0.1:8099/admin/>, in `apiplant cli`, or over the API:

| | |
|---|---|
| **Email** | `admin@example.com` — example 05 signs in as `admin`, since it keys its user by username |
| **Password** | `password` |
| **Organisation** | Acme, Inc., which that account administers |

Two more accounts, `editor@example.com` and `member@example.com`, same
password, hold different roles in Acme — a permission is much easier to
understand from two sessions than from a paragraph. Underneath, each example
seeds its own subject: notes, books and reviews, projects and tasks, and in
example 13 a whole business — products, stock, orders, shipments, payments,
refunds and support tickets.

Seeding is idempotent (ids are derived from the names in the files, and inserts
are `ON CONFLICT DO NOTHING`), so `--seed` on every run is harmless and never
overwrites a row you changed. `cargo run -p apiplant -- seed examples/13-real-world`
loads a fixture without starting a server. See [Seed data](../docs/seed.md).

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
| 13-real-world | `apiplant_real_world` |
| 14-email-domains | `apiplant_domains` |
| 15-email | `apiplant_email` |
| 16-caching | `apiplant_caching` |
| 18-payments | `apiplant_payments` |
| 19-ai | `apiplant_ai` |
| 20-streaming | `apiplant_streaming` |

Separate databases keep the examples genuinely independent: example 05 redefines
`user` to log in by username, which cannot share a table with the built-in
email-based one. Create them all at once:

```bash
for db in hello resources relationships tenancy auth permissions functions hooks; do
  createdb -h 127.0.0.1 -p 5432 -U postgres "apiplant_$db"
done
```

Tables appear on first boot — `auto_migrate` reconciles the database with the
models. To reset an example, drop and recreate its database.

## Reading them

Each example's `README.md` explains the idea and gives curl commands to try; the
TOML files are commented too. They're meant to be read in order — later examples
assume the earlier ones.
