//! Built-in resources.
//!
//! `organization`, `membership`, `user`, `api_key` and `oauth_connection` exist
//! in every app. Together they make apps multitenant out of the box: users join
//! organisations through memberships (which also carry their **role within that
//! organisation**), and every other resource is isolated per organisation by
//! default.
//!
//! `invitation` and `auth_token` are the two tables behind the flows that reach
//! somebody through their mailbox — being invited to an organisation,
//! confirming an address, resetting a password. They exist in every app, and in
//! one with no `[email]` provider they simply stay empty: the endpoints that
//! write to them are not mounted at all.
//!
//! Drop a `models/<name>.toml` with the same `name` to replace a built-in and
//! add fields or tweak permissions while keeping the machinery working.

use crate::schema::Resource;

/// The `organization` — the tenant. `global` because its rows *are* the
/// organisations; membership (not an `organization_id`) decides who sees them.
pub const ORGANIZATION_TOML: &str = r#"
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
"#;

/// A user's membership in an organisation, carrying their role there. The
/// join table behind the N:N between `user` and `organization`.
pub const MEMBERSHIP_TOML: &str = r#"
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

# Built-in function: lets `create` name the person by `email` instead of by
# `user_id`, and refuses a duplicate membership. The lookup has to happen here
# because `user` is only readable by people you already share an org with.
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
                           # Further roles are `membership_role` rows; a
                           # `role:` permission is checked against all of them.
"#;

/// A single role held by a membership.
///
/// Roles are a set, not a slot: someone can be a `billing` *and* a `support`
/// person without either displacing the other, which one column cannot express.
/// [`MEMBERSHIP_TOML`]'s `role` stays as the member's **primary** role — it is
/// what existing apps and hook contexts read — and these rows are the rest.
/// Together they are the roles a `role:` permission is checked against.
///
/// `admin` is special: it satisfies every role check in its organisation
/// without needing a row per role, so granting someone `admin` grants them
/// everything the app defines.
pub const MEMBERSHIP_ROLE_TOML: &str = r#"
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
"#;

/// The default `user`: global (users are shared across organisations) with
/// email + password auth. Extend via `models/users.toml`.
pub const USER_TOML: &str = r#"
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
# anyone when `[auth] require_email_verification` is on, so an app with no
# mailer carries the column and never looks at it.
[fields.email_verified_at]
type = "timestamp"

[fields.email_verified_at.admin]
visible = false

[fields.display_name]
type = "string"
"#;

/// An invitation to join an organisation, addressed to someone who may not have
/// an account yet.
///
/// This is the table behind `POST <base>/auth/invitations`. The emailed link
/// carries a token whose **hash** is what lives here, for the same reason an
/// API key's does: a leaked database should not be a pile of working links.
///
/// It is org-scoped, so an admin only ever sees the invitations to their own
/// organisation, and `read`/`create` are `role:admin` — the endpoints that a
/// person *without* an account uses (previewing a link, accepting it) are not
/// CRUD on this resource at all, and reach the row through the token they were
/// sent rather than through the API's permissions.
pub const INVITATION_TOML: &str = r#"
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
"#;

/// A single-use token sent to an address to prove someone reads it.
///
/// Both address confirmation and password reset are the same shape — mint a
/// secret, mail it, accept it once, before it expires — so they are one table
/// distinguished by `kind` rather than two that would drift apart.
///
/// Entirely `private`: every row is created and spent by the framework's own
/// endpoints, and there is nothing here anybody should read over the API. The
/// plaintext exists only in the message that was sent.
pub const AUTH_TOKEN_TOML: &str = r#"
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
"#;

/// Default `api_key` resource (global). A valid key authenticates as its owner.
pub const API_KEY_TOML: &str = r#"
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
"#;

/// Default `oauth_connection` resource (global) linking a user to a provider.
pub const OAUTH_TOML: &str = r#"
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
"#;

// --- billing ------------------------------------------------------------
//
// The five `billing_*` resources exist only in an app whose `[payments]`
// section names a provider — see [`billing_builtins`]. They are prefixed for
// the same reason the built-in functions are: `product` and `price` are words
// an ordinary app wants for its own domain, and a framework that took them
// would be taking them from a shop that sells things.
//
// The split between them is the split Stripe already makes, and copying it is
// deliberate: a **product** is the thing you sell, a **price** is one way to
// pay for it (monthly, yearly, one-off), and a product with three prices is
// three ways to buy one thing rather than three things. Because these are
// ordinary resources, the catalogue is CRUD — a `role:admin` can add a plan
// from the dashboard, and the hooks below mirror it into Stripe — while
// `billing_customer`, `billing_subscription` and `billing_payment` are
// `private`: Stripe decides what is paid for and the webhook writes it down.

/// A thing the app sells. Global: the catalogue is the same for every tenant.
///
/// Writable by an admin, and every write is mirrored into Stripe by the
/// [`apiplant_stripe_product`] built-in, so a plan added in the dashboard
/// exists in Stripe before the row is committed. `stripe_product_id` is what
/// that hook fills in; nothing else should.
///
/// [`apiplant_stripe_product`]: https://docs.rs/apiplant-server
pub const BILLING_PRODUCT_TOML: &str = r#"
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
"#;

/// One way to pay for a [product](BILLING_PRODUCT_TOML): an amount, a
/// currency, and either a billing interval (a subscription) or none (a one-off
/// payment).
///
/// Amounts are in the currency's **smallest unit** — 1000 is €10.00 — because
/// that is the only representation that is exact, and it is what Stripe,
/// every card network and every accountant's ledger already use. A float here
/// would be a rounding error waiting for a big enough invoice.
pub const BILLING_PRICE_TOML: &str = r#"
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
update = "role:admin"    # only `active` and presentation: see the hook
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

# The charge, in the currency's smallest unit: 1000 = €10.00 = $10.00.
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
"#;

/// The organisation as Stripe knows it: one row per tenant, holding the
/// customer id every charge and subscription hangs off.
///
/// Org-scoped and `private`. It is written by the checkout endpoint and the
/// webhook, never by hand — a second customer for one organisation would
/// split its payment methods, its invoices and its tax status across two
/// records that neither system would ever reconcile.
pub const BILLING_CUSTOMER_TOML: &str = r#"
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
"#;

/// A live (or lapsed) subscription: one organisation paying for one
/// [price](BILLING_PRICE_TOML) on a schedule.
///
/// `private` for writes and readable by any member, which is what makes it
/// usable as an entitlement check — a function asking "is this org on a paid
/// plan" reads a row through the same permissions as everything else. Stripe
/// is the source of truth and the webhook is what writes here: a row edited by
/// hand would say the customer is paying when they are not.
pub const BILLING_SUBSCRIPTION_TOML: &str = r#"
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
# for a paying customer wants "active" or "trialing" — see the `payments`
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
"#;

/// One payment that happened: a one-off purchase, or an invoice a
/// subscription generated.
///
/// A ledger, not a state machine — a row per attempt Stripe told us about,
/// kept whether it succeeded or not, because "the card was declined on the
/// 3rd" is the answer to most billing questions.
pub const BILLING_PAYMENT_TOML: &str = r#"
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
"#;

/// Every webhook Stripe has delivered, by its event id.
///
/// This table is the idempotency: Stripe retries a delivery until it is
/// acknowledged and may deliver the same event twice regardless, so the
/// handler inserts the id first and does the work only if the insert was new.
/// Without it a retried `invoice.paid` is a second row in the ledger and a
/// customer who appears to have paid twice.
///
/// Entirely `private`, and hidden from the dashboard: it is a log with one
/// reader, which is the handler itself.
pub const BILLING_EVENT_TOML: &str = r#"
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
"#;

/// The name → embedded-TOML table of built-ins, in dependency order so foreign
/// keys resolve (organization and user before membership/api_key/oauth).
pub fn builtins() -> Vec<(&'static str, &'static str)> {
    vec![
        ("organization", ORGANIZATION_TOML),
        ("user", USER_TOML),
        ("membership", MEMBERSHIP_TOML),
        ("membership_role", MEMBERSHIP_ROLE_TOML),
        ("api_key", API_KEY_TOML),
        ("oauth_connection", OAUTH_TOML),
        ("invitation", INVITATION_TOML),
        ("auth_token", AUTH_TOKEN_TOML),
    ]
}

/// The billing resources, in dependency order (product before price, customer
/// before subscription before payment).
///
/// Unlike [`builtins`] these are conditional: an app gets them when its
/// `[payments]` section names a provider, and not otherwise. Five tables and
/// five sets of endpoints is a lot to hand an app that never takes money, and
/// unlike `invitation` — which is two columns behind a feature most apps do
/// eventually turn on — a catalogue nobody sells from is just noise in the
/// dashboard.
///
/// Migrations are additive, so switching payments *off* leaves the tables
/// where they are: the data outlives the config, which is the right way round
/// for anything that recorded money changing hands.
pub fn billing_builtins() -> Vec<(&'static str, &'static str)> {
    vec![
        ("billing_product", BILLING_PRODUCT_TOML),
        ("billing_price", BILLING_PRICE_TOML),
        ("billing_customer", BILLING_CUSTOMER_TOML),
        ("billing_subscription", BILLING_SUBSCRIPTION_TOML),
        ("billing_payment", BILLING_PAYMENT_TOML),
        ("billing_event", BILLING_EVENT_TOML),
    ]
}

/// Parse one built-in by its embedded TOML. Panics on a malformed built-in —
/// that would be a bug in this crate, caught by the test below.
pub fn parse_builtin(toml_src: &str) -> Resource {
    let r: Resource = toml::from_str(toml_src).expect("built-in resource TOML is valid");
    r.validate().expect("built-in resource is valid");
    r
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_builtins_parse() {
        for (name, src) in builtins().into_iter().chain(billing_builtins()) {
            let r = parse_builtin(src);
            assert_eq!(r.meta.name, name);
        }
    }

    /// The prefix is what keeps a shop's own `product` model out of the
    /// framework's way, so it holds for every billing resource, not just the
    /// two that would have collided today.
    #[test]
    fn every_billing_resource_is_namespaced() {
        for (name, _) in billing_builtins() {
            assert!(name.starts_with("billing_"), "`{name}` is unprefixed");
        }
    }

    /// A `reference` that names a resource nothing declares migrates to a
    /// foreign key against a table that isn't there, and the app fails to
    /// boot. Every target here is either a billing resource or a core one.
    #[test]
    fn billing_references_resolve_within_the_app() {
        let known: Vec<&str> = builtins()
            .into_iter()
            .chain(billing_builtins())
            .map(|(name, _)| name)
            .collect();
        for (name, src) in billing_builtins() {
            let resource = parse_builtin(src);
            for (field, spec) in &resource.fields {
                if let Some(target) = &spec.references {
                    assert!(
                        known.contains(&target.as_str()),
                        "{name}.{field} points at an unknown `{target}`"
                    );
                }
            }
        }
    }
}
