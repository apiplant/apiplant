---
name: apiplant-app
description: Build a REST API application with apiplant — a directory of TOML resource definitions, permissions, seed data and compiled functions served by the apiplant binary. Use when creating or modifying an apiplant app: writing resources/*.toml, main.toml, seed data, agents, lifecycle hooks or functions, wiring auth, multitenancy, permissions, relationships, queues, storage, email, payments or AI, and when running apiplant init/seed/run/build.
---

# Building apiplant apps

apiplant serves an **app directory**. There is no server code: resources are
TOML, the database is migrated to match at boot, CRUD endpoints and their
permissions are generated, and compiled functions are loaded from disk.

```
my-app/
├── main.toml       # optional server/db/auth config; safe defaults if absent
├── https/          # cert + key here => the server runs HTTPS
├── resources/      # one <name>.toml per resource => a table + CRUD endpoints
├── seed/           # optional <resource>.toml|csv => initial rows
├── agents/         # optional <name>.toml per AI agent
└── functions/      # function sources, their config, and built libraries
```

## Workflow

1. **Scaffold.** `apiplant init <dir>` writes a sample app (one resource, seed
   rows, one function). For an existing directory, create `resources/` and add
   files to it — every part of the tree is optional.
2. **Model the data first.** One `resources/<name>.toml` per entity. Copy the
   shape from `examples/02-resources/resources/note.toml`, then check field
   types and options against [references/resources.md](references/resources.md).
   Do not invent field types — the list is closed.
3. **Decide scope before writing permissions.** `scope = "organization"` (the
   default) isolates rows per tenant; `scope = "global"` opts out. Read
   [references/multitenancy.md](references/multitenancy.md) once per app, not
   once per resource.
4. **Set every permission explicitly.** `list/read/create/update/delete` each
   take a policy. `"public"` is almost never right outside an example. See
   [references/permissions.md](references/permissions.md).
5. **Link resources with `reference` fields**, not with ad-hoc id columns —
   they produce real foreign keys, nested endpoints and `?expand=`.
6. **Seed.** Put an administrator and some rows in `seed/`. `apiplant seed
   <dir>` is re-runnable and does not duplicate.
7. **Add behaviour only where configuration cannot reach.** A function is a
   compiled library mounted as an endpoint; a hook is that function attached to
   a CRUD lifecycle event. Write plain `.rs` files and let `apiplant build`
   compile them. See [references/functions.md](references/functions.md) and
   [references/hooks.md](references/hooks.md).
8. **Run and verify.** `apiplant run <dir>` serves on
   `http://127.0.0.1:8099/api`. Confirm the endpoints with curl against
   [references/api-reference.md](references/api-reference.md) before declaring
   the app done.

## Commands

```bash
apiplant init  <dir>            # scaffold (also takes a git URL as a template)
apiplant build <dir>            # compile functions/ into loadable libraries
apiplant seed  <dir>            # migrate, then load seed/
apiplant run   <dir>            # serve the app
apiplant cli   <dir>            # the admin dashboard, in a terminal
```

A Postgres URL is required — `$DATABASE_URL`, or `[database] url` in
`main.toml`. Any string in any config file can reference the environment
(`url = "$DATABASE_URL"`, `region = "${AWS_REGION:-eu-west-1}"`), so committed
files hold no credentials.

## Rules that prevent the common failures

- `organization`, `membership`, `user`, `api_key` and `oauth_connection` exist
  already. To extend one, drop a file with the *same name* — do not define a
  parallel resource.
- Migrations are additive. A renamed field is a new column plus an orphan, not
  a rename; plan the schema before seeding production data.
- Functions are compiled artefacts. After editing a function source, `apiplant
  build` must run before `apiplant run` picks the change up.
- The container image has no shell or toolchain: build functions before
  mounting the directory.
- Before exposing a server, walk [references/security.md](references/security.md).

## Reference

Read the guide for the area you are touching; they are the authority, this file
is only the map.

| Guide | What's in it |
|-------|--------------|
| [resources.md](references/resources.md) | Define a resource: field types, options, scope, migrations |
| [configuration.md](references/configuration.md) | `main.toml`: server, database, auth, TLS, workers, env vars |
| [permissions.md](references/permissions.md) | Per-action policies, ownership, org roles |
| [relationships.md](references/relationships.md) | `reference` fields, `has_many`, `?expand=`, `on_delete` |
| [multitenancy.md](references/multitenancy.md) | Organisations, memberships, per-tenant isolation |
| [authentication.md](references/authentication.md) | Users, API keys, sessions, OAuth, extending `user` |
| [seed.md](references/seed.md) | `seed/`: initial rows in TOML or CSV |
| [functions.md](references/functions.md) | Compiled plugins over the stable ABI (Rust, C, Zig, Go, TypeScript) |
| [hooks.md](references/hooks.md) | Attaching functions to CRUD lifecycle events |
| [api-reference.md](references/api-reference.md) | Every endpoint, query parameter and status code |
| [queues.md](references/queues.md) | `publish` from a function, `[queues.subscribe]` on Postgres alone |
| [storage.md](references/storage.md) | The `file` field type, directory or S3-compatible bucket |
| [email.md](references/email.md) | One `[email]` provider, `ctx.send_email` |
| [caching.md](references/caching.md) | The optional `[cache]` Redis a function can reach |
| [payments.md](references/payments.md) | Catalogue, subscriptions, checkout, tax |
| [ai.md](references/ai.md) | `[ai]` provider, `agents/`, `ctx.chat`, streaming |
| [admin.md](references/admin.md) | The built-in operator UI and `[admin]` config |
| [cli.md](references/cli.md) | `apiplant cli`: the dashboard in a terminal |
| [security.md](references/security.md) | What to configure before exposing the server |
| [openapi.md](references/openapi.md) | The generated spec and Swagger UI |

## Example apps

Complete, runnable apps under `examples/`, each introducing one concept. Read
the `README.md` inside one before copying from it.

| Example | Concept |
|---------|---------|
| `01-hello-world` | Hello world |
| `02-resources` | Resources |
| `03-relationships` | Relationships |
| `04-multitenancy` | Multitenancy |
| `05-auth` | Authentication |
| `06-permissions` | Permissions |
| `07-functions` | Functions |
| `08-hooks` | Hooks (the full app) |
| `09-c-functions` | Functions in C |
| `10-zig-functions` | Functions in Zig |
| `11-go-functions` | Functions in Go |
| `12-function-dependencies` | Functions with dependencies |
| `13-real-world` | A real-world app |
| `14-email-domains` | Email domains (auto-join on registration) |
| `15-email` | Email (one provider, named in config) |
| `16-caching` | Caching (optional Redis, for functions) |
| `17-typescript-functions` | Functions in TypeScript |
| `18-payments` | Payments (Stripe, as ordinary resources) |
| `19-ai` | AI (one provider, streamed) |
| `20-streaming` | Streaming functions |
| `21-docker` | Docker |
| `22-oauth` | OAuth |
| `23-queues` | Queues |
| `24-nested-resources` | Nested resources |
| `25-observability` | Observability |
| `26-file-upload` | File upload |
