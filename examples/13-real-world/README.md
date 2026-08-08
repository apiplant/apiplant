# 13 · A real-world app

Everything so far, at the size of an actual product: **20 resources, 52 foreign
keys, no code**. It's the back office of a small distributor — catalogue,
stock, procurement, orders, fulfilment, payments and support — and it is all
`resources/*.toml`.

```
                       ┌── product_category ─┐ (self-referencing tree)
  global / shared      │   country           │  carrier
  ─────────────────────┼─────────────────────┼──────────────────────────
  per organisation     ▼                     ▼
                    product ── variant ──┬── inventory_level ── warehouse
                                         │                          ▲
                                         ├── purchase_order_line ── purchase_order ── supplier
                                         │
     customer ── address ────┐           │
        │  ▲   ▲             │           │
        │  │   └── billing ──┤           │
        │  └────── shipping ─┤           │
        │                    ▼           │
        │                  order ── order_line ──┐
        │                    │  │                │
        │                    │  └── shipment ── shipment_line
        │                    │        │
        │                    └── payment ── refund
        │                         │
        └── support_ticket ── ticket_message
```

## The resources

| Resource | Scope | Shows |
|----------|-------|-------|
| `country` | global | shared reference data, `unique` code, `max_length` |
| `carrier` | global | shared reference data |
| `product_category` | global | **self-reference** (`parent_id` → itself): a category tree |
| `product` | org | scoped row referencing global reference data, `json` field |
| `variant` | org | `unique` SKU; the hub four other resources point at |
| `warehouse` | org | scoped row sited in a global `country` |
| `inventory_level` | org | **join row**: variant × warehouse |
| `supplier` | org | the procurement branch |
| `purchase_order` | org | supplier → warehouse |
| `purchase_order_line` | org | join row: purchase order × variant |
| `customer` | org | reference to the built-in `user` that is *not* the owner |
| `address` | org | one customer, many addresses |
| `order` | org | `table` override, **two references to `address`**, `owner` policy |
| `order_line` | org | join row: order × variant, price frozen at sale time |
| `shipment` | org | order + warehouse + carrier in one row |
| `shipment_line` | org | join row three levels deep; partial shipments |
| `payment` | org | **append-only ledger** (`update`/`delete` = `private`), `hidden` field |
| `refund` | org | order → payment → refund |
| `support_ticket` | org | **two references to `user`**, `owner_field` override |
| `ticket_message` | org | `owner_field = "author_id"` + `update = "owner"` |

Three roles appear in the policies — `admin`, `manager`, `agent` — and they live
on the **membership**, so the same person can be an admin here and nothing at
all next door. Roles are a set: a membership's `role` is the primary one and
each `membership_role` row adds another, so Max can be a `manager` *and* an
`agent` without giving either up. An `admin` holds all three without being
granted any, which is why Ana below never needs a second role.

## Run it

```bash
createdb -h 127.0.0.1 -p 5432 -U postgres apiplant_real_world
cargo run -p apiplant -- run examples/13-real-world
```

First boot creates 25 tables — these 20 plus the five built-ins — and 56 foreign
keys, 17 of which are the `organization_id` you never declared.
<http://127.0.0.1:8099/api/docs> has every endpoint, including the nested ones.

## A cast of three

```bash
API=localhost:8099/api
J='content-type: application/json'
reg() { curl -s -XPOST $API/auth/register -H "$J" -d "{\"email\":\"$1\",\"password\":\"pw\"}"; }
post() { curl -s -XPOST "$API/$1" -H "$J" -H "authorization: Bearer $2" -d "$3"; }
get()  { curl -s "$API/$1" -H "authorization: Bearer $2"; }

ANA=$(reg ana@acme.test);  ANAT=$(jq -r .token <<<"$ANA")
MAX=$(reg max@acme.test);  MAXT=$(jq -r .token <<<"$MAX");  MAXID=$(jq -r .user.id <<<"$MAX")
SAM=$(reg sam@acme.test);  SAMT=$(jq -r .token <<<"$SAM");  SAMID=$(jq -r .user.id <<<"$SAM")

# Ana creates the org, so she is its admin. She hires the other two.
post organization "$ANAT" '{"name":"Acme Supply","slug":"acme"}'
post membership   "$ANAT" "{\"user_id\":\"$MAXID\",\"role\":\"manager\"}"   # ops
post membership   "$ANAT" "{\"user_id\":\"$SAMID\",\"role\":\"agent\"}"     # support
```

## Reference data, then a catalogue

`country`, `carrier` and `product_category` are `global`: one list, shared by
every tenant.

```bash
DE=$(post country  "$ANAT" '{"code":"DE","name":"Germany","eu":true}' | jq -r .id)
FR=$(post country  "$ANAT" '{"code":"FR","name":"France","eu":true}'  | jq -r .id)
DHL=$(post carrier "$ANAT" '{"code":"dhl","name":"DHL"}'              | jq -r .id)

ROOT=$(post product_category "$ANAT" '{"name":"Tools","slug":"tools"}' | jq -r .id)
KID=$(post product_category  "$ANAT" "{\"name\":\"Drills\",\"slug\":\"drills\",\"parent_id\":\"$ROOT\"}" | jq -r .id)
```

The category tree is a resource referencing **itself**, which means the usual
two directions come for free:

```bash
get "product_category/$KID?expand=parent"        "$ANAT"   # → "parent": { "name": "Tools", … }
get "product_category/$ROOT/product_category"    "$ANAT"   # → the children of Tools
```

Catalogue writes are `role:manager`, so Sam can't make products:

```bash
post product "$SAMT" "{\"name\":\"x\",\"slug\":\"x\",\"category_id\":\"$KID\"}"
# → 403 {"error":"requires the `manager` role in this organisation"}

PROD=$(post product "$MAXT" "{\"name\":\"Hammer Drill\",\"slug\":\"hd\",\"category_id\":\"$KID\",\"status\":\"active\",\"attributes\":{\"voltage\":18}}" | jq -r .id)
VAR=$(post variant  "$MAXT" "{\"product_id\":\"$PROD\",\"sku\":\"HD-18V\",\"name\":\"18V\",\"price_cents\":24900}" | jq -r .id)
WH=$(post warehouse "$MAXT" "{\"code\":\"BER\",\"name\":\"Berlin\",\"country_id\":\"$DE\"}" | jq -r .id)
post inventory_level "$MAXT" "{\"variant_id\":\"$VAR\",\"warehouse_id\":\"$WH\",\"on_hand\":40}"
```

`inventory_level` is a plain join row, and both of its parents get a nested
collection:

```bash
get "variant/$VAR/inventory_level"   "$ANAT"   # this SKU, everywhere
get "warehouse/$WH/inventory_level"  "$ANAT"   # everything in this warehouse
get "inventory_level?warehouse_id=$WH&expand=variant,warehouse" "$ANAT"
```

## An order, and the two things that make it interesting

```bash
CUST=$(post customer "$ANAT" "{\"name\":\"Baumarkt Nord\",\"email\":\"buy@nord.test\",\"country_id\":\"$DE\",\"account_manager_id\":\"$MAXID\",\"tier\":\"plus\"}" | jq -r .id)
BILL=$(post address  "$ANAT" "{\"customer_id\":\"$CUST\",\"label\":\"HQ\",\"line1\":\"Hauptstr. 1\",\"city\":\"Hamburg\",\"country_id\":\"$DE\"}" | jq -r .id)
SHIP=$(post address  "$ANAT" "{\"customer_id\":\"$CUST\",\"label\":\"Depot\",\"line1\":\"Rue 2\",\"city\":\"Lille\",\"country_id\":\"$FR\"}" | jq -r .id)

ORD=$(post order "$ANAT" "{\"number\":\"SO-1001\",\"customer_id\":\"$CUST\",\"billing_address_id\":\"$BILL\",\"shipping_address_id\":\"$SHIP\",\"total_cents\":49800,\"status\":\"confirmed\"}" | jq -r .id)
OL=$(post order_line "$ANAT" "{\"order_id\":\"$ORD\",\"variant_id\":\"$VAR\",\"quantity\":2,\"unit_price_cents\":24900}" | jq -r .id)
```

**1. Two references to the same parent.** `order` reaches `address` twice, so
the reverse collection has to be told which link to walk:

```bash
get "address/$SHIP/order" "$ANAT"
# → 400 `order` references `address` more than once; add ?via=<field>
get "address/$SHIP/order?via=shipping_address_id" "$ANAT"   # → [ SO-1001 ]
```

`support_ticket` has the same shape against `user` (`reporter_id`,
`assignee_id`), which is how an agent gets a queue:

```bash
get "user/$SAMID/support_ticket?via=assignee_id" "$SAMT"
```

**2. Ownership.** `order.owner_id` is the resource's owner column, so the server
stamps it with the caller on create and `update = "owner"` means "the rep who
took the order". Max is a manager, but the order isn't his:

```bash
get "order/$ORD" "$ANAT" | jq '.owner_id'        # → Ana

curl -XPATCH "$API/order/$ORD" -H "$J" -H "authorization: Bearer $MAXT" -d '{"notes":"hi"}'
# → 404 — ownership filters the row out; it isn't even reported as forbidden
curl -XPATCH "$API/order/$ORD" -H "$J" -H "authorization: Bearer $ANAT" -d '{"notes":"rush"}'
# → 200
```

`customer.account_manager_id` also references `user`, but it is *not* the owner
column, so it stays a normal assignable field. One resource, two very different
references to the same table.

Expansion inlines any of them, one level, batched:

```bash
get "order/$ORD?expand=customer,shipping_address,owner" "$ANAT"
# → { …, "customer": {…}, "shipping_address": { "city": "Lille" }, "owner": { "email": "ana@acme.test" } }
```

## Fulfilment and money

```bash
SH=$(post shipment "$MAXT" "{\"order_id\":\"$ORD\",\"warehouse_id\":\"$WH\",\"carrier_id\":\"$DHL\",\"tracking_code\":\"JD014\",\"status\":\"in_transit\"}" | jq -r .id)
post shipment_line "$MAXT" "{\"shipment_id\":\"$SH\",\"order_line_id\":\"$OL\",\"quantity\":2}"
```

`shipment_line` joins a parcel to an order line, which is what makes partial
shipments expressible without a single custom endpoint.

`payment` is a **ledger**: `update` and `delete` are `private`, so those routes
answer `404` and don't appear in the docs. `gateway_reference` is `unique`,
which turns a replayed capture into a `409` instead of a double charge, and
`gateway_payload` is `hidden` — stored, never returned.

```bash
PAY=$(post payment "$MAXT" "{\"order_id\":\"$ORD\",\"amount_cents\":49800,\"method\":\"card\",\"gateway_reference\":\"ch_1\",\"gateway_payload\":{\"raw\":\"secret\"}}" | jq -r .id)

get "payment/$PAY" "$MAXT" | jq 'has("gateway_payload")'      # → false
curl -XPATCH "$API/payment/$PAY" … -d '{"status":"failed"}'   # → 404, payments are immutable
post payment "$MAXT" '{…,"gateway_reference":"ch_1"}'         # → 409, idempotent
post refund  "$MAXT" "{\"payment_id\":\"$PAY\",\"amount_cents\":2490,\"reason\":\"damaged box\"}"
```

Corrections happen by *writing a row*, never by editing one — the policy makes
that the only option available.

## Support

```bash
TIC=$(post support_ticket "$ANAT" "{\"subject\":\"Where is my drill?\",\"customer_id\":\"$CUST\",\"order_id\":\"$ORD\",\"assignee_id\":\"$SAMID\",\"priority\":\"urgent\"}" | jq -r .id)
get "support_ticket/$TIC" "$ANAT" | jq '{reporter_id, assignee_id}'   # reporter = Ana, stamped
```

`owner_field = "reporter_id"` means the reporter is *the caller*, never a value
from the body — you cannot open a ticket in someone else's name. Triage is
`role:agent`:

```bash
curl -XPATCH "$API/support_ticket/$TIC" -H "authorization: Bearer $SAMT" -d '{"status":"pending"}'   # → 200
curl -XPATCH "$API/support_ticket/$TIC" -H "authorization: Bearer $MAXT" -d '{"status":"resolved"}'  # → 403
```

`ticket_message` uses the same trick for authorship *and* for editing:
`owner_field = "author_id"` with `update = "owner"` gives "edit your own
messages only" in two lines of TOML.

## The delete graph

`on_delete` is where a schema of this size earns its keep. Every reference here
picks one deliberately:

| Intent | Action | Example |
|--------|--------|---------|
| composition — the child is part of the parent | `cascade` | `address` → `customer`, `order_line` → `order`, `variant` → `product` |
| history — never delete what was transacted | `restrict` | `order_line` → `variant`, `payment` → `order`, `warehouse` → `country` |
| soft link — the fact survives the actor | `set_null` | `order.owner_id`, `support_ticket.order_id`, `refund.approved_by_id` |

Deleting an *unpaid* order takes its lines, its shipments and their shipment
lines with it, and leaves the customer's ticket standing with `order_id: null`:

```bash
curl -XDELETE "$API/order/$ORD" -H "authorization: Bearer $ANAT"   # → 204
get "order_line?order_id=$ORD" "$ANAT"                             # → []
get "support_ticket/$TIC" "$ANAT" | jq '.order_id'                 # → null
```

Once it has a payment, the same call is refused (`400`) — `payment.order_id` is
`restrict`, and Postgres, not application code, is the thing saying no. Same for
a variant that has ever been sold, and for a category that still has children:

```bash
curl -XDELETE "$API/variant/$VAR"           -H "authorization: Bearer $ANAT"   # → 400
curl -XDELETE "$API/product_category/$ROOT" -H "authorization: Bearer $ANAT"   # → 400
```

Deleting the *customer*, by contrast, is a real cascade: addresses, tickets and
ticket messages all go.

## Isolation, still free

Nothing above mentions `organization_id`. Register a second user, give them
their own organisation, and the whole catalogue is empty for them — while the
global reference data is right where they left it:

```bash
BO=$(reg bo@other.test); BOT=$(jq -r .token <<<"$BO")
post organization "$BOT" '{"name":"Other","slug":"other"}'
get product "$BOT"   # → []          17 org-scoped resources, all filtered
get country "$BOT"   # → [DE, FR]    3 global ones, shared
```

## A back office for it

Twenty resources is exactly the point at which a raw table of rows stops being
usable by anyone who did not write the resources. So each one carries an `[admin]`
section saying what to call it and where it belongs, and the fields whose
comments already listed their values (`status`, `priority`, `tier`) declare
those values properly so they render as dropdowns:

```toml
# resources/product.toml
[admin]
group   = "Catalogue"
order   = 1
columns = ["name", "status", "category_id"]

[fields.status.admin]
widget  = "select"
options = ["draft", "active", "discontinued"]
help    = "Only active products can be sold."
```

The line resources — `order_line`, `shipment_line`, `purchase_order_line`,
`ticket_message` — set `visible = false`. They are not hidden from the API; they
simply have no business being a top-level menu item, because you always reach
them *through* the order or ticket they belong to, where the record screen
already lists them. `carrier` and `country` set `roles = ["admin"]`: shared
reference data that a sales rep should not be editing.

```bash
apiplant build examples/13-real-world               # the actions, below
apiplant admin examples/13-real-world --api http://127.0.0.1:8099
apiplant run   examples/13-real-world               # serves it at /admin/
```

`functions/back_office.rs` adds the three things no CRUD form covers, one per
access level — a `member` report, a `role:manager` stock correction that asks
for confirmation before it writes, and a `role:admin` housekeeping job:

```rust
{
    name: "restock_variant",
    description: "Corrects the recorded stock count for one warehouse line.",
    method: Post,
    permission: "role:manager",
    admin: {
        label: "Correct stock count",
        group: "Operations",
        confirm: "This overwrites the recorded stock for that line. Continue?",
        run_label: "Update stock",
    },
    handler: restock_variant,
}
```

Sign in as Ana (admin) and Bo (manager) in turn and the sidebar differs: the
reference data and the housekeeping job are Ana's, the stock correction is Bo's.
Nothing enforces that in the dashboard — the API refuses either way — but there
is no reason to show someone a door that answers `403`.

See [Admin dashboard](../../docs/admin.md).

## What's deliberately not here

Totals aren't recalculated when a line changes, stock isn't decremented on
shipment, and `order.status` doesn't advance on its own — those are *behaviour*,
and behaviour is what [functions](../07-functions) and [hooks](../08-hooks) are
for. Attach `after_create` on `order_line` to sum the order, `after_create` on
`shipment_line` to move `inventory_level.reserved`, and the schema above stops
being a filing cabinet and starts being an application.

Details in [Relationships](../../docs/relationships.md),
[Permissions](../../docs/permissions.md) and
[Multitenancy](../../docs/multitenancy.md).

**Back to:** [the example index](..)
