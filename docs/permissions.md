# Permissions

Every resource declares a `[permissions]` policy with one access level per CRUD
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
| `member` | a member of the active organisation (see [below](#member-on-global-resources) for global resources) |
| `owner` | authenticated **and** the row belongs to them (see [Ownership](#ownership)) |
| `role:<name>` | authenticated **and** holding the named role in the active organisation |
| `private` | nobody; the endpoint is not exposed (returns 404 and is omitted from the docs) |

## Defaults

If you omit `[permissions]` entirely, or any individual key, these safe defaults
apply: **member-only within the active organisation**.

| Action | Default |
|--------|---------|
| `list` | `member` |
| `read` | `member` |
| `create` | `member` |
| `update` | `member` |
| `delete` | `member` |

## `member` on global resources

On an org-scoped resource `member` is implicit, since every request is already
filtered to the active organisation. On a `scope = "global"` resource there is no
tenant column to filter on, so `member` is interpreted per resource:

| Resource | What `member` scopes to |
|----------|-------------------------|
| `organization` | the organisations you belong to |
| `user` | users who share at least one organisation with you (plus yourself) |
| anything else global | any authenticated caller (same as `authenticated`) |

This is why the built-in `user` ships with `list = "member"` and
`read = "member"`: colleagues can see each other's name and email, so a
membership list can use `?expand=user`, while unrelated users remain
inaccessible. The password column is `hidden` in either case and never leaves
the server.

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
  column exists**; otherwise it falls back to the row's own `id`, so on the
  `user` resource `owner` means the user themselves.
* On **create**, if the resource has a real owner column, apiplant **stamps it
  with the caller's id** and ignores any client-supplied value. This is what
  makes a later `update = "owner"` enforceable, and prevents spoofing.
* On **list / read / update / delete**, `owner` transparently adds
  `WHERE owner_column = <caller>`. A non-owner therefore gets an empty list, or a
  `404` for a specific row, which is filtered out and indistinguishable from a
  missing one.

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

Roles live on the built-in `membership` resource, not on the user globally. A
`role:admin` policy therefore means **admin of the active organisation**.

A member holds a **set** of roles rather than a single one: `membership.role` is
their primary role, and each `membership_role` row adds another. A user can hold
`billing` and `support` at the same time, and a `role:` policy passes if **any**
of their roles matches.

When a request is authorised against an org-scoped resource, apiplant:

1. resolves the active organisation,
2. checks that the caller is a member of it,
3. checks the requested `role:<name>` against every role that membership holds.

That means the same user can be an admin in one organisation and a plain member
in another. See [Multitenancy](multitenancy.md#roles-are-per-organisation).

### `admin` holds every role

An admin of an organisation satisfies **every** `role:` check in it. Granting
someone `admin` grants them `role:billing`, `role:support` and anything else the
app defines, without requiring a row per role, so adding a new role to a model
never locks out the organisation's administrators.

This is a rule about *checks*, not about stored data: `admin` is never expanded
into a user's roles. Their stored roles remain exactly the ones granted, so
revoking one has a well-defined effect, and demoting an admin also removes
everything the admin role implied.

### Nobody may remove their own `admin`

An admin can demote or remove **another** admin, but not themselves. This is a
structural guarantee rather than a convention: the only way for an organisation
to lose its last admin is for that admin to remove themselves, so forbidding it
ensures **every organisation always has at least one administrator**.

The rule covers every path to the same result (clearing your own `role`,
deleting your own `admin` grant, and deleting your own membership) and returns
`403`:

```
you cannot remove your own admin role; another admin can do it for you
```

Granting a role a user already holds returns `409`. Duplicate grants do not
confer additional permission, and they are error-prone, because revoking the
visible copy appears to have no effect.

### Granting and revoking

The [dashboard](admin.md)'s Team screen and the [console](cli.md#roles) both
list everyone in the organisation with the roles they hold, and allow granting
and revoking there. Neither offers an action the rules above would reject: no
duplicates, and never removing your own `admin`.

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

## What a policy does *not* cover

A policy decides who may perform an action; three things are decided regardless
of it, because no policy can express them safely:

* **Server-owned columns.** `organization_id`, the owner column and the `user`
  model's password column are stamped by the server and stripped from any body
  that carries them, on create and on update alike. Details in the
  [API reference](api-reference.md#server-owned-columns).
* **Expansion.** `?expand=` is a read of the *target* resource and runs that
  resource's `read` policy; a relation the caller may not read inlines as
  `null`. Granting `list` on one resource never grants reads of another
  through a foreign key.
* **Hidden fields.** A `hidden` field is stripped from responses *and* refused
  as a filter, so `?password_hash=…` cannot be used to confirm a value.

## How the caller is determined

* `Authorization: Bearer <jwt>`: a session token from login or register.
* `Authorization: ApiKey <key>` or `X-Api-Key: <key>`: an API key. The request
  acts **as the key's owning user**, with that user's memberships and
  permissions.
* No/invalid credentials ⇒ anonymous (only `public` actions succeed).

See [Authentication](authentication.md) for details.

## The same grammar elsewhere

Two other parts of the system use the same vocabulary:

* A [function](functions.md#permissions) declares a `permission` from the same
  set, excluding `owner`, since a function call has no row to own.
* The [admin dashboard](admin.md) evaluates these policies client-side to decide
  what to display. That is presentation only: it hides controls that would
  return `403`, but enforcement is always done by the server.
