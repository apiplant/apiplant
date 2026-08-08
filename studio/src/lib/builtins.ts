/**
 * The resources every apiplant app has whether or not a file describes them,
 * generated from `crates/apiplant-core/src/defaults.rs` by
 * `scripts/gen-builtins.mjs` — do not edit by hand.
 *
 * The studio shows them alongside custom resources; editing one writes a
 * `resources/*.toml` that replaces the default, exactly as the framework intends.
 * The billing set is conditional: the framework adds it only when `[payments]`
 * names a provider, so the studio lists it on the same condition.
 */

import { parseResource } from "./toml";
import type { Resource } from "./types";

export const ALWAYS_BUILTIN_NAMES = [
  "organization",
  "user",
  "membership",
  "membership_role",
  "api_key",
  "oauth_connection",
  "invitation",
  "auth_token",
] as const;

/** Present only when `[payments].provider` is set. */
export const BILLING_BUILTIN_NAMES = [
  "billing_product",
  "billing_price",
  "billing_customer",
  "billing_subscription",
  "billing_payment",
  "billing_event",
] as const;

export const BUILTIN_NAMES = [...ALWAYS_BUILTIN_NAMES, ...BILLING_BUILTIN_NAMES] as const;
export type BuiltinName = (typeof BUILTIN_NAMES)[number];

/** Conventional file name for a built-in, matching the docs (`user` → users.toml). */
export const BUILTIN_FILENAME: Record<BuiltinName, string> = {
  organization: "organizations.toml",
  user: "users.toml",
  membership: "memberships.toml",
  membership_role: "membership_roles.toml",
  api_key: "api_keys.toml",
  oauth_connection: "oauth_connections.toml",
  invitation: "invitations.toml",
  auth_token: "auth_tokens.toml",
  billing_product: "billing_products.toml",
  billing_price: "billing_prices.toml",
  billing_customer: "billing_customers.toml",
  billing_subscription: "billing_subscriptions.toml",
  billing_payment: "billing_payments.toml",
  billing_event: "billing_events.toml",
};

export const BUILTIN_SUMMARY: Record<BuiltinName, string> = {
  organization: "The tenant. Membership decides who sees it, so it is global.",
  user: "Login identity. Carries the [auth] section the framework authenticates against.",
  membership: "Joins a user to an organisation and carries their role there.",
  membership_role: "Extra roles a membership holds, beyond the one on the membership itself.",
  api_key: "A hashed key that authenticates as its owning user.",
  oauth_connection: "Links a user to an external identity provider.",
  invitation: "A pending invite to an organisation, issued by POST /auth/invitations.",
  auth_token: "Single-use email tokens for address verification and password reset. Private throughout.",
  billing_product: "Something you sell, mirrored from the payment provider's catalogue.",
  billing_price: "What a product costs — one-off or recurring, per currency.",
  billing_customer: "Ties a user or organisation to the provider's customer record.",
  billing_subscription: "An active recurring plan and where it is in its cycle.",
  billing_payment: "One payment, recorded when the provider says it settled.",
  billing_event: "The raw webhook log — what the provider said and whether it was handled.",
};

const SOURCES: Record<BuiltinName, string> = {
  organization: `
[resource]
name = "organization"
scope = "global"
timestamps = true

[permissions]
list   = "member"          # organisations you belong to
read   = "member"
create = "authenticated"   # anyone may start one (and becomes its admin)
update = "role:admin"      # an admin *of that organisation*
delete = "role:admin"

[fields.name]
type = "string"
required = true

[fields.slug]
type = "string"
unique = true
`,

  user: `
[resource]
name = "user"
scope = "global"
timestamps = true

[permissions]
list   = "member"          # people you share an organisation with
read   = "member"
create = "public"          # registration
update = "owner"
delete = "private"

[auth]
identity_field = "email"
password_field = "password_hash"
oauth_providers = []

[fields.email]
type = "string"
required = true
unique = true
max_length = 320

[fields.password_hash]
type = "string"
hidden = true

# When the address was confirmed. Null means unconfirmed — which only *stops*
# anyone when \`[auth] require_email_verification\` is on, so an app with no
# mailer carries the column and never looks at it.
[fields.email_verified_at]
type = "timestamp"

[fields.email_verified_at.admin]
visible = false

[fields.display_name]
type = "string"
`,

  membership: `
[resource]
name = "membership"
scope = "organization"
timestamps = true

[permissions]
list   = "member"          # members can see who else is in the org
read   = "member"
create = "role:admin"      # admins add members
update = "role:admin"
delete = "role:admin"

# Built-in function: lets \`create\` name the person by \`email\` instead of by
# \`user_id\`, and refuses a duplicate membership. The lookup has to happen here
# because \`user\` is only readable by people you already share an org with.
[hooks]
before_create = "apiplant_organization_join"

[fields.user_id]
type = "reference"
references = "user"
required = true
on_delete = "cascade"      # a deleted account takes its memberships with it

[fields.organization_id]
type = "reference"
references = "organization"
required = true

[fields.role]
type = "string"            # the member's *primary* role here, e.g. "admin".
                           # Further roles are \`membership_role\` rows; a
                           # \`role:\` permission is checked against all of them.
`,

  membership_role: `
[resource]
name = "membership_role"
scope = "organization"
timestamps = true

[permissions]
list   = "member"          # members can see who holds what
read   = "member"
create = "role:admin"      # admins grant roles
update = "role:admin"
delete = "role:admin"

[fields.membership_id]
type = "reference"
references = "membership"
required = true
on_delete = "cascade"      # a removed member takes their roles with them

[fields.organization_id]
type = "reference"
references = "organization"
required = true

[fields.role]
type = "string"
required = true
`,

  api_key: `
[resource]
name = "api_key"
scope = "global"
timestamps = true

# "Api key" is what titleising the name produces, and it is not how anybody
# writes it.
[admin]
label = "API key"
plural = "API keys"

[permissions]
list   = "owner"
read   = "owner"
create = "authenticated"
update = "private"
delete = "owner"

[fields.name]
type = "string"

[fields.token_hash]
type = "string"
required = true
unique = true
hidden = true

[fields.owner_id]
type = "reference"
references = "user"
required = true
`,

  oauth_connection: `
[resource]
name = "oauth_connection"
scope = "global"
timestamps = true

[permissions]
list   = "owner"
read   = "owner"
create = "private"
update = "private"
delete = "owner"

[fields.provider]
type = "string"
required = true

[fields.provider_user_id]
type = "string"
required = true

[fields.owner_id]
type = "reference"
references = "user"
required = true
`,

  invitation: `
[resource]
name = "invitation"
scope = "organization"
timestamps = true

[permissions]
list   = "role:admin"      # admins see who is still pending
read   = "role:admin"
create = "private"         # issued by POST /auth/invitations, never by hand:
                           # a row written directly has no token to send
update = "private"
delete = "role:admin"      # revoking is deleting the row

[fields.email]
type = "string"
required = true
max_length = 320

[fields.role]
type = "string"            # the role they will hold once they accept

[fields.token_hash]
type = "string"
required = true
unique = true
hidden = true

[fields.invited_by]
type = "reference"
references = "user"
on_delete = "set_null"

[fields.expires_at]
type = "timestamp"
required = true

# Set when the invitation is used. A row with this filled in is history, kept
# so "who let them in, and when" survives the membership being edited later.
[fields.accepted_at]
type = "timestamp"
`,

  auth_token: `
[resource]
name = "auth_token"
scope = "global"
timestamps = true

[admin]
label = "Auth token"
plural = "Auth tokens"
visible = false

[permissions]
list   = "private"
read   = "private"
create = "private"
update = "private"
delete = "private"

[fields.user_id]
type = "reference"
references = "user"
required = true
on_delete = "cascade"      # a deleted account takes its live links with it

[fields.kind]
type = "string"
required = true            # "email_verification" or "password_reset"

[fields.token_hash]
type = "string"
required = true
unique = true
hidden = true

[fields.expires_at]
type = "timestamp"
required = true

# Set the moment the token is spent, so a link in a mailbox works exactly once.
[fields.used_at]
type = "timestamp"
`,

  billing_product: `
[resource]
name = "billing_product"
scope = "global"
timestamps = true

[admin]
label = "Product"
plural = "Products"

[permissions]
list   = "public"          # a pricing page is read by people with no account
read   = "public"
create = "role:admin"
update = "role:admin"
delete = "role:admin"

# Mirror the catalogue into Stripe. Creating the row creates the product;
# renaming it renames it; archiving it (active = false) archives it there too.
[hooks]
before_create = "apiplant_stripe_product"
before_update = "apiplant_stripe_product"

[fields.name]
type = "string"
required = true

[fields.description]
type = "text"

# Off the price list without deleting the history that points at it. Stripe
# calls this "archived", and a product with live subscriptions cannot be
# deleted in either system — only stopped from being bought again.
[fields.active]
type = "boolean"
default = true

# Free-form facts the app checks when deciding what a plan may do: seat
# limits, feature flags, an internal tier name. Copied to Stripe as metadata
# so an operator reading either system sees the same thing.
[fields.features]
type = "json"

[fields.stripe_product_id]
type = "string"
unique = true

[fields.stripe_product_id.admin]
readonly = true
help = "Filled in by Stripe when the product is first saved."
`,

  billing_price: `
[resource]
name = "billing_price"
scope = "global"
timestamps = true

[admin]
label = "Price"
plural = "Prices"

[permissions]
list   = "public"
read   = "public"
create = "role:admin"
update = "role:admin"    # only \`active\` and presentation: see the hook
delete = "role:admin"

[hooks]
before_create = "apiplant_stripe_price"
before_update = "apiplant_stripe_price"

[fields.product_id]
type = "reference"
references = "billing_product"
required = true
on_delete = "cascade"      # a deleted product takes its price list with it

[fields.nickname]
type = "string"            # "Monthly", "Yearly (2 months free)"

# The charge, in the currency's smallest unit: 1000 = €10.00 = \$10.00.
[fields.unit_amount]
type = "big_int"
required = true

[fields.unit_amount.admin]
help = "In the smallest unit of the currency — 1000 is 10.00."

[fields.currency]
type = "string"
max_length = 3             # ISO 4217; empty takes [payments] currency

# How often it recurs: "month", "year", "week", "day", or empty for a price
# that is charged once. This is what decides whether buying it starts a
# subscription or takes a single payment.
[fields.interval]
type = "string"

[fields.interval.admin]
options = ["|One-off", "day|Daily", "week|Weekly", "month|Monthly", "year|Yearly"]

# Charge every N intervals — 3 with interval = "month" is quarterly.
[fields.interval_count]
type = "integer"
default = 1

# Days before the first charge. 0 (the default) starts billing immediately.
[fields.trial_days]
type = "integer"
default = 0

# Whether the amount already includes tax. "exclusive" (the default) means tax
# is added on top, which is what [payments] automatic_tax computes; "inclusive"
# means the amount is the total and the tax is worked out from within it.
[fields.tax_behavior]
type = "string"
default = "exclusive"

[fields.tax_behavior.admin]
options = ["exclusive|Tax added on top", "inclusive|Tax included in the amount"]

[fields.active]
type = "boolean"
default = true

# Stripe prices are immutable once created: changing an amount there means
# creating a new price and archiving the old one, which is exactly what the
# hook does. The id therefore points at whichever price object is current.
[fields.stripe_price_id]
type = "string"
unique = true

[fields.stripe_price_id.admin]
readonly = true
help = "Filled in by Stripe. Changing the amount creates a new price and archives the old one."
`,

  billing_customer: `
[resource]
name = "billing_customer"
scope = "organization"
timestamps = true

[admin]
label = "Billing customer"
plural = "Billing customers"

[permissions]
list   = "role:admin"      # billing is the admins' business
read   = "role:admin"
create = "private"
update = "private"
delete = "private"

[fields.stripe_customer_id]
type = "string"
required = true
unique = true

# Where invoices and receipts go. Defaults to the address of whoever first
# started a checkout, and is changed from the Stripe portal.
[fields.email]
type = "string"
max_length = 320

[fields.name]
type = "string"

# The buyer's VAT/GST number, once they have given one. Kept because it is
# what turns a taxed sale into a reverse-charge one, and support gets asked
# about it more than anything else on this table.
[fields.tax_id]
type = "string"

# Country Stripe places the customer in, for tax. Two-letter ISO 3166-1.
[fields.tax_country]
type = "string"
max_length = 2

# Everything else Stripe holds about the customer, as last delivered. Kept so
# an operator can answer a billing question without opening two dashboards.
[fields.details]
type = "json"

[fields.details.admin]
visible = false
`,

  billing_subscription: `
[resource]
name = "billing_subscription"
scope = "organization"
timestamps = true

[admin]
label = "Subscription"
plural = "Subscriptions"

[permissions]
list   = "member"          # members can see what plan they are on
read   = "member"
create = "private"         # started by POST /billing/checkout
update = "private"         # written by the Stripe webhook
delete = "private"

[fields.price_id]
type = "reference"
references = "billing_price"
on_delete = "set_null"     # an archived price outlives its row

[fields.customer_id]
type = "reference"
references = "billing_customer"
on_delete = "cascade"

# Stripe's own words: "trialing", "active", "past_due", "canceled",
# "incomplete", "incomplete_expired", "unpaid", "paused". Anything checking
# for a paying customer wants "active" or "trialing" — see the \`payments\`
# guide for why "past_due" is a judgement call and not a status.
[fields.status]
type = "string"
required = true

[fields.quantity]
type = "integer"
default = 1

[fields.current_period_end]
type = "timestamp"

# Set when the customer has asked to stop but has already paid through the end
# of the period: still entitled, not renewing.
[fields.cancel_at_period_end]
type = "boolean"
default = false

[fields.trial_ends_at]
type = "timestamp"

[fields.canceled_at]
type = "timestamp"

[fields.stripe_subscription_id]
type = "string"
required = true
unique = true

[fields.stripe_subscription_id.admin]
readonly = true
`,

  billing_payment: `
[resource]
name = "billing_payment"
scope = "organization"
timestamps = true

[admin]
label = "Payment"
plural = "Payments"

[permissions]
list   = "role:admin"
read   = "role:admin"
create = "private"
update = "private"
delete = "private"

[fields.customer_id]
type = "reference"
references = "billing_customer"
on_delete = "cascade"

[fields.price_id]
type = "reference"
references = "billing_price"
on_delete = "set_null"

[fields.subscription_id]
type = "reference"
references = "billing_subscription"
on_delete = "set_null"     # null for a one-off

# What was actually taken, in the smallest unit, and what of it was tax.
[fields.amount]
type = "big_int"
required = true

[fields.tax_amount]
type = "big_int"
default = 0

[fields.currency]
type = "string"
max_length = 3

# "succeeded", "pending", "failed", "refunded".
[fields.status]
type = "string"
required = true

[fields.description]
type = "string"

# The buyer's own receipt, hosted by Stripe. Worth storing: it is the link
# support is asked for, and it outlives the session that produced it.
[fields.receipt_url]
type = "string"
max_length = 2048

[fields.paid_at]
type = "timestamp"

[fields.stripe_payment_intent_id]
type = "string"
unique = true

[fields.stripe_invoice_id]
type = "string"
`,

  billing_event: `
[resource]
name = "billing_event"
scope = "global"
timestamps = true

[admin]
label = "Billing event"
plural = "Billing events"
visible = false

[permissions]
list   = "private"
read   = "private"
create = "private"
update = "private"
delete = "private"

[fields.stripe_event_id]
type = "string"
required = true
unique = true

[fields.kind]
type = "string"
required = true            # "checkout.session.completed", "invoice.paid", …

# When the work finished. A row with this null is an event that arrived and
# then failed to process — Stripe will retry it, and this is where to look
# when it stops retrying.
[fields.processed_at]
type = "timestamp"

[fields.error]
type = "text"

[fields.payload]
type = "json"
hidden = true
`,
};

/** A fresh copy of a built-in's default definition. */
export function builtinResource(name: BuiltinName): Resource {
  return parseResource(SOURCES[name]);
}
