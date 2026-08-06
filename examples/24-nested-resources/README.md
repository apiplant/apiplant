# 24 · Nested resources

Every `reference` field gives you a nested collection for free —
`GET /api/{parent}/{id}/{child}`. Example 03 showed the shape. This one shows
what changes when the two ends live in different **scopes**, because that is
where the surprises are.

```
24-nested-resources/
└── models/
    ├── municipality.toml      # global — the shared reference table
    ├── ingestion_job.toml     # global,     municipality_id → municipality
    ├── collection_point.toml  # org-scoped, municipality_id → municipality
    ├── depot.toml             # org-scoped, municipality_id → municipality
    └── shipment.toml          # org-scoped, origin/destination → depot
```

Three pairings, one rule:

| Parent | Child | Endpoint |
|--------|-------|----------|
| global | global | `/municipality/{id}/ingestion_job` |
| global | org-scoped | `/municipality/{id}/collection_point` |
| org-scoped | org-scoped | `/depot/{id}/shipment` |

**The nested endpoint authorizes the child's policy, not the parent's.** The
parent is only a filter: the URL adds `WHERE <reference> = {id}` and nothing
else. So a `public` parent never widens a locked-down child, and reaching a
child through a parent is never a way around the child's own rules.

## Run it

```bash
createdb -h 127.0.0.1 -p 5432 -U postgres apiplant_nested
cargo run -p apiplant -- run --seed examples/24-nested-resources
```

```bash
A=localhost:8099/api
TOKEN=$(curl -s -XPOST $A/auth/login -H 'content-type: application/json' \
  -d '{"email":"admin@example.com","password":"password"}' | jq -r .token)
AUTH="authorization: Bearer $TOKEN"

ACME=$(  curl -s $A/organization -H "$AUTH" | jq -r '.[]|select(.slug=="acme").id')
GLOBEX=$(curl -s $A/organization -H "$AUTH" | jq -r '.[]|select(.slug=="globex").id')
BOLOGNA=$(curl -s $A/municipality | jq -r '.[]|select(.name=="Bologna").id')
```

`admin@example.com` administers both organisations, so one account can look at
the same URL from either side.

## 1. Global under global — and why it still wants a header

`municipality` is `public`; `ingestion_job` is `role:admin`. Neither has a
tenant column, and yet:

```bash
curl -s "$A/municipality/$BOLOGNA/ingestion_job" -H "$AUTH"
# → 403 select an organisation with the X-Organization header
```

Nothing is being isolated here — the question is unanswerable. `role:admin`
means "admin **of the organisation you are acting in**", and the request never
said which one. Name it and the collection is there:

```bash
curl -s "$A/municipality/$BOLOGNA/ingestion_job?expand=municipality" \
  -H "$AUTH" -H "X-Organization: $ACME" | jq -c '.[] | {source, status}'
# → {"source":"registry-2025.csv","status":"running"}
#   {"source":"registry-2024.csv","status":"done"}
```

Same answer from Globex — the rows are global, the header only decided *who is
asking*. As `member@example.com`, who is in Acme but not an admin:

```bash
# → 403 requires the `admin` role in this organisation
```

This is the one case worth remembering: **a global resource can need
`X-Organization` too.** Any client that decides whether to send the header by
looking at the scope alone will get a 403 here. The manifest publishes a
`requires_org` flag per action so it never has to guess, and the admin
dashboard uses it.

## 2. Org-scoped under global — one URL, two answers

`collection_point` is org-scoped and hangs off the shared table. Acme and
Globex both have points in Bologna:

```bash
curl -s "$A/municipality/$BOLOGNA/collection_point" -H "$AUTH" -H "X-Organization: $ACME"   | jq -c '.[].label'
# → "Porta Mazzini"
curl -s "$A/municipality/$BOLOGNA/collection_point" -H "$AUTH" -H "X-Organization: $GLOBEX" | jq -c '.[].label'
# → "Piazza Verdi"
```

Same URL, same parent row, different lists. A shared parent does not make its
children shared: the child's tenant filter is applied first and the parent id
narrows what is left.

## 3. Org-scoped under org-scoped — and `?via=`

`shipment` references `depot` twice, so the server refuses to guess:

```bash
DEPOT=$(curl -s $A/depot -H "$AUTH" -H "X-Organization: $ACME" \
  | jq -r '.[]|select(.name=="Acme Bologna").id')

curl -s "$A/depot/$DEPOT/shipment" -H "$AUTH" -H "X-Organization: $ACME"
# → 400 `shipment` references `depot` more than once; add ?via=<field>

curl -s "$A/depot/$DEPOT/shipment?via=origin_depot_id"      … | jq -c '.[].reference'  # → "ACME-0001"
curl -s "$A/depot/$DEPOT/shipment?via=destination_depot_id" … | jq -c '.[].reference'  # → "ACME-0002"
```

Two filters end up on that query and you wrote neither: the tenant, from your
active organisation, and the parent, from the URL. Which is why a parent id
from the *other* organisation is not an information leak — it is just an id
that matches nothing:

```bash
GDEPOT=$(curl -s $A/depot -H "$AUTH" -H "X-Organization: $GLOBEX" \
  | jq -r '.[]|select(.name=="Globex Bologna").id')

curl -s "$A/depot/$GDEPOT/shipment?via=origin_depot_id" -H "$AUTH" -H "X-Organization: $ACME"
# → []      not 403, not Globex's shipments
```

## What else nested collections take

Everything the flat list does, because it *is* the list with one more filter:
`?expand=`, `?field=value`, `?field~=` search, `?sort=`, `?limit=` and
`?offset=`, and the child's `before_list` / `after_list` hooks.

## Pointing at a shared table from inside a tenant

`depot.municipality_id` crosses from org-scoped to global, which is allowed and
common — a tenant row referring to a shared one. The direction that does *not*
work is the reverse: a global row cannot be made to point at one tenant's data
and stay meaningful to everybody else.

`on_delete = "restrict"` on that reference is what keeps the shared table
honest — a municipality somebody is using cannot quietly disappear:

```bash
curl -s -XDELETE "$A/municipality/$BOLOGNA" -H "$AUTH" -H "X-Organization: $ACME"
# → 400 other records still reference this one
```

Details in [Relationships](../../docs/relationships.md) and
[Multitenancy](../../docs/multitenancy.md).
