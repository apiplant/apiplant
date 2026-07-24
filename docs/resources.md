# Resources

A **resource** is one `models/<name>.toml` file. apiplant turns it into a
Postgres table and a set of RESTful CRUD endpoints. Everything about the table
and its API is declared here — you never write migrations or handlers.

```toml
# models/post.toml
[resource]
name       = "post"          # required; also the URL segment (/api/post)
# table    = "posts"         # optional physical table name (default apiplant_post)
# timestamps = true          # add created_at / updated_at (default true)
# owner_field = "owner_id"   # column used for `owner` permissions (default owner_id)

[permissions]                # see permissions.md — omitted keys use safe defaults
list   = "public"
read   = "public"
create = "authenticated"
update = "owner"
delete = "role:admin"

[fields.title]
type       = "string"
required   = true
max_length = 200

[fields.body]
type = "text"

[fields.published]
type    = "boolean"
default = false

[fields.owner_id]
type       = "reference"
references = "user"

[hooks]                      # see hooks.md — optional custom logic per operation
before_create = "post_before_create"
```

This publishes:

| Method | Path | Action |
|--------|------|--------|
| GET | `/api/post` | list |
| POST | `/api/post` | create |
| GET | `/api/post/{id}` | read |
| PATCH / PUT | `/api/post/{id}` | update |
| DELETE | `/api/post/{id}` | delete |

## `[resource]`

| Key | Default | Meaning |
|-----|---------|---------|
| `name` | *(required)* | Logical name and URL segment. Use `snake_case`. |
| `table` | `apiplant_<name>` | Physical Postgres table name. |
| `timestamps` | `true` | Adds `created_at` and `updated_at` (`timestamptz`, default `now()`, `updated_at` bumped on every update). |
| `owner_field` | `owner_id` | Column that identifies the owner for `owner` permissions. See [Permissions](permissions.md#ownership). |

## Automatic columns

Every resource always gets:

* `id` — `uuid` primary key, defaulted with `gen_random_uuid()`.
* `created_at`, `updated_at` — unless `timestamps = false`.

You cannot declare a field named `id` (it's reserved).

## Fields

Each `[fields.<name>]` table declares one column.

### Field types

| `type` | Postgres type | JSON in/out |
|--------|---------------|-------------|
| `string` | `varchar` (or `varchar(N)` with `max_length`) | string |
| `text` | `text` | string |
| `integer` | `integer` | number |
| `big_int` | `bigint` | number |
| `float` | `double precision` | number |
| `boolean` | `boolean` | boolean |
| `uuid` | `uuid` | string |
| `timestamp` | `timestamptz` | RFC 3339 string |
| `json` | `jsonb` | any JSON |
| `reference` | `uuid` + foreign key | string (uuid) — see [Relationships](relationships.md) |

### Field options

| Option | Applies to | Effect |
|--------|-----------|--------|
| `required` | any | `NOT NULL`. |
| `unique` | any | `UNIQUE` constraint. A conflict returns **409**. |
| `hidden` | any | Column is **stripped from every API response** (e.g. password hashes). Still writable. |
| `max_length` | `string` | Emits `varchar(N)`. |
| `default` | scalar (bool/number/string) | Column `DEFAULT`. |
| `references` | `reference` | Target resource name (required for references). |
| `on_delete` | `reference` | Referential action: `restrict` (default), `set_null`, `cascade`, `no_action`. |

Example with several options:

```toml
[fields.email]
type       = "string"
required   = true
unique     = true
max_length = 320

[fields.password_hash]
type   = "string"
hidden = true          # never returned to clients

[fields.status]
type    = "string"
default = "draft"
```

## `[hooks]`

An optional section binding a [function](functions.md) to points in the
resource's request lifecycle, so you can validate, rewrite or observe each
operation:

```toml
[hooks]
before_create = "post_before_create"    # validate/normalise, or reject the request
after_create  = "post_after_create"    # record it, or reshape the response
after_list    = "post_after_list"
```

Available keys are `before_`/`after_` × `list`, `read`, `create`, `update`,
`delete`. Unknown keys are rejected at load time. Full details, including the
data each hook receives and what it can return, are in
[Lifecycle hooks](hooks.md).

## Migrations

There are **no migration files**. Your resource definitions are the desired
state; on every boot (`auto_migrate = true`) apiplant reconciles the database in
three idempotent, additive passes:

1. **Create** missing tables (with all current columns).
2. **Add** missing columns to existing tables (`ALTER TABLE ADD COLUMN IF NOT
   EXISTS`), applying declared defaults.
3. **Add** missing foreign keys for `reference` fields.

This is safe to run repeatedly. What it deliberately does **not** do:

* drop or rename columns/tables,
* change a column's type,
* add `NOT NULL` to a new column on a populated table without a default.

Destructive or type-changing migrations are left to you (run SQL directly, or
disable `auto_migrate`).

## Built-in resources

`organization`, `membership`, `user`, `api_key`, and `oauth_connection` exist in
every app with sensible defaults. Drop a `models/<name>.toml` with the same
`name` to **replace** the default and add fields or change permissions — the
framework keeps using it for auth, ownership, org resolution, and key lookup.
See [Authentication](authentication.md).

## Validation

A resource fails to load (and the server refuses to start) if:

* a field is named `id`,
* a `reference` field has no `references` target,
* `[hooks]` contains an unknown key or an empty function name.

Invalid SQL identifiers (table/column names outside `[A-Za-z_][A-Za-z0-9_]*`)
are rejected at query build time.
