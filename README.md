# apiplant

**Ship one executable. Point it at a directory. Get a database-backed REST API.**

apiplant is a Rust web framework where your API is *configuration + plugins*, not
code you compile into the server. You run the `apiplant` binary against an **app
directory**; it reads your resource definitions, migrates a Postgres database to
match, generates RESTful CRUD endpoints with automatic per-organisation
isolation and per-resource permissions, wires up user/api-key/OAuth
authentication, and loads any compiled **function** libraries you drop in — all
at boot.

Functions are separately compiled shared libraries (`.so`/`.dylib`/`.dll`) that
talk to the host across a stable C ABI (via [`abi_stable`]), so you can write
them in any language, ship them independently, and never recompile the server.
They can be mounted as their own endpoints *and* attached to a resource's
lifecycle as **hooks**, running before or after any CRUD operation.

```
$ apiplant run ./my-app
  INFO apiplant_server: running migrations
  INFO apiplant_server:   fn greet -> /api/functions/greet
  INFO apiplant_server:   hook post.before_create -> post_before_create
  INFO apiplant_server: apiplant listening on http://127.0.0.1:8099/api
```

## Install

On macOS or Linux with [Homebrew](https://brew.sh):

```bash
brew tap apiplant/tap
brew install apiplant/tap/apiplant
```

On Arch Linux, from the pacman repository:

```bash
curl -sSfL https://apiplant.github.io/pacman/apiplant.gpg -o /tmp/apiplant.gpg
keyid=$(gpg --show-keys --with-colons /tmp/apiplant.gpg | awk -F: '/^pub:/ { print $5; exit }')
sudo pacman-key --add /tmp/apiplant.gpg
sudo pacman-key --finger "$keyid"
sudo pacman-key --lsign-key "$keyid"
printf '\n[apiplant]\nSigLevel = Required DatabaseOptional\nServer = https://apiplant.github.io/pacman/$arch\n' \
  | sudo tee -a /etc/pacman.conf > /dev/null

sudo pacman -Sy apiplant-bin
```

On Debian or Ubuntu, from the apt repository — `amd64` and `arm64`, and it
depends on nothing:

```bash
curl -sSfL https://apt.apiplant.com/apiplant-archive-keyring.gpg \
  | sudo tee /usr/share/keyrings/apiplant.gpg > /dev/null
echo "deb [signed-by=/usr/share/keyrings/apiplant.gpg] https://apt.apiplant.com stable main" \
  | sudo tee /etc/apt/sources.list.d/apiplant.list > /dev/null

sudo apt update && sudo apt install apiplant
```

`apt upgrade` picks up releases from then on. The suite is `stable` on every
Debian and Ubuntu — one package serves both, so the line survives a
distribution upgrade.

Or take the `.deb` from a
[release](https://github.com/apiplant/apiplant/releases) without adding the
repository, if you would rather pin a version or cannot use the apt repository:

```bash
curl -sSfLO https://github.com/apiplant/apiplant/releases/latest/download/apiplant_0.7.0-1_amd64.deb
sudo dpkg -i apiplant_0.7.0-1_amd64.deb
```

Either way it needs glibc 2.35 — Debian 12 (bookworm) or newer, Ubuntu 22.04 or
newer.

Prebuilt binaries for Linux (x86_64, aarch64) and macOS (Apple silicon) are
attached to every
[release](https://github.com/apiplant/apiplant/releases), each with a `.sha256`
next to it:

```bash
# Linux x86_64 — swap the target triple for yours.
curl -sSfL https://github.com/apiplant/apiplant/releases/latest/download/apiplant-v0.7.0-x86_64-unknown-linux-gnu.tar.gz \
  | tar xz --strip-components=1
sudo mv apiplant /usr/local/bin/
```

Or run the container image, which is published to the GitHub registry for
`linux/amd64` and `linux/arm64`:

The image tags carry no `v` prefix — `0.7.0`, `0.7`, or `latest`:

```bash
docker pull ghcr.io/apiplant/apiplant:0.7.0   # or :0.7, or :latest
docker run --rm -p 8080:8080 -v "$PWD:/app" ghcr.io/apiplant/apiplant:latest run /app
```

The image carries the server only: glibc, libgcc and the binary, on
`gcr.io/distroless/cc-debian12` — about 125MB, most of which is the binary.
There is no shell and no package manager in it, so compiling `functions/*`
happens elsewhere: run `apiplant build` before mounting the directory, or build
in a stage that has the toolchain and copy the libraries in (see
[`examples/21-docker`](examples/21-docker)). TypeScript functions need nothing,
they are transpiled and run in-process. `apiplant init --from <repo>` is the
other thing the image cannot do — it clones with `git`, which is not in there.

Or build it yourself, which is the same thing the release does:

```bash
cargo build --release --bin apiplant
```

## Documentation

This README is the tour. The [`docs/`](docs/) directory is the full reference:

| Guide | Covers |
|-------|--------|
| [Configuration](docs/configuration.md) | `main.toml`, TLS, database, workers |
| [Resources](docs/resources.md) | defining resources, field types & options, migrations |
| [Permissions](docs/permissions.md) | access levels, per-action policies, ownership, org roles |
| [Rate limiting](docs/configuration.md#rate_limit) | `[rate_limit]` app-wide, per resource action, per function |
| [Observability](docs/configuration.md#observability) | `[observability]` — OpenTelemetry traces & metrics, JSON logs, OTLP export |
| [Seed data](docs/seed.md) | `seed/` — the rows an app starts with, in TOML or CSV |
| [Multitenancy](docs/multitenancy.md) | organisations, memberships, automatic per-tenant isolation |
| [Relationships](docs/relationships.md) | references, `has_many`, expansion, filtering, `on_delete` |
| [Authentication](docs/authentication.md) | users, API keys, sessions, OAuth, extending `user` |
| [Functions](docs/functions.md) | writing & loading compiled plugins over the stable ABI |
| [Lifecycle hooks](docs/hooks.md) | running functions before/after every CRUD operation |
| [Sending email](docs/email.md) | one `[email]` provider — SMTP, SES, SendGrid, Brevo, Mailjet… |
| [Caching](docs/caching.md) | the optional `[cache]` Redis a function can reach |
| [Queues](docs/queues.md) | background work on Postgres alone: `publish`, `[queues.subscribe]`, retries |
| [File storage](docs/storage.md) | the `file` field type, `[storage]` on a directory or an S3-compatible bucket |
| [Payments](docs/payments.md) | one `[payments]` provider — catalogue, subscriptions, checkout, tax |
| [AI](docs/ai.md) | one `[ai]` provider — a streaming chat endpoint, `ctx.chat`, streaming functions |
| [Admin dashboard](docs/admin.md) | the built-in operator UI, `[admin]` config, action forms |
| [Security model](docs/security.md) | what the server enforces, and what you must configure before exposing it |
| [API reference](docs/api-reference.md) | every endpoint, query parameter and status code |
| [OpenAPI & Swagger UI](docs/openapi.md) | the generated spec and interactive docs |

---

## The app directory

Everything is optional — an empty directory is a valid app. The server fills in
safe defaults for anything you don't provide.

```
my-app/
├── main.toml            # server / database / auth config (all optional)
├── https/               # presence of a cert + key here → serve HTTPS
│   ├── cert.pem
│   └── key.pem
├── resources/           # one .toml per resource → a table + CRUD endpoints
│   ├── post.toml
│   └── users.toml       # optional: extend/override the built-in `user`
├── seed/                # optional: the rows the app starts with
│   ├── organization.toml
│   ├── user.toml        #   an administrator who can sign in
│   └── product.csv      #   …in TOML or CSV, whichever suits the table
├── storage/             # uploaded files, when [storage] backend is `local`
└── functions/           # function sources, their config, and the built libraries
    ├── greet.rs         # you write this…
    ├── greet.toml       #   …plus optional config
    └── libgreet.so      # …and `apiplant build` produces this
```

| Piece            | Default when absent                                        |
|------------------|------------------------------------------------------------|
| `main.toml`      | bind `0.0.0.0:8080`, base path `/`, autogenerated defaults |
| `https/`         | plain HTTP                                                  |
| `resources/*.toml`  | just the built-in `organization`, `membership`, `user`, `api_key`, `oauth_connection` |
| `seed/`          | an empty database — nobody to sign in as                    |
| `storage/`       | created on first boot; uploads land here unless `[storage]` names a bucket |
| `functions/`     | no function endpoints                                       |

Functions are ordinary `.rs` files in `functions/`, each written as if it were a
`lib.rs`. `apiplant build <dir>` wraps every source in a generated cdylib crate,
compiles it with cargo, and drops the library beside it — so the only toolchain
requirement is a working `cargo`.

When a function needs a dependency, a second source file, or its own build setup,
an entry in `functions/` can be a **directory** instead of a file — a real crate
(`Cargo.toml`), a Go module (`go.mod`), an npm project (`package.json`), or a set
of C/Zig files. apiplant builds it its native way — your manifest, your
dependencies, your bundler — and drops the result beside it, loaded just like a
single-file function. See
[`examples/12-function-dependencies`](examples/12-function-dependencies).

## Defining a resource

```toml
# resources/post.toml → GET/POST /api/post, GET/PATCH/DELETE /api/post/{id}
[resource]
name = "post"

[permissions]                 # public | authenticated | member | owner | role:<name> | private
list   = "member"
read   = "member"
create = "member"
update = "owner"              # only the row's owner may edit
delete = "role:admin"         # only admins of the active org may delete

[fields.title]
type = "string"
required = true
max_length = 200

[fields.body]
type = "text"

[fields.owner_id]            # a reference the framework auto-stamps on create
type = "reference"
references = "user"
```

Field types: `string`, `text`, `integer`, `big_int`, `float`, `boolean`,
`uuid`, `timestamp`, `json`, `file` (an uploaded file, held as the URL it is
served from — see [File storage](docs/storage.md)), and `reference` (with
`references = "<resource>"`).
Field options include `required`, `unique`, `hidden`, `max_length`, `default`,
and `on_delete` — see [Resources](docs/resources.md). Every resource gets a
`uuid` primary key and `created_at`/`updated_at` automatically. Fields marked
`hidden = true` (like password hashes) are stripped from every API response.

### Migrations

There are no migration files. Your schemas *are* the desired state: on boot
apiplant creates missing tables and adds missing columns idempotently. Set
`database.auto_migrate = false` to manage it yourself.

### Seed data

A schema with no rows is a form with nothing behind it: nobody can sign in and
every list is empty. A `seed/` directory is the fixture the app starts life
with — one file per resource, named after it, in TOML or CSV:

```toml
# seed/user.toml — sign in as admin@example.com / password
[[row]]
id = "admin"
email = "admin@example.com"
password = "password"        # hashed with argon2 on the way in
display_name = "Ada Admin"

# seed/membership.toml — which makes them an admin of the organisation above
[[row]]
user_id = "admin"
organization_id = "acme"
role = "admin"
```

Ids are written as names. `acme` is hashed into the same UUID every time, on
every machine, so one file points at another's rows by a readable word — and,
because the ids are derived rather than random, `apiplant seed` inserts each
row once no matter how often you run it, and never overwrites one you have
since edited. `apiplant run --seed` does the same on the way up. See
[Seed data](docs/seed.md).

## Authentication & authorization

`organization`, `membership`, `user`, `api_key`, and `oauth_connection` are
ordinary resources with built-in defaults; drop a same-named file in `resources/`
(for `user`, `resources/users.toml`) to extend them. The `user` resource carries an
`[auth]` section (configurable identity/password field names, OAuth providers).
Built-in endpoints:

| Endpoint                      | Purpose                                            |
|-------------------------------|----------------------------------------------------|
| `POST <base>/auth/register`   | create a user, returns a session JWT               |
| `POST <base>/auth/login`      | email + password → session JWT                     |
| `POST <base>/auth/apikeys`    | issue an API key for the caller (shown once)       |

Requests authenticate via `Authorization: Bearer <jwt>` or
`Authorization: ApiKey <key>`. An API key acts **as its owning user** — same
identity, same permissions. Passwords are argon2id-hashed; API keys are stored
as SHA-256 and never recoverable.

Each request's permission check turns a resource's `Access` policy plus the
caller into *allow* / *allow-if-owner* / *deny*. `owner` policies transparently
scope queries to rows the caller owns.

### Signing in with GitHub, Google, LinkedIn or X

Two credentials, and the handshake is the framework's problem:

```toml
[oauth.github]
client_id     = "${GITHUB_CLIENT_ID}"
client_secret = "${GITHUB_CLIENT_SECRET}"
```

That mounts `<base>/auth/oauth/…`, and

```html
<a href="/api/auth/oauth/github/start">Sign in with GitHub</a>
```

is the client side in full: the endpoint redirects to the provider, the provider
redirects back into the API, and the browser lands on your page with a session
token — the same JWT `POST /auth/login` issues, accepted everywhere without
anything knowing OAuth exists. apiplant knows each provider's endpoints, scopes,
PKCE and profile shape; anything speaking OpenID Connect can be added with three
URLs and no code.

Behind it: `state` and PKCE in a table, single-use codes, accounts matched only
on **verified** addresses, sign-ups that run the same hooks registration does,
and a refusal to unlink the last way into an account. See
[Authentication](docs/authentication.md#signing-in-with-somebody-elses-account)
and [`examples/22-oauth`](examples/22-oauth).

## Rate limiting

Off until asked for, then one line switches it on everywhere:

```toml
# main.toml
[rate_limit]
default = "100/1m"
```

A resource narrows or lifts it per action, and a function does the same in its
own config file — so the expensive endpoint can be strict while the cached one
is not limited at all:

```toml
# resources/order.toml
[rate_limit]
create = "5/1m"
list   = "off"
```

```toml
# functions/summarise.toml
rate_limit = "10/1m"
```

Clients are counted by their peer socket address, which they cannot forge;
behind a proxy, set `trust_proxy_headers = true` and have the proxy overwrite
`X-Forwarded-For`. Answers carry `X-RateLimit-Limit`, `-Remaining` and `-Reset`;
a refusal is `429` with `Retry-After`. See
[Configuration → `[rate_limit]`](docs/configuration.md#rate_limit).

## Observability

Off until asked for, then one line gives every request a span, structured logs
that carry its trace id, and an `X-Trace-Id` the caller can quote back:

```toml
# main.toml
[observability]
enabled = true

[observability.logs]
format = "json"
```

Point it at any OTLP collector — Jaeger, Tempo, Honeycomb, Datadog, Grafana
Cloud, the OpenTelemetry Collector — and the traces and metrics leave the
process:

```toml
[observability.otlp]
endpoint = "http://otel-collector:4318"
headers  = { authorization = "$OTEL_TOKEN" }
```

Spans follow the OpenTelemetry HTTP conventions, continue an incoming
`traceparent` so a trace does not stop at this service, and name what went
wrong when something does: a 500 is marked `ERROR` and carries an `error.type`
of `database`, `function_panic`, `hook_fault` and so on. Metrics are
`http.server.request.duration` and `http.server.active_requests`, labelled by a
route *template* (`/products/{id}`) rather than a path, so a busy table does not
become a time series per row. See
[Configuration → `[observability]`](docs/configuration.md#observability).

## Multitenancy

apiplant apps are multitenant by default. `organization` is the tenant,
`membership` joins users to organisations and carries their per-organisation
role, and every other resource is organisation-scoped unless you set
`scope = "global"`.

| Automatic behaviour on org-scoped resources | Effect |
|---------------------------------------------|--------|
| Active organisation | `X-Organization: <org-id>` on every org-scoped request — never implied |
| Query isolation | every list/read/update/delete filters to `organization_id = <active org>` |
| Create stamping | `organization_id` is filled in by the server; clients cannot spoof it |
| Update refusal | `organization_id` in a body is dropped, so a row cannot be moved between organisations |
| Role checks | `role:admin` means admin of the active organisation |
| Organisation classes | `role:admin@org_class=school` narrows a permission to organisations of one class; `org_class` is server-owned, written only by `[organization] org_class_editors` |

Single-org users never need the header; multi-org users send it per request.
Full details in [Multitenancy](docs/multitenancy.md).

## Interactive API docs (OpenAPI + Swagger UI)

apiplant generates an OpenAPI 3.0 document from your app — every resource
becomes CRUD paths with request/response schemas, the auth routes and each
loaded function are described, and hidden fields never appear. It's served,
along with a Swagger UI, with no configuration:

| Endpoint               | What                                             |
|------------------------|--------------------------------------------------|
| `GET <base>/openapi.json` | the generated OpenAPI 3.0 spec                |
| `GET <base>/docs`      | Swagger UI (try requests straight from the browser) |

**Auth works in the UI.** Two security schemes are declared and attached to
every operation that needs them, so Swagger's **Authorize** button is live:

* `bearerAuth` — paste a session token from `POST /auth/login`.
* `apiKeyAuth` — paste an API key; it's sent as the `X-Api-Key` header.

Public operations carry no security requirement; a `role:admin` delete is
documented as requiring the `admin` role in the active organisation.
Authorization persists across reloads.
Configure it in `main.toml`:

```toml
[docs]
enabled = true        # default
path = "/docs"        # where Swagger UI mounts (under base_path)
# title = "My API"    # defaults to [app] name — set it only if they differ
```

## Relationships

Add a `reference` field and you get a real relationship — a Postgres foreign
key, a reverse `has_many` endpoint, on-demand inlining, and filtering — with no
extra wiring.

```toml
# resources/comment.toml
[fields.post_id]
type       = "reference"
references = "post"
required   = true
on_delete  = "cascade"      # restrict (default) | set_null | cascade | no_action

[fields.owner_id]
type       = "reference"
references = "user"
```

From that single declaration:

```bash
GET /api/post/{id}/comment          # has_many: a post's comments (reverse side)
GET /api/comment?expand=post,owner  # inline the referenced records (batched, no N+1)
GET /api/comment?post_id=<uuid>     # filter by any field, incl. foreign keys
```

* **`belongs_to`** — the `reference` field itself (a FK-backed `uuid` column).
* **`has_many`** — the automatic `GET /parent/{id}/child` endpoint (add `?via=`
  when a child references the same parent more than once).
* **Expansion** — `?expand=owner` inlines the referenced record under `owner`
  (the field name minus `_id`); hidden fields stay hidden, and the target's own
  `read` permission decides whether the record inlines at all or as `null`.
* **`on_delete`** — enforced by Postgres (`restrict` blocks orphaning by
  default; `cascade` / `set_null` as configured).

Full details in [Relationships](docs/relationships.md).

## Functions: compiled plugins over a stable ABI

A function is a shared library exposing one root module through the
[`apiplant-abi`](crates/apiplant-abi) contract. Everything crosses the boundary
as JSON or small `#[repr(C)]` enums, so the host never shares a sea-orm, ntex, or
tokio type with your plugin — the ABI stays tiny and genuinely stable across
compiler/allocator versions.

With the `apiplant-function` crate you write **one typed Rust function** — no ABI
traits, no root-module export, no manual JSON. Types are inferred from the
handler's signature:

```rust
use apiplant_function::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Deserialize, JsonSchema)] struct Input { name: String }
#[derive(Serialize, JsonSchema)]   struct Output { message: String }

fn greet(ctx: &Context<()>, input: Input) -> Result<Output, String> {
    let users = ctx.query_one("SELECT count(*)::int AS n FROM apiplant_user", &[])?;  // borrow the host DB
    ctx.info("greet invoked");
    Ok(Output { message: format!("Hello, {}!", input.name) })
}

apiplant_function::function! {
    name: "greet",
    description: "Greets a person",
    method: Post,
    visibility: Public,   // public | authenticated | role-gated | private
    handler: greet,
}
```

The macro generates the ABI glue, reads/writes JSON, resolves typed config and
input, and turns any `Err(_)` into a `400`. Deriving `JsonSchema` on the input
and output makes the function **fully typed in the OpenAPI docs** (Swagger UI
renders a typed form). The host loads every library in `functions/`, mounts it at
`<base>/functions/<name>` with its declared method and visibility, and runs it on
a blocking worker with a [`Context`] giving it the database, config, and caller
identity. Use `functions! { {…}, {…} }` to export several from one library, each
with its own name and config. See
[`examples/07-functions`](examples/07-functions) and the
[Functions guide](docs/functions.md).

A panic in a handler fails only that request (`500`) rather than taking the
server down, and functions do not have to be Rust: a library can instead export
[four plain C symbols](crates/apiplant-abi/include/apiplant.h). `apiplant build`
compiles `.rs` with cargo, `.c` with cc, `.zig` with zig and `.go` with go, all
from the same `functions/` directory — see the same two endpoints written in
[C](examples/09-c-functions), [Zig](examples/10-zig-functions) and
[Go](examples/11-go-functions). Any of these can be a whole directory rather than
a single file when it needs dependencies —
[`examples/12-function-dependencies`](examples/12-function-dependencies).

A `.ts` file works there too, and is the one that compiles to no library at all:
`apiplant build` strips the types itself (no node, no deno, no bun, nothing to
install) and the server runs the result in a pool of V8 isolates. Manifest,
config, permissions, generated docs and lifecycle hooks are identical — see
[TypeScript](examples/17-typescript-functions).

TypeScript functions import one module, [`apiplant`](typescript), which is
compiled into the server rather than installed. It declares the endpoint and its
handler together, and gives typed access to everything the host offers:

```ts
import { defineFunctions, db, s, sql } from "apiplant";

export default defineFunctions({
  createNote: {
    permission: "authenticated",
    input: s.object({ title: s.string({ minLength: 1 }) }),
    handler(input) {                       // `input` is typed from the schema,
      return db.one(                       // checked before this runs, and
        sql`INSERT INTO apiplant_note (title) VALUES (${input.title}) RETURNING id`,
      );                                   // published in the OpenAPI document
    },
  },
});
```

## Lifecycle hooks: custom logic on CRUD

Functions can also run *inside* a resource's request lifecycle. Point each event
at a function and it fires around the generated endpoints:

```toml
# resources/post.toml
[hooks]
before_create = "post_before_create"   # validate & normalise, or reject the request
after_create  = "post_after_create"    # record it, notify, reshape the response
after_list    = "post_after_list"
```

There is a `before_` and an `after_` for every operation — `list`, `read`,
`create`, `update`, `delete` — and **one function per event**, so a handler
never has to work out why it was called. A hook receives the operation's payload
plus a context describing it, and its return value decides what happens next:

```rust
fn post_before_create(ctx: &Context<()>, mut input: serde_json::Value) -> Result<serde_json::Value, String> {
    //  ctx.hook() → .resource / .url / .method / .query
    //               .authenticated / .principal_id / .organization_id / .role
    //               .data() — submitted body   .row() — row created, fetched or deleted
    //               .rows() — rows a list returned
    if input["title"].as_str().unwrap_or_default().trim().is_empty() {
        return Ok(reply::abort(422, "title is required"));   // stop the request
    }
    input["published"] = serde_json::json!(false);
    Ok(reply::replace(input))                                 // rewrite what gets stored
}
```

`before_*` hooks run after the permission check but before anything is written,
so they can validate, rewrite the payload, or abort; `after_*` hooks see the
resulting row (or rows) and can replace the response body. Hooks ignore a
function's `visibility`, so hook functions are usually `Private` — invisible
over HTTP, fully wired into the lifecycle. One library can carry a resource's
whole set: see [`examples/08-hooks`](examples/08-hooks)
and the [Hooks guide](docs/hooks.md).

`POST /auth/register` is a create on `user`, so the `user` resource's create hooks
fire there too — which is how
[`examples/14-email-domains`](examples/14-email-domains) drops a new account
straight into the organisation that owns its email domain.

The built-in auth endpoints have their own hooks for the parts that aren't a
create at all, declared in the same `[hooks]` section on the `user` resource:

```toml
# resources/users.toml
[hooks]
after_create = "index_user"      # the table's own lifecycle
before_login = "check_lockout"   # 423 an account that has failed too often
after_login  = "record_attempt"  # fires on failures too — count them, or 429
```

`before_register` / `after_register`, `before_login` / `after_login`, and
`before_api_key` / `after_api_key` — same protocol, only meaningful on `user`,
and none of them ever sees a plaintext password. See the
[Auth hooks](docs/hooks.md#auth-hooks) section.

## Email and caching: two optional services

A function can reach two things beyond the database, each switched on by a
section of `main.toml` and each off by default:

```toml
[email]                                   # send mail through any of eight providers
provider = "sendgrid"                     # smtp | ses | sendgrid | brevo | mailjet |
from     = "no-reply@example.com"         # mailgun | postmark | resend
api_key  = "$SENDGRID_API_KEY"            # $VAR is read from the environment

[cache]                                   # an optional Redis
url    = "redis://127.0.0.1:6379"
prefix = "my-app:"
```

```rust
ctx.send_email(Email::to(&user.email).subject("Welcome").text("Glad you're here."))?;

let hits = ctx.cache_incr(&format!("quota:{}", ctx.principal_id()), 1, Some(60))?;
```

The function never names a provider, so swapping SendGrid for SES is a config
change and not a rebuild. Misconfiguration fails the boot — naming the missing
field — rather than the first password reset.

Neither service is used by the framework itself: nothing sends a message you
didn't write, and **nothing is cached for you**. A cache in front of generic
CRUD would have to guess when a row goes stale; the work worth caching is the
work a function does, because that is the code that knows how to invalidate it.
See [Sending email](docs/email.md), [Caching](docs/caching.md),
[`examples/15-email`](examples/15-email) and
[`examples/16-caching`](examples/16-caching).

## File storage: uploads that outlive the backend

A `file` field is a column that holds a file:

```toml
[fields.photo]
type = "file"
```

```toml
[storage]                                 # `local` unless told otherwise
backend = "local"
dir     = "storage"                       # a mounted volume, in a container
```

The dashboard renders that field as an upload button *and* a URL box — store a
file, or point at one that already exists. `POST <base>/uploads` takes the file
as a raw body and answers with the link; `GET /files/<key>` serves it back.

What the row stores is a **relative** URL:

```json
{ "name": "Chair", "photo": "/files/2026/08/1a2b3c4d5e6f-chair.png" }
```

Never a bucket, never a signed link with an expiry. So moving to block storage
is five lines of TOML and no data migration — every stored link keeps working,
because none of them ever named the backend:

```toml
[storage]                                 # S3, R2, MinIO and B2 differ only here
backend           = "s3"
bucket            = "app-uploads"
region            = "auto"
endpoint          = "https://${R2_ACCOUNT_ID}.r2.cloudflarestorage.com"
access_key_id     = "${R2_ACCESS_KEY_ID}"
secret_access_key = "${R2_SECRET_ACCESS_KEY}"
```

The bucket stays private — reads are proxied — so there is no public-read policy
and no signed-URL scheme to get wrong. An app that would rather serve from a CDN
sets `base_url` and stores absolute links instead.

`user.avatar_url` and `organization.avatar_url` are `file` fields, so a picture
picker for people and workspaces is there without configuring anything. See
[File storage](docs/storage.md) and
[`examples/21-docker`](examples/21-docker) for the volume it needs in a
container.

## Queues: background work, with nothing else to run

Some work should not be in the request that caused it. A receipt email, a
warehouse call, an analytics sync — the buyer does not care, and none of them
should be able to fail the sale.

```ts
db.execute(sql`UPDATE apiplant_order SET status = 'paid' WHERE id = ${id}::uuid`);
queue.publish("order.paid", { orderId: id });   // returns; the rest happens after
```

```toml
[queues.subscribe]                        # topic -> the function(s) that handle it
"order.paid" = ["fulfilOrder", "notifyOps"]
```

There is no broker to deploy. `publish` writes a row to `queue_message` and
fires a Postgres `NOTIFY`; a subscriber wakes on the notification and claims the
row with `FOR UPDATE SKIP LOCKED`. The two halves do different jobs, and most
home-made queues have only one: the **row** is what survives a restart, records
a failure and lets it be retried, and the **notification** is what makes it
happen in milliseconds rather than on the next poll.

A resource can announce its own writes with no function at all, in which case
the row is the message:

```toml
# resources/order.toml
[publish]
after_delete = "order.cancelled"
```

Failures retry on a doubling backoff and then stay in the table, marked
`failed`, with the reason on them — a dead-letter you have to go and look at,
which is the point. Delivery is at-least-once, so handlers must be safe to run
twice; [the guide](docs/queues.md) is blunt about why that is the only honest
guarantee on offer. See [`examples/23-queues`](examples/23-queues).

## Payments: billing as resources, not as a bolt-on

A third optional section, and the one that adds the most:

```toml
[payments]
provider       = "stripe"
secret_key     = "$STRIPE_SECRET_KEY"
webhook_secret = "$STRIPE_WEBHOOK_SECRET"
currency       = "eur"
automatic_tax  = true                     # Stripe Tax works out what is owed
```

Naming a provider adds five resources — `billing_product`, `billing_price`,
`billing_customer`, `billing_subscription`, `billing_payment` — and four
endpoints (`/billing/config`, `/checkout`, `/portal`, `/webhook`). Because they
are ordinary resources, the catalogue is CRUD an admin can edit from the
dashboard, the price list is a `GET` anyone can make, and "is this org
subscribed" is a query with permissions and roles already on it.

```bash
curl -X POST $API/billing_product -d '{"name":"Pro"}'      # created in Stripe too
curl -X POST $API/billing/checkout -d '{"price_id":"…"}'   # → a Stripe URL
```

The split is the design: **the catalogue is yours, what has been paid for is
Stripe's.** Products and prices are your tables, mirrored out by a hook when you
save one. Subscriptions and payments are `private` for writes and filled in by
the signed webhook, because a row claiming somebody is subscribed when Stripe
disagrees is not a stale cache — it is an entitlement somebody granted
themselves.

See [Payments](docs/payments.md) and
[`examples/18-payments`](examples/18-payments).

## AI: an assistant, and a way to stream one

A fourth optional section. Naming a provider mounts `POST /api/ai/chat`, which
streams a reply token by token, and gives every function a `chat` call over the
same provider:

```toml
[ai]
provider = "custom"                       # openai | anthropic | custom
endpoint = "http://localhost:8080"        # llama.cpp, vLLM, Ollama, LM Studio, a gateway
model    = "local"
# api_key = "$OPENAI_API_KEY"             # optional: a local model wants none
access   = "authenticated"                # who may ask. Not public by default
```

```rust
// The whole answer, for a function that returns one value.
let reply = ctx.chat(Chat::ask(question).system("Be terse"))?;

// The whole answer *and* every token, forwarded to this function's caller.
let reply = ctx.chat_streaming(Chat::ask(question))?;
```

The three providers differ in URL, authentication, request shape and event
format; nothing above them does. `custom` covers everything that speaks the
OpenAI chat-completions shape, which is nearly everything — including a model
on your own machine, with no key at all.

**Streaming is not an AI feature.** Every function gets a second endpoint,
`POST /api/functions/<name>/stream`, which forwards whatever the function
`emit`s as it produces it and ends with the return value. The same handler
serves both endpoints and never asks which — so a slow report, a batch job's
progress and a model's tokens all reach a browser the same way, as server-sent
events with three event types and no library on the client.

The endpoint is `authenticated` by default because it spends money, or a GPU,
on behalf of whoever calls it. And nothing in the framework calls the model on
your behalf: no CRUD operation summarises a row, nothing is embedded or
indexed. It is a service a function can reach, like `[email]` and `[cache]`.

See [AI](docs/ai.md), [`examples/19-ai`](examples/19-ai) and
[`examples/20-streaming`](examples/20-streaming).

## Workspace layout

| Crate                  | Responsibility                                                    |
|------------------------|-------------------------------------------------------------------|
| `apiplant-abi`         | the stable C-ABI contract shared with function authors            |
| `apiplant-function`    | ergonomic `Context` + `function!` macro for writing functions     |
| `apiplant-core`        | config, errors, the resource-schema model, app-directory loader   |
| `apiplant-db`          | dynamic DDL/DML over Postgres (sea-query + sea-orm), migrations    |
| `apiplant-auth`        | passwords, JWT sessions, API keys, permission evaluation          |
| `apiplant-email`       | outbound mail: SMTP, SES (SigV4), SendGrid, Brevo, Mailjet, Mailgun, Postmark, Resend |
| `apiplant-cache`       | the optional Redis a function can reach                            |
| `apiplant-queue`       | background messages: the Postgres outbox behind `[queues]`          |
| `apiplant-payments`    | the optional Stripe integration behind `[payments]`                |
| `apiplant-ai`          | the optional assistant behind `[ai]`: OpenAI, Anthropic, or anything OpenAI-shaped |
| `apiplant-server`      | the ntex server: CRUD routing, auth, functions, OpenAPI/Swagger, TLS |
| `apiplant-assets`      | the admin and studio builds, embedded in the binary                |
| `apiplant`             | the executable                                                     |
| `examples/`            | runnable apps, from hello-world to a 20-resource domain model — see [examples/](examples/) |
| `studio/`              | a local browser editor for an app directory, served by `apiplant studio`, and the design system reused by the admin dashboard — see [studio/](studio/) |
| `admin/`               | the admin dashboard, embedded in the binary and served at `/admin/` |

## Quickstart

```bash
# 1. Start Postgres on the port the examples expect.
docker run -d --name apiplant-postgres \
  -e POSTGRES_HOST_AUTH_METHOD=trust \
  -p 5432:5432 \
  postgres:16
# (stop & remove later with: docker rm -f apiplant-postgres)
#
#    No Docker? Run a throwaway local cluster in ./pgdata instead:
#      initdb -D ./pgdata --username=postgres --auth=trust
#      pg_ctl -D ./pgdata -o "-p 5432 -k /tmp" -l ./pgdata/log.txt start
#    (stop later with: pg_ctl -D ./pgdata stop)

# 2. Start with the smallest example: config only, no resources.
createdb -h 127.0.0.1 -p 5432 -U postgres apiplant_hello
cargo run -p apiplant -- run --seed examples/01-hello-world

curl -s localhost:8099/api/_health
curl -s -XPOST localhost:8099/api/auth/login \
  -H 'content-type: application/json' \
  -d '{"email":"admin@example.com","password":"password"}'   # → { "token": ... }
#
# That account came from examples/01-hello-world/seed/ — every example carries
# the same one, so the dashboard at localhost:8099/admin/ has somebody to be.

# 3. Then work up through the others — each adds one idea.
createdb -h 127.0.0.1 -p 5432 -U postgres apiplant_functions
cargo run -p apiplant -- build examples/07-functions   # compiles functions/*.rs
cargo run -p apiplant -- run examples/07-functions

curl -s -XPOST localhost:8099/api/functions/greet \
  -H 'content-type: application/json' -d '{"name":"World"}'
# → { "message": "Buongiorno, World!", "registered_users": 1 }
```

The [examples](examples/) go from a bare `main.toml` to a full app with
lifecycle hooks, one concept at a time — and
[`examples/13-real-world`](examples/13-real-world) puts the lot together in a
20-resource domain model.

## The CLI

```
apiplant init [APP_DIR]      write a new app directory (APP_DIR defaults to `.`)
apiplant run [APP_DIR]       serve the app (APP_DIR defaults to `.`)
apiplant build [APP_DIR]     compile functions/* into loadable libraries
apiplant check [APP_DIR]     load and validate the app, then exit
apiplant seed [APP_DIR]      migrate, then load seed/ into the database
apiplant call NAME [APP_DIR] run one function and print what it returned
apiplant admin [APP_DIR]     bake a static admin panel to host elsewhere
apiplant cli [SERVER|DIR]    interactive console for a running server
apiplant studio              serve the visual editor from this binary
apiplant version             print the version and exit (also `-V`, `--version`)
```

`init` writes a sample app — one resource, the rows to sign in with and a
function — into an empty (or not-yet-existing) directory, or clones a template
of your own with `--from <git-url>` (also accepted as a second argument):

```bash
apiplant init my-app                                   # the sample app
apiplant init my-app https://github.com/acme/template  # your own starting point
apiplant init my-app --from git@github.com:acme/template.git --branch v2
```

It refuses a directory that already has anything in it (a bare `.git` aside),
and a cloned template keeps none of its history. `run` takes `--build` to
compile out-of-date sources first, `--watch` to rebuild and restart on every
edit (see below), and `--seed` to load
the app's fixture after migrating; `build` takes
`--release` and `--force`; `admin` takes `--api <domain-or-base-url>` and
optionally `--out <dir>`; `cli` takes a server address — a URL, a `host:port` or
a domain — or, failing that, an app directory whose `main.toml` names one;
`studio` takes `--host` and `--port`; `call` takes `--input`, `--as` and
`--quiet`.

The command is always spelled out — `apiplant ./my-app` is an error that tells
you to write `apiplant run ./my-app` — and an app directory that doesn't exist
is refused rather than served as an empty app.

### The development loop

```bash
apiplant run --watch ./my-app
```

Every write under the app directory — a resource, `main.toml`, a function source,
a seed file, something in `public/` — rebuilds what went stale and restarts the
server. A *restart*, not a reload: a built function is a shared library the
process has already loaded, and the only honest way to replace one is to
replace the process. That also means there are no special cases — a new
resource, a renamed field and a new function all arrive the same way.

Build output is ignored (`.apiplant-build/`, `target/`, `node_modules/`, the
compiled libraries themselves), so a build never triggers the next one, and a
build that fails leaves the server down until the next edit rather than
restarting into the same error. Changes are found by polling mtimes twice a
second, which is what makes the same command work inside a container with the
app bind-mounted from the host — see
[`examples/21-docker`](examples/21-docker), whose `dev` service is exactly
that.

### Scheduled jobs

A function you can call over HTTP is a function you can schedule, with no
server in the way:

```bash
apiplant call nightly_report ./my-app
apiplant call nightly_report --input '{"day":"yesterday"}'
apiplant call send_digests --input @/etc/apiplant/digest.json --as "$USER_ID"
```

It builds what a request would have — the same database, email provider, cache,
payments and AI assistant from the same `main.toml` — hands the function its
input, and prints what it returned on stdout. Anything the function emits goes
to stderr as it happens (`--quiet` drops it), so a long job's progress shows up
in the logs while the result stays pipeable into `jq`.

Two things differ from the HTTP endpoint, both on purpose: there is no access
check, because there is no request to authenticate and anyone who can run the
binary against your database already has more than an endpoint would give them
(`--as <USER_ID>` sets the caller a function sees); and `[access] private`
functions can be called, because a scheduled job is the same kind of trusted
caller a hook is. It does not migrate — that stays `apiplant seed` or a boot.

Which makes a Kubernetes CronJob the backend image with different arguments:

```yaml
apiVersion: batch/v1
kind: CronJob
metadata:
  name: nightly-report
spec:
  schedule: "0 2 * * *"
  jobTemplate:
    spec:
      template:
        spec:
          restartPolicy: Never
          containers:
            - name: apiplant
              image: ghcr.io/acme/my-app:latest   # the backend image, as deployed
              args: ["call", "nightly_report", "/app"]
              envFrom:
                - secretRef:
                    name: my-app-env             # the same DATABASE_URL etc.
```

`--input @-` reads stdin, and `--input @<file>` reads a file — which is the one
that survives a manifest, since a ConfigMap mounted next to the job beats
quoting JSON inside YAML inside a container's arguments. A non-zero exit means
the function failed, so the Job's `backoffLimit` retries what you'd expect it
to.

### The admin dashboard

Every served app has one, at `/admin/`, with nothing to generate:

```bash
apiplant run ./my-app     # dashboard at http://localhost:8080/admin/
```

The interface is embedded in the `apiplant` binary and its manifest — resources,
permissions, auth model, callable functions — is derived from the app on boot,
so it talks to its own origin and never goes stale after a resource change. Turn it
off with `[admin] enabled = false`, or move it with `[admin] path`.

### The console

```bash
apiplant cli api.example.com
apiplant cli ./my-app
```

The dashboard's job, in a terminal. Give it a server address, or an app
directory whose `main.toml` names one; either way it asks the *server* what it
holds — the same
manifest the dashboard loads — so it never describes an app different from the
one running.

Signing in offers three doors: open the dashboard in a browser and have it hand
a key straight back to the console, sign in with an email and password (the
console mints and saves a key for you), or paste a key you already have. The key
is saved per server in `~/.config/apiplant/cli.json`, so the next run starts
connected.

If your account belongs to no organisation yet, the first thing after signing in
is a modal to create one — or, when the app provisions tenants itself, a note
saying which administrator to ask. The dashboard does the same as a full page.

Inside, a sidebar lists every resource and callable function you are allowed to
reach. Lists page and search, records open into forms, references are pickers
rather than UUIDs to type, and functions get a form generated from their input
schema. Press `?` for the keys.

See [the console guide](docs/cli.md).

### A public directory

A `public/` directory in the app is served at the site root, alongside the API:

```
my-app/
├── main.toml
├── resources/
└── public/
    ├── index.html      # GET /
    ├── style.css       # GET /style.css
    ├── guide/index.html # GET /guide and /guide/
    └── 404.html        # anything that matches nothing
```

A file is only routed if it exists, so `/products` still reaches the API while
`/about.html` reaches the file. The 404 page is `404.html` when there is one, or
whatever `[public] not_found` names.

### Studio

```bash
apiplant studio          # http://127.0.0.1:5273
```

Serves the visual editor out of the same binary — no `pnpm`, no checkout. It is
local-first: the browser opens your app directory directly and reads and writes
it in place, so nothing is uploaded and this command is only a file server. It
binds loopback by default.

### The dashboard, in more detail

It is built for the people who *run* the app, not the developer who wrote the
resources — so it shows names rather than ids, forms rather than JSON, and only
what the person signed in is allowed to touch:

* **Sign in / create account**, with any required profile fields as real inputs.
* **A searchable, paginated table** per resource; click a row to open a form
  carrying every field, its relationships (as pickers that show names), and the
  records attached to it.
* **Actions** — callable functions, with a form generated from the handler's own
  input type and an optional confirmation step. Lifecycle hooks never appear.
* **Purpose-built screens** for the auth resources — Team, Organization, Your
  account, API keys — instead of raw `membership` and `user` tables.

The UI is a dedicated top-level **`admin/` Vite app**, built with Solid and
Tailwind while reusing studio's design system, and embedded in the binary.
Everything it knows about an app — resources, permissions, auth model, callable
functions — is resolved on boot and served as a JSON manifest, so the same
JavaScript serves every app and can never describe a model you've since changed.

Tune it per resource with an `[admin]` section (`label`, `group`, `columns`,
`roles`, …), per field with `[fields.<name>.admin]` (`widget`, `options`,
`help`), and per function with an `admin { … }` block. All of it is
presentation: `[permissions]` is what the server enforces. See
[docs/admin.md](docs/admin.md).

Want something else entirely? Set `[admin] enabled = false` and serve your own
console out of `public/admin/` like any other static page.

### A static panel, hosted elsewhere

```bash
apiplant admin ./my-app --api https://api.example.com --out ./panel
```

Writes the same dashboard out as a directory of plain files — `index.html`,
`app.js`, `app.css`, the images, and a JSON manifest baked from the app — for
hosting somewhere other than the API: a CDN, a bucket, a different origin. It
holds no secrets, and reaches the API over CORS, so that origin has to allow it.

The manifest is frozen at the moment you run the command, so re-run it when
resources or functions change. The dashboard served at `/admin/` has no such
problem — it is rebuilt from the app on every boot — and nothing written here is
ever read back by the server.

Develop the admin app like studio itself:

```bash
cd admin
pnpm install
pnpm dev
pnpm build     # → crates/apiplant-assets/assets/admin, which the Rust build embeds
```

Both front ends build straight into `crates/apiplant-assets/assets/`, and that
directory is tracked, because `cargo build` embeds it into the binary — a
checkout builds without running `pnpm` first, and `cargo publish` ships it
inside the crate. Rebuild and commit those assets when you change either front
end.

The `typescript/` npm package works the same way in reverse: its two shipped
files live in `crates/apiplant-js/assets/`, which the Rust build embeds, and
`pnpm sync` (run automatically by `prepack`) copies them into `typescript/` for
an `npm publish`.

## Built on

[ntex] · [sea-orm] / [sea-query] · [abi_stable] · [argon2] · [jsonwebtoken] · [rustls]

Licensed under MIT OR Apache-2.0.

[`abi_stable`]: https://docs.rs/abi_stable
[ntex]: https://ntex.rs
[sea-orm]: https://www.sea-ql.org/SeaORM/
[sea-query]: https://www.sea-ql.org/SeaQuery/
[argon2]: https://docs.rs/argon2
[jsonwebtoken]: https://docs.rs/jsonwebtoken
[rustls]: https://docs.rs/rustls
[`HostApi`]: crates/apiplant-abi/src/lib.rs
[`Context`]: crates/apiplant-function/src/lib.rs
