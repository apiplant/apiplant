# Configuration (`main.toml`)

`main.toml` lives at the root of the app directory. **Every key is optional** —
a missing file, section, or key falls back to a safe default. The only setting
that is *inferred* rather than declared is TLS (see [HTTPS](#https)).

```toml
[app]
name = "Acme Logistics"      # what people are shown; default: the directory name

[server]
host      = "0.0.0.0"        # interface to bind
port      = 8080             # TCP port
domain    = "api.example.com" # optional: only answer this Host header
base_path = "/api"          # mount the whole API under a sub-path
workers   = 8               # worker threads (default: one per CPU)

[database]
url             = "postgres://user:pass@localhost:5432/mydb"  # full URL...
# ...or assemble from parts (used only when `url` is empty):
host            = "localhost"
port            = 5432
name            = "apiplant"
user            = "postgres"
password        = "postgres"
max_connections = 16
auto_migrate    = true       # run additive migrations on boot

[auth]
jwt_secret         = "change-me"  # signs session tokens; empty ⇒ ephemeral
session_ttl_secs   = 604800       # 7 days
allow_registration = true         # enable POST /auth/register

[docs]
enabled = true               # serve OpenAPI spec + Swagger UI
path    = "/docs"            # where Swagger UI mounts (under base_path)
# title = "My API"           # only when the docs differ from [app] name

[admin]
enabled = true               # serve the built-in admin dashboard
path    = "/admin"           # where it mounts (outside base_path)
# logo  = "/logo.png"        # your mark instead of the apiplant one

[public]
enabled   = true             # serve `dir` at the site root when it exists
dir       = "public"         # static site directory, relative to the app root
not_found = "404.html"       # page for unmatched requests (default when present)

[email]                      # optional: outbound mail, off unless configured
provider = "sendgrid"        # none | smtp | ses | sendgrid | brevo | mailjet |
                             # mailgun | postmark | resend
from     = "no-reply@example.com"
api_key  = "${SENDGRID_API_KEY}"

[cache]                      # optional: Redis, off unless a url is given
url    = "redis://127.0.0.1:6379"
prefix = "my-app:"
```

## `[app]`

| Key | Default | Notes |
|-----|---------|-------|
| `name` | the app directory's name | What the app is called wherever a person reads it: the [dashboard](admin.md) header (`<name> admin`), the browser title, and the [API docs](openapi.md). The directory an app lives in is a filing decision — `07-functions`, `backend`, `api-v2` — and operators should not have to read it. |

## `[server]`

| Key | Default | Notes |
|-----|---------|-------|
| `host` | `0.0.0.0` | Bind address. |
| `port` | `8080` | TCP port. |
| `domain` | *(none)* | When set, requests with a different `Host` header get no match (404). Useful for virtual-hosting. |
| `base_path` | `/` (empty) | Sub-path prefix for **all** routes. Normalised to start with `/` and not end with one. `/api` ⇒ endpoints live at `/api/...`. |
| `workers` | one per CPU | Number of OS worker threads. |

## `[database]`

| Key | Default | Notes |
|-----|---------|-------|
| `url` | *(empty)* | Full Postgres URL. When empty, built from the parts below. |
| `host` / `port` / `name` / `user` / `password` | `localhost` / `5432` / `apiplant` / `postgres` / `postgres` | Used only if `url` is empty. |
| `max_connections` | `16` | Connection-pool size. |
| `auto_migrate` | `true` | Create missing tables/columns/foreign-keys on boot. Set `false` to manage schema yourself. |

apiplant targets **PostgreSQL** (it uses `to_jsonb`, `gen_random_uuid()`,
`jsonb_agg`, and real foreign keys).

## `[auth]`

| Key | Default | Notes |
|-----|---------|-------|
| `jwt_secret` | *(empty)* | HMAC secret for session JWTs. **Set this in production** — an empty value generates a random secret at boot, so tokens don't survive a restart. |
| `session_ttl_secs` | `604800` (7d) | Lifetime of issued session tokens. |
| `allow_registration` | `true` | Whether self-service signup is open. `false` closes `POST /auth/register` *and* anonymous `POST <base>/user`. |

See [Authentication](authentication.md) for the full auth model.

## `[docs]`

| Key | Default | Notes |
|-----|---------|-------|
| `enabled` | `true` | Serve `GET <base>/openapi.json` and the Swagger UI. |
| `path` | `/docs` | UI mount path (under `base_path`). |
| `title` | the app's `[app] name` | Shown in the UI and the spec's `info.title`. Set it only when the published API answers to a different name than the app does. |

See [OpenAPI & Swagger UI](openapi.md).

## `[admin]`

| Key | Default | Notes |
|-----|---------|-------|
| `enabled` | `true` | Serve the admin dashboard. Every app has one: the interface is built into the `apiplant` binary, and its manifest is derived from the app on boot — there is nothing to generate. Set `false` for a deployment that should expose no operator console. |
| `path` | `/admin` | Where it mounts. Outside `base_path`, so `/admin/` stays put when the API moves to `/api`. Normalised to start with `/` and not end with one. |
| `logo` | unset | Image shown beside the app name, as a URL the browser can fetch — usually a file in [`public/`](#public), e.g. `/logo.png`. Unset keeps the apiplant mark. |

The dashboard is served entirely from the binary — the files from the embedded
build, the manifest from the app being served. Nothing on disk feeds into it, so
it needs no CORS and can't go stale after a model change. For a console of your
own, set `enabled = false` and serve one from `public/admin/`. To host a copy of
this one on another origin, bake it with `apiplant admin`. See
[Admin dashboard](admin.md).

## `[public]`

Drop a `public/` directory in the app and it is served at the site root:
`public/index.html` answers `/`, `public/style.css` answers `/style.css`, and
`public/guide/index.html` answers both `/guide` and `/guide/`.

| Key | Default | Notes |
|-----|---------|-------|
| `enabled` | `true` | Serve `dir` when it exists. A missing directory is not an error — it just means no static site. |
| `dir` | `public` | Directory holding the site, relative to the app root. |
| `not_found` | *(none)* | Page for requests that match nothing, relative to `dir`. When unset, `404.html` is used if it exists. Served with a `404` status. |

One route is registered per file at boot, so the site and the API can share the
root: `/about.html` reaches the file, while `/products` still reaches the API's
CRUD routes. A path with no file *and* no route — `/no/such/page` — gets the
404 page. Two-segment paths belong to the API's `/{resource}/{id}` and keep
answering in JSON.

## `[email]`

Outbound mail, for functions to send. Off by default (`provider = "none"`), and
never used by the framework itself — nothing sends a message you didn't write.

| Key | Default | Notes |
|-----|---------|-------|
| `provider` | `none` | `smtp`, `ses` (`aws`), `sendgrid`, `brevo` (`sendinblue`), `mailjet`, `mailgun`, `postmark` or `resend`. |
| `from` | *(empty)* | Envelope sender. Required once a provider is named; a message may override it. |
| `from_name` | *(empty)* | Display name beside `from`. |
| `reply_to` | *(empty)* | Default `Reply-To`. |
| `api_key` | *(empty)* | The provider's key — the AWS access key id for `ses`, the public key for `mailjet`, the server token for `postmark`. |
| `api_secret` | *(empty)* | The second half of a two-part credential: `ses`, `mailjet`. |
| `region` | *(empty)* | `ses` only, e.g. `eu-west-1`. |
| `domain` | *(empty)* | `mailgun` only, e.g. `mg.example.com`. |
| `timeout_secs` | `15` | How long one send may take. |

`[email.smtp]`, for `provider = "smtp"`:

| Key | Default | Notes |
|-----|---------|-------|
| `host` | *(empty)* | Required for `smtp`. |
| `port` | `0` | `0` picks from `encryption`: 465 (`tls`), 587 (`starttls`), 25 (`none`). |
| `username` / `password` | *(empty)* | Omit `username` for a relay that authenticates by IP. |
| `encryption` | `starttls` | `starttls`, `tls` (implicit) or `none` (cleartext — warned about at boot). |

A provider that can't work — unknown name, missing key, no `from` — fails the
boot rather than the first send. See [Sending email](email.md).

## `[cache]`

An optional Redis, reachable only from functions. Off unless `url` is set, and
nothing in the framework caches through it. See [Caching](caching.md).

| Key | Default | Notes |
|-----|---------|-------|
| `enabled` | `true` | Off switch that keeps the rest of the section. |
| `url` | *(empty)* | `redis://…`, `rediss://…`; may carry a password and a database index. Empty means no cache. |
| `prefix` | *(empty)* | Prepended to every key, so apps can share one Redis. |
| `default_ttl_secs` | `0` | Expiry for a write that doesn't ask for one. `0` = keys persist. |
| `timeout_secs` | `5` | How long one operation may take. |

A `url` that can't be reached fails the boot.

## Environment variables

**Any** string value in **any** of the app's TOML files — `main.toml`, every
`models/*.toml`, every `functions/*.toml` — may reference the environment:

```toml
[database]
url = "$DATABASE_URL"

[auth]
jwt_secret = "${JWT_SECRET}"

[email]
api_key = "$SENDGRID_API_KEY"
```

This is what lets a `main.toml` you commit hold no credentials: the file says
*where* each secret comes from, and the deployment supplies it.

| Written | Means |
|---------|-------|
| `$VAR`, `${VAR}` | the variable's value, or `""` (with a warning) when unset |
| `${VAR:-default}` | the variable's value, or `default` when unset or empty |
| `$$` | a literal `$` |
| `$` followed by anything else | itself, unchanged |

A name is a letter or `_` followed by letters, digits or `_`. Anything that
isn't one is left exactly as written, so `$19.99`, `100 US$` and `a $ b` need no
escaping; `$$` is only for a genuine ambiguity like `$$USD`.

References can appear anywhere in a string, and a string can hold several:

```toml
[database]
url = "postgres://$DB_USER:$DB_PASSWORD@$DB_HOST:${DB_PORT:-5432}/$DB_NAME"

[server]
domain = "${APP_DOMAIN:-api.example.com}"
```

Defaults are what make one file work in development and in production: name the
variable, give the local value as the default, and set it for real where it
matters.

Two deliberate limits:

* **Values only, never keys.** A table or field named by the environment would
  make a file's *shape* depend on the deployment; that isn't what this is for.
* **Substitution happens after parsing**, into a string TOML has already
  produced. A password containing `"` or a newline stays one string value — it
  cannot become a syntax error, and it cannot inject extra TOML.

An unset variable with no default expands to the empty string and logs a
warning naming the variable and the file. Leaving `$DATABASE_URL` in place
instead would hand the literal text to whatever consumes it, and fail much
later and much less clearly.

## HTTPS

TLS is **not** configured in `main.toml`. Instead, if the app directory contains
an `https/` folder with a certificate and a private key, the server serves HTTPS
automatically. Recognised filenames:

* **cert**: `cert.pem`, `fullchain.pem`, `certificate.pem`, or `server.crt`
* **key**: `key.pem`, `privkey.pem`, `server.key`, or `private.pem`

```
my-app/
└── https/
    ├── fullchain.pem
    └── privkey.pem
```

No `https/` directory ⇒ plain HTTP.

## Precedence & safety

* Absent file ⇒ all defaults.
* Absent section or key ⇒ that key's default (other keys still read).
* `url` beats the individual `[database]` parts.
* An empty `jwt_secret` is allowed but logs a warning.
* `[email]` and `[cache]` are opt-in; a misconfigured one fails the boot rather
  than failing quietly at the first use.
* `$VAR` is expanded in every app TOML file, before any of the above is read.
