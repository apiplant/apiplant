# 18 · Payments (Stripe, as ordinary resources)

An app names a payment provider, and billing arrives as five resources and four
endpoints — with the permissions, roles and org-scoping everything else already
has. The catalogue is CRUD you can edit from the dashboard; the paywalls are
lifecycle hooks; entitlement is a query.

This example sells the three things a real app sells, because they are three
genuinely different problems:

| | What it is | What decides access | Needs an address? |
|---|---|---|---|
| **Pro** | a subscription, €29/month | is it active *right now* | no |
| **Field Guide** | a download, €12 once | was it ever bought | no |
| **Enamel Mug** | a physical thing, €18 once | nothing — it gets posted | **yes** |

```
18-payments/
├── main.toml                     # [payments] provider, keys, tax, shipping
├── models/
│   ├── document.toml             # needs a subscription
│   └── download.toml             # needs a purchase
└── functions/
    ├── plan.rs                   # the subscription paywall
    ├── require_plan.toml         # …which statuses count as paying
    ├── require_purchase.rs       # the bought-outright paywall
    └── require_purchase.toml     # …which payment statuses count as bought
```

Nothing in `models/` or `functions/` mentions Stripe. That is the shape of the
integration: `[payments]` brings the machinery, and the app talks about
documents, downloads and plans.

## Run it

You need a Stripe **test** account — no card, no verification, two minutes.

```bash
createdb -h 127.0.0.1 -p 5432 -U postgres apiplant_payments

export STRIPE_SECRET_KEY=sk_test_…          # Developers → API keys
export STRIPE_PUBLISHABLE_KEY=pk_test_…

# In another terminal: forward webhooks, and print the signing secret.
stripe listen --forward-to localhost:8101/api/billing/webhook
export STRIPE_WEBHOOK_SECRET=whsec_…        # what that just printed

cargo run -p apiplant -- build examples/18-payments
cargo run -p apiplant -- run --seed examples/18-payments
```

```
INFO apiplant_server:   payments -> stripe (eur, automatic tax on)
INFO apiplant_server:   hook document.before_create -> require_plan
INFO apiplant_server:   hook download.before_create -> require_purchase
INFO apiplant_server:   admin -> /admin/
```

Without `STRIPE_WEBHOOK_SECRET` the app still boots and still takes money — and
warns, loudly, that nothing will ever be recorded. That is the one billing
misconfiguration that looks like silence rather than an error, so it is said out
loud in three places: the boot log, the dashboard's billing screen and
`GET /api/billing/config`.

The seed makes `admin@example.com` (password `password`) an admin of Acme.

```bash
API=http://localhost:8101/api

TOKEN=$(curl -s -X POST $API/auth/login -H 'Content-Type: application/json' \
  -d '{"email":"admin@example.com","password":"password"}' | jq -r .token)
ORG=$(curl -s $API/organization -H "Authorization: Bearer $TOKEN" | jq -r '.[0].id')

auth=(-H "Authorization: Bearer $TOKEN" -H "X-Organization: $ORG" \
      -H "Content-Type: application/json")
```

Buying takes `role:admin`, because a member who can start a subscription can
commit their employer to a recurring charge.

## 1. Build a catalogue

Three products, and the only column that separates them is `shippable`.

```bash
PRO=$(curl -s -X POST $API/billing_product "${auth[@]}" \
  -d '{"name":"Pro","description":"For teams","features":{"seats":25}}' | jq -r .id)

GUIDE=$(curl -s -X POST $API/billing_product "${auth[@]}" \
  -d '{"name":"Field Guide","description":"A PDF you keep","tax_code":"txcd_10302000"}' | jq -r .id)

MUG=$(curl -s -X POST $API/billing_product "${auth[@]}" \
  -d '{"name":"Enamel Mug","description":"Holds coffee","shippable":true}' | jq -r .id)
```

Every row comes back with a `stripe_product_id` already filled in: a built-in
hook created the object in Stripe *before* the row was written, so a Stripe that
refuses fails the request and the catalogue never contains something nobody can
buy.

Look at what reached Stripe:

```
Pro          shippable=false  tax_code=txcd_10000000   ← the digital default
Field Guide  shippable=false  tax_code=txcd_10302000   ← named on the row
Enamel Mug   shippable=true   tax_code=txcd_99999999   ← the physical default
```

**`shippable` is one column with two consequences.** It decides whether the
checkout asks for a shipping address, and it decides which tax category the
product is filed under. Both follow from the same fact, so it is one field and
not three.

**The tax code is the input to automatic tax that nobody remembers to set.**
In most of the EU a downloaded file and a posted mug are taxed by different
rules, so `[payments]` carries *two* defaults rather than one — a single default
is a confident wrong answer half the time. `Field Guide` overrides it, because
an e-book is not taxed like software.

## 2. Put prices on them

```bash
PRICE_SUB=$(curl -s -X POST $API/billing_price "${auth[@]}" -d '{
  "product_id":"'$PRO'", "nickname":"Monthly",
  "unit_amount":2900, "currency":"eur", "interval":"month", "trial_days":14
}' | jq -r .id)

PRICE_GUIDE=$(curl -s -X POST $API/billing_price "${auth[@]}" -d '{
  "product_id":"'$GUIDE'", "nickname":"Field Guide (download)",
  "unit_amount":1200, "currency":"eur", "interval":""
}' | jq -r .id)

PRICE_MUG=$(curl -s -X POST $API/billing_price "${auth[@]}" -d '{
  "product_id":"'$MUG'", "nickname":"Enamel Mug",
  "unit_amount":1800, "currency":"eur", "interval":""
}' | jq -r .id)
```

**`unit_amount` is in the smallest unit.** 2900 is €29.00. This is what Stripe,
every card network and every ledger already use, and it is the only exact
representation — a float here is a rounding error waiting for a big enough
invoice.

**`interval` decides everything else.** Empty is a one-off purchase; `month`
recurs. Nothing has to restate it at the checkout, because it is a fact about
the price.

## 3. Try to use the app without paying

```bash
curl -s -X POST $API/document "${auth[@]}" -d '{"title":"Q3 plan"}'
curl -s -X POST $API/download "${auth[@]}" -d '{"product":"Field Guide"}'
```

```json
{ "error": "this organization is not on a plan; see /api/billing_price" }
{ "error": "nobody here has bought \"Field Guide\"; see /api/billing_price" }
```

Both **402 Payment Required** — not `403`. A client can tell those apart: one
shows a pricing page, the other an apology.

Two hooks, because they answer genuinely different questions. `require_plan`
asks *are they subscribed right now*, which has an expiry date on it.
`require_purchase` asks *did they ever pay for this*, which never stops being
true. Neither talks to Stripe: both are queries against tables the webhook keeps
current, so "may they do this" costs a query and not a round trip over the
internet on every request.

## 4. Pay

```bash
curl -s -X POST $API/billing/checkout "${auth[@]}" -d '{"price_id":"'$PRICE_SUB'"}'   | jq -r .url
curl -s -X POST $API/billing/checkout "${auth[@]}" -d '{"price_id":"'$PRICE_GUIDE'"}' | jq -r .url
curl -s -X POST $API/billing/checkout "${auth[@]}" -d '{"price_id":"'$PRICE_MUG'"}'   | jq -r .url
```

Open any of them and pay with `4242 4242 4242 4242`, any future expiry, any CVC.

The three sessions differ in exactly the ways they should, and nothing asked for
any of it:

| | mode | asks for a shipping address |
|---|---|---|
| Pro | `subscription` | no |
| Field Guide | `payment` | no |
| Enamel Mug | `payment` | **yes — IT, DE, FR, ES, GB, US** |

The countries come from `[payments] shipping_countries`, not from the request. A
caller that could turn shipping off would be a caller that could sell a mug with
nowhere to send it. And an app that lists no countries but has a `shippable`
product gets its checkout refused, rather than taking the money first.

Everything on that page — the card form, 3-D Secure, the wallet buttons, the VAT
number box, the address form, the receipt — is Stripe's, on Stripe's domain.
This app has no PCI scope worth the name, and the entire integration is
"redirect them to a URL we were handed".

In the `stripe listen` terminal, watch the events arrive:

```
checkout.session.completed        [200]
customer.subscription.created     [200]
invoice.paid                      [200]
payment_intent.succeeded          [200]
```

And now both paywalls open — each for its own thing:

```bash
curl -s -X POST $API/document "${auth[@]}" -d '{"title":"Q3 plan"}' | jq -r .title
curl -s -X POST $API/download "${auth[@]}" -d '{"product":"Field Guide"}' | jq -r .product
curl -s -X POST $API/download "${auth[@]}" -d '{"product":"Enamel Mug"}'   # still 402
```

Buying the guide does not entitle you to the mug. `require_purchase` matches the
*product*, deliberately: raising a price mints a new Stripe price and archives
the old one, so somebody who bought at last year's price holds a price id that
is no longer on the shelf, and matching the product is what stops a price rise
from repossessing what people already own.

## 5. Let them manage it themselves

```bash
curl -s -X POST $API/billing/portal "${auth[@]}" -d '{}' | jq -r .url
```

Card, invoices, VAT number, cancellation — all of it on Stripe's screens, none
of it implemented here, and every change coming back as a webhook that updates
the rows. There is deliberately no "cancel" endpoint in this app: a function
re-exporting one would be a worse version of a page that already exists.

The dashboard's **Billing** screen is the same three calls with buttons on them.

## What is where

| Question | Where it is answered |
|----------|----------------------|
| What do we sell? | `billing_product` / `billing_price` — your tables, mirrored to Stripe |
| Is it posted or downloaded? | `billing_product.shippable` |
| How is it taxed? | `billing_product.tax_code`, defaulted per kind by `[payments]` |
| Who is the customer? | `billing_customer` — one per organisation, never two |
| What has been paid for? | `billing_subscription` / `billing_payment` — written *only* by the webhook |
| May they do this? | a query, in `functions/plan.rs` or `functions/require_purchase.rs` |
| How do they pay? | `POST /billing/checkout` → a Stripe URL |
| How do they change it? | `POST /billing/portal` → a Stripe URL |

The split is deliberate and it is the whole design: **the catalogue is yours,
what has been paid for is Stripe's.** That is why `billing_subscription` is
`private` for writes — a row saying somebody is subscribed when Stripe
disagrees is not a stale cache, it is an entitlement somebody granted
themselves.

## A note on API versions

Stripe delivers webhooks in the shape of whichever API version your endpoint was
created under, and those shapes move between versions — `current_period_end`
left the subscription for its items, an invoice's subscription moved under
`parent`, `tax` became `total_taxes`.

apiplant reads the fields it needs by name, in each of the places they have
lived, so a newer account keeps recording subscriptions rather than returning
400 to every delivery. Requests, meanwhile, go out at the bundled client's own
version — the latest — so both halves are current without you configuring
either. It is worth knowing about anyway, because the failure this prevents is
the silent kind: the money still moves, and only the tables stay empty.

## Turning it off

Delete the `[payments]` section. The routes and the resources go away; the
tables stay, because migrations are additive and data recording money changing
hands should outlive a config change.

## See also

* [Payments](../../docs/payments.md) — the full guide: tax, idempotency, price
  immutability, what this does not do.
* [Permissions](../../docs/permissions.md) — what `role:admin` and `member`
  mean here.
* [Hooks](../../docs/hooks.md) — the protocol the two paywalls speak.
