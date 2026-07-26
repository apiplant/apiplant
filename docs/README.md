# apiplant documentation

apiplant turns an **app directory** into a running, database-backed REST API.
You don't write server code — you declare resources, permissions and
relationships in TOML, drop in optional compiled functions, and run the
`apiplant` binary against the directory.

These guides cover everything the framework does. To learn by running things
instead, the [examples](../examples/) go from a bare `main.toml` to a full app
with lifecycle hooks, one idea at a time.

| Guide | What's in it |
|-------|--------------|
| [Configuration](configuration.md) | `main.toml` reference, TLS, database, workers |
| [Resources](resources.md) | defining resources, field types & options, scope, migrations |
| [Permissions](permissions.md) | the access model, per-action policies, ownership, org roles |
| [Multitenancy](multitenancy.md) | organisations, memberships, automatic per-tenant isolation |
| [Relationships](relationships.md) | references, `has_many`, expansion, filtering, `on_delete` |
| [Authentication](authentication.md) | users, organisations, API keys, sessions, extending `user` |
| [Functions](functions.md) | writing & loading compiled plugins over the stable ABI |
| [Lifecycle hooks](hooks.md) | running functions before/after every CRUD operation |
| [Sending email](email.md) | one `[email]` provider — SMTP, SES, SendGrid, Brevo, Mailjet… |
| [Caching](caching.md) | the optional `[cache]` Redis a function can reach |
| [Admin dashboard](admin.md) | the built-in operator UI, `[admin]` config, action forms |
| [The console](cli.md) | `apiplant cli` — the dashboard's job in a terminal |
| [Security model](security.md) | what the server enforces, and what you must configure before exposing it |
| [API reference](api-reference.md) | every endpoint, query parameter and status code |
| [OpenAPI & Swagger UI](openapi.md) | the generated spec and interactive docs |

## The 60-second model

```
my-app/
├── main.toml       # optional server/db/auth/docs config — safe defaults if absent
├── https/          # cert + key here ⇒ the server runs HTTPS
├── models/         # one <name>.toml per resource ⇒ a table + CRUD endpoints
└── functions/      # function sources (.rs), their config, and built libraries
```

* A **resource** (`models/post.toml`) becomes a Postgres table and a set of
  RESTful endpoints, each gated by a per-action **permission**.
* `organization`, `membership`, `user`, `api_key` and `oauth_connection`
  resources exist by default and can be extended by dropping a file with the
  same name.
* **Relationships** come from `reference` fields — enforced with real foreign
  keys, navigable via nested endpoints, and inlinable with `?expand=`.
* **Functions** are separately-compiled libraries mounted as endpoints, talking
  to the host over a stable C ABI. Write them as plain `.rs` files and let
  `apiplant build` compile them.
* **Hooks** attach those functions to a resource's lifecycle (`before_create`,
  `after_list`, …) so custom logic can validate, rewrite or observe every CRUD
  operation.
* **Email** and a **cache** are optional services a function can reach: name a
  provider in `[email]` and a Redis in `[cache]`, and `ctx.send_email(…)` /
  `ctx.cache_get(…)` work. Neither is used by the framework itself.
* An **admin dashboard** is generated from all of the above — a static,
  self-hosted operator UI, tuned per resource and per function with `[admin]`.
* Migrations are automatic and additive: your schemas *are* the desired state.
* Any string in any of these files can reference the environment —
  `url = "$DATABASE_URL"`, `region = "${AWS_REGION:-eu-west-1}"` — so the files
  you commit hold no credentials. See
  [Configuration](configuration.md#environment-variables).

Everything is optional. An empty directory is a valid (bare) app.
