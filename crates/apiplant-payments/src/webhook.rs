//! Hearing back from Stripe.
//!
//! A checkout that completes tells the *buyer* something; it tells the app
//! nothing at all. The buyer's browser goes to `success_url` — a page they may
//! close, a redirect they may never follow, a tab their laptop may sleep
//! through — and the only thing that reliably reports what was actually paid
//! for is the webhook. So this is not an optimisation or an extra: without it
//! the `billing_subscription` table stays empty while customers are billed.
//!
//! ## Verifying
//!
//! [`Payments::verify_webhook`] refuses anything without a valid signature
//! from `[payments] webhook_secret`. The endpoint is a public URL that edits
//! subscriptions; the signature is the entire reason it isn't a way to grant
//! yourself a plan.
//!
//! ## Normalising
//!
//! Stripe has dozens of event types describing a handful of things that
//! actually happened. This module collapses them into [`Change`], so the
//! server writes rows from plain data and does not grow a match arm every time
//! Stripe adds an event — `customer.subscription.created`, `.updated`,
//! `.deleted`, `.paused` and `.resumed` are all one write: *this is the
//! subscription's state now*.

use serde_json::Value;
use stripe::{CheckoutSession, Customer, EventObject, EventType, Invoice, PaymentIntent, Webhook};

use crate::checkout::subscription_state;
use crate::types::{Change, CheckoutOutcome, CustomerState, Delivery, PaymentRecord};
use crate::{Payments, PaymentsError, ORG_METADATA_KEY, PRICE_METADATA_KEY};

impl Payments {
    /// Verify a delivery and work out what it means.
    ///
    /// `payload` must be the **raw** request body, byte for byte. The
    /// signature covers the bytes Stripe sent, so a body that has been parsed
    /// and re-serialised — even into identical-looking JSON — will not verify.
    pub fn verify_webhook(
        &self,
        payload: &str,
        signature: &str,
    ) -> Result<Delivery, PaymentsError> {
        let secret = self.config.webhook_secret.trim();
        if secret.is_empty() {
            return Err(PaymentsError::Signature(
                "[payments] webhook_secret is not set, so no delivery can be trusted".into(),
            ));
        }
        let event = Webhook::construct_event(payload, signature.trim(), secret)
            .map_err(|e| PaymentsError::Signature(e.to_string()))?;

        let kind = event.type_.to_string();
        let change = classify(event.type_, &event.data.object);
        Ok(Delivery {
            id: event.id.to_string(),
            kind,
            payload: serde_json::from_str(payload).unwrap_or(Value::Null),
            change,
        })
    }
}

/// Turn one Stripe event into the app's own terms.
///
/// The match is on the *object*, not only on the type: an event type tells you
/// what Stripe calls the moment, and the object is what carries the facts. An
/// unrecognised pairing is [`Change::Ignored`] rather than an error — Stripe
/// delivers every event the endpoint is subscribed to, and failing one we
/// don't handle would have Stripe retry it for three days.
fn classify(kind: EventType, object: &EventObject) -> Change {
    match (kind, object) {
        (EventType::CheckoutSessionCompleted, EventObject::CheckoutSession(session))
        | (
            EventType::CheckoutSessionAsyncPaymentSucceeded,
            EventObject::CheckoutSession(session),
        ) => Change::CheckoutCompleted(checkout_outcome(session)),

        // Every subscription event is the same write: this is its state now.
        (_, EventObject::Subscription(subscription)) => {
            Change::Subscription(subscription_state(subscription))
        }

        // An invoice is what a subscription's renewal looks like. Both the
        // paid and the failed case are recorded: "the card was declined on the
        // 3rd" is the answer to most billing questions.
        (EventType::InvoicePaid, EventObject::Invoice(invoice))
        | (EventType::InvoicePaymentSucceeded, EventObject::Invoice(invoice)) => {
            Change::Payment(invoice_payment(invoice, "succeeded"))
        }
        (EventType::InvoicePaymentFailed, EventObject::Invoice(invoice)) => {
            Change::Payment(invoice_payment(invoice, "failed"))
        }

        // A one-off purchase. Invoiced payments arrive as invoice events too,
        // and are skipped here so a subscription renewal isn't recorded twice.
        (EventType::PaymentIntentSucceeded, EventObject::PaymentIntent(intent))
            if intent.invoice.is_none() =>
        {
            Change::Payment(intent_payment(intent, "succeeded"))
        }
        (EventType::PaymentIntentPaymentFailed, EventObject::PaymentIntent(intent))
            if intent.invoice.is_none() =>
        {
            Change::Payment(intent_payment(intent, "failed"))
        }

        (EventType::CustomerUpdated, EventObject::Customer(customer))
        | (EventType::CustomerCreated, EventObject::Customer(customer)) => {
            Change::Customer(customer_state(customer))
        }

        _ => Change::Ignored,
    }
}

/// What a completed checkout session tells us.
fn checkout_outcome(session: &CheckoutSession) -> CheckoutOutcome {
    let metadata = session.metadata.clone().unwrap_or_default();
    CheckoutOutcome {
        session_id: session.id.to_string(),
        stripe_customer_id: session
            .customer
            .as_ref()
            .map(|c| c.id().to_string())
            .unwrap_or_default(),
        stripe_subscription_id: session
            .subscription
            .as_ref()
            .map(|s| s.id().to_string())
            .unwrap_or_default(),
        stripe_payment_intent_id: session
            .payment_intent
            .as_ref()
            .map(|p| p.id().to_string())
            .unwrap_or_default(),
        organization_id: metadata.get(ORG_METADATA_KEY).cloned().unwrap_or_default(),
        price_id: metadata
            .get(PRICE_METADATA_KEY)
            .cloned()
            .unwrap_or_default(),
        customer_email: session
            .customer_details
            .as_ref()
            .and_then(|details| details.email.clone())
            .or_else(|| session.customer_email.clone())
            .unwrap_or_default(),
        amount_total: session.amount_total.unwrap_or(0),
        currency: currency_code(session.currency),
    }
}

/// A payment as an invoice describes it — the shape a subscription's renewals
/// arrive in.
fn invoice_payment(invoice: &Invoice, status: &str) -> PaymentRecord {
    let metadata = invoice.metadata.clone().unwrap_or_default();
    PaymentRecord {
        stripe_payment_intent_id: invoice
            .payment_intent
            .as_ref()
            .map(|p| p.id().to_string())
            .unwrap_or_default(),
        stripe_invoice_id: invoice.id.to_string(),
        stripe_customer_id: invoice
            .customer
            .as_ref()
            .map(|c| c.id().to_string())
            .unwrap_or_default(),
        stripe_subscription_id: invoice
            .subscription
            .as_ref()
            .map(|s| s.id().to_string())
            .unwrap_or_default(),
        organization_id: metadata.get(ORG_METADATA_KEY).cloned().unwrap_or_default(),
        // What was actually taken, not what was billed: a partly-paid invoice
        // says so, and a failed one is zero.
        amount: match status {
            "succeeded" => invoice.amount_paid.or(invoice.total).unwrap_or(0),
            _ => invoice.total.unwrap_or(0),
        },
        tax_amount: invoice.tax.unwrap_or(0),
        currency: currency_code(invoice.currency),
        status: status.to_string(),
        description: invoice.description.clone().unwrap_or_default(),
        // The hosted page is the link support gets asked for; the PDF is the
        // fallback for an invoice that has no hosted page.
        receipt_url: invoice
            .hosted_invoice_url
            .clone()
            .or_else(|| invoice.invoice_pdf.clone())
            .unwrap_or_default(),
        paid_at: invoice
            .status_transitions
            .as_ref()
            .and_then(|transitions| transitions.paid_at),
    }
}

/// A payment as a payment intent describes it — the shape a one-off purchase
/// arrives in.
fn intent_payment(intent: &PaymentIntent, status: &str) -> PaymentRecord {
    PaymentRecord {
        stripe_payment_intent_id: intent.id.to_string(),
        stripe_invoice_id: String::new(),
        stripe_customer_id: intent
            .customer
            .as_ref()
            .map(|c| c.id().to_string())
            .unwrap_or_default(),
        stripe_subscription_id: String::new(),
        organization_id: intent
            .metadata
            .get(ORG_METADATA_KEY)
            .cloned()
            .unwrap_or_default(),
        amount: match status {
            "succeeded" => intent.amount_received,
            _ => intent.amount,
        },
        // A bare payment intent carries no tax breakdown — that lives on the
        // invoice or the checkout session's total details. Zero here means
        // "not reported", not "no tax was charged".
        tax_amount: 0,
        currency: intent.currency.to_string().to_ascii_lowercase(),
        status: status.to_string(),
        description: intent.description.clone().unwrap_or_default(),
        receipt_url: String::new(),
        paid_at: (status == "succeeded").then_some(intent.created),
    }
}

/// What Stripe now holds about a customer, after they changed it themselves.
fn customer_state(customer: &Customer) -> CustomerState {
    CustomerState {
        stripe_customer_id: customer.id.to_string(),
        organization_id: customer
            .metadata
            .as_ref()
            .and_then(|m| m.get(ORG_METADATA_KEY).cloned())
            .unwrap_or_default(),
        email: customer.email.clone().unwrap_or_default(),
        name: customer.name.clone().unwrap_or_default(),
        tax_id: customer
            .tax_ids
            .as_ref()
            .and_then(|list| list.data.first())
            .and_then(|id| id.value.clone())
            .unwrap_or_default(),
        tax_country: customer
            .address
            .as_ref()
            .and_then(|address| address.country.clone())
            .unwrap_or_default(),
        details: serde_json::to_value(customer).unwrap_or(Value::Null),
    }
}

/// A currency as the app stores it: lowercase ISO 4217, empty when Stripe sent
/// none.
fn currency_code(currency: Option<stripe::Currency>) -> String {
    currency
        .map(|c| c.to_string().to_ascii_lowercase())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use apiplant_core::PaymentsConfig;

    fn payments(webhook_secret: &str) -> Payments {
        Payments::from_config(
            &PaymentsConfig {
                provider: "stripe".into(),
                secret_key: "sk_test_abc".into(),
                webhook_secret: webhook_secret.into(),
                ..PaymentsConfig::default()
            },
            "https://example.com/admin/#/billing",
        )
        .unwrap()
        .unwrap()
    }

    /// An unsigned request to this endpoint is somebody trying to grant
    /// themselves a plan, so it is refused before anything is parsed.
    #[test]
    fn an_unsigned_delivery_is_refused() {
        let payments = payments("whsec_test");
        let error = payments
            .verify_webhook(r#"{"id":"evt_1","type":"invoice.paid"}"#, "")
            .unwrap_err();
        assert!(matches!(error, PaymentsError::Signature(_)), "{error}");

        // A signature that is merely wrong fails the same way.
        let error = payments
            .verify_webhook(r#"{"id":"evt_1","type":"invoice.paid"}"#, "t=1,v1=deadbeef")
            .unwrap_err();
        assert!(matches!(error, PaymentsError::Signature(_)), "{error}");
    }

    /// Without a secret there is nothing to verify against, and accepting the
    /// delivery anyway would make the endpoint an unauthenticated write.
    #[test]
    fn no_configured_secret_means_no_delivery_is_accepted() {
        let error = payments("")
            .verify_webhook(r#"{"id":"evt_1"}"#, "t=1,v1=whatever")
            .unwrap_err();
        assert!(error.to_string().contains("webhook_secret"), "{error}");
    }

    /// Stripe sends every event the endpoint is subscribed to. Failing the
    /// ones we don't handle would have it retry them for three days.
    #[test]
    fn an_event_we_do_not_handle_is_ignored_rather_than_failed() {
        let change = classify(
            EventType::PayoutPaid,
            &EventObject::Payout(stripe::Payout::default()),
        );
        assert!(matches!(change, Change::Ignored));
    }

    /// A renewal arrives as both an invoice event and a payment-intent event.
    /// Recording both would double every subscription charge in the ledger.
    #[test]
    fn an_invoiced_payment_intent_is_left_to_the_invoice_event() {
        let mut intent = PaymentIntent::default();
        intent.invoice = Some(stripe::Expandable::Id(
            "in_123".parse().expect("a valid invoice id"),
        ));
        let change = classify(
            EventType::PaymentIntentSucceeded,
            &EventObject::PaymentIntent(intent),
        );
        assert!(matches!(change, Change::Ignored));

        // A one-off purchase has no invoice, and is recorded.
        let change = classify(
            EventType::PaymentIntentSucceeded,
            &EventObject::PaymentIntent(PaymentIntent::default()),
        );
        assert!(matches!(change, Change::Payment(_)));
    }

    /// Every subscription event is the same write, so a type we have never
    /// seen still updates the row rather than being dropped.
    #[test]
    fn any_subscription_event_reports_the_subscription_state() {
        for kind in [
            EventType::CustomerSubscriptionCreated,
            EventType::CustomerSubscriptionUpdated,
            EventType::CustomerSubscriptionDeleted,
            EventType::CustomerSubscriptionPaused,
            EventType::CustomerSubscriptionResumed,
            EventType::CustomerSubscriptionTrialWillEnd,
        ] {
            let change = classify(
                kind,
                &EventObject::Subscription(stripe::Subscription::default()),
            );
            assert!(matches!(change, Change::Subscription(_)), "{kind:?}");
        }
    }

    /// A failed invoice is recorded at its total and a succeeded one at what
    /// was actually taken — the two are not the same number on a partial
    /// payment, and the ledger should say what happened.
    #[test]
    fn a_failed_charge_is_recorded_as_well_as_a_successful_one() {
        let mut invoice = Invoice::default();
        invoice.total = Some(2400);
        invoice.amount_paid = Some(2400);
        invoice.tax = Some(400);

        let paid = invoice_payment(&invoice, "succeeded");
        assert_eq!(paid.amount, 2400);
        assert_eq!(paid.tax_amount, 400);
        assert_eq!(paid.status, "succeeded");

        let mut failed_invoice = invoice.clone();
        failed_invoice.amount_paid = Some(0);
        let failed = invoice_payment(&failed_invoice, "failed");
        assert_eq!(failed.amount, 2400, "a failure records what was owed");
        assert_eq!(failed.status, "failed");
    }
}
