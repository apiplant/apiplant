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
//! Two sets are *conditional*, and for the same reason: they are machinery for
//! a feature most apps do not turn on, and a table nobody ever writes to is
//! noise in a dashboard. The `billing_*` resources arrive with a `[payments]`
//! provider ([`billing_builtins`]), and `oauth_state` with an `[oauth]` one
//! ([`oauth_builtins`]).
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

# What *kind* of organisation this is — `school`, `staff`, `customer`. A
# permission can be narrowed to one (`role:admin@org_class=school`), so this
# column decides who gets in, and it is therefore server-owned: it is stripped
# from every client body like `organization_id` is, and only somebody the
# `[organization] org_class_editors` setting names may set it. Unset by
# default, which no class-qualified policy matches.
[fields.org_class]
type = "string"
max_length = 64

# A logo, as a URL a browser can fetch. Nothing sets it — an organisation is not
# handed to us by an identity provider the way a person is — so it is here for
# an app to fill and for every interface to read: the dashboard's workspace
# switcher shows it in place of the initials it would otherwise draw.
#
# A `file` field, so the dashboard offers an upload into `[storage]` as well as
# a URL box. What is stored is a string either way — an uploaded logo is a
# relative link this server answers, a pasted one is whatever was pasted.
[fields.avatar_url]
type = "file"
max_length = 1024
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

# What to call somebody, and what to show beside their name.
#
# Both are ordinary nullable columns nothing requires — an app that has no use
# for either can leave them empty or drop them by replacing this model. They are
# here because they are what almost every app wants and what every identity
# provider hands over: a sign-in through [`[oauth]`](crate::config::OAuthConfig)
# fills them in, so an account that arrives that way arrives with a name and a
# picture rather than an email address and a blank.
#
# The picture is a `file` field: a provider fills it with the URL it gave us,
# and somebody changing it in the dashboard uploads one into `[storage]` or
# types a URL of their own. Both are the same string in the same column.
[fields.display_name]
type = "string"

[fields.avatar_url]
type = "file"
max_length = 1024

# True when the address above is one apiplant invented rather than one somebody
# gave it.
#
# A sign-in through a provider that releases no address — X, today — still has
# to put *something* in the identity column, which is required and unique. It
# writes `<provider>_<id>@oauth.invalid`, at a TLD RFC 2606 reserves so that it
# can never resolve.
#
# This flag is here, in every app, because the framework is the one inventing
# that value: fabricating an address and not recording that it was fabricated
# leaves an app to find out by watching mail bounce. With it, "we have an
# address for this person" stops being the same question as "there is a string
# in the email column" — which is the question a welcome email, a newsletter
# and a CSV export all actually mean to ask.
[fields.email_placeholder]
type = "boolean"
default = false

[fields.email_placeholder.admin]
visible = false
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
///
/// One row per (provider, account at that provider). Somebody may hold four of
/// them, and signing in through any one reaches the same `user` — which is the
/// whole point of the table: an account is not its GitHub account, it *has* one.
///
/// It is a live, ordinary resource: `GET <base>/oauth_connection` is how a
/// client draws somebody's linked accounts, and it is `owner`-scoped, so that
/// question needs no filter and cannot answer anybody else's.
///
/// The profile columns are refreshed on every sign-in, because people change
/// their name and their picture. There is deliberately no access token and no
/// refresh token here: `<base>/auth/oauth` authenticates people and never acts
/// on their behalf afterwards, so the token is used once — to read the profile —
/// and dropped. An app that does need to keep calling the provider adds those
/// columns itself and encrypts them at rest; `hidden` keeps a value out of API
/// responses, which is not the same as keeping it out of a database dump.
pub const OAUTH_TOML: &str = r#"
[resource]
name = "oauth_connection"
scope = "global"
timestamps = true

# Readable by whoever it belongs to — `GET <base>/oauth_connection` is "my
# linked accounts", with no filter in the request saying so — and written by
# nobody but the framework.
#
# `delete` is **private**, which is the one that looks wrong and is not:
# removing a connection has an invariant that a row deletion cannot see. An
# account with no password and no second provider becomes permanently
# unreachable the moment its last connection goes, so unlinking is
# `DELETE <base>/auth/oauth/{provider}`, which checks what else is left and
# refuses the last one. Leaving `delete = "owner"` here would put a door beside
# that check with nothing behind it but the same table.
[permissions]
list   = "owner"
read   = "owner"
create = "private"
update = "private"
delete = "private"

[fields.provider]
type = "string"
required = true

# The provider's own immutable id for this person — GitHub's numeric id,
# Google's `sub`, X's user id. Never a username: GitHub and X both let people
# change theirs and let the freed name be taken by somebody else, so an account
# keyed on one would hand the new owner the old owner's account.
[fields.provider_user_id]
type = "string"
required = true

# `provider:provider_user_id`, so the pair can carry a UNIQUE constraint — a
# single column is what `unique` can express. It is what makes two simultaneous
# first-time sign-ins from one GitHub account produce one user instead of two:
# the loser's insert conflicts, and it reads back the winner's row rather than
# creating a second account.
[fields.provider_key]
type = "string"
unique = true
max_length = 320

[fields.owner_id]
type = "reference"
references = "user"
required = true
on_delete = "cascade"

# What the provider last said about them. `email_verified` is the field the
# account-matching rule hangs on, so it records what the *provider* claimed
# rather than what this app would like to be true.
[fields.email]
type = "string"
max_length = 320

[fields.email_verified]
type = "boolean"
default = false

[fields.display_name]
type = "string"

# A plain string, not a `file`: this row is a record of what the provider
# claimed, so nothing should offer to replace it with an upload.
[fields.avatar_url]
type = "string"
max_length = 1024

[fields.last_login_at]
type = "timestamp"
"#;

/// The half-finished handshake: `oauth_state`.
///
/// Present only in an app with an `[oauth]` provider, because it is machinery
/// and not domain: two requests, minutes apart, with a consent screen between
/// them, and everything the second one must not take on trust from the browser
/// waiting somewhere the browser cannot reach.
///
/// A cache with a TTL would do the same job in less space. A table is used
/// because it survives a restart — a redeploy in the ninety seconds somebody
/// spends reading a consent screen should not fail their sign-in — and because
/// it needs no Redis to exist.
pub const OAUTH_STATE_TOML: &str = r#"
[resource]
name = "oauth_state"
scope = "global"
timestamps = true

# Machinery, like `auth_token`: rows appear for ninety seconds while somebody
# reads a consent screen and are never worth looking at afterwards, so the
# dashboard does not offer a screen for them.
[admin]
label = "OAuth sign-in"
plural = "OAuth sign-ins"
visible = false

# Nothing may reach this table over the API — not even the person whose sign-in
# it is. Every column on it is either a secret or a decision that stops being
# safe the moment a client can change it, and its only reader is the callback
# endpoint, which goes to the table directly.
[permissions]
list   = "private"
read   = "private"
create = "private"
update = "private"
delete = "private"

[fields.provider]
type = "string"
required = true

# SHA-256 of the `state` parameter, not the parameter. The value itself travels
# in a URL, through browser history and the provider's logs; only its hash is
# kept, so this table leaking does not let anybody finish a flow in progress.
# SHA-256 rather than argon2 for the same reason an API key's hash is: the
# callback looks the row up *by* this column.
[fields.state_hash]
type = "string"
required = true
unique = true
hidden = true

# The PKCE verifier, where the provider supports PKCE. Only its SHA-256 travels
# with the authorize redirect, so intercepting that redirect — or the code that
# comes back on it — is not enough to redeem anything.
[fields.verifier]
type = "string"
hidden = true

# Repeated verbatim in the token request, because the provider compares the two
# and refuses on any difference. Recorded rather than recomputed so that a
# config change mid-flow cannot strand a sign-in.
[fields.redirect_uri]
type = "string"
max_length = 1024

# Set when the flow was started by somebody already signed in: this is "connect
# my GitHub", not "sign me in". It is decided here, while holding that account's
# session, and never read from the callback — which is the difference between
# linking an account you can prove is yours and linking any account whose id you
# can name.
[fields.link_user_id]
type = "reference"
references = "user"
on_delete = "cascade"

# Where to send the browser afterwards. Only ever a path on this site; see the
# `return_to` handling in the oauth routes.
[fields.return_to]
type = "string"
max_length = 1024

# How the caller asked to be given the token, overriding `[oauth]
# token_delivery` for this one flow. Empty means "however the app is
# configured".
#
# It is recorded rather than read from the callback for the ordinary reason:
# the callback is a request the provider makes, and nothing about it is the
# app's word. A first-party client — the admin dashboard, say — knows how it
# wants to receive a token, and this is where it says so.
[fields.token_delivery]
type = "string"
max_length = 16

[fields.expires_at]
type = "timestamp"
required = true

# Stamped by the callback. A code may be redeemed once, and so may the state
# that authorised it: a second callback carrying the same one is a double-click
# or an attack, and is refused either way.
[fields.used_at]
type = "timestamp"
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

/// The one resource an app gets for having an `[oauth]` provider.
///
/// Conditional for the same reason the billing tables are: `oauth_state` is
/// pure machinery, empty except during a sign-in, and an app that signs nobody
/// in through a third party has no use for a table in its dashboard that never
/// holds a row. `oauth_connection` is *not* here — it is a built-in for every
/// app, because "which accounts is this person known by" is a question worth
/// having a shape for even before anybody answers it.
pub fn oauth_builtins() -> Vec<(&'static str, &'static str)> {
    vec![("oauth_state", OAUTH_STATE_TOML)]
}

/// One published message, waiting for one subscriber to handle it.
///
/// This table *is* the queue. A `publish` writes a row here and fires a
/// `NOTIFY`; a subscriber claims the row, runs the function, and marks it. The
/// row is what makes the message durable — a broker-less queue whose messages
/// only lived in a notification would lose everything published while nothing
/// was listening, and would have nowhere to record that an attempt failed.
///
/// One row per *subscriber*, not per message: a topic two functions listen to
/// produces two rows, so a handler that keeps failing retries on its own
/// schedule without dragging its neighbour along. A message nobody subscribes
/// to still gets a row, marked handled with no subscriber — the answer to "I
/// published it, why did nothing happen?" is then a row rather than a silence.
///
/// Readable by an admin and writable by nobody: the columns are a state machine
/// the subscriber owns, and editing one by hand is how a message gets handled
/// twice or never.
pub const QUEUE_MESSAGE_TOML: &str = r#"
[resource]
name = "queue_message"
scope = "global"
timestamps = true

[admin]
label = "Background task"
plural = "Background tasks"

# "System" is a reserved group: the dashboard lists it down with the other
# screens every app has whether or not it uses the feature — your account, the
# team, the organization — rather than as a heading of its own among the app's
# own resources. An app that never publishes a message still gets this table
# (it is a built-in), and a top-level "Operations" section holding one
# permanently empty list is the sort of thing that makes a dashboard look like
# it belongs to somebody else's app.
group = "System"

# Spelled out rather than left to the defaults, because the questions this
# screen answers are asked in a hurry: what is stuck, on which topic, for whom,
# and how many times has it tried. The generic guess — first string column —
# would name a row by `claimed_by`, which is the one column that is usually
# empty.
display_field = "topic"
columns = ["topic", "subscriber", "status", "attempts", "available_at", "processed_at"]

# `error` is in here because the way somebody arrives at this table is usually
# with half a message from a log line and no idea which row it came from.
search_fields = ["topic", "subscriber", "error"]

# Visible and readable, because the questions this table answers — what is
# stuck, what failed, what is retrying — are asked by a person under time
# pressure, and making them write SQL for it is not a kindness. Nothing here is
# writable over the API: the subscriber owns these columns.
[permissions]
list   = "role:admin"
read   = "role:admin"
create = "private"
update = "private"
delete = "private"

[fields.topic]
type = "string"
required = true

# The function this row is for. Empty means the message was published to a
# topic nothing subscribes to; the row is kept as the record that it happened.
[fields.subscriber]
type = "string"

# pending → running → done, or → failed once the attempts run out. A `pending`
# row whose `available_at` is in the future is waiting out a retry backoff; a
# `running` row whose `claimed_at` is older than `[queues] lease_secs` belongs
# to a subscriber that died, and is taken back on the next sweep.
[fields.status]
type = "string"
required = true
default = "pending"

[fields.status.admin]
options = ["pending|Pending", "running|Running", "done|Done", "failed|Failed"]

[fields.payload]
type = "json"
required = true

[fields.attempts]
type = "integer"
required = true
default = 0

# The earliest this row may be claimed. Set on publish (now) and pushed forward
# by each failure, which is how the retry backoff is expressed without a timer
# living in any one process.
[fields.available_at]
type = "timestamp"
required = true

# When the current attempt started, and which process took it. Together with
# `[queues] lease_secs` these are what let a message survive the subscriber
# that was holding it: a `running` row nobody has finished within the lease is
# assumed abandoned and offered again.
[fields.claimed_at]
type = "timestamp"

[fields.claimed_by]
type = "string"

[fields.processed_at]
type = "timestamp"

# Why the last attempt failed. Kept on a row that later succeeds, because "it
# worked on the third try" is worth knowing.
[fields.error]
type = "text"

# Who published it: a principal id, or empty when the publisher was the server
# itself (a resource `[publish]` declaration, or a function with no caller).
[fields.published_by]
type = "string"
"#;

/// The one resource an app gets for using queues.
///
/// Unconditional, unlike the billing tables: `[queues]` needs no configuration
/// to be useful — a function calling `publish` works in an app whose `main.toml`
/// never mentions queues at all — so the table has to be there before anyone
/// declares anything. It costs one empty table.
pub fn queue_builtins() -> Vec<(&'static str, &'static str)> {
    vec![("queue_message", QUEUE_MESSAGE_TOML)]
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
        for (name, src) in builtins()
            .into_iter()
            .chain(billing_builtins())
            .chain(oauth_builtins())
            .chain(queue_builtins())
        {
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
