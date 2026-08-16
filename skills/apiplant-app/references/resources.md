# Resources

A **resource** is one `resources/<name>.toml` file. apiplant turns it into a
Postgres table and a set of RESTful CRUD endpoints. Everything about the table
and its API is declared in that file; there are no migrations or handlers to
write.

```toml
# resources/post.toml
[resource]
name       = "post"          # required; also the URL segment (/api/post)
# table    = "posts"         # optional physical table name (default apiplant_post)
# timestamps = true          # add created_at / updated_at (default true)
# owner_field = "owner_id"   # column used for `owner` permissions (default owner_id)

[permissions]                # see permissions.md; omitted keys use safe defaults
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

[hooks]                      # see hooks.md; optional custom logic per operation
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

* `id`: a `uuid` primary key, defaulted with `gen_random_uuid()`.
* `created_at` and `updated_at`, unless `timestamps = false`.

A field named `id` cannot be declared; the name is reserved.

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
| `timestamp` | `timestamptz` | RFC 3339 string in; ISO 8601 out, as Postgres renders the column in the session's `TimeZone` (UTC unless configured otherwise) |
| `json` | `jsonb` | any JSON |
| `file` | `varchar(1024)` | string — the URL the file is served from; see [File storage](storage.md) |
| `reference` | `uuid` + foreign key | string (uuid); see [Relationships](relationships.md) |

### Field options

| Option | Applies to | Effect |
|--------|-----------|--------|
| `required` | any | `NOT NULL`. |
| `unique` | any | `UNIQUE` constraint. A conflict returns **409**. |
| `hidden` | any | Column is **stripped from every API response**, for example password hashes. It remains writable. |
| `max_length` | `string`, `file` | Emits `varchar(N)`. A `file` defaults to `varchar(1024)`. |
| `case` | `string`, `text` | `upper` or `lower`. The value is forced into that case. See [Forcing a case](#forcing-a-case). |
| `default` | scalar (bool/number/string) | Column `DEFAULT`. See [Defaults](#defaults). |
| `default_type` | any with a `default` | `literal` (default) or `expression` — whether the `default` is a value or SQL. See [Defaults](#defaults). |
| `references` | `reference` | Target resource name (required for references). |
| `on_delete` | `reference` | Referential action: `restrict` (default), `set_null`, `cascade`, `no_action`. |

### Defaults

The default itself is always `default`. What changes is how it is read, and
that is `default_type`: a **value** or a **computation**.

`default_type = "literal"` is the default and the usual case. The value is
rendered as a SQL literal, quoted and escaped, so whatever it says is exactly
what an omitted field stores:

```toml
[fields.published]
type    = "boolean"
default = false

[fields.status]
type    = "string"
default = "draft"      # default_type = "literal" is implied
```

`default_type = "expression"` says the `default` is SQL, passed to the database
verbatim, so the column can be defaulted to something the database works out per
row:

```toml
[fields.issued_at]
type         = "timestamp"
default      = "now()"
default_type = "expression"

[fields.due_date]
type         = "timestamp"
default      = "now() + interval '30 days'"
default_type = "expression"

[fields.reference]
type         = "string"
default      = "'INV-' || to_char(now(), 'YYYYMMDD')"
default_type = "expression"

[fields.token]
type         = "uuid"
default      = "gen_random_uuid()"
default_type = "expression"
```

The distinction matters most for timestamps: `default = "now()"` alone emits
`DEFAULT 'now()'` — a string literal a `timestamptz` rejects — while adding
`default_type = "expression"` emits `DEFAULT now()`, the call. The same string,
read two ways, which is why the choice is a key of its own rather than something
inferred from how the text happens to look. An `expression` needs a `default`,
and that `default` must be a string; anything else fails the load.

Two things follow from an expression being real SQL:

* **It is code you write, not input you accept.** It is pasted into the
  `CREATE TABLE` / `ALTER TABLE`, on the same footing as the rest of the resource
  file. Loading rejects an expression containing `;`, a SQL comment, an unclosed
  quote or unbalanced parentheses — the shapes that would stop being a single
  expression — and Postgres reports anything else it cannot evaluate when the
  migration runs. Nothing a request supplies belongs here.
* **The dashboard and CLI cannot pre-fill it.** A literal `default` shows up as
  the initial value in a create form; an expression has no value until the row
  is inserted, so the form leaves the field empty and the database fills it.

Both kinds apply on **insert only**, and only when the request omits the field.
A value that must be recomputed on update, or derived from another column in the
same row, belongs in a [`before_create`/`before_update` hook](hooks.md#recipes)
instead — a hook can also enforce the relationship between two columns, which
two independent defaults cannot.

### Forcing a case

Some text is prose and some is a **code** — a currency, a country, a ticket
prefix, a coupon. A code has a conventional spelling, and `eur` and `EUR` are
the same value written two ways. Left alone that produces two of everything: two
rows a `unique` constraint sees as distinct, two groups in a report, and a
`?currency=eur` filter that finds only whichever the caller guessed.

```toml
[fields.currency]
type       = "string"
max_length = 3
case       = "upper"        # or "lower"
```

This applies in three places, which together are what make it a guarantee
rather than a convention:

* **On write.** The column stores one spelling, so `unique`, sorting and
  equality all mean what they look like.
* **On filters.** `?currency=eur` is cased the same way before it is compared,
  so it matches the rows stored as `EUR`.
* **On read.** Rows are cased on the way out as well, so a value written by a
  seed file, a webhook or a hand-run `UPDATE` reads back like one written
  through the API. Without this the promise would hold only for rows that
  happened to arrive through `POST` — which is rarely where the interesting
  data comes from.

Ignored on non-text fields: there is no case to force on a number, and quietly
doing nothing is better than upper-casing the digits of a price.

Case folding is ASCII-only, deliberately. These are codes, and full Unicode
folding turns a Turkish `i` into a character that no longer matches the code it
came from.

A field may also carry a `[fields.<name>.admin]` sub-table describing how it is
*presented* in the generated dashboard: a label, help text, a widget, or a set
of choices. See [Admin dashboard](admin.md#admin-on-a-field).

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

## `[rate_limit]`

An optional section narrowing — or lifting — the app-wide
[`[rate_limit]`](configuration.md#rate_limit), per action:

```toml
[rate_limit]
all    = "60/1m"   # every endpoint of this resource
create = "5/1m"    # …except this one, which is stricter
list   = "off"     # …and this one, which isn't limited at all
```

The keys are the ones `[permissions]` uses (`list`, `read`, `create`, `update`,
`delete`) plus `all`, which applies wherever an action names no rule of its own.
An action that names nothing and no `all` falls through to `main.toml`. A value
is `"<requests>/<window>"` (`"100/1m"`, `"30/30s"`, `"1000/1h"`), `"off"`, or
`"inherit"`; anything else fails the load.

Each rule gets its own count, so a caller who has spent this resource's `create`
allowance can still read.

## `[publish]`

An optional section announcing the resource's successful writes on a
[queue topic](queues.md), so that work which should follow a write does not have
to happen inside the request that made it:

```toml
[publish]
after_create = "order.placed"
after_update = "order.changed"
after_delete = "order.cancelled"
```

The row *is* the message: a subscriber gets the record exactly as the API would
have returned it, which is why no function is needed here at all. The caller's
response goes out as soon as the row is committed, and nothing a handler does
can fail the write.

Only the three `after_*` writes can be announced. A `before_*` topic would
announce something that has not happened yet and may still be rejected, and a
read or a list has nothing worth announcing. Unknown keys and unusable topic
names are both rejected at load time.

Which functions run is `[queues.subscribe]` in `main.toml` — nothing here names
a handler, which is what lets one be added without touching this file. See
[Queues](queues.md).

## `[admin]`

An optional section controlling how the resource appears in the generated
[admin dashboard](admin.md): its label, its position in the navigation, the
columns its table shows, and which roles can see it:

```toml
[admin]
label   = "Product"
plural  = "Products"
group   = "Catalogue"
roles   = ["manager"]        # who sees it; empty means anyone who may list it
columns = ["name", "status", "category_id"]
```

This is **presentation only**. Hiding a resource here does not close its
endpoints; that is the role of `[permissions]`, and the two are intentionally
independent. Every key is optional, and without the section a resource still
appears, with labels and columns inferred from its fields.

Full reference: [Admin dashboard](admin.md#admin-on-a-resource).

## Migrations

There are **no migration files**. Your resource definitions are the desired
state; on every boot (`auto_migrate = true`) apiplant reconciles the database in
three idempotent, additive passes:

1. **Create** missing tables (with all current columns).
2. **Add** missing columns to existing tables (`ALTER TABLE ADD COLUMN IF NOT
   EXISTS`), applying declared defaults.
3. **Add** missing foreign keys for `reference` fields.

This is safe to run repeatedly. It does **not**:

* drop or rename columns or tables,
* change a column's type,
* add `NOT NULL` to a new column on a populated table without a default.

Destructive or type-changing migrations remain your responsibility: run the SQL
directly, or disable `auto_migrate`.

## Built-in resources

`organization`, `membership`, `user`, `api_key`, and `oauth_connection` exist in
every app with sensible defaults. Add a `resources/<name>.toml` with the same
`name` to **replace** the default, adding fields or changing permissions; the
framework continues to use it for auth, ownership, organisation resolution and
key lookup. See [Authentication](authentication.md).

## Validation

A resource fails to load (and the server refuses to start) if:

* a field is named `id`,
* a `reference` field has no `references` target,
* `[hooks]` contains an unknown key or an empty function name,
* `[admin]` names a field that does not exist (in `columns`, `display_field`
  or `search_field`), or carries an unrecognised key.

Invalid SQL identifiers (table or column names outside
`[A-Za-z_][A-Za-z0-9_]*`) are rejected at query build time.
