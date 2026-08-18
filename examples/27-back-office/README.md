# 27 · Back office

A whole deployment, seeded: one support organisation, three customers, nine
accounts, and four resources whose policies disagree with each other on purpose.
[06 · Permissions](../06-permissions) introduces the levels one at a time; this
one is the playground you sign into as eight different people to watch the same
endpoint answer eight different ways.

```
27-back-office/
├── main.toml               # global_admin_role, allow_impersonation
├── resources/
│   ├── ticket.toml         # allow / own / deny in one table
│   ├── invoice.toml        # read-only to the tenant, private to everyone
│   ├── service_status.toml # public reads, class-gated writes
│   └── audit_note.toml     # support-only, and append-only
└── seed/                   # the organisations, the people, and their roles
```

## Who is who

| Account | Organisation | Role | So they are |
|---------|--------------|------|-------------|
| `root@example.com` | Vantage Support (`support`) | `admin` | **the back office** — `global_admin_role` names exactly this |
| `agent@example.com` | Vantage Support (`support`) | `agent` | support staff, and *not* the back office |
| `nadia@northwind.example` | Northwind (`customer`) | `admin` | a tenant's administrator — and a plain member of Lumen |
| `noel@northwind.example` | Northwind | `manager` | sees the whole queue |
| `nina@northwind.example` | Northwind | `member` | sees her own tickets |
| `uma@umbra.example` | Umbra (`customer`) | `admin` | |
| `ugo@umbra.example` | Umbra | `member` + `suspended` | reads everything, writes nothing |
| `lea@lumen.example` | Lumen (`trial`) | `admin` | |
| `liam@lumen.example` | Lumen | `member` + `manager` | a second role, without being an admin |

Everyone's password is `password`. Nothing about an *account* grants anything:
every line of the table above is a membership row in `seed/membership.toml`.

## Run it

```bash
createdb -h 127.0.0.1 -p 5432 -U postgres apiplant_back_office
cargo run -p apiplant -- run --seed examples/27-back-office
```

Then, for the tour below:

```bash
A=localhost:8106/api
login() { curl -s -XPOST $A/auth/login -H 'content-type: application/json' \
  -d "{\"email\":\"$1\",\"password\":\"password\"}" | jq -r .token; }

ROOT=$(login root@example.com);            AGENT=$(login agent@example.com)
NADIA=$(login nadia@northwind.example);    NOEL=$(login noel@northwind.example)
NINA=$(login nina@northwind.example);      UGO=$(login ugo@umbra.example)

NW=$(curl -s $A/organization -H "authorization: Bearer $NADIA" | jq -r '.[]|select(.slug=="northwind").id')
VG=$(curl -s $A/organization -H "authorization: Bearer $ROOT"  | jq -r '.[]|select(.slug=="vantage").id')
UM=$(curl -s $A/organization -H "authorization: Bearer $ROOT"  | jq -r '.[]|select(.slug=="umbra").id')
```

That first `curl` already says something: `$ROOT` lists **four** organisations,
`$NADIA` lists the two she belongs to, and `$AGENT` lists one.

## Two answers to one question

`ticket` uses the table form of `[permissions]`, because "managers see the queue,
everybody else sees their own" is two answers and no single level says both:

```toml
[permissions.list]
allow = ["role:manager"]
own   = ["member"]
```

```bash
curl -s $A/ticket -H "authorization: Bearer $NOEL" -H "x-organization: $NW" | jq -r '.[].subject'
# → all three Northwind tickets

curl -s $A/ticket -H "authorization: Bearer $NINA" -H "x-organization: $NW" | jq -r '.[].subject'
# → "Export runs out of memory" — hers, and nothing else
```

Nina is not refused; her query is *filtered*. Someone else's ticket reads `404`
rather than `403`, so ids cannot be probed by the error they produce.

`deny` is the other half, and it outranks every `allow`:

```bash
curl -s -XPOST $A/ticket -H "authorization: Bearer $UGO" -H "x-organization: $UM" \
  -H 'content-type: application/json' -d '{"subject":"anything"}' -i    # → 403

curl -s $A/ticket -H "authorization: Bearer $UGO" -H "x-organization: $UM" | jq -r '.[].subject'
# → still reads his own: `deny` is on create and update, not on list
```

A `deny` matches only a role somebody **actually holds**, never the blanket one
an `admin` gets — otherwise granting `suspended` would lock out the admins who
granted it.

## The class gate

`service_status` is global and writable by `member@org_class=support`: any member
will do, provided the organisation they have **selected** is of that class.

```bash
SID=$(curl -s $A/service_status | jq -r '.[0].id')       # → 200 without a token

curl -s -XPATCH $A/service_status/$SID -H "authorization: Bearer $AGENT" \
  -H "x-organization: $VG" -H 'content-type: application/json' \
  -d '{"state":"operational"}' -i                                       # → 200

curl -s -XPATCH $A/service_status/$SID -H "authorization: Bearer $NADIA" \
  -H "x-organization: $NW" -H 'content-type: application/json' \
  -d '{"state":"down"}' -i                                              # → 403
```

Ash is an `agent`, not the deployment's administrator — publishing a status page
should not require being one. Nadia is an `admin`, but of a `customer`.

The class itself is server-owned, which is what stops a tenant from classing its
way in:

```bash
curl -s -XPATCH $A/organization/$NW -H "authorization: Bearer $NADIA" \
  -H "x-organization: $NW" -H 'content-type: application/json' \
  -d '{"org_class":"support"}' | jq -r .org_class                       # → "customer"
```

No error — the key is dropped from the body, exactly like `organization_id`.
Only `global_admin_role` writes it, so the same request as `$ROOT` returns `200`
with the new class.

## What the back office is, and what it is not

Being named by `global_admin_role` lifts the role check and the organisation
check for that caller. They stand outside the tenants:

```bash
curl -s $A/invoice -H "authorization: Bearer $ROOT" -H "x-organization: $NW" | jq -r '.[].reference'
# → NW-2024-001, NW-2024-002 — an organisation Rae is not a member of

curl -s $A/invoice -H "authorization: Bearer $AGENT" -H "x-organization: $NW" -i    # → 403
```

Ash is in the *support organisation* and Rae is the *back office*; only the
second is a standing. Dropping the `X-Organization` header takes nothing away
from Rae — this is the one check answered by who you are rather than where you
stand. What the header does instead is narrow: send one and a list is that
tenant's, send none and it is the whole deployment's.

What it does not lift is `private`:

```bash
curl -s -XPOST $A/invoice -H "authorization: Bearer $ROOT" \
  -H 'content-type: application/json' -d '{"reference":"X"}' -i         # → 404
```

`private` says the endpoint is not on the API, not that you lack a permission
for it — so it answers `404`, and it answers it to everybody. `audit_note` uses
the same wall for `update` and `delete`: support may add a note about a customer
and nobody, its own administrators included, may quietly amend one.

## Acting as somebody else

Two doors, deliberately different sizes. The narrow one is an organisation's own
admin, and the session it hands back is **pinned**:

```bash
NINA_ID=$(curl -s $A/user -H "authorization: Bearer $NADIA" -H "x-organization: $NW" \
  | jq -r '.[]|select(.email=="nina@northwind.example").id')

BT=$(curl -s -XPOST $A/auth/impersonate -H "authorization: Bearer $NADIA" \
  -H "x-organization: $NW" -H 'content-type: application/json' \
  -d "{\"user_id\":\"$NINA_ID\"}" | jq -r .token)

curl -s $A/auth/me -H "authorization: Bearer $BT" | jq -c .
# → user_id nina, impersonator nadia, organization_id northwind — the pin

curl -s $A/organization -H "authorization: Bearer $BT" | jq -r '.[].slug'
# → northwind, and only northwind
```

That last line is the property that makes this safe to leave on: Nina's other
memberships are not loaded at all, so an admin who borrows a member cannot ride
that account into a tenant they have nothing to do with. Nadia may not borrow a
stranger either — Liam at Lumen answers `403` — and a borrowed session cannot
borrow again (`409`). Going back needs no stored credential:

```bash
curl -s -XPOST $A/auth/impersonate/stop -H "authorization: Bearer $BT" | jq -c '{user_id,impersonator}'
```

The wide door is the back office, and it is the same setting as everything
above — there is no second list of who may impersonate:

```bash
LIAM_ID=$(curl -s $A/user -H "authorization: Bearer $ROOT" \
  | jq -r '.[]|select(.email=="liam@lumen.example").id')

curl -s -XPOST $A/auth/impersonate -H "authorization: Bearer $AGENT" \
  -H 'content-type: application/json' -d "{\"user_id\":\"$LIAM_ID\"}" -i   # → 403

BT=$(curl -s -XPOST $A/auth/impersonate -H "authorization: Bearer $ROOT" \
  -H 'content-type: application/json' -d "{\"user_id\":\"$LIAM_ID\"}" | jq -r .token)

curl -s $A/auth/me -H "authorization: Bearer $BT" | jq -c .
# → organization_id: null — unpinned, so Liam's memberships are all reachable
curl -s $A/audit_note -H "authorization: Bearer $BT" -i                    # → 403
```

Rae may borrow anybody, in any organisation, and the session is not pinned:
moving around the account's organisations is what support access is for. But
`global_admin` comes back `false` while she is wearing it, and the audit notes
she can read as herself are refused as Liam. A borrowed session is never a back
office — otherwise somebody could wear another name and keep their own powers,
which is the one arrangement no audit trail untangles.

Switch it off with `[auth] allow_impersonation = false` and the narrow door
closes; the back office keeps its own. Close both and neither endpoint is
mounted at all.

## In the dashboard

`http://localhost:8106/admin/` — sign in as any of the nine. As Rae the sidebar
grows a **Back office** group: *Organizations* lists all four tenants (class
editable in the row, and a **Team** button that opens one she is no member of),
and *Users* lists all nine accounts across the deployment. Neither is there for
anybody else.

**Act as** appears on the team screen beside each member an admin may borrow —
including, for Rae, the team of an organisation she is no member of — and on
every row of the Users list and the `user` record. As Nadia it is the people she
shares an organisation with, and only while Northwind is the organisation she
has selected: roles are per-organisation, so the same account is an admin on one
team screen and a plain member on another.

A strip across the top of every screen says whose account is in use and holds the
way out. Signing in as Rae also puts the organisation switcher across all four
tenants and makes `org_class` editable; as Nadia, neither.

## Which status you get

| Code | When |
|------|------|
| `401` | anonymous caller hit a protected action |
| `403` | signed in, wrong role or wrong class — or no organisation selected |
| `404` | a `private` action, or a row an `own`/`owner` policy filtered away |

Details in [Permissions](../../docs/permissions.md) and
[Authentication](../../docs/authentication.md#acting-as-somebody-else).
