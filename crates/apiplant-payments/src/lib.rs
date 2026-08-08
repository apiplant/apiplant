//! # apiplant-payments
//!
//! Taking money, as an optional part of an app rather than a rewrite of it.
//!
//! An app names a provider in `main.toml`:
//!
//! ```toml
//! [payments]
//! provider       = "stripe"
//! secret_key     = "${STRIPE_SECRET_KEY}"
//! webhook_secret = "${STRIPE_WEBHOOK_SECRET}"
//! currency       = "eur"
//! automatic_tax  = true
//! ```
//!
//! …and gets five things it would otherwise have written by hand: a catalogue
//! it can edit (`billing_product`, `billing_price`), a customer per tenant
//! (`billing_customer`), the subscriptions and payments that result
//! (`billing_subscription`, `billing_payment`), and the endpoints that start a
//! checkout and receive Stripe's webhooks.
//!
//! ## Why the catalogue is a resource
//!
//! Because everything else in apiplant is. A plan is data an operator changes
//! on a Tuesday afternoon, so it belongs in a table with permissions on it —
//! `role:admin` can add one, anyone can read the price list — rather than in a
//! config file that wants a deployment, or in a Stripe dashboard that the
//! app's own queries cannot see. The [hooks] this crate backs keep Stripe in
//! step: saving a `billing_product` row creates the Stripe product, and
//! changing a price's amount creates a new Stripe price and archives the old
//! one, because a Stripe price is immutable and pretending otherwise is how an
//! app ends up charging last quarter's amount.
//!
//! ## Where the truth lives
//!
//! Split, deliberately, and in the only way that survives a network partition:
//!
//! * The **catalogue** is the app's. Stripe holds a copy.
//! * **What has been paid for** is Stripe's. The `billing_subscription` and
//!   `billing_payment` tables hold a copy, written by the webhook.
//!
//! So those two tables are `private`: no endpoint writes them, because a row
//! saying somebody is subscribed when Stripe disagrees is worse than no row at
//! all. Read them freely — that is what they are for — but the way to *change*
//! one is to change the subscription, and let the webhook arrive.
//!
//! ## Tax
//!
//! `automatic_tax = true` (the default) hands the question to Stripe Tax: it
//! decides what the buyer owes from where they are and what you have
//! registered for, adds it to the charge, and reports it back on the invoice —
//! which is where `billing_payment.tax_amount` comes from. It needs an origin
//! address and at least one active registration configured in the Stripe
//! dashboard; with none, it computes nothing and the buyer pays the price. It
//! is not a substitute for having registered anywhere.
//!
//! [hooks]: https://docs.rs/apiplant-server

mod catalog;
mod checkout;
mod types;
mod webhook;

use std::time::Duration;

use apiplant_core::PaymentsConfig;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

pub use catalog::PriceOutcome;
pub use types::{
    Change, CheckoutOutcome, CustomerSpec, CustomerState, Delivery, Interval, PaymentRecord,
    PriceSpec, ProductSpec, StartedCheckout, SubscriptionState, TaxBehavior,
};

/// The metadata key every Stripe object we create carries, naming the tenant
/// it belongs to.
///
/// A webhook arrives with no session, no headers and no caller — it is Stripe
/// talking to a URL. This key is the only thread back to an organisation, so
/// it is written on the customer, the checkout session and the subscription,
/// and read off whichever of them a given event happens to carry.
pub const ORG_METADATA_KEY: &str = "apiplant_organization_id";

/// The metadata key naming the `billing_price` row a purchase was made from,
/// so the resulting subscription points at the app's price and not only at
/// Stripe's.
pub const PRICE_METADATA_KEY: &str = "apiplant_price_id";

/// What went wrong taking money.
#[derive(Debug, thiserror::Error)]
pub enum PaymentsError {
    /// The `[payments]` section can't produce a working client — an unknown
    /// provider, a missing key. Raised at startup, so a deployment fails to
    /// boot rather than failing at somebody's first checkout.
    #[error("payments configuration: {0}")]
    Config(String),

    /// The request is unusable: no price, an amount of zero, a customer that
    /// isn't one.
    #[error("invalid payment request: {0}")]
    Request(String),

    /// Stripe could not be reached, or took too long.
    #[error("payments transport: {0}")]
    Transport(String),

    /// Stripe answered, and said no.
    #[error("stripe rejected the request: {0}")]
    Provider(String),

    /// A webhook delivery didn't verify. Treat as hostile: an unsigned
    /// request to this endpoint is somebody trying to grant themselves a plan.
    #[error("webhook signature: {0}")]
    Signature(String),
}

/// Which service takes the money. One today; the name is here so that adding a
/// second is a variant rather than a rename of everything.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Provider {
    Stripe,
}

impl Provider {
    pub fn as_str(self) -> &'static str {
        match self {
            Provider::Stripe => "stripe",
        }
    }

    fn parse(value: &str) -> Result<Provider, PaymentsError> {
        match value.trim().to_ascii_lowercase().as_str() {
            "stripe" => Ok(Provider::Stripe),
            other => Err(PaymentsError::Config(format!(
                "unknown provider {other:?}; expected \"stripe\" or \"none\""
            ))),
        }
    }
}

/// A configured payment provider, shared by every worker.
///
/// Cloning is cheap — the Stripe client is an HTTP client behind an `Arc` —
/// so one of these is built on boot and handed to everything that needs it.
#[derive(Clone)]
pub struct Payments {
    client: stripe::Client,
    provider: Provider,
    config: PaymentsConfig,
    timeout: Duration,
    /// Where a buyer lands when neither the call nor `[payments]` said. The
    /// server passes its own billing screen; this crate has no idea what
    /// origin it is deployed at and should not have to guess.
    fallback_base: String,
}

impl std::fmt::Debug for Payments {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // The secret key must never reach a log line, so the client — which
        // holds it — is not printed at all.
        f.debug_struct("Payments")
            .field("provider", &self.provider.as_str())
            .field("currency", &self.config.default_currency())
            .field("automatic_tax", &self.config.automatic_tax)
            .finish()
    }
}

impl Payments {
    /// Build the provider an app's `[payments]` section describes.
    ///
    /// `Ok(None)` means the app configured none, which is the default and not
    /// an error. `Err` means it asked for one it cannot use — worth failing
    /// the boot over, because the alternative is discovering it at the first
    /// checkout, which is to say from a customer.
    ///
    /// `fallback_base` is where a buyer is returned to when neither the call
    /// nor `[payments]` named a URL — the server passes its own billing
    /// screen. Stripe refuses a checkout with no `success_url`, so there has
    /// to be one, and inventing it here would mean guessing at an origin this
    /// crate cannot see.
    pub fn from_config(
        config: &PaymentsConfig,
        fallback_base: &str,
    ) -> Result<Option<Payments>, PaymentsError> {
        if !config.enabled() {
            return Ok(None);
        }
        let provider = Provider::parse(&config.provider)?;
        let secret = config.secret_key.trim();
        if secret.is_empty() {
            return Err(PaymentsError::Config(
                "[payments] secret_key is required once a provider is named".into(),
            ));
        }
        // A publishable key in the secret slot is the single most common way
        // to misconfigure this, and it fails later with an authentication
        // error that says nothing about which key is wrong.
        if secret.starts_with("pk_") {
            return Err(PaymentsError::Config(
                "[payments] secret_key looks like a publishable key (pk_…); it wants the secret key (sk_…)"
                    .into(),
            ));
        }
        if !config.webhooks_enabled() {
            tracing::warn!(
                "[payments] has no webhook_secret: checkouts will complete, but no \
                 subscription or payment will ever be recorded"
            );
        }

        // The Stripe client builds a rustls connector that asks for the
        // process-wide crypto provider, and this workspace compiles more than
        // one — so with none installed the first HTTPS call panics rather than
        // failing. Installing it here, where the only client is built, means a
        // caller cannot forget to; "already set" is the normal case and not an
        // error, since the server installs the same provider at boot.
        let _ = rustls::crypto::ring::default_provider().install_default();

        let timeout = Duration::from_secs(config.timeout_secs.max(1));
        Ok(Some(Payments {
            client: stripe::Client::new(secret),
            provider,
            config: config.clone(),
            timeout,
            fallback_base: fallback_base.trim_end_matches('/').to_string(),
        }))
    }

    /// Which provider this is.
    pub fn provider(&self) -> Provider {
        self.provider
    }

    /// The `[payments]` section it was built from.
    pub fn config(&self) -> &PaymentsConfig {
        &self.config
    }

    /// The publishable key, for a front end that mounts Stripe's own elements.
    /// Empty when the app didn't configure one.
    pub fn publishable_key(&self) -> &str {
        self.config.publishable_key.trim()
    }

    /// Whether deliveries to the webhook endpoint can be verified.
    pub fn webhooks_enabled(&self) -> bool {
        self.config.webhooks_enabled()
    }

    /// Run one operation, given as the JSON a function sent across the ABI,
    /// and return the JSON reply.
    ///
    /// One entry point rather than one ABI call per verb, for the reason the
    /// [cache](apiplant_cache) has one: a new operation costs a variant of
    /// [`Op`], not a change to the contract every compiled function was built
    /// against.
    pub async fn execute(&self, request: &str) -> Result<Value, PaymentsError> {
        let op: Op = serde_json::from_str(request)
            .map_err(|e| PaymentsError::Request(format!("{e}; expected {}", Op::grammar())))?;
        match op {
            Op::Checkout(spec) => Ok(serde_json::to_value(self.checkout(spec).await?).unwrap()),
            Op::Portal {
                stripe_customer_id,
                return_url,
            } => {
                let url = self.portal(&stripe_customer_id, &return_url).await?;
                Ok(json!({ "url": url }))
            }
            Op::Customer(spec) => {
                let state = self.ensure_customer(&spec).await?;
                Ok(json!({
                    "stripe_customer_id": state.stripe_customer_id,
                    "email": state.email,
                    "name": state.name,
                    "tax_id": state.tax_id,
                    "tax_country": state.tax_country,
                }))
            }
            Op::Product(spec) => {
                let id = self.upsert_product(&spec).await?;
                Ok(json!({ "stripe_product_id": id }))
            }
            Op::Price(spec) => {
                let outcome = self.upsert_price(&spec).await?;
                Ok(json!({
                    "stripe_price_id": outcome.id,
                    "replaced": outcome.replaced,
                }))
            }
            Op::Subscription { id } => {
                let state = self.subscription(&id).await?;
                Ok(subscription_json(&state))
            }
            Op::Cancel { id, at_period_end } => {
                let state = self.cancel_subscription(&id, at_period_end).await?;
                Ok(subscription_json(&state))
            }
        }
    }

    /// Apply the configured timeout to one Stripe call and label its failure.
    ///
    /// Every call goes through here so that a slow Stripe costs one request
    /// its `timeout_secs` and nothing more — a hook blocking on a checkout
    /// would otherwise hold its worker for as long as Stripe felt like taking.
    async fn call<T>(
        &self,
        what: &str,
        future: impl std::future::Future<Output = Result<T, stripe::StripeError>>,
    ) -> Result<T, PaymentsError> {
        match tokio::time::timeout(self.timeout, future).await {
            Ok(Ok(value)) => Ok(value),
            Ok(Err(e)) => Err(match e {
                stripe::StripeError::Stripe(response, status) => PaymentsError::Provider(format!(
                    "{what}: {} (HTTP {status})",
                    response.message.clone().unwrap_or_default()
                )),
                other => PaymentsError::Transport(format!("{what}: {other}")),
            }),
            Err(_) => Err(PaymentsError::Transport(format!(
                "{what}: timed out after {:?}",
                self.timeout
            ))),
        }
    }
}

/// A subscription rendered for a function that asked about one.
fn subscription_json(state: &SubscriptionState) -> Value {
    json!({
        "stripe_subscription_id": state.stripe_subscription_id,
        "stripe_customer_id": state.stripe_customer_id,
        "stripe_price_id": state.stripe_price_id,
        "status": state.status,
        "quantity": state.quantity,
        "entitled": state.is_entitled(),
        "organization_id": state.organization_id,
        "current_period_end": state.current_period_end,
        "trial_end": state.trial_end,
        "canceled_at": state.canceled_at,
        "cancel_at_period_end": state.cancel_at_period_end,
    })
}

/// One payment operation, as a function sends it over the ABI.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum Op {
    /// Start a checkout and get a URL to send the buyer to.
    Checkout(CheckoutSpec),
    /// Open Stripe's customer portal, where somebody manages their own card,
    /// invoices and cancellation without the app implementing any of it.
    Portal {
        stripe_customer_id: String,
        #[serde(default)]
        return_url: String,
    },
    /// Find or create the Stripe customer for an organisation.
    Customer(CustomerSpec),
    /// Create or update a product.
    Product(ProductSpec),
    /// Create or update a price, replacing it if the change is one Stripe
    /// won't apply in place.
    Price(PriceSpec),
    /// Read a subscription's current state.
    Subscription { id: String },
    /// Cancel a subscription, now or at the end of the paid period.
    Cancel {
        id: String,
        /// `true` (the default) leaves them subscribed until the period they
        /// have paid for runs out, which is what "cancel" means to a customer.
        #[serde(default = "yes")]
        at_period_end: bool,
    },
}

fn yes() -> bool {
    true
}

impl Op {
    /// The accepted requests, for the error a malformed one produces.
    pub fn grammar() -> &'static str {
        r#"{"op":"checkout"|"portal"|"customer"|"product"|"price"|"subscription"|"cancel", …}"#
    }
}

/// What to buy, and who is buying it.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct CheckoutSpec {
    /// The Stripe price to charge. Required.
    pub stripe_price_id: String,
    /// The `billing_price` row it came from, carried through the session so
    /// the resulting subscription can point back at the app's own catalogue.
    pub price_id: String,
    /// Whether this price recurs — which is what decides between a
    /// subscription and a single payment.
    pub recurring: bool,
    /// How many. Zero is read as one.
    pub quantity: u64,
    /// The customer to bill. Empty creates one from `customer`.
    pub stripe_customer_id: String,
    /// Who to create the customer as, when there isn't one yet.
    pub customer: CustomerSpec,
    /// The tenant this purchase belongs to. Written into the session's
    /// metadata, and the reason the webhook can find its way home.
    pub organization_id: String,
    /// Free days before the first charge; overrides the price's own trial.
    pub trial_days: u32,
    /// Where Stripe returns the buyer. Empty takes `[payments] success_url`.
    pub success_url: String,
    /// Where Stripe returns a buyer who backed out. Empty takes
    /// `[payments] cancel_url`.
    pub cancel_url: String,
    /// Let the buyer enter a promotion code on the Stripe page.
    pub allow_promotion_codes: bool,
    /// Whether this purchase has to be posted, so the page must ask where to.
    ///
    /// Taken from the product rather than from the request: whether a thing is
    /// physical is a fact about the thing, and a caller that has to remember
    /// to say so is a caller that will one day sell a mug with no address on
    /// it.
    pub shipping: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Stands in for the dashboard's billing screen, which is what the server
    /// passes in a real app.
    const BASE: &str = "https://example.com/admin/#/billing";

    fn stripe_config() -> PaymentsConfig {
        PaymentsConfig {
            provider: "stripe".into(),
            secret_key: "sk_test_abc".into(),
            ..PaymentsConfig::default()
        }
    }

    #[test]
    fn a_provider_is_only_built_when_one_is_configured() {
        assert!(Payments::from_config(&PaymentsConfig::default(), BASE)
            .unwrap()
            .is_none());
        let payments = Payments::from_config(&stripe_config(), BASE)
            .unwrap()
            .unwrap();
        assert_eq!(payments.provider(), Provider::Stripe);
    }

    #[test]
    fn an_unusable_configuration_fails_at_boot_rather_than_at_a_checkout() {
        let missing_key = PaymentsConfig {
            secret_key: String::new(),
            ..stripe_config()
        };
        assert!(matches!(
            Payments::from_config(&missing_key, BASE),
            Err(PaymentsError::Config(_))
        ));

        let unknown = PaymentsConfig {
            provider: "paypal".into(),
            ..stripe_config()
        };
        assert!(matches!(
            Payments::from_config(&unknown, BASE),
            Err(PaymentsError::Config(_))
        ));
    }

    /// The publishable key in the secret slot otherwise fails much later, as
    /// an authentication error that names neither key.
    #[test]
    fn a_publishable_key_in_the_secret_slot_is_caught_by_name() {
        let swapped = PaymentsConfig {
            secret_key: "pk_test_abc".into(),
            ..stripe_config()
        };
        let error = Payments::from_config(&swapped, BASE).unwrap_err();
        assert!(error.to_string().contains("publishable"), "{error}");
    }

    /// Nothing that could carry the secret key may be printed: `Debug` is
    /// derived on a struct that holds the client, and a derive would have.
    #[test]
    fn the_secret_key_is_not_in_the_debug_output() {
        let payments = Payments::from_config(&stripe_config(), BASE)
            .unwrap()
            .unwrap();
        let printed = format!("{payments:?}");
        assert!(!printed.contains("sk_test_abc"), "{printed}");
        assert!(printed.contains("stripe"));
    }

    #[test]
    fn operations_parse_from_the_json_a_function_sends() {
        let checkout: Op = serde_json::from_str(
            r#"{"op":"checkout","stripe_price_id":"price_1","recurring":true,"organization_id":"org-1"}"#,
        )
        .unwrap();
        match checkout {
            Op::Checkout(spec) => {
                assert_eq!(spec.stripe_price_id, "price_1");
                assert!(spec.recurring);
                assert_eq!(spec.organization_id, "org-1");
                // Unmentioned fields take their defaults rather than failing.
                assert_eq!(spec.quantity, 0);
            }
            other => panic!("expected a checkout, got {other:?}"),
        }

        // "Cancel" means "at the end of what I paid for" unless it says
        // otherwise — the other reading takes away time somebody bought.
        let cancel: Op = serde_json::from_str(r#"{"op":"cancel","id":"sub_1"}"#).unwrap();
        assert!(matches!(
            cancel,
            Op::Cancel {
                at_period_end: true,
                ..
            }
        ));
        let now: Op =
            serde_json::from_str(r#"{"op":"cancel","id":"sub_1","at_period_end":false}"#).unwrap();
        assert!(matches!(
            now,
            Op::Cancel {
                at_period_end: false,
                ..
            }
        ));
    }

    #[test]
    fn an_unknown_operation_is_rejected() {
        assert!(serde_json::from_str::<Op>(r#"{"op":"refund_everything"}"#).is_err());
        assert!(serde_json::from_str::<Op>(r#"{"op":"subscription"}"#).is_err());
        assert!(serde_json::from_str::<Op>("not json").is_err());
    }
}
