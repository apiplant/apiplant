# apiplant documentation

apiplant turns an **app directory** into a running, database-backed REST API.
There is no server code to write: declare resources, permissions and
relationships in TOML, add optional compiled functions, and run the `apiplant`
binary against the directory.

These guides cover the full feature set. The [examples](../examples/) take a
more hands-on route, progressing from a bare `main.toml` to a complete app with
lifecycle hooks, introducing one concept at a time.

| Guide | What's in it |
|-------|--------------|
| [Configuration](configuration.md) | `main.toml` reference, TLS, database, workers |
| [Resources](resources.md) | defining resources, field types & options, scope, migrations |
| [Permissions](permissions.md) | the access model, per-action policies, ownership, org roles |
| [Seed data](seed.md) | `seed/`: an app's initial rows, in TOML or CSV |
| [Multitenancy](multitenancy.md) | organisations, memberships, automatic per-tenant isolation |
| [Relationships](relationships.md) | references, `has_many`, expansion, filtering, `on_delete` |
| [Authentication](authentication.md) | users, organisations, API keys, sessions, extending `user` |
| [Functions](functions.md) | writing & loading compiled plugins over the stable ABI |
| [Lifecycle hooks](hooks.md) | running functions before/after every CRUD operation |
| [Sending email](email.md) | one `[email]` provider: SMTP, SES, SendGrid, Brevo, Mailjet and others |
| [Caching](caching.md) | the optional `[cache]` Redis a function can reach |
| [Payments](payments.md) | one `[payments]` provider: catalogue, subscriptions, checkout, tax |
| [AI](ai.md) | one `[ai]` provider: a streaming chat endpoint, configured `agents/`, `ctx.chat`, streaming functions, live admin action output |
| [Admin dashboard](admin.md) | the built-in operator UI, `[admin]` config, action forms |
| [The console](cli.md) | `apiplant cli`: the dashboard's functionality in a terminal |
| [Security model](security.md) | what the server enforces, and what you must configure before exposing it |
| [API reference](api-reference.md) | every endpoint, query parameter and status code |
| [OpenAPI & Swagger UI](openapi.md) | the generated spec and interactive docs |

## Install

Prebuilt binaries for Linux (x86_64, aarch64) and macOS (Apple silicon) are
attached to every [release](https://github.com/apiplant/apiplant/releases), each
with a `.sha256` next to it:

```bash
# Linux x86_64 — swap the target triple for yours.
curl -sSfL https://github.com/apiplant/apiplant/releases/latest/download/apiplant-v0.5.0-x86_64-unknown-linux-gnu.tar.gz \
  | tar xz --strip-components=1
sudo mv apiplant /usr/local/bin/
```

Or run the container image, published to the GitHub registry for `linux/amd64`
and `linux/arm64`. The image tags carry no `v` prefix — `0.5.0`, `0.5`, or
`latest`:

```bash
docker pull ghcr.io/apiplant/apiplant:0.5.0   # or :0.4, or :latest
docker run --rm -p 8080:8080 -v "$PWD:/app" ghcr.io/apiplant/apiplant:latest run /app
```

The image carries the server only. Compiling `functions/*` needs the toolchain
for whichever language you wrote them in, so run `apiplant build` before
mounting the directory — TypeScript functions need nothing, they are transpiled
and run in-process.

Or from crates.io, or from source:

```bash
cargo install apiplant
# or
cargo build --release --bin apiplant
```

## Starting an app

```bash
apiplant init my-app     # a sample app: one resource, seed rows, one function
apiplant seed my-app     # create the tables, load seed/
apiplant run  my-app     # serve it on http://127.0.0.1:8099/api
```

`init` also accepts a git URL
(`apiplant init my-app https://github.com/acme/template`) to start from your own
template; the clone retains none of the source history. It writes only into an
empty or not-yet-existing directory.

## The 60-second model

```
my-app/
├── main.toml       # optional server/db/auth/docs config; safe defaults if absent
├── https/          # cert + key here ⇒ the server runs HTTPS
├── models/         # one <name>.toml per resource ⇒ a table + CRUD endpoints
├── seed/           # optional <resource>.toml|csv ⇒ initial rows
├── agents/         # optional <name>.toml per configured AI agent
└── functions/      # function sources (.rs), their config, and built libraries
```

* A **resource** (`models/post.toml`) becomes a Postgres table and a set of
  RESTful endpoints, each gated by a per-action **permission**.
* `organization`, `membership`, `user`, `api_key` and `oauth_connection`
  resources exist by default and can be extended by dropping a file with the
  same name.
* **Relationships** come from `reference` fields, enforced with real foreign
  keys, navigable via nested endpoints, and inlinable with `?expand=`.
* **Functions** are separately-compiled libraries mounted as endpoints, talking
  to the host over a stable C ABI. Write them as plain `.rs` files and let
  `apiplant build` compile them.
* **Agents** are optional `agents/*.toml` files: named AI chat surfaces with a
  fixed prompt, their own access policy, and optionally persisted history.
* **Hooks** attach those functions to a resource's lifecycle (`before_create`,
  `after_list`, …) so custom logic can validate, rewrite or observe every CRUD
  operation.
* **Signing in with GitHub, Google, LinkedIn or X** is two credentials in an
  `[oauth.<provider>]` block: the redirect, the callback, PKCE, the state table
  and the account matching are the framework's, and the session it issues is the
  one `POST /auth/login` issues.
* **Email** and a **cache** are optional services a function can reach: name a
  provider in `[email]` and a Redis in `[cache]`, and `ctx.send_email(…)` /
  `ctx.cache_get(…)` work. Neither is used by the framework itself.
* **Payments** are optional too, and go further: naming a provider in
  `[payments]` adds a catalogue, subscriptions and a checkout as ordinary
  resources and endpoints, so billing arrives with the permissions and roles
  everything else already has.
* An **admin dashboard** is generated from all of the above: a static,
  self-hosted operator UI, configurable per resource and per function with
  `[admin]`.
* Migrations are automatic and additive: your schemas *are* the desired state.
* **Seed data** is a `seed/` directory of TOML or CSV files named after
  resources, providing an administrator who can sign in and rows to work with.
  It is loaded by `apiplant seed` and can be re-run without creating duplicates.
  See [Seed data](seed.md).
* Any string in any of these files can reference the environment
  (`url = "$DATABASE_URL"`, `region = "${AWS_REGION:-eu-west-1}"`), so committed
  files hold no credentials. See
  [Configuration](configuration.md#environment-variables).

Everything is optional. An empty directory is a valid, if bare, app.
