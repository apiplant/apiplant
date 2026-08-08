# 04 · Multitenancy

Every resource so far used `scope = "global"`. Drop that line and the resource
becomes **organisation-scoped**, which is the default — and the tenant isolation
is automatic.

```
04-multitenancy/
└── models/
    ├── project.toml    # no organization_id field, no filtering code
    └── task.toml       # project_id → project
```

## What the framework does for you

For an org-scoped resource it:

1. injects an `organization_id` foreign key column (you never declare it),
2. requires the caller to be a **member** of an active organisation,
3. filters every list/read/update/delete to that organisation,
4. stamps `organization_id` on create — the client cannot set or spoof it.

There is no way to write a query that crosses tenants, because you don't write
the query.

## Run it

```bash
createdb -h 127.0.0.1 -p 5432 -U postgres apiplant_tenancy
cargo run -p apiplant -- run examples/04-multitenancy
```

```bash
# 1. an account
TOKEN=$(curl -s -XPOST localhost:8099/api/auth/register \
  -H 'content-type: application/json' \
  -d '{"email":"ana@example.com","password":"pw"}' | jq -r .token)

# 2. an organisation — whoever creates one becomes its `admin` member
ORG=$(curl -s -XPOST localhost:8099/api/organization \
  -H "authorization: Bearer $TOKEN" -H 'content-type: application/json' \
  -d '{"name":"Acme","slug":"acme"}' | jq -r .id)

# 3. work inside it
curl -s -XPOST localhost:8099/api/project \
  -H "authorization: Bearer $TOKEN" -H 'content-type: application/json' \
  -d '{"name":"Website relaunch"}'
# → the response already carries "organization_id": "<ORG>"
```

## Choosing the active organisation

Belong to exactly one organisation and it's implied. Belong to several and you
must pick per request:

```bash
curl -s localhost:8099/api/project \
  -H "authorization: Bearer $TOKEN" -H "X-Organization: $ORG"
```

Without a resolvable organisation an org-scoped endpoint answers
`403 select an organisation with the X-Organization header`.

## Try the isolation

Register a second user, give them their own organisation, and list `/api/project`
as them — the first organisation's projects are simply not there. Naming an
organisation you don't belong to in `X-Organization` is ignored rather than
honoured, so the request fails with `403 select an organisation with the
X-Organization header`; membership is re-checked against the database on every
request.

Sending `organization_id` in a create body is ignored too — the server always
stamps the caller's active organisation over it.

## Members and roles

`organization`, `membership`, `membership_role` and `user` are built-in
resources. A membership row carries the member's **primary** `role`, and each
`membership_role` row adds another — roles are a set, and `role:admin` in
`task.toml` passes if any of them is `admin`. Try deleting a task as a non-admin
member and you'll get `403`.

An `admin` holds every role the app defines, and nobody may remove their own
`admin` — so an organisation always has somebody who can administer it.

## Classes of organisation

The seed gives each organisation an `org_class`: Acme and Globex are
`customer`, and Operations — which `admin@example.com` also belongs to — is
`admin`. A permission can be narrowed to a class by appending
`@org_class=<name>` to any level, and `main.toml` names who may change a class
at all:

```toml
[organization]
org_class_editors = "member@org_class=admin"
```

The column is server-owned like `organization_id`, so any other caller sending
`org_class` has it dropped. See
[06 · Permissions](../06-permissions) for a resource guarded by a class.

Details in [Multitenancy](../../docs/multitenancy.md).

**Next:** [05 · Authentication](../05-auth) reshapes the `user` resource itself.
