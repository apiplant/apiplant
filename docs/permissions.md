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

Any of them can be narrowed to a **class of organisation** by appending
`@org_class=<name>` — see [Organisation classes](#organisation-classes).

## More than one answer per action

A single level gives everybody the same answer. Often an action needs several —
a `parent` who may edit the whole family's chores while a `kid` may edit only
their own is two answers to one question, and no level says both. So an action
may instead be a **set of rules**:

```toml
[permissions.update]
allow = ["role:parent@org_class=family"]
own   = ["role:kid@org_class=family"]
deny  = ["role:suspended"]
```

Each key takes one policy string or a list of them, each written in exactly the
grammar above — level, optionally narrowed to a class. What differs is what
matching one **gets you**:

| Key | Effect |
|-----|--------|
| `allow` | the action, unrestricted |
| `own` | the action, scoped to rows you own — the *allow-if-owner* outcome |
| `deny` | nothing, whatever else you match |

Two shorthands cover the ordinary cases, and both mean exactly a set:

```toml
update = "role:parent"                    # one allow rule — the original grammar
update = ["role:manager", "role:worker"]  # an allow list
```

So nothing written before rule sets existed changes meaning, and a policy that
never needs `own` or `deny` never needs the table form.

### How a caller is matched

In the order the server asks:

1. **`deny` first.** A caller matching any `deny` is refused even if they also
   match an `allow`, so an exception carved out of a broad grant cannot be won
   back by a second role they happen to hold.
2. **`allow` before `own`.** Both are a yes and `own` is the narrower one, so
   somebody matching both gets the wider answer — a parent who is also a kid
   edits everything.
3. **Otherwise, no.** There is no implicit "everyone else": naming who may act
   is the whole statement, and `deny` exists only to carve an exception out of
   an `allow`.

A rule matches exactly when its policy would have allowed the caller **on its
own**. That is the same code path the single-level form takes, which is what
keeps the two from drifting.

### Rules use the ordinary levels

Every level works in every clause, and this is where a set earns its keep:

```toml
# Admins manage the whole catalogue; anyone else signed in manages their own.
[permissions.update]
allow = ["role:admin"]
own   = ["authenticated"]
```

```toml
# Public to read, except for one organisation that has been cut off. `deny`
# wins over `public`, which is the only way to say this at all.
[permissions.read]
allow = ["public"]
deny  = ["member@org_class=suspended"]
```

```toml
# Anybody in the organisation may file one; only the two roles may resolve one.
[permissions.create]
allow = ["member"]

[permissions.update]
allow = ["role:support", "role:engineer"]
own   = ["member"]          # the reporter can still edit their own report
```

```toml
# The same action, answered per class: a school's teachers edit everything,
# a family's parents edit everything, and everyone else edits their own.
[permissions.update]
allow = ["role:teacher@org_class=school", "role:parent@org_class=family"]
own   = ["member"]
```

`owner` as a *level* and `own` as an *effect* are the same outcome reached two
ways: `update = "owner"` and `[permissions.update] own = ["authenticated"]` are
the same policy. The effect exists because the level cannot be combined with a
role — `own = ["role:kid"]` is the thing that had no spelling before.

### What `deny` does and does not match

A `deny` naming a role matches only a role the caller **actually holds**, never
the blanket one an [admin gets](#admin-holds-every-role). Read the other way,
`deny = ["role:kid"]` would lock out the very administrators who granted the
role — so denials are answered against stored roles alone.

### The shapes that mean `private`

An action is `private` — not exposed, `404`, omitted from the docs — when it is
written as the string `"private"`, and a table naming nobody at all collapses to
the same thing, since it has granted nothing:

```toml
update = "private"     # not exposed
[permissions.update]   # …and so is this: an empty table grants nobody
```

A set that merely matches nobody *today* is different: it is a live endpoint
refusing callers, because granting the role tomorrow changes the answer without
touching the config.

A misspelled key is an error rather than an omission — `alow = [...]` would
otherwise be an empty table, and a typo that silently locks everyone out is only
marginally better than one that lets everyone in.

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

## `role:` on global resources

`role:` works on a global resource too, but it **gates** rather than filters:
there is no tenant column to narrow to, so the question is only whether you hold
that role in the organisation you selected with `X-Organization`.

| Resource | What `role:admin` means |
|----------|-------------------------|
| `organization` | the organisations you are an admin *of* — a filter, so a list returns only those |
| anything else global | you are an admin of the active organisation, and every row is yours to see |

A request with no active organisation cannot answer the question, so it is a
`403` asking for the header rather than a silent pass.

This is what the built-in `billing_product` and `billing_price` rely on to be
admin-writable, and the [queue](queues.md) to be admin-readable. In a
single-tenant deployment — one organisation, the operator in it — it reads as
"the operator", which is what an ops table wants.

## Organisation classes

An organisation carries an `org_class`: what *kind* of tenant it is — `school`,
`customer`, `staff`. Any access level can be narrowed to one:

```toml
[permissions]
list   = "member"
update = "role:admin@org_class=school"   # admins, but only in a school
create = "member@org_class=staff"        # anyone in a staff organisation
```

A qualifier only ever **narrows**: `role:admin@org_class=school` is strictly
fewer people than `role:admin`. A policy without one applies in every
organisation, which is what every policy written before classes existed means —
so classing your organisations changes nothing until a permission asks for a
class.

| Where it is written | What the class is checked against |
|---------------------|-----------------------------------|
| an org-scoped resource | the **active** organisation (`X-Organization`) |
| the `organization` resource | each row — so a list returns only organisations of that class |
| any other global resource | the active organisation, as a gate |
| a function, an agent, `[ai] access`, `[queues] publish` | the active organisation, as a gate |

Because the qualifier needs an organisation, it applies even to the levels that
name none: `public@org_class=staff` is *not* public — it requires a caller who
has selected a staff organisation. A caller in the wrong class gets

```
403 requires an organisation of class `staff`
```

An organisation with **no** class matches no qualifier at all, and an empty
class in a policy (`"role:admin@org_class="`) is a typo, so it collapses to
`private` like any other unparseable access string.

### Who may set a class

The class decides what a `@org_class=` permission lets people do, so an
organisation able to write its own class could grant itself whatever those
permissions guard. `organization.org_class` is therefore **server-owned**: it is
stripped from every request body, exactly like `organization_id`, unless the
caller is named by one deployment-wide setting:

```toml
# main.toml
[organization]
org_class_editors = "member@org_class=admin"
```

It is written in this same grammar and answered against the organisation the
caller has **selected**, not the one being edited — which is how one back-office
organisation comes to administer everybody's classes. Unset, it is `private`:
nobody, and classes come only from seed data or SQL.

New organisations can start classed rather than waiting to be classified:

```toml
[organization]
default_org_class = "customer"
```

That covers both doors an organisation comes through — `POST /organization` and
the personal one every account is created with — so a deployment whose ordinary
tenant is one kind has its permissions apply from the moment an organisation
exists. Unset, new organisations carry no class, which no qualifier matches.

### What a class editor may do

Classing organisations is deployment-wide work, so the `organization` resource
answers a class editor differently — but only as far as the job needs:

| Action | For a class editor |
|--------|--------------------|
| `list` / `read` | **every** organisation, not only their own — you cannot class what you cannot find |
| `update` | `org_class` on any organisation, **and nothing else** |
| everything else | exactly the resource's ordinary policy |

A body carrying anything besides `org_class` goes by the normal `update`
policy, so an organisation's own admins keep sole control of its name, slug and
logo; a rename smuggled in beside a class change is refused rather than
half-applied. The rule is judged on the body the client sent *and* on the body a
`before_update` hook returns, so a hook cannot widen the write either.

All of this depends on the organisation the caller has **selected**: the setting
is answered against `X-Organization`, so the same account acting from an
unclassed organisation is nobody in particular, sees only its own organisations,
and may class nothing.

The [dashboard](admin.md)'s Organization screen shows the class as an editable
field to those the setting names and as plain text to everyone else, so nobody
is offered an input the server would ignore. For a class editor its
organisation list covers the whole deployment, with a **Mine / All** switch and
a name filter, and lets the class be set inline on any row — including
organisations they do not belong to, which are marked as such because switching
into one is not something membership allows.

## Decision model

For a given action and caller, evaluation yields one of:

| Result | Behaviour |
|--------|-----------|
| **Allow** | proceed unrestricted |
| **Allow-if-owner** | proceed, but every query is scoped with `owner_column = caller` |
| **Deny** | `401` if the caller is anonymous, `403` if authenticated but not permitted |

`owner` produces *allow-if-owner*, as does an [`own` clause](#more-than-one-answer-per-action);
the rest produce *allow* or *deny*.

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
app defines, without requiring a row per role, so adding a new role to a resource
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
  resource's password column are stamped by the server and stripped from any body
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
