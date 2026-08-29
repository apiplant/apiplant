# Multitenancy

apiplant apps are **multitenant by default**. Every resource you create belongs
to an organisation, and the framework isolates data per organisation
automatically, with no tenancy code to write.

## The model

* **`organization`**: the tenant. A built-in, `global` resource.
* **`membership`**: the N:N join between users and organisations. A user can
  belong to any number of organisations, and each membership carries the user's
  **primary role in that organisation** (`admin`, `member`, or any name the app
  defines).
* **`membership_role`**: the additional roles a membership holds. Roles form a
  set: `membership.role` plus these rows is what a `role:` permission is checked
  against.
* Every other resource is **organisation-scoped** by default: it carries an
  `organization_id` (injected automatically if you don't declare one), and all
  access is confined to the caller's *active organisation*.

```
user ─┐            ┌─ organization
      ├─ membership ┤   (role: the primary one)
      └─────────────┘
             │       ▲
             │       └─ post, comment, …  (organization_id, auto-added & enforced)
             └─ membership_role  (one row per further role held)
```

## What "automatic" means

For an organisation-scoped resource, on every request the framework:

1. **resolves the active organisation** (see below) and **requires membership**;
   non-members receive `403`;
2. **filters every query** to `organization_id = <active org>`, so only the
   active organisation's rows are visible;
3. **stamps `organization_id`** on create and refuses it on update, so clients
   can neither spoof the tenant nor move a row between organisations;
4. then applies the resource's [permission](permissions.md) for *who among the
   members* may act.

So a plain resource with no tenancy code is already isolated:

```toml
# resources/project.toml: organisation-scoped automatically
[resource]
name = "project"

[fields.title]
type = "string"
required = true
```

Members of org A never see org B's projects; the creator's organisation is
stamped for them; deleting an organisation cascades to its rows.

## The personal organisation

Every account is created with its own organisation and is its `admin`. It is
created by whichever route created the account (`POST <base>/auth/register`, an
accepted invitation, or `POST <base>/user`), named after the account using its
name or the local part of the identity it registered with. In every other
respect it is an **ordinary organisation**: it can be renamed, have people
invited into it, or be left alone while others are created alongside it.

This ensures no account is ever in a state where every org-scoped read is empty
and every write is refused. Deleting an account also deletes its memberships,
along with any organisation left with no members.

## The active organisation

Org-scoped requests act within one organisation, named by the
**`X-Organization: <org-id>`** header. It must name an org the caller belongs
to; a request without it has no active organisation and gets
`403 select an organisation with the X-Organization header`.

There is no fallback, even for a caller with a single membership. Every account
is created with a **personal organisation** and may create others, so an
implicit choice is never safe: a client relying on one would silently change
behaviour as soon as its user created a second organisation.

## Roles are per-organisation

There is no global "admin". A user's roles live on their **membership**, so
`role:admin` means *admin of the active organisation*. The same user can be an
admin of one org and a plain member of another.

A membership holds any number of roles: the primary `role` plus one
`membership_role` row per additional role. An `admin` satisfies every role the
app defines without being granted them explicitly. Nobody may remove their own
`admin`, which guarantees an organisation always retains an administrator. The
rules are in [Permissions](permissions.md#roles).

```toml
[permissions]
list   = "member"       # any member of the active org
create = "member"
update = "owner"        # the row's owner (within the org)
delete = "role:admin"   # an admin *of this organisation*
```

## Access levels on scoped resources

| Level | Meaning on an org-scoped resource |
|-------|------------------------------------|
| `member` | any member of the active organisation (the default) |
| `role:<name>` | a member holding `<name>` among their roles, or holding `admin` |
| `owner` | the row's owner (`owner_id`), within the organisation |
| `public` / `authenticated` | treated like `member` (org context is always required) |
| `<level>@org_class=<name>` | the same level, but only in an organisation of that class |
| `private` | never exposed |

Membership and organisation isolation are enforced *before* these levels, which
can only narrow access, never widen it.

## Classes of organisation

Organisations carry an `org_class` — `school`, `customer`, `staff` — and a
permission can be narrowed to one:

```toml
[permissions]
update = "role:admin@org_class=school"   # admins, in schools only
```

An unqualified permission applies in every organisation, so adding classes
changes nothing until something asks for one. The column is server-owned: only
callers named by `[organization] global_admin_role` in `main.toml` may write it,
which is what stops an organisation from classing itself into access it was not
given.

`[organization] default_org_class` gives every new organisation a starting
class — the personal one each account is created with included — so a
deployment whose tenants are all of one kind does not have to classify them one
by one.

Those callers are the one exception to organisation isolation anywhere: role
checks and organisation checks do not apply to them, so they list every
organisation and every user and reach data in all of them — which is what makes
one organisation the deployment's back office. What they do *not* bypass is
`private`, which says a thing is not on the API rather than that they lack a
permission for it. The rules are in
[Permissions](permissions.md#organisation-classes).

## Lifecycle

**Creating an organisation** (`POST /organization`, any authenticated user) makes
the creator an `admin` member automatically, so they can immediately manage it:

```bash
# 1. sign up
curl -XPOST $API/auth/register -d '{"email":"a@co.com","password":"pw"}'   # → token

# 2. create an org; the creator becomes its admin
curl -XPOST $API/organization -H "authorization: Bearer $TOKEN" \
     -d '{"name":"Acme","slug":"acme"}'

# 3. with an active org, scoped resources work without further setup
curl -XPOST $API/post -H "authorization: Bearer $TOKEN" -d '{"title":"hello"}'
# organization_id + owner_id stamped for you
```

**Inviting members.** An org admin creates memberships, naming the person either
by id or by the address they registered with:

```bash
curl -XPOST $API/membership -H "authorization: Bearer $ADMIN_TOKEN" \
     -d '{"email":"new.hire@example.com","role":"member"}'   # organization_id stamped automatically

curl -XPOST $API/membership -H "authorization: Bearer $ADMIN_TOKEN" \
     -d '{"user_id":"<uuid>","role":"member"}'
```

**Giving somebody another role** adds a `membership_role` row; their existing
roles are untouched.

```bash
curl -XPOST $API/membership_role -H "authorization: Bearer $ADMIN_TOKEN" \
     -d '{"membership_id":"<uuid>","role":"billing"}'   # organization_id stamped automatically

curl -XDELETE $API/membership_role/<uuid> -H "authorization: Bearer $ADMIN_TOKEN"
```

The email form is necessary because it is the only one that works for someone
outside the organisation: `user` is read at `member` level, so an admin can look
up colleagues but not people they do not already share an organisation with,
which is precisely who they are adding. The lookup therefore happens on the
server, in the built-in
`apiplant_organization_join` hook on `membership` (see
[Lifecycle hooks](hooks.md#built-in-functions)). It returns `404` when no
account is registered with that address and `409` when the user is already a
member. `email` never reaches the table; it is an instruction to the hook rather
than a column.

That `404` marks the limit of this endpoint: it can only add existing accounts.
An app with an [`[email]` provider](email.md) also gets
[`POST <base>/auth/invitations`](authentication.md#invitations), which emails a
link that creates the account and the membership together, so adding someone no
longer requires them to have registered first. The dashboard and the console
expose both through the same button, choosing based on whether the server can
send mail.

**Switching organisation.** Send `X-Organization: <org-id>`, naming an
organisation you belong to.

## Opting out: the whole thing

An app that is not multitenant says so once:

```toml
[organization]
enabled = false
```

One implicit organisation, and everybody is in it. No table is dropped and no
column removed — `organization_id` stays on every scoped resource, pointing at a
single auto-provisioned row — so this is a switch rather than a migration, and
turning tenancy on later leaves the existing rows in an organisation that
already exists. What changes is that `X-Organization` selects nothing, `member`
and `role:` stop being questions about where a caller stands, and the dashboard
drops its organisation switcher and organisation screens. `[auth] enabled =
false` implies it: a membership is a user's, so an app with no accounts has
nobody to be a member. See
[Turning organizations off](configuration.md#turning-organizations-off).

## Opting out: global resources

Shared reference data (a public catalogue, feature flags, or the
`user` and `organization` resources themselves) is not tenant-scoped. Mark it
`global`:

```toml
[resource]
name = "plan"
scope = "global"        # not organisation-scoped

[permissions]
list = "public"         # now `public`/`authenticated` behave normally
read = "public"
create = "private"
```

On a `global` resource there is no organisation filtering, `member` and `role:`
do not apply since there is no active organisation to check, and `public` and
`authenticated` have their literal meanings. The built-in `organization`
resource is a special case: `member` and `role:` on it are scoped to the
organisations you belong to, so listing organisations returns your own and
`role:admin` targets those where you are an admin.

## In the API & docs

* Send `X-Organization` on every org-scoped request to pick the tenant.
* `403 select an organisation with the X-Organization header`: the request did
  not name one.
* `403 you are not a member of this organisation`: the caller does not belong to
  the active organisation.
* The generated OpenAPI docs note each operation's requirement (e.g. *"Requires
  the `admin` role in the active organisation."*).

See also: [Permissions](permissions.md), [Authentication](authentication.md),
[Resources](resources.md).
