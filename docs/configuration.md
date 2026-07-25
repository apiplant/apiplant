# Configuration (`main.toml`)

`main.toml` lives at the root of the app directory. **Every key is optional** —
a missing file, section, or key falls back to a safe default. The only setting
that is *inferred* rather than declared is TLS (see [HTTPS](#https)).

```toml
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
title   = "My API"          # spec info.title and UI title

[admin]
enabled = true               # serve the built-in admin dashboard
path    = "/admin"           # where it mounts (outside base_path)

[public]
enabled   = true             # serve `dir` at the site root when it exists
dir       = "public"         # static site directory, relative to the app root
not_found = "404.html"       # page for unmatched requests (default when present)
```

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
| `allow_registration` | `true` | Whether `POST /auth/register` is open. |

See [Authentication](authentication.md) for the full auth model.

## `[docs]`

| Key | Default | Notes |
|-----|---------|-------|
| `enabled` | `true` | Serve `GET <base>/openapi.json` and the Swagger UI. |
| `path` | `/docs` | UI mount path (under `base_path`). |
| `title` | `apiplant API` | Shown in the UI and the spec. |

See [OpenAPI & Swagger UI](openapi.md).

## `[admin]`

| Key | Default | Notes |
|-----|---------|-------|
| `enabled` | `true` | Serve the admin dashboard. Every app has one: the interface is built into the `apiplant` binary, and its manifest is derived from the app on boot — there is nothing to generate. Set `false` for a deployment that should expose no operator console. |
| `path` | `/admin` | Where it mounts. Outside `base_path`, so `/admin/` stays put when the API moves to `/api`. Normalised to start with `/` and not end with one. |

The dashboard talks to its own origin, so it needs no CORS and never goes stale
after a model change. An `admin/` directory in the app — the output of
`apiplant admin` — overrides the embedded build file by file, which is the hook
for a customised console; the manifest is always the live one either way. See
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
