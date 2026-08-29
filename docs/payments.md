# Payments with Stripe

An app names a provider and gets an editable catalogue, a customer per tenant,
the resulting subscriptions and payments, and the endpoints that start a
checkout and receive the results:

```toml
[payments]
provider       = "stripe"
secret_key     = "$STRIPE_SECRET_KEY"
webhook_secret = "$STRIPE_WEBHOOK_SECRET"
currency       = "eur"
```

Payments are **off by default**: no provider, no billing tables, no `/billing`
endpoints, no dependency on anything ever reaching Stripe.

## What turning it on adds

Five resources and four routes.

| Resource | Scope | Who reads it | Who writes it |
|----------|-------|--------------|---------------|
| `billing_product` | global | anyone | `role:admin` |
| `billing_price` | global | anyone | `role:admin` |
| `billing_customer` | organization | `role:admin` | the server only |
| `billing_subscription` | organization | `member` | the webhook only |
| `billing_payment` | organization | `role:admin` | the webhook only |

(There is a sixth, `billing_event`, which is the webhook's own ledger. It is
`private` and hidden from the dashboard; see [Idempotency](#idempotency).)

| Route | Who | What |
|-------|-----|------|
| `GET  <base>/billing/config` | anyone | publishable key, currency, whether tax is added |
| `POST <base>/billing/checkout` | org **admin** | starts a purchase, returning `{ "url": … }` |
| `POST <base>/billing/portal` | org **admin** | Stripe's self-service screens, returning `{ "url": … }` |
| `POST <base>/billing/webhook` | Stripe | the only source that records what was paid for |

These are ordinary resources and ordinary endpoints, which is the point: the
price list is a `GET /billing_price` any caller can make, checking whether an
organisation is subscribed is a query, and the admin dashboard and
`apiplant cli` list, filter and edit all of it with no billing-specific code.

## Where the source of truth is

Ownership is split, and in the only way that remains correct across a network
partition:

* **The catalogue is yours.** `billing_product` and `billing_price` are the
  source; Stripe holds a copy, pushed by a hook when you save a row.
* **What has been paid for is Stripe's.** `billing_subscription` and
  `billing_payment` are the copy, written by the webhook.

That is why those two are `private` for writes. A row claiming an active
subscription that Stripe does not recognise is not a stale cache but a
self-granted entitlement. Read them freely; to *change* one, change the
subscription at the provider and let the webhook record it.

## Products and prices

A **product** is the thing you sell. A **price** is one way to pay for it. A
product with three prices is three ways to buy one thing, not three things.

```bash
curl -X POST $API/billing_product \
  -H "Authorization: Bearer $TOKEN" -H "X-Organization: $ORG" \
  -d '{"name": "Pro", "description": "For teams", "features": {"seats": 25}}'
```

```bash
curl -X POST $API/billing_price \
  -H "Authorization: Bearer $TOKEN" -H "X-Organization: $ORG" \
  -d '{"product_id": "…", "nickname": "Monthly", "unit_amount": 2900,
       "currency": "eur", "interval": "month", "trial_days": 14}'
```

### Physical and digital goods

A product carries two more columns, and both answer the same question — is this
thing posted, or downloaded?

```bash
curl -X POST $API/billing_product \
  -H "Authorization: Bearer $TOKEN" -H "X-Organization: $ORG" \
  -d '{"name": "Enamel Mug", "shippable": true}'
```

`shippable` decides two things at once: whether the checkout collects a shipping
address (from `[payments] shipping_countries`), and which tax category the
product is filed under. A checkout for a shippable product in an app that lists
no shipping countries is refused, rather than taking money for an order with
nowhere to send it.

`tax_code` is Stripe's [tax category] for the product. Left empty it takes the
`[payments]` default *for its kind* — `digital_tax_code` or `physical_tax_code`
— because in much of the EU a downloaded file and a posted object are taxed by
different rules, and a single default would be wrong for half of any mixed
catalogue. Set it explicitly where the rate is more specific than "general":
an e-book is not taxed like software.

[tax category]: https://stripe.com/docs/tax/tax-categories

Saving either row runs a built-in hook (`apiplant_stripe_product` and
`apiplant_stripe_price`) that creates or updates the object in Stripe and writes
the resulting id back into the row. The hook runs **before** the write, so if
Stripe rejects the call the request fails and no row is committed, ensuring the
catalogue never contains a plan that cannot be purchased.

### Amounts are integers

`unit_amount` is expressed in the currency's **smallest unit**, so 2900 is
€29.00. This matches Stripe, the card networks and standard accounting practice,
and it is the only exact representation; a float would introduce rounding errors
on large invoices.

### `interval` determines the purchase type

An empty `interval` means a one-off purchase, while `day`, `week`, `month` and
`year` recur. Nothing else needs to be specified: the checkout is built as a
subscription or a single payment from this field alone, since it is a property
of the price and repeating it at the call site would invite inconsistency.

### Prices are immutable, so changing one replaces it

Stripe fixes a price's amount, currency, interval, trial and tax behaviour the
moment it exists. Only its nickname and whether it is `active` can change. So
`PATCH`ing an amount does this:

1. creates a **new** Stripe price at the new amount,
2. archives the old one,
3. writes the new id into your row.

Existing subscriptions continue at the old amount, which is what those customers
agreed to. Migrating them to the new price is a separate, explicit action; the
alternative would be a framework that raises prices silently.

## Buying something

```bash
curl -X POST $API/billing/checkout \
  -H "Authorization: Bearer $TOKEN" -H "X-Organization: $ORG" \
  -d '{"price_id": "…", "quantity": 1}'
# → {"url": "https://checkout.stripe.com/c/pay/…", "mode": "subscription"}
```

Redirect the buyer to `url`. Everything after that (the card form, 3-D Secure,
wallets, the tax number field and the receipt) happens on Stripe's domain, which
keeps this app effectively out of PCI scope.

**Checkout requires `role:admin`.** A purchase charges the organisation's card,
so a member who could start a subscription could commit their employer to a
recurring charge.

The organisation comes from `X-Organization`, as with every other org-scoped
request, and is written into the Stripe session's metadata. That metadata is the
only link back to a tenant when the webhook arrives; see below.

### The customer portal

```bash
curl -X POST $API/billing/portal \
  -H "Authorization: Bearer $TOKEN" -H "X-Organization: $ORG" -d '{}'
```

This is where a customer changes their card, downloads invoices, updates their
VAT number or cancels. None of it is implemented here; the results arrive as
webhooks.

## Webhooks

**Without a webhook, nothing is recorded.** The redirect to `success_url`
depends on the buyer's browser, which may close the tab, never follow the
redirect, or suspend. The webhook is the only reliable record of what was paid
for.

Point Stripe at `<base>/billing/webhook` and put the signing secret in
`webhook_secret`. Locally:

```bash
stripe listen --forward-to localhost:8080/api/billing/webhook
```

Subscribe it to at least:

```
checkout.session.completed
customer.subscription.created  customer.subscription.updated
customer.subscription.deleted  customer.subscription.paused
customer.subscription.resumed
invoice.paid  invoice.payment_failed
payment_intent.succeeded  payment_intent.payment_failed
customer.updated
```

Any other event is accepted, recorded and ignored. Returning a non-2xx would
tell Stripe to retry, and retrying an unhandled event only builds a backlog.

### Verification

Every delivery is checked against `webhook_secret`, over the request's **raw
bytes**. An unverified delivery is rejected before parsing and nothing is
written. This endpoint is a public URL that modifies subscriptions, and the
signature is what prevents it from being used to grant a plan.

An app with no `webhook_secret` still mounts the route and rejects every
delivery, reporting this in the boot log and on the dashboard's billing screen.
Returning a 404 instead would leave an operator unable to tell which part is
misconfigured.

### API versions

There are two directions, and they behave differently on purpose.

**Requests** are sent at the API version the bundled Stripe client is generated
against — currently `2026-07-29.dahlia`, the latest. It is pinned rather than
configurable, and that is what makes the typed request and response objects
correct: a client generated for one version cannot reliably read another's
answers. Your account's own default version does not affect these calls, because
the version is sent explicitly on every request.

**Webhooks** are the direction you do not control. Stripe delivers events in the
shape of whichever version the endpoint was created under, and those shapes
change: `current_period_end` moved from a subscription onto its items, an
invoice's subscription moved under `parent`, `tax` became a `total_taxes`
breakdown. So deliveries are read field by field, each in the places it has
lived, rather than parsed into a fixed set of structs.

The result is that **any account version works**, with nothing to configure. You
need not pin your account, match it to anything, or upgrade in step with this
framework. This matters more than it sounds: the failure it prevents is the
silent kind, where the charges still go through and only your tables stay empty.

### Idempotency

Stripe retries until it receives a 2xx, and may deliver the same event more than
once regardless. Every delivery's event id is inserted into `billing_event`
first, and the work proceeds only if the insert was new. The unique constraint
acts as the lock, so two workers processing the same retry contend for the same
row and exactly one proceeds. Without this, a retried `invoice.paid` would add a
second ledger row and make a customer appear to have paid twice.

A `billing_event` row with a null `processed_at` and an `error` records an event
that arrived and failed to process. That is the place to look when a customer
reports a payment the app does not reflect.

## Tax

`automatic_tax` is enabled by default and delegates the calculation to Stripe
Tax, which determines what the buyer owes from their location and your
registrations, adds it to the charge, and reports it on the invoice. That figure
becomes `billing_payment.tax_amount`.

```toml
[payments]
automatic_tax     = true       # Stripe computes and adds tax
tax_id_collection = true       # ask business buyers for a VAT/GST number
billing_address   = "auto"     # or "required"
digital_tax_code  = "txcd_10000000"   # general — electronically supplied services
physical_tax_code = "txcd_99999999"   # general — tangible goods
```

Three points to note:

* It requires an **origin address and at least one active registration**
  configured in the Stripe dashboard. Without them it computes no tax and the
  buyer pays the listed price. It is not a substitute for registering in the
  relevant jurisdictions.
* `tax_behavior` on a price declares whether the amount is quoted **before** tax
  (`exclusive`, the default) or **inclusive** of it. An incorrect setting makes
  every invoice wrong by the tax rate.
* The **tax code** on the product is what makes the computed rate right rather
  than merely plausible. See [Physical and digital
  goods](#physical-and-digital-goods).

Collecting a tax number is significant: a business buyer's VAT number is what
triggers the reverse charge, which is the difference between charging a German
company 19% and charging them nothing.

## Checking entitlement

The usual question is whether an organisation is entitled to what it pays for,
and the answer is a query:

```sql
SELECT 1 FROM billing_subscription
WHERE organization_id = $1 AND status IN ('active', 'trialing')
```

`trialing` counts, since a trial is a commitment already made. `past_due`
intentionally does not: Stripe is still retrying the card, and continuing to
serve through a dunning cycle is a business decision rather than a status read.
Make that decision explicitly by adding `'past_due'` to the list.

From a function it is a hook or an ordinary query; from a front end it is
`GET /billing_subscription`, which any member may read.

## From a function

The typed helpers cover the common things:

```rust
let url = ctx.checkout("price_1234", true, &organization_id)?;
let url = ctx.billing_portal("cus_1234")?;
let state = ctx.subscription("sub_1234")?;
ctx.cancel_subscription("sub_1234", true)?;   // at the end of the paid period
```

`ctx.payments(json!({ "op": … }))` covers the remaining operations: `customer`,
`product`, `price` and anything added later.

Note their purpose: these call the **provider** over the network. Checking
whether an organisation is subscribed is a query against `billing_subscription`,
which the webhook keeps current and which costs no round trip. Use these helpers
to perform an action, or when a decision requires asking Stripe directly.

The same call is available over the C ABI (`host->payments`) and in TypeScript
functions, so a function in any supported language can process payments.

## Configuration

```toml
[payments]
provider          = "stripe"                  # "none" (default) or "stripe"
secret_key        = "$STRIPE_SECRET_KEY"      # sk_…, required
publishable_key   = "$STRIPE_PUBLISHABLE_KEY" # pk_…, served to browsers
webhook_secret    = "$STRIPE_WEBHOOK_SECRET"  # whsec_…, see Webhooks
currency          = "usd"                     # for prices that do not name one
automatic_tax     = true
tax_id_collection = true                      # unset follows automatic_tax
billing_address   = "auto"                    # or "required"
shipping_countries = []                       # ISO codes a shippable product may go to
digital_tax_code  = "txcd_10000000"           # default for a product that is not posted
physical_tax_code = "txcd_99999999"           # default for one that is
success_url       = ""                        # empty means the dashboard's billing screen
cancel_url        = ""
portal_return_url = ""
timeout_secs      = 20                        # per Stripe call
```

Each of these holds a credential or an environment-specific URL, so reference an
environment variable rather than the value; see
[Configuration → Environment variables](configuration.md#environment-variables).

A `secret_key` that is actually a publishable key is detected by its prefix at
boot, rather than surfacing as an authentication error at the first checkout
that identifies neither key.

## Turning it off

Set `enabled = false` — or `provider = "none"`, which means the same thing here
— and the routes and resources are removed. The switch is the one to reach for
when the keys should stay: it turns payments off without unpicking the
credentials, currency and redirect URLs below it.

The **tables are not** removed either way: migrations are additive, and records
of money changing hands should outlive a configuration change.

## See also

* [Example 18 · Payments](../examples/18-payments/): a runnable app with a
  catalogue, an entitlement check and a paywalled endpoint.
* [Permissions](permissions.md): what `role:admin` and `member` mean here.
* [Multitenancy](multitenancy.md): why the organisation is the customer.
