# apiplant documentation

apiplant turns an **app directory** into a running, database-backed REST API.
You don't write server code — you declare resources, permissions and
relationships in TOML, drop in optional compiled functions, and run the
`apiplant` binary against the directory.

These guides cover everything the framework does.

| Guide | What's in it |
|-------|--------------|
| [Configuration](configuration.md) | `main.toml` reference, TLS, database, workers |
| [Resources](resources.md) | defining resources, field types & options, scope, migrations |
| [Permissions](permissions.md) | the access model, per-action policies, ownership, org roles |
| [Multitenancy](multitenancy.md) | organisations, memberships, automatic per-tenant isolation |
| [Relationships](relationships.md) | references, `has_many`, expansion, filtering, `on_delete` |
| [Authentication](authentication.md) | users, organisations, API keys, sessions, extending `user` |
| [Functions](functions.md) | writing & loading compiled plugins over the stable ABI |
| [API reference](api-reference.md) | every endpoint, query parameter and status code |
| [OpenAPI & Swagger UI](openapi.md) | the generated spec and interactive docs |

## The 60-second model

```
my-app/
├── main.toml       # optional server/db/auth/docs config — safe defaults if absent
├── https/          # cert + key here ⇒ the server runs HTTPS
├── models/         # one <name>.toml per resource ⇒ a table + CRUD endpoints
└── functions/      # compiled .so/.dylib/.dll plugins + their <name>.toml config
```

* A **resource** (`models/post.toml`) becomes a Postgres table and a set of
  RESTful endpoints, each gated by a per-action **permission**.
* `organization`, `membership`, `user`, `api_key` and `oauth_connection`
  resources exist by default and can be extended by dropping a file with the
  same name.
* **Relationships** come from `reference` fields — enforced with real foreign
  keys, navigable via nested endpoints, and inlinable with `?expand=`.
* **Functions** are separately-compiled libraries mounted as endpoints, talking
  to the host over a stable C ABI.
* Migrations are automatic and additive: your schemas *are* the desired state.

Everything is optional. An empty directory is a valid (bare) app.
