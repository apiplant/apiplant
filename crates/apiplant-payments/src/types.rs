//! The vocabulary between this crate and its callers.
//!
//! Everything here is plain data — no Stripe types cross this boundary in
//! either direction. That is what lets the server write a `billing_*` row
//! without knowing what a `PaymentIntent` is, and what would let a second
//! provider be added without touching the code that stores the results.

use serde::{Deserialize, Serialize};

/// How often a price recurs, or [`Interval::OneOff`] for a single charge.
///
/// This is the field that decides everything else about a purchase: a
/// recurring price starts a subscription, a one-off takes a payment, and the
/// checkout session is built differently for each.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Interval {
    /// Charged once. The empty string in a `billing_price` row.
    #[default]
    #[serde(rename = "", alias = "one_off", alias = "once")]
    OneOff,
    Day,
    Week,
    Month,
    Year,
}

impl Interval {
    /// Parse the `interval` column of a `billing_price` row. Anything
    /// unrecognised is a one-off, because the alternative — guessing at
    /// "monthly" — bills somebody every month for something they bought once.
    pub fn parse(value: &str) -> Interval {
        match value.trim().to_ascii_lowercase().as_str() {
            "day" | "daily" => Interval::Day,
            "week" | "weekly" => Interval::Week,
            "month" | "monthly" => Interval::Month,
            "year" | "yearly" | "annual" | "annually" => Interval::Year,
            _ => Interval::OneOff,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Interval::OneOff => "",
            Interval::Day => "day",
            Interval::Week => "week",
            Interval::Month => "month",
            Interval::Year => "year",
        }
    }

    /// Whether buying this starts a subscription.
    pub fn is_recurring(self) -> bool {
        self != Interval::OneOff
    }
}

/// Whether a price is quoted before or after tax.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TaxBehavior {
    /// The amount is what you charge, and tax is added to it (the default —
    /// it is how B2B pricing is quoted nearly everywhere).
    #[default]
    Exclusive,
    /// The amount is the total, and the tax is worked out from inside it.
    Inclusive,
    /// Not decided. Stripe refuses to compute automatic tax on such a price,
    /// so this exists to round-trip an existing one, not to be chosen.
    Unspecified,
}

impl TaxBehavior {
    pub fn parse(value: &str) -> TaxBehavior {
        match value.trim().to_ascii_lowercase().as_str() {
            "inclusive" | "included" | "gross" => TaxBehavior::Inclusive,
            "unspecified" | "" => TaxBehavior::Unspecified,
            _ => TaxBehavior::Exclusive,
        }
    }
}

/// A product as the app holds it, on its way to Stripe.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ProductSpec {
    /// The Stripe id to update. Empty creates a new product.
    pub stripe_product_id: String,
    pub name: String,
    pub description: String,
    pub active: bool,
    /// Copied to Stripe as metadata, so an operator reading either system
    /// sees the same facts about a plan.
    pub metadata: serde_json::Map<String, serde_json::Value>,
}

/// A price on its way to Stripe.
///
/// Stripe prices are immutable: the amount, currency, interval and tax
/// behaviour of a price object can never change once it exists. So an "update"
/// that touches any of them is really a new price plus an archived old one —
/// see [`Payments::upsert_price`](crate::Payments::upsert_price), which is
/// where that decision is made rather than left to each caller.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct PriceSpec {
    /// The existing Stripe price, if this is a change to one.
    pub stripe_price_id: String,
    /// The Stripe product it belongs to. Required.
    pub stripe_product_id: String,
    pub nickname: String,
    /// In the currency's smallest unit: 1000 is 10.00.
    pub unit_amount: i64,
    /// ISO 4217, lowercase. Empty takes `[payments] currency`.
    pub currency: String,
    pub interval: Interval,
    /// Charge every N intervals; 0 and 1 both mean "every one".
    pub interval_count: u64,
    /// Free days before the first charge.
    pub trial_days: u32,
    pub tax_behavior: TaxBehavior,
    pub active: bool,
}

impl PriceSpec {
    /// Whether the difference between this and `current` is one Stripe will
    /// let us apply in place.
    ///
    /// Only `nickname` and `active` are mutable on a Stripe price. Everything
    /// else is baked into the object, which is the entire reason
    /// [`upsert_price`](crate::Payments::upsert_price) sometimes replaces one.
    pub fn differs_materially_from(&self, current: &PriceSpec) -> bool {
        self.unit_amount != current.unit_amount
            || self.currency != current.currency
            || self.interval != current.interval
            || self.interval_count.max(1) != current.interval_count.max(1)
            || self.trial_days != current.trial_days
            || self.tax_behavior != current.tax_behavior
    }
}

/// What the app knows about the organisation that is paying.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct CustomerSpec {
    /// An existing Stripe customer to reuse. Empty creates one.
    pub stripe_customer_id: String,
    pub email: String,
    pub name: String,
    /// The organisation this customer belongs to, written into Stripe's
    /// metadata. This is the thread that ties a webhook — which arrives with
    /// no session, no headers and no caller — back to a tenant.
    pub organization_id: String,
}

/// A started checkout: the URL to send the buyer to, and the session it
/// belongs to.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StartedCheckout {
    pub session_id: String,
    /// Stripe's hosted payment page. This is the whole point of the call.
    pub url: String,
    /// The customer the session will bill, created if there wasn't one.
    pub stripe_customer_id: String,
    /// `subscription` or `payment` — which of the two things this buys.
    pub mode: String,
}

/// A verified webhook delivery, and what it means for the app's own rows.
#[derive(Debug, Clone)]
pub struct Delivery {
    /// Stripe's event id (`evt_…`). The idempotency key: a `billing_event`
    /// row with this id already in it means the work is already done.
    pub id: String,
    /// The event type, e.g. `invoice.paid`.
    pub kind: String,
    /// The event as delivered, for the audit row.
    pub payload: serde_json::Value,
    /// What, if anything, the app should write down.
    pub change: Change,
}

/// The state change a delivery describes, in the app's own terms.
#[derive(Debug, Clone)]
pub enum Change {
    /// A checkout finished. For a subscription this arrives alongside
    /// `customer.subscription.created`; what it uniquely carries is the
    /// organisation, from the metadata the session was started with.
    CheckoutCompleted(CheckoutOutcome),
    /// A subscription was created, changed plan, renewed, lapsed or was
    /// cancelled. All of them are the same write: this is its state now.
    Subscription(SubscriptionState),
    /// Money moved — successfully or not.
    Payment(PaymentRecord),
    /// The customer's own details changed, usually from the billing portal.
    Customer(CustomerState),
    /// A delivery we have nothing to do with. Recorded and acknowledged:
    /// Stripe sends every event the endpoint is subscribed to, and retrying
    /// one forever because we don't handle it helps nobody.
    Ignored,
}

/// The facts a completed checkout session carries.
#[derive(Debug, Clone, Default)]
pub struct CheckoutOutcome {
    pub session_id: String,
    pub stripe_customer_id: String,
    /// Set when the session started a subscription.
    pub stripe_subscription_id: String,
    /// Set when the session took a one-off payment.
    pub stripe_payment_intent_id: String,
    /// From the session metadata — see [`CustomerSpec::organization_id`].
    pub organization_id: String,
    /// The `billing_price` row the buyer chose, also from metadata, so the
    /// resulting row points at the app's price and not only at Stripe's.
    pub price_id: String,
    pub customer_email: String,
    /// Total including tax, in the smallest unit.
    pub amount_total: i64,
    pub currency: String,
}

/// A subscription as Stripe now sees it.
#[derive(Debug, Clone, Default)]
pub struct SubscriptionState {
    pub stripe_subscription_id: String,
    pub stripe_customer_id: String,
    /// The Stripe price of the first (usually only) item.
    pub stripe_price_id: String,
    /// `active`, `trialing`, `past_due`, `canceled`, …
    pub status: String,
    pub quantity: i64,
    pub organization_id: String,
    /// Unix seconds; `None` where Stripe sent none.
    pub current_period_end: Option<i64>,
    pub trial_end: Option<i64>,
    pub canceled_at: Option<i64>,
    pub cancel_at_period_end: bool,
}

impl SubscriptionState {
    /// Whether this subscription entitles its organisation to the thing it
    /// pays for, right now.
    ///
    /// `trialing` counts — a trial is a promise the app made — and `past_due`
    /// does not: Stripe is still retrying the card, and a business that keeps
    /// serving through a dunning cycle is making a deliberate choice, not
    /// reading a status. Apps that want to make it read
    /// `status in ("active", "trialing", "past_due")` themselves.
    pub fn is_entitled(&self) -> bool {
        matches!(self.status.as_str(), "active" | "trialing")
    }
}

/// One movement of money, successful or not.
#[derive(Debug, Clone, Default)]
pub struct PaymentRecord {
    pub stripe_payment_intent_id: String,
    pub stripe_invoice_id: String,
    pub stripe_customer_id: String,
    pub stripe_subscription_id: String,
    pub organization_id: String,
    /// Total charged, in the smallest unit.
    pub amount: i64,
    /// How much of `amount` was tax. Zero when Stripe computed none.
    pub tax_amount: i64,
    pub currency: String,
    /// `succeeded`, `failed`, `pending` or `refunded`.
    pub status: String,
    pub description: String,
    /// Stripe-hosted receipt or invoice PDF, when there is one.
    pub receipt_url: String,
    /// Unix seconds, when it succeeded.
    pub paid_at: Option<i64>,
}

/// The customer's own details, as last changed.
#[derive(Debug, Clone, Default)]
pub struct CustomerState {
    pub stripe_customer_id: String,
    pub organization_id: String,
    pub email: String,
    pub name: String,
    /// The VAT/GST number they gave, if any.
    pub tax_id: String,
    /// Two-letter country Stripe places them in for tax.
    pub tax_country: String,
    pub details: serde_json::Value,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_unrecognised_interval_is_a_one_off() {
        // Guessing "monthly" here would bill somebody every month for
        // something they bought once, so the safe reading is the only one.
        for value in ["", "  ", "fortnightly", "nonsense"] {
            assert_eq!(Interval::parse(value), Interval::OneOff);
            assert!(!Interval::parse(value).is_recurring());
        }
        assert_eq!(Interval::parse("Monthly"), Interval::Month);
        assert_eq!(Interval::parse(" YEAR "), Interval::Year);
        assert!(Interval::parse("year").is_recurring());
    }

    #[test]
    fn tax_behaviour_defaults_to_adding_tax_on_top() {
        assert_eq!(TaxBehavior::parse("anything"), TaxBehavior::Exclusive);
        assert_eq!(TaxBehavior::parse("inclusive"), TaxBehavior::Inclusive);
        assert_eq!(TaxBehavior::parse(""), TaxBehavior::Unspecified);
    }

    /// The immutable fields are the ones that force a replacement; the two
    /// mutable ones must not.
    #[test]
    fn only_immutable_fields_make_a_price_a_different_price() {
        let base = PriceSpec {
            unit_amount: 1000,
            currency: "eur".into(),
            interval: Interval::Month,
            interval_count: 1,
            ..PriceSpec::default()
        };

        let renamed = PriceSpec {
            nickname: "Standard".into(),
            active: !base.active,
            ..base.clone()
        };
        assert!(!renamed.differs_materially_from(&base));

        for changed in [
            PriceSpec {
                unit_amount: 1200,
                ..base.clone()
            },
            PriceSpec {
                currency: "usd".into(),
                ..base.clone()
            },
            PriceSpec {
                interval: Interval::Year,
                ..base.clone()
            },
            PriceSpec {
                interval_count: 3,
                ..base.clone()
            },
            PriceSpec {
                trial_days: 14,
                ..base.clone()
            },
            PriceSpec {
                tax_behavior: TaxBehavior::Inclusive,
                ..base.clone()
            },
        ] {
            assert!(changed.differs_materially_from(&base));
        }

        // 0 and 1 are the same "every interval", and must not churn a price.
        let implicit = PriceSpec {
            interval_count: 0,
            ..base.clone()
        };
        assert!(!implicit.differs_materially_from(&base));
    }

    #[test]
    fn entitlement_covers_trials_and_stops_at_dunning() {
        let state = |status: &str| SubscriptionState {
            status: status.into(),
            ..SubscriptionState::default()
        };
        assert!(state("active").is_entitled());
        assert!(state("trialing").is_entitled());
        for status in ["past_due", "canceled", "unpaid", "incomplete", "paused"] {
            assert!(!state(status).is_entitled(), "{status}");
        }
    }
}
