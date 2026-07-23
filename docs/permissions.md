# Permissions

Every resource declares a `[permissions]` policy — one access level per CRUD
action. On each request apiplant resolves the caller (see
[Authentication](authentication.md)), evaluates the relevant action's policy, and
either allows it, allows it **scoped to owned rows**, or rejects it.

```toml
[permissions]
list   = "public"
read   = "public"
create = "authenticated"
update = "owner"
delete = "role:admin"
```

## The five actions

| Action | HTTP | Applies to |
|--------|------|------------|
| `list` | `GET /res` | the collection |
| `read` | `GET /res/{id}` | a single row |
| `create` | `POST /res` | new rows |
| `update` | `PATCH`/`PUT /res/{id}` | existing rows |
| `delete` | `DELETE /res/{id}` | existing rows |

The same `list` policy also governs the nested `GET /parent/{id}/res` endpoint
(see [Relationships](relationships.md)).

## Access levels

| Value | Who can do it |
|-------|---------------|
| `public` | anyone, no authentication |
| `authenticated` | any authenticated caller (session token or API key) |
| `owner` | authenticated **and** the row belongs to them (see [Ownership](#ownership)) |
| `role:<name>` | authenticated **and** holding the named role |
| `private` | nobody — the endpoint isn't exposed (returns 404 / omitted from docs) |

## Defaults

If you omit `[permissions]` entirely, or any individual key, these safe defaults
apply — **readable by anyone, writable only when authenticated**:

| Action | Default |
|--------|---------|
| `list` | `public` |
| `read` | `public` |
| `create` | `authenticated` |
| `update` | `authenticated` |
| `delete` | `authenticated` |

## Decision model

For a given action and caller, evaluation yields one of:

| Result | Behaviour |
|--------|-----------|
| **Allow** | proceed unrestricted |
| **Allow-if-owner** | proceed, but every query is scoped with `owner_column = caller` |
| **Deny** | `401` if the caller is anonymous, `403` if authenticated but not permitted |

`owner` produces *allow-if-owner*; the rest produce *allow* or *deny*.

## Ownership

`owner` policies compare a row's **owner column** to the caller's user id:

* The owner column is the resource's `owner_field` (default `owner_id`) **if that
  column exists**; otherwise it falls back to the row's own `id` — so on the
  `user` resource, `owner` means "the user themselves".
* On **create**, if the resource has a real owner column, apiplant **stamps it
  with the caller's id** and ignores any client-supplied value. This is what
  makes a later `update = "owner"` enforceable, and prevents spoofing.
* On **list / read / update / delete**, `owner` transparently adds
  `WHERE owner_column = <caller>`. A non-owner therefore gets an empty list, or a
  `404` for a specific row (it's filtered out, indistinguishable from missing).

```toml
# posts: anyone reads, authors edit their own, admins remove any
[permissions]
list = "public"
read = "public"
create = "authenticated"   # owner_id auto-stamped to the creator
update = "owner"
delete = "role:admin"
```

## Roles

Roles are just rows in the built-in `role` resource. A user references one via
`role_id`. When a user logs in, their role name is baked into their session
token; API-key callers get the role resolved by join. A `role:admin` policy
passes only when the caller's role name equals `admin`.

Assigning roles is ordinary data management:

```sql
INSERT INTO apiplant_role (name) VALUES ('admin');
UPDATE apiplant_user SET role_id =
  (SELECT id FROM apiplant_role WHERE name = 'admin')
  WHERE email = 'you@example.com';
```

(By default only an existing admin can create roles or assign them via the API —
bootstrap the first admin with SQL.)

## Worked examples

```toml
# Fully public read-only reference data
[permissions]
list = "public"
read = "public"
create = "private"
update = "private"
delete = "private"
```

```toml
# Private per-user data (todo items): each user sees only their own
[permissions]
list   = "owner"
read   = "owner"
create = "authenticated"   # owner_id stamped automatically
update = "owner"
delete = "owner"
```

```toml
# Staff-managed catalog: public reads, admin writes
[permissions]
list = "public"
read = "public"
create = "role:admin"
update = "role:admin"
delete = "role:admin"
```

## How the caller is determined

* `Authorization: Bearer <jwt>` — a session token from login/register.
* `Authorization: ApiKey <key>` or `X-Api-Key: <key>` — an API key; the request
  then acts **as the key's owning user**, with that user's role.
* No/invalid credentials ⇒ anonymous (only `public` actions succeed).

See [Authentication](authentication.md) for details.
