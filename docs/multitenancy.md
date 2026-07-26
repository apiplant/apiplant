# Multitenancy

apiplant apps are **multitenant by default**. Every resource you create belongs
to an organisation, and the framework isolates data per organisation
automatically — you don't write a single line of tenancy code.

## The model

* **`organization`** — the tenant. A built-in, `global` resource.
* **`membership`** — the N:N join between users and organisations. A user can
  belong to any number of organisations, and each membership carries the user's
  **primary role in that organisation** (`admin`, `member`, or anything you
  like).
* **`membership_role`** — the additional roles a membership holds. Roles are a
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

1. **resolves the active organisation** (see below) and **requires membership** —
   non-members get `403`;
2. **filters every query** to `organization_id = <active org>` — you only ever
   see your organisation's rows;
3. **stamps `organization_id`** on create and refuses it on update, so clients
   can neither spoof the tenant nor move a row between organisations;
4. then applies the resource's [permission](permissions.md) for *who among the
   members* may act.

So a plain model with no tenancy code is already isolated:

```toml
# models/project.toml — organisation-scoped automatically
[resource]
name = "project"

[fields.title]
type = "string"
required = true
```

Members of org A never see org B's projects; the creator's organisation is
stamped for them; deleting an organisation cascades to its rows.

## The active organisation

Org-scoped requests act within one organisation, resolved as:

1. the **`X-Organization: <org-id>`** header, if it names an org the caller
   belongs to;
2. otherwise the caller's **only** organisation, if they belong to exactly one;
3. otherwise **none** — a multi-org caller must send the header, or they get
   `403 select an organisation`.

Single-org users never need the header; multi-org users pass it per request.

## Roles are per-organisation

There is no global "admin". A user's roles live on their **membership**, so
`role:admin` means *admin of the active organisation*. The same user can be an
admin of one org and a plain member of another.

A membership holds as many roles as you give it — the primary `role` plus a
`membership_role` row each — and an `admin` holds every role the app defines
without being granted them. Nobody may remove their own `admin`, which is what
guarantees an organisation always has someone who can administer it. The rules
are in [Permissions](permissions.md#roles).

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
| `role:<name>` | a member holding `<name>` among their roles — or holding `admin` |
| `owner` | the row's owner (`owner_id`), within the organisation |
| `public` / `authenticated` | treated like `member` (org context is always required) |
| `private` | never exposed |

Membership and org isolation are enforced *before* these — they only narrow who,
never widen it.

## Lifecycle

**Creating an organisation** (`POST /organization`, any authenticated user) makes
the creator an `admin` member automatically, so they can immediately manage it:

```bash
# 1. sign up
curl -XPOST $API/auth/register -d '{"email":"a@co.com","password":"pw"}'   # → token

# 2. create an org — you become its admin
curl -XPOST $API/organization -H "authorization: Bearer $TOKEN" \
     -d '{"name":"Acme","slug":"acme"}'

# 3. now you have an active org; scoped resources just work
curl -XPOST $API/post -H "authorization: Bearer $TOKEN" -d '{"title":"hello"}'
# organization_id + owner_id stamped for you
```

**Inviting members** — an org admin creates memberships, naming the person
either by id or by the address they registered with:

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

The email form matters because it is the only one that works for someone you
have never worked with: `user` reads as `member`, so an admin can look up their
colleagues but not a stranger — and a stranger is exactly who they are adding.
The lookup therefore happens on the server, in the built-in
`apiplant_organization_join` hook on `membership` (see
[Lifecycle hooks](hooks.md#built-in-functions)). It answers `404` when nobody is
registered with that address and `409` when they are already a member, and
`email` never reaches the table — it is an instruction to the hook, not a column.

**Switching org** — send `X-Organization: <org-id>` (must be one you belong to).

## Opting out: global resources

Reference data that everyone shares — a public catalog, feature flags, the
`user`/`organization` resources themselves — isn't tenant-scoped. Mark it
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

On a `global` resource there is no org filtering, `member`/`role:` don't apply
(there's no active org to check), and `public`/`authenticated` mean what they say.
The built-in `organization` resource is a special case: `member`/`role:` on it are
scoped to the organisations you belong to (so listing organisations returns
yours, and `role:admin` targets those where you're an admin).

## In the API & docs

* Send `X-Organization` to pick the tenant (or rely on your sole membership).
* `403 select an organisation with the X-Organization header` — you belong to
  several orgs; pick one.
* `403 you are not a member of this organisation` — the active org isn't yours.
* The generated OpenAPI docs note each operation's requirement (e.g. *"Requires
  the `admin` role in the active organisation."*).

See also: [Permissions](permissions.md), [Authentication](authentication.md),
[Resources](resources.md).
