# 14 · Email domains (auto-join on registration)

An organisation owns an email domain. Anyone who registers with an address at
that domain lands inside it — already a member, already able to read its data,
without an invite and without an admin doing anything.

```
14-email-domains/
├── main.toml
├── models/
│   ├── organization.toml       # the built-in + a `domain` field
│   ├── users.toml              # the built-in + [hooks] after_create
│   └── note.toml               # ordinary org-scoped data, to prove access
└── functions/
    ├── domain_join.rs          # the hook: match the domain, insert a membership
    └── user_after_create.toml  #   …its config: the role, and the public domains
```

Two ideas meet here. [Example 04](../04-multitenancy) showed that membership is
what grants access to an organisation's data; [example 08](../08-hooks) showed
that a hook can run custom code around a write. Membership is an ordinary row,
so a hook can create one — and registration is the moment to do it.

## Run it

```bash
createdb -h 127.0.0.1 -p 55432 -U postgres apiplant_domains
cargo run -p apiplant -- build examples/14-email-domains   # needs cargo on PATH
cargo run -p apiplant -- examples/14-email-domains
```

```
INFO apiplant_server:   fn user_after_create (private — no endpoint)
INFO apiplant_server:   hook user.after_create -> user_after_create
```

## `domain` on the organisation

`models/organization.toml` is the built-in `organization` copied out and given
one more field. A model file **replaces** the built-in of the same name rather
than merging into it, so the `global` scope and the membership-based permissions
are repeated verbatim; only this part is new:

```toml
[fields.domain]
type       = "string"
unique     = true
max_length = 253
```

`unique` is the load-bearing part: a domain belongs to at most one organisation,
so "which org does `ben@acme.com` belong to?" has a single answer. The column is
nullable, and Postgres allows any number of NULLs in a unique column — an
organisation that never sets a domain simply never auto-admits anyone.

## Registering is a create on `user`

`POST /api/auth/register` writes a row to the `user` table, so it is a `create`
on the `user` resource and the resource's create hooks fire there, exactly as
they do on `POST /api/user`. That's all `models/users.toml` has to say:

```toml
[hooks]
after_create = "user_after_create"
```

Registration differs from a plain create in two ways the hook can feel:

* the plaintext `password` has already been swapped for the hashed
  `password_hash` before `before_create` runs — a hook never sees the secret;
* nobody is authenticated yet, so the hook context's `principal_id` is null.
  The new account is in the row the hook receives (and in `record_id`).

A replacement returned from `after_create` replaces the `user` object in the
register response; the issued `token` is left alone.

## The hook

`functions/domain_join.rs` does three things: take the domain off the address,
find the organisation that owns it, insert the membership.

```rust
let org = ctx.query_one(
    "SELECT id::text AS id, name FROM apiplant_organization WHERE lower(domain) = $1",
    &[json!(domain)],
)?;
…
ctx.execute(
    "INSERT INTO apiplant_membership (user_id, organization_id, role) \
     VALUES ($1::uuid, $2::uuid, $3)",
    &[json!(user_id), json!(org_id), json!(settings.role)],
)?;
```

Two judgement calls are config, in `functions/user_after_create.toml`:

| Key | Why |
|-----|-----|
| `role = "member"` | Matching a domain proves an address, not authority — an auto-joined account is never an `admin`. |
| `public_domains = ["gmail.com", …]` | Nobody may claim a free-mail domain. Without this, one `gmail.com` organisation would adopt every Gmail user who ever signs up. |

Change either and restart — config is read at boot, no rebuild needed.

## Try it

```bash
API=http://127.0.0.1:8099/api

# Ana registers first, while acme.com belongs to nobody, then starts the org.
# Creating an organisation makes you its admin, so she gets in the normal way.
FT=$(curl -s -XPOST $API/auth/register -H 'content-type: application/json' \
  -d '{"email":"ana@acme.com","password":"pw"}' | jq -r .token)
curl -s -XPOST $API/organization -H "authorization: Bearer $FT" \
  -H 'content-type: application/json' \
  -d '{"name":"Acme","slug":"acme","domain":"acme.com"}'
curl -s -XPOST $API/note -H "authorization: Bearer $FT" \
  -H 'content-type: application/json' -d '{"body":"Q3 roadmap"}'

# Ben registers. Nobody invited him.
BT=$(curl -s -XPOST $API/auth/register -H 'content-type: application/json' \
  -d '{"email":"ben@acme.com","password":"pw"}' | jq -r .token)

curl -s $API/organization -H "authorization: Bearer $BT"
# → [{"name":"Acme","domain":"acme.com", …}]        he is already a member
curl -s $API/note -H "authorization: Bearer $BT"
# → [{"body":"Q3 roadmap", …}]                      and Acme's data is his data
```

The log says what happened:

```
INFO apiplant::function: ben@acme.com joined Acme as member via its acme.com domain
```

Ben belongs to exactly one organisation, so he never needs the
`X-Organization` header — his sole membership *is* the active org.

Nobody else gets in:

```bash
# an address at an unclaimed domain
ZT=$(curl -s -XPOST $API/auth/register -H 'content-type: application/json' \
  -d '{"email":"zoe@other.com","password":"pw"}' | jq -r .token)
curl -s $API/organization -H "authorization: Bearer $ZT"
# → []
curl -s $API/note -H "authorization: Bearer $ZT"
# → 403 {"error":"select an organisation with the X-Organization header"}
#   no memberships means no active organisation — Zoe needs an invite, or an
#   organisation of her own.

# and a free-mail address, even if someone claims the domain
curl -s -XPOST $API/organization -H "authorization: Bearer $FT" \
  -H 'content-type: application/json' -d '{"name":"Free","slug":"free","domain":"gmail.com"}'
CT=$(curl -s -XPOST $API/auth/register -H 'content-type: application/json' \
  -d '{"email":"cal@gmail.com","password":"pw"}' | jq -r .token)
curl -s $API/organization -H "authorization: Bearer $CT"
# → []      INFO apiplant::function: gmail.com is a public mail domain; not auto-joining
```

## What this example is not

Matching a domain is only as trustworthy as the address behind it, and nothing
here verifies that Ben can read mail at `ben@acme.com` — this app takes any
address at registration. In production the hook belongs *after* an email
confirmation, not after the insert, and an organisation's claim on a domain
wants proving too (a DNS TXT record is the usual way). The mechanism is the
same; only the moment it fires changes.

Two natural extensions, both a few lines in the same library:

* an `after_update` hook on `organization` that back-fills — when an org sets or
  changes its `domain`, adopt the accounts that already match it, which is what
  makes the feature work for users who registered first;
* a `before_create` hook on `organization` refusing a `domain` on the
  `public_domains` list outright, instead of quietly ignoring it later.

Details in [Lifecycle hooks](../../docs/hooks.md),
[Multitenancy](../../docs/multitenancy.md) and
[Authentication](../../docs/authentication.md).
