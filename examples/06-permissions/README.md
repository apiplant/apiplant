# 06 · Permissions

Each resource declares a policy **per action**. Three resources here cover the
whole access model.

```
06-permissions/
└── resources/
    ├── plan.toml           # public catalogue, private writes
    ├── diary.toml          # owner-only rows
    └── announcement.toml   # org-scoped, role-gated
```

## The levels

| Level | Meaning |
|-------|---------|
| `public` | no authentication |
| `authenticated` | any signed-in caller |
| `member` | a member of the caller's active organisation |
| `role:<name>` | a member holding that role **in that organisation** — or holding `admin`, which holds them all |
| `owner` | the caller owns the row (`owner_field`, default `owner_id`) |
| `private` | never exposed — answers `404` |

Omit a key and it defaults to `member`, i.e. multitenant-by-default.

## Run it

```bash
createdb -h 127.0.0.1 -p 5432 -U postgres apiplant_permissions
cargo run -p apiplant -- run examples/06-permissions
```

**`plan` — public reads, no writes.**

```bash
curl -s localhost:8099/api/plan                        # → 200 []
curl -s -XPOST localhost:8099/api/plan -i \
  -H 'content-type: application/json' -d '{"name":"Pro"}'   # → 404, not 403
```

`private` answers `404` on purpose: a forbidden endpoint that says "forbidden"
still confirms it exists. Seed rows with SQL, or a hook, or by relaxing the
policy.

**`diary` — owner-only.**

```bash
T1=$(curl -s -XPOST localhost:8099/api/auth/register -H 'content-type: application/json' \
      -d '{"email":"ana@example.com","password":"pw"}' | jq -r .token)
T2=$(curl -s -XPOST localhost:8099/api/auth/register -H 'content-type: application/json' \
      -d '{"email":"bo@example.com","password":"pw"}' | jq -r .token)

ID=$(curl -s -XPOST localhost:8099/api/diary -H "authorization: Bearer $T1" \
      -H 'content-type: application/json' -d '{"entry":"Dear diary…"}' | jq -r .id)

curl -s localhost:8099/api/diary -H "authorization: Bearer $T2"        # → [] — not Ana's
curl -s localhost:8099/api/diary/$ID -H "authorization: Bearer $T2" -i # → 404
```

`owner_id` is stamped from the caller on create; a client can't set it. Ownership
filters the query, so other people's rows are invisible rather than forbidden.

**`announcement` — organisation roles.**

This resource is org-scoped (no `scope` line), so the caller must be a member
*and* satisfy the policy:

```bash
ORG=$(curl -s -XPOST localhost:8099/api/organization -H "authorization: Bearer $T1" \
       -H 'content-type: application/json' -d '{"name":"Acme","slug":"acme"}' | jq -r .id)

# The creator is the organisation's `admin`, and an admin holds every role the
# app defines — so `role:editor` passes without anyone granting `editor`:
curl -s -XPOST localhost:8099/api/announcement -H "authorization: Bearer $T1" \
  -H 'content-type: application/json' -d '{"headline":"Hello"}' -i   # → 201
```

Somebody who is *not* an admin has to be given the role. Add a second account,
then grant it:

```bash
T2=$(curl -s -XPOST localhost:8099/api/auth/register -H 'content-type: application/json' \
      -d '{"email":"ed@example.com","password":"pw"}' | jq -r .token)

# Added as a plain `member`: `role:editor` refuses them.
MEM2=$(curl -s -XPOST localhost:8099/api/membership -H "authorization: Bearer $T1" \
        -H "x-organization: $ORG" -H 'content-type: application/json' \
        -d '{"email":"ed@example.com","role":"member"}' | jq -r .id)
curl -s -XPOST localhost:8099/api/announcement -H "authorization: Bearer $T2" \
  -H "x-organization: $ORG" -H 'content-type: application/json' \
  -d '{"headline":"Mine"}' -i                                        # → 403

# Roles are a set: this *adds* `editor` and leaves `member` alone.
curl -s -XPOST localhost:8099/api/membership_role -H "authorization: Bearer $T1" \
  -H "x-organization: $ORG" -H 'content-type: application/json' \
  -d "{\"membership_id\":\"$MEM2\",\"role\":\"editor\"}"
curl -s -XPOST localhost:8099/api/announcement -H "authorization: Bearer $T2" \
  -H "x-organization: $ORG" -H 'content-type: application/json' \
  -d '{"headline":"Mine"}' -i                                        # → 201
```

Two rules go with that. Granting a role somebody already holds answers `409` —
one role granted twice is not twice the permission, just a copy that makes
revoking the other one look broken. And nobody may take `admin` off themselves:

```bash
MEM1=$(curl -s "localhost:8099/api/membership?organization_id=$ORG" \
        -H "authorization: Bearer $T1" | jq -r '.[] | select(.role=="admin") | .id')
curl -s -XPATCH localhost:8099/api/membership/$MEM1 -H "authorization: Bearer $T1" \
  -H "x-organization: $ORG" -H 'content-type: application/json' \
  -d '{"role":"member"}' -i                                          # → 403
```

Another admin could demote them; they cannot demote themselves. That is what
keeps every organisation with at least one administrator.

Roles live on the **membership**, so the same person can be an admin in one
organisation and a reader in another.

## Classes of organisation

`feature_flag` is a global resource written `member@org_class=admin`: any member
will do, provided the organisation they have **selected** is of class `admin`.
The seed gives Operations that class and puts `admin@example.com` in it, so the
same account passes or fails depending on which organisation it names:

```bash
OPS=$(curl -s localhost:8099/api/organization -H "authorization: Bearer $T1" \
       | jq -r '.[] | select(.slug=="operations") | .id')

curl -s -XPATCH localhost:8099/api/feature_flag/$FLAG -H "authorization: Bearer $T1" \
  -H "x-organization: $OPS" -H 'content-type: application/json' \
  -d '{"enabled":false}'                                             # → 200

curl -s -XPATCH localhost:8099/api/feature_flag/$FLAG -H "authorization: Bearer $T1" \
  -H "x-organization: $ORG" -H 'content-type: application/json' \
  -d '{"enabled":false}' -i                                          # → 403, Acme is a customer
```

A qualifier only ever narrows: `role:admin@org_class=school` is fewer people
than `role:admin`, and a permission with no class applies everywhere — so
classing organisations changes nothing until a permission asks for one.

The class itself is server-owned. `main.toml` says who may write it:

```toml
[organization]
org_class_editors = "member@org_class=admin"
```

Anyone else sending `org_class` has it dropped from the body, exactly like
`organization_id` — otherwise an organisation could class itself into whatever
these permissions guard.

Being named there also widens what you can see, because you cannot class an
organisation you cannot find:

```bash
# acting from Operations: every organisation, including ones you are not in
curl -s "localhost:8099/api/organization" -H "authorization: Bearer $T1" \
  -H "x-organization: $OPS" | jq '.[].slug'

# the class of one of them — and only the class
curl -s -XPATCH localhost:8099/api/organization/$SOMEONE_ELSES \
  -H "authorization: Bearer $T1" -H "x-organization: $OPS" \
  -H 'content-type: application/json' -d '{"org_class":"customer"}'   # → 200

curl -s -XPATCH localhost:8099/api/organization/$SOMEONE_ELSES \
  -H "authorization: Bearer $T1" -H "x-organization: $OPS" \
  -H 'content-type: application/json' -d '{"name":"Hijacked"}' -i     # → 404
```

Drop the `x-organization` header, or point it at Acme, and both the wide list
and the class write go away: the setting is answered against the organisation
you selected.

## Which status you get

| Code | When |
|------|------|
| `401` | anonymous caller hit a protected action |
| `403` | signed in, but wrong role — or no organisation selected |
| `404` | `private` action, or an owned row you don't own |

Details in [Permissions](../../docs/permissions.md).

**Next:** [07 · Functions](../07-functions) — writing custom code.
