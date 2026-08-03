# 18 · Payments (Stripe, as ordinary resources)

An app names a payment provider, and billing arrives as five resources and four
endpoints — with the permissions, roles and org-scoping everything else already
has. The catalogue is CRUD you can edit from the dashboard; the paywall is a
lifecycle hook; entitlement is a query.

```
18-payments/
├── main.toml                  # [payments] provider, keys, tax
├── models/
│   └── document.toml          # the thing you have to be subscribed to make
└── functions/
    ├── plan.rs                # the paywall: a before_create hook
    └── require_plan.toml      # …which statuses count as paying
```

Nothing in `models/` or `functions/` mentions Stripe. That is the shape of the
integration: `[payments]` brings the machinery, and the app talks about
documents and plans.

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
cargo run -p apiplant -- run examples/18-payments
```

```
INFO apiplant_server:   payments -> stripe (eur, automatic tax on)
INFO apiplant_server:   hook document.before_create -> require_plan
INFO apiplant_server:   admin -> /admin/
```

Without `STRIPE_WEBHOOK_SECRET` the app still boots and still takes money — and
warns, loudly, that nothing will ever be recorded. That is the one billing
misconfiguration that looks like silence rather than an error, so it is said out
loud in three places: the boot log, the dashboard's billing screen and
`GET /api/billing/config`.

## 1. Sign up and start an organisation

```bash
API=http://localhost:8101/api

TOKEN=$(curl -s -X POST $API/auth/register \
  -d '{"email":"ann@example.test","password":"hunter2"}' | jq -r .token)

ORG=$(curl -s -X POST $API/organization -H "Authorization: Bearer $TOKEN" \
  -d '{"name":"Acme"}' | jq -r .id)

auth=(-H "Authorization: Bearer $TOKEN" -H "X-Organization: $ORG")
```

Whoever creates an organisation becomes its admin, which matters here: buying
takes `role:admin`, because a member who can start a subscription can commit
their employer to a recurring charge.

## 2. Build a price list

```bash
PRODUCT=$(curl -s -X POST $API/billing_product "${auth[@]}" \
  -d '{"name":"Pro","description":"For teams","features":{"seats":25}}' | jq -r .id)

PRICE=$(curl -s -X POST $API/billing_price "${auth[@]}" -d '{
  "product_id": "'$PRODUCT'",
  "nickname": "Monthly",
  "unit_amount": 2900,
  "currency": "eur",
  "interval": "month",
  "trial_days": 14
}' | jq -r .id)

curl -s $API/billing_price/$PRICE | jq '{id, stripe_price_id, unit_amount}'
```

Two ordinary `POST`s — and both rows come back with a `stripe_product_id` and a
`stripe_price_id` already filled in. A built-in hook created the objects in
Stripe *before* the row was written, so a Stripe that refuses fails the request
and the catalogue never contains a plan nobody can buy.

Open the Stripe dashboard; the product is there. Open apiplant's dashboard at
<http://localhost:8101/admin/>; the same product is a table you can edit.

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
```

```json
{ "error": "this organization is not on a plan; see /api/billing_price" }
```

`402 Payment Required` — not `403`. A client can tell those apart: one shows a
pricing page, the other an apology. The check is `functions/plan.rs`, a
`before_create` hook, and it is a query against `billing_subscription` rather
than a call to Stripe — that table is a local copy of Stripe's fact, kept
current by the webhook precisely so that "may they do this" costs a query.

## 4. Pay

```bash
curl -s -X POST $API/billing/checkout "${auth[@]}" \
  -d '{"price_id":"'$PRICE'"}' | jq -r .url
```

Open the URL. Pay with `4242 4242 4242 4242`, any future expiry, any CVC.

Everything on that page — the card form, 3-D Secure, the wallet buttons, the
VAT number box, the receipt — is Stripe's, on Stripe's domain. This app has no
PCI scope worth the name, and the entire integration is "redirect them to a URL
we were handed".

In the `stripe listen` terminal, watch the events arrive:

```
checkout.session.completed        [200]
customer.subscription.created     [200]
invoice.paid                      [200]
```

And now:

```bash
curl -s $API/billing_subscription "${auth[@]}" | jq '.[0] | {status, current_period_end}'
curl -s -X POST $API/document "${auth[@]}" -d '{"title":"Q3 plan"}' | jq .title
```

```json
{ "status": "trialing", "current_period_end": "2026-08-14T…" }
"Q3 plan"
```

The subscription is `trialing`, not `active` — the price has a 14-day trial, and
no money has moved yet. The hook lets it through anyway, because a trial is a
promise the app made. Which statuses count is in
`functions/require_plan.toml`, not in the code.

## 5. Let them manage it themselves

```bash
curl -s -X POST $API/billing/portal "${auth[@]}" -d '{}' | jq -r .url
```

Card, invoices, VAT number, cancellation — all of it on Stripe's screens, none
of it implemented here, and every change coming back as a webhook that updates
the rows. There is deliberately no "cancel" endpoint in this app: a function
re-exporting one would be a worse version of a page that already exists.

The dashboard's **Billing** screen is the same three calls with buttons on
them: the plan, the price list, the payment history and the two links.

## What is where

| Question | Where it is answered |
|----------|----------------------|
| What do we sell? | `billing_product` / `billing_price` — your tables, mirrored to Stripe |
| Who is the customer? | `billing_customer` — one per organisation, never two |
| What has been paid for? | `billing_subscription` / `billing_payment` — written *only* by the webhook |
| May they do this? | a query, in `functions/plan.rs` |
| How do they pay? | `POST /billing/checkout` → a Stripe URL |
| How do they change it? | `POST /billing/portal` → a Stripe URL |

The split is deliberate and it is the whole design: **the catalogue is yours,
what has been paid for is Stripe's.** That is why `billing_subscription` is
`private` for writes — a row saying somebody is subscribed when Stripe
disagrees is not a stale cache, it is an entitlement somebody granted
themselves.

## Turning it off

Delete the `[payments]` section. The routes and the resources go away; the
tables stay, because migrations are additive and data recording money changing
hands should outlive a config change.

## See also

* [Payments](../../docs/payments.md) — the full guide: tax, idempotency, price
  immutability, what this does not do.
* [Permissions](../../docs/permissions.md) — what `role:admin` and `member`
  mean here.
* [Hooks](../../docs/hooks.md) — the protocol `require_plan` speaks.
