# 06 · Permissions

Each resource declares a policy **per action**. Three resources here cover the
whole access model.

```
06-permissions/
└── models/
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
| `role:<name>` | a member holding that role **in that organisation** |
| `owner` | the caller owns the row (`owner_field`, default `owner_id`) |
| `private` | never exposed — answers `404` |

Omit a key and it defaults to `member`, i.e. multitenant-by-default.

## Run it

```bash
createdb -h 127.0.0.1 -p 55432 -U postgres apiplant_permissions
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

# The creator is an `admin`, so this is 403 — admin is not editor:
curl -s -XPOST localhost:8099/api/announcement -H "authorization: Bearer $T1" \
  -H 'content-type: application/json' -d '{"headline":"Hello"}' -i

# Grant yourself the editor role by updating your membership, then retry.
MEM=$(curl -s "localhost:8099/api/membership?organization_id=$ORG" \
       -H "authorization: Bearer $T1" | jq -r '.[0].id')
curl -s -XPATCH localhost:8099/api/membership/$MEM -H "authorization: Bearer $T1" \
  -H 'content-type: application/json' -d '{"role":"editor"}'
```

Roles live on the **membership**, so the same person can be an admin in one
organisation and a reader in another.

## Which status you get

| Code | When |
|------|------|
| `401` | anonymous caller hit a protected action |
| `403` | signed in, but wrong role — or no organisation selected |
| `404` | `private` action, or an owned row you don't own |

Details in [Permissions](../../docs/permissions.md).

**Next:** [07 · Functions](../07-functions) — writing custom code.
