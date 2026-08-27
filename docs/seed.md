# Seed data

An app directory may hold a `seed/` directory: one file per resource, named
after it, holding the initial rows for that resource.

```
my-app/
├── main.toml
├── resources/
│   └── product.toml
└── seed/
    ├── organization.toml
    ├── user.toml
    ├── membership.toml
    └── product.csv
```

Load it with `apiplant seed`, or with `apiplant run --seed`:

```bash
apiplant seed ./my-app       # migrate, seed, stop
apiplant run --seed ./my-app # seed after migrating, then serve
```

Neither happens on an ordinary `apiplant run`. A fixture belongs to a fresh
database and a development machine, not to every restart of a production
server.

## Why seed data matters

An empty schema is not usable: nobody can sign in, every list is empty, and the
first task on any new app is creating an administrator by hand, either through
SQL (which requires knowing the password hashing scheme) or through the
registration endpoint followed by a manual promotion.

`seed/` captures that setup once: an administrator who can sign in, the
organisation they administer, and enough supporting data for the dashboard to
be useful. It is checked in alongside the resources, so every developer machine
and CI start from the same state.

## The file format

**TOML**, because it is the format the app is already written in. A seed file
is a list of `[[row]]` tables, and each key is a field of the resource:

```toml
# seed/product.toml
[[row]]
id = "orbit"
name = "Orbit Over-Ear"
slug = "orbit-over-ear"
price_cents = 24900
active = true
released_at = 2024-02-01T00:00:00Z
attributes = { driver_mm = 40, wireless = true }
```

TOML is typed: a boolean is a boolean, a `json` column takes a whole table, and
a `timestamp` may be written as a bare TOML datetime. A number written for a
string column is read as its digits, so a numeric postcode is accepted rather
than rejected.

**CSV** is accepted for larger fixtures, where a header line and one column per
field is more compact than repeating `[[row]]` headers, and because it is the
format a spreadsheet or `COPY … TO` produces:

```csv
# seed/country.csv
id,code,name,eu
it,IT,Italy,true
us,US,United States,false
```

In CSV, an empty cell means *no value*: the column is omitted and its default
(or NULL) applies. `""` means the empty string, which is distinct and sometimes
intended. A line starting with `#` is a comment in either format.

One file per resource: `seed/product.toml` **or** `seed/product.csv`, not both.

## Aliases instead of UUIDs

Ids are UUIDs, which are impractical to write by hand or repeat across several
files. Anywhere an id is expected (the `id` column, or any `reference` field) a
value that is not a UUID is treated as a **name** and hashed into a UUID:

```toml
# seed/organization.toml
[[row]]
id = "acme"
name = "Acme, Inc."

# seed/membership.toml
[[row]]
user_id = "admin"
organization_id = "acme"   # this is the organisation above
role = "admin"
```

`acme` resolves to the same UUID every time, on every machine and in every
file, so one file can reference another's rows by a readable name. A literal
UUID is used as-is, for fixtures that must pin an exact id.

A row with no `id` at all is given one derived from the resource and its
position in the file, which is what makes the CSV above safe to re-run.

## Running it twice changes nothing

Because those ids are derived rather than random, inserting is `ON CONFLICT DO
NOTHING`:

* Seeding twice inserts once.
* A seed file that grew a row since the last run adds only that row.
* A row already in the database is **never overwritten**, so edits made through
  the dashboard are preserved.

`apiplant seed` reports both counts:

```
  seed ./my-app

    organization            2 inserted, 0 already there
    user                    3 inserted, 0 already there
    product                 0 inserted, 4 already there

  ✓ seeded 5 rows (4 already there)
```

## Passwords

A `password` column on a resource with an [`[auth]`](authentication.md) section
is hashed with argon2, the same as registration, into whichever field
`password_field` names:

```toml
# seed/user.toml
[[row]]
id = "admin"
email = "admin@example.com"
password = "password"
display_name = "Ada Admin"
```

The plaintext never reaches the database, the fixture stays readable, and the
seeded account can sign in. Writing `password_hash` directly is rejected, since
a copied hash corresponds to an unknown password.

## Order, and what does not run

Files are loaded **parents before children**: a resource is seeded only after
every resource it references, so `seed/order_line.toml` may point at rows from
`seed/order.toml` regardless of the order the files were read in.

Seeding writes rows **directly**, so [hooks](hooks.md) do not run: no
`before_create` validation, no `after_create` side effects, no email. A fixture
describes the final state of a row rather than a request to be normalised on
the way in. As a result, a seeded row must supply anything a hook would
otherwise fill in: `seed/membership.toml` sets `user_id` explicitly, because
the hook that resolves a member by email does not run.

Two further consequences: permissions are not consulted (a fixture may populate
a table whose `create` policy is `private`, which is often the intent), and
`organization_id` is not applied automatically, so rows of an org-scoped
resource must name their tenant.

## Errors

Seeding fails loudly rather than skipping input:

* A file named after a resource the app does not define is an **error**, not a
  file that silently seeds nothing.
* So is a column that is not a field of that resource.
* So is a value the column's type cannot hold. These are reported with the file
  and row, rather than surfacing as a Postgres error much later in the run.

## In the examples

Every [example](../examples/) ships the same core fixture, so any of them can
be started and signed into at <http://127.0.0.1:8099/admin/> immediately:

| | |
|---|---|
| **Email** | `admin@example.com` (example 05 signs in as `admin`, since it keys its user by username) |
| **Password** | `password` |
| **Organisation** | Acme, Inc., administered by the seeded account |

Two further accounts, `editor@example.com` and `member@example.com`, use the
same password and hold different roles in Acme, which makes the permission
model easier to compare across two sessions. Example 13 (real-world) adds a
complete business dataset: products, stock, orders, shipments, payments,
refunds and support tickets.
