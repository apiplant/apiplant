//! Hearing back from Stripe.
//!
//! A checkout that completes tells the *buyer* something; it tells the app
//! nothing at all. The buyer's browser goes to `success_url` — a page they may
//! close, a redirect they may never follow — and the only thing that reliably
//! reports what was actually paid for is the webhook. Without it the
//! `billing_subscription` table stays empty while customers are billed.
//!
//! ## Verifying
//!
//! [`Payments::verify_webhook`] refuses anything without a valid signature
//! from `[payments] webhook_secret`. The endpoint is a public URL that edits
//! subscriptions; the signature is the entire reason it isn't a way to grant
//! yourself a plan.
//!
//! ## Reading
//!
//! Events are read as **JSON**, field by field, rather than deserialized into
//! the Stripe client's structs. That is a deliberate choice and it is about
//! versions.
//!
//! An account has an API version, and Stripe sends webhooks in the shape of
//! whichever version the *endpoint* was created under — which is nearly always
//! newer than the one a pinned client library was generated from. Those shapes
//! move: `current_period_end` left the subscription for its items, an invoice's
//! subscription moved under `parent`, `tax` became `total_taxes`. Struct
//! parsing turns each of those into a delivery that fails to parse, which is a
//! 400 back to Stripe, which is a subscription nobody records — silently,
//! because the money still moves.
//!
//! So the fields we actually use are read by name, each with the places it has
//! lived, and anything unrecognised is [`Change::Ignored`]. The cost is that
//! the compiler does not check these paths; the benefit is that upgrading a
//! Stripe account does not stop payments being recorded.
//!
//! ## Normalising
//!
//! Stripe has dozens of event types describing a handful of things that
//! actually happened. This module collapses them into [`Change`], so the
//! server writes rows from plain data and does not grow a match arm every time
//! Stripe adds an event — `customer.subscription.created`, `.updated`,
//! `.deleted`, `.paused` and `.resumed` are all one write: *this is the
//! subscription's state now*.

use hmac::{Hmac, Mac};
use serde_json::Value;
use sha2::Sha256;

use crate::types::{
    Change, CheckoutOutcome, CustomerState, Delivery, PaymentRecord, SubscriptionState,
};
use crate::{Payments, PaymentsError, ORG_METADATA_KEY, PRICE_METADATA_KEY};

/// How far apart the signature's timestamp and ours may be.
///
/// Stripe's own tolerance. It is what stops a delivery captured off the wire
/// from being replayed at leisure — the signature stays valid forever, the
/// timestamp does not.
const TIMESTAMP_TOLERANCE_SECS: i64 = 300;

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
        verify_signature(
            payload,
            signature.trim(),
            secret,
            chrono::Utc::now().timestamp(),
        )?;

        let event: Value = serde_json::from_str(payload)
            .map_err(|e| PaymentsError::Signature(format!("the delivery is not JSON: {e}")))?;

        let kind = text(&event, "type");
        let object = event
            .pointer("/data/object")
            .cloned()
            .unwrap_or(Value::Null);
        Ok(Delivery {
            id: text(&event, "id"),
            change: classify(&kind, &object),
            kind,
            payload: event,
        })
    }
}

/// Check the `Stripe-Signature` header against the payload.
///
/// The same computation Stripe documents: HMAC-SHA256 over `timestamp.payload`
/// keyed with the endpoint secret, compared in constant time, and refused if
/// the timestamp is too far from now.
fn verify_signature(
    payload: &str,
    header: &str,
    secret: &str,
    now: i64,
) -> Result<(), PaymentsError> {
    let mut timestamp = None;
    // A header may carry several `v1` signatures during a secret rotation, and
    // any one of them matching is a valid delivery.
    let mut signatures: Vec<&str> = Vec::new();
    for part in header.split(',') {
        match part.split_once('=') {
            Some(("t", value)) => timestamp = value.trim().parse::<i64>().ok(),
            Some(("v1", value)) => signatures.push(value.trim()),
            _ => {}
        }
    }

    let Some(timestamp) = timestamp else {
        return Err(PaymentsError::Signature(
            "the Stripe-Signature header carries no timestamp".into(),
        ));
    };
    if signatures.is_empty() {
        return Err(PaymentsError::Signature(
            "the Stripe-Signature header carries no v1 signature".into(),
        ));
    }
    if (now - timestamp).abs() > TIMESTAMP_TOLERANCE_SECS {
        return Err(PaymentsError::Signature(format!(
            "the delivery is signed {timestamp}, too far from now to accept"
        )));
    }

    let mut mac = Hmac::<Sha256>::new_from_slice(secret.as_bytes())
        .map_err(|_| PaymentsError::Signature("the webhook secret is unusable".into()))?;
    mac.update(format!("{timestamp}.{payload}").as_bytes());
    let expected = mac.finalize().into_bytes();

    let matched = signatures.iter().any(|signature| {
        hex::decode(signature)
            // `ct_eq` via the mac's own verify would consume it; comparing the
            // decoded bytes with a constant-time equality keeps the property
            // that matters — no early exit on the first differing byte.
            .map(|bytes| bytes.len() == expected.len() && constant_time_eq(&bytes, &expected))
            .unwrap_or(false)
    });
    match matched {
        true => Ok(()),
        false => Err(PaymentsError::Signature(
            "the delivery's signature does not match the webhook secret".into(),
        )),
    }
}

/// Compare two equal-length byte strings without leaking where they differ.
fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

/// Turn one Stripe event into the app's own terms.
///
/// The match is on the object's own `object` field as well as the event type:
/// the type names the moment, and the object carries the facts. An
/// unrecognised pairing is [`Change::Ignored`] rather than an error — Stripe
/// delivers every event the endpoint is subscribed to, and failing one we
/// don't handle would have Stripe retry it for three days.
fn classify(kind: &str, object: &Value) -> Change {
    match (kind, text(object, "object").as_str()) {
        ("checkout.session.completed", _) | ("checkout.session.async_payment_succeeded", _) => {
            Change::CheckoutCompleted(checkout_outcome(object))
        }

        // Every subscription event is the same write: this is its state now.
        (_, "subscription") => Change::Subscription(subscription_state(object)),

        // An invoice is what a subscription's renewal looks like. Both the
        // paid and the failed case are recorded: "the card was declined on the
        // 3rd" is the answer to most billing questions.
        //
        // Only `invoice.paid` and not `invoice.payment_succeeded`, though the
        // two nearly always arrive together for the same money. Newer API
        // versions send an invoice with no payment-intent id on it, which
        // leaves the row with no key to deduplicate on — so the pair would
        // become two rows saying the same charge happened twice. `paid` is the
        // one that means the invoice is settled, so it is the one kept.
        ("invoice.paid", _) => Change::Payment(invoice_payment(object, "succeeded")),
        ("invoice.payment_failed", _) => Change::Payment(invoice_payment(object, "failed")),

        // A one-off purchase. Invoiced payments arrive as invoice events too,
        // and are skipped here so a subscription renewal isn't recorded twice.
        ("payment_intent.succeeded", _) if !invoiced(object) => {
            Change::Payment(intent_payment(object, "succeeded"))
        }
        ("payment_intent.payment_failed", _) if !invoiced(object) => {
            Change::Payment(intent_payment(object, "failed"))
        }

        ("customer.created", _) | ("customer.updated", _) => {
            Change::Customer(customer_state(object))
        }

        _ => Change::Ignored,
    }
}

/// Whether a payment intent belongs to an invoice, and so will be reported by
/// the invoice event instead.
///
/// Two tests, because one stopped being enough. Until 2025 an invoice's intent
/// said so, in an `invoice` field; newer versions send neither the invoice's
/// intent id nor the intent's invoice id, so there is no link left to follow
/// and a subscription's charge would be recorded twice — once as its invoice,
/// once as a "one-off".
///
/// What still distinguishes them is *whose* intent it is. Anything bought
/// through this app's checkout carries its organisation in the metadata,
/// because [`Payments::checkout`] puts it there; an intent Stripe raised for an
/// invoice carries none. So an intent we cannot recognise is treated as
/// somebody else's — the invoice event is the record for it.
fn invoiced(intent: &Value) -> bool {
    !id_at(intent, "invoice").is_empty() || metadata(intent, ORG_METADATA_KEY).is_empty()
}

/// What a completed checkout session tells us.
fn checkout_outcome(session: &Value) -> CheckoutOutcome {
    CheckoutOutcome {
        session_id: text(session, "id"),
        stripe_customer_id: id_at(session, "customer"),
        stripe_subscription_id: id_at(session, "subscription"),
        stripe_payment_intent_id: id_at(session, "payment_intent"),
        organization_id: metadata(session, ORG_METADATA_KEY),
        price_id: metadata(session, PRICE_METADATA_KEY),
        customer_email: match session
            .pointer("/customer_details/email")
            .and_then(Value::as_str)
        {
            Some(email) => email.to_string(),
            None => text(session, "customer_email"),
        },
        amount_total: number(session, "amount_total").unwrap_or(0),
        currency: text(session, "currency").to_ascii_lowercase(),
    }
}

/// A subscription's current state.
pub(crate) fn subscription_state(subscription: &Value) -> SubscriptionState {
    let item = subscription.pointer("/items/data/0");

    SubscriptionState {
        stripe_subscription_id: text(subscription, "id"),
        stripe_customer_id: id_at(subscription, "customer"),
        stripe_price_id: item.map(|item| id_at(item, "price")).unwrap_or_default(),
        status: text(subscription, "status"),
        quantity: item.and_then(|item| number(item, "quantity")).unwrap_or(1),
        organization_id: metadata(subscription, ORG_METADATA_KEY),
        // Stripe moved this onto the items in 2025: a subscription can have
        // items on different cycles, so the period stopped being a property of
        // the whole thing. Both places are read, newest first, because the
        // answer to "when does this renew" must not depend on which API
        // version an account happens to be on.
        current_period_end: item
            .and_then(|item| number(item, "current_period_end"))
            .or_else(|| number(subscription, "current_period_end")),
        trial_end: number(subscription, "trial_end"),
        canceled_at: number(subscription, "canceled_at"),
        cancel_at_period_end: subscription
            .get("cancel_at_period_end")
            .and_then(Value::as_bool)
            .unwrap_or(false),
    }
}

/// A payment as an invoice describes it — the shape a subscription's renewals
/// arrive in.
fn invoice_payment(invoice: &Value, status: &str) -> PaymentRecord {
    PaymentRecord {
        // In newer versions the intent hangs off the invoice's payment records
        // rather than the invoice itself; an empty id is survivable, since the
        // invoice id identifies the row either way.
        stripe_payment_intent_id: id_at(invoice, "payment_intent"),
        stripe_invoice_id: text(invoice, "id"),
        stripe_customer_id: id_at(invoice, "customer"),
        // The subscription moved under `parent` when invoices grew other kinds
        // of parent; the old top-level field is still what most accounts send.
        stripe_subscription_id: match id_at(invoice, "subscription") {
            id if !id.is_empty() => id,
            _ => invoice
                .pointer("/parent/subscription_details/subscription")
                .map(id_of)
                .unwrap_or_default(),
        },
        organization_id: match metadata(invoice, ORG_METADATA_KEY) {
            org if !org.is_empty() => org,
            // A renewal's invoice carries no metadata of its own; the
            // subscription's is copied onto its details.
            _ => invoice
                .pointer("/parent/subscription_details/metadata")
                .and_then(|m| m.get(ORG_METADATA_KEY))
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
        },
        price_id: match metadata(invoice, PRICE_METADATA_KEY) {
            price if !price.is_empty() => price,
            _ => invoice
                .pointer("/parent/subscription_details/metadata")
                .and_then(|m| m.get(PRICE_METADATA_KEY))
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
        },
        // What was actually taken, not what was billed: a partly-paid invoice
        // says so, and a failed one is zero.
        amount: match status {
            "succeeded" => number(invoice, "amount_paid")
                .or_else(|| number(invoice, "total"))
                .unwrap_or(0),
            _ => number(invoice, "total").unwrap_or(0),
        },
        tax_amount: invoice_tax(invoice),
        currency: text(invoice, "currency").to_ascii_lowercase(),
        status: status.to_string(),
        description: text(invoice, "description"),
        // The hosted page is the link support gets asked for; the PDF is the
        // fallback for an invoice that has no hosted page.
        receipt_url: match text(invoice, "hosted_invoice_url") {
            url if !url.is_empty() => url,
            _ => text(invoice, "invoice_pdf"),
        },
        paid_at: invoice
            .pointer("/status_transitions/paid_at")
            .and_then(Value::as_i64),
    }
}

/// What of an invoice was tax.
///
/// `tax` was a single number until Stripe replaced it with a breakdown, which
/// can list more than one authority — a state and a city, say — and the total
/// is their sum rather than the first of them.
fn invoice_tax(invoice: &Value) -> i64 {
    if let Some(tax) = number(invoice, "tax") {
        return tax;
    }
    invoice
        .get("total_taxes")
        .and_then(Value::as_array)
        .map(|taxes| taxes.iter().filter_map(|tax| number(tax, "amount")).sum())
        .unwrap_or(0)
}

/// A payment as a payment intent describes it — the shape a one-off purchase
/// arrives in.
fn intent_payment(intent: &Value, status: &str) -> PaymentRecord {
    PaymentRecord {
        stripe_payment_intent_id: text(intent, "id"),
        stripe_invoice_id: String::new(),
        stripe_customer_id: id_at(intent, "customer"),
        stripe_subscription_id: String::new(),
        organization_id: metadata(intent, ORG_METADATA_KEY),
        price_id: metadata(intent, PRICE_METADATA_KEY),
        amount: match status {
            "succeeded" => number(intent, "amount_received").unwrap_or(0),
            _ => number(intent, "amount").unwrap_or(0),
        },
        // A bare payment intent carries no tax breakdown — that lives on the
        // invoice or the checkout session's total details. Zero here means
        // "not reported", not "no tax was charged".
        tax_amount: 0,
        currency: text(intent, "currency").to_ascii_lowercase(),
        status: status.to_string(),
        description: text(intent, "description"),
        receipt_url: String::new(),
        paid_at: (status == "succeeded").then(|| number(intent, "created").unwrap_or(0)),
    }
}

/// What Stripe now holds about a customer, after they changed it themselves.
fn customer_state(customer: &Value) -> CustomerState {
    CustomerState {
        stripe_customer_id: text(customer, "id"),
        organization_id: metadata(customer, ORG_METADATA_KEY),
        email: text(customer, "email"),
        name: text(customer, "name"),
        tax_id: customer
            .pointer("/tax_ids/data/0/value")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        tax_country: customer
            .pointer("/address/country")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        details: customer.clone(),
    }
}

// --- reading fields off an event ----------------------------------------
//
// Stripe's JSON is full of fields that are either an id or the whole object
// (an "expandable"), numbers that may be absent, and metadata that may be
// null. These four helpers absorb all of that so the mapping above reads as a
// list of facts rather than a list of unwrappings.

/// A string field, or `""` when it is absent or null.
fn text(value: &Value, key: &str) -> String {
    value
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// An integer field, or `None` when absent or null.
fn number(value: &Value, key: &str) -> Option<i64> {
    value.get(key).and_then(Value::as_i64)
}

/// The id at `key`, whether Stripe sent the id or the expanded object.
fn id_at(value: &Value, key: &str) -> String {
    value.get(key).map(id_of).unwrap_or_default()
}

/// The id of an expandable: the string itself, or the object's `id`.
fn id_of(value: &Value) -> String {
    match value {
        Value::String(id) => id.clone(),
        Value::Object(_) => text(value, "id"),
        _ => String::new(),
    }
}

/// One of our own metadata keys, or `""`.
fn metadata(value: &Value, key: &str) -> String {
    value
        .pointer("/metadata")
        .and_then(|metadata| metadata.get(key))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use apiplant_core::PaymentsConfig;
    use serde_json::json;

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

    /// Sign a payload the way Stripe does, so the verifier can be tested
    /// against something it should actually accept.
    fn sign(payload: &str, secret: &str, timestamp: i64) -> String {
        let mut mac = Hmac::<Sha256>::new_from_slice(secret.as_bytes()).unwrap();
        mac.update(format!("{timestamp}.{payload}").as_bytes());
        format!(
            "t={timestamp},v1={}",
            hex::encode(mac.finalize().into_bytes())
        )
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

    /// The signature Stripe actually sends is accepted, and the same bytes
    /// with one character changed are not.
    #[test]
    fn a_correctly_signed_delivery_is_accepted() {
        let payload = r#"{"id":"evt_1","type":"ping","data":{"object":{}}}"#;
        let now = 1_700_000_000;

        assert!(verify_signature(
            payload,
            &sign(payload, "whsec_test", now),
            "whsec_test",
            now
        )
        .is_ok());

        // Signed with a different secret.
        let wrong = sign(payload, "whsec_other", now);
        assert!(verify_signature(payload, &wrong, "whsec_test", now).is_err());

        // The right signature over a payload that has since been edited.
        let signature = sign(payload, "whsec_test", now);
        let tampered = r#"{"id":"evt_1","type":"ping","data":{"object":{"amount":9999}}}"#;
        assert!(verify_signature(tampered, &signature, "whsec_test", now).is_err());
    }

    /// The signature stays valid forever; the timestamp is what stops a
    /// captured delivery from being replayed tomorrow.
    #[test]
    fn a_stale_delivery_is_refused_however_well_signed() {
        let payload = r#"{"id":"evt_1","type":"ping"}"#;
        let signed_at = 1_700_000_000;
        let signature = sign(payload, "whsec_test", signed_at);

        assert!(verify_signature(payload, &signature, "whsec_test", signed_at + 60).is_ok());
        let error =
            verify_signature(payload, &signature, "whsec_test", signed_at + 3600).unwrap_err();
        assert!(matches!(error, PaymentsError::Signature(_)), "{error}");
    }

    /// A secret being rotated means two signatures in one header, and either
    /// one matching is a genuine delivery.
    #[test]
    fn one_matching_signature_among_several_is_enough() {
        let payload = r#"{"id":"evt_1"}"#;
        let now = 1_700_000_000;
        let good = sign(payload, "whsec_test", now);
        let header = format!("{good},v1=00ff00ff");
        assert!(verify_signature(payload, &header, "whsec_test", now).is_ok());
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
        let change = classify("payout.paid", &json!({ "object": "payout", "id": "po_1" }));
        assert!(matches!(change, Change::Ignored));
    }

    /// A renewal arrives as both an invoice event and a payment-intent event.
    /// Recording both would double every subscription charge in the ledger.
    #[test]
    fn an_invoiced_payment_intent_is_left_to_the_invoice_event() {
        // Older API versions say so outright.
        let change = classify(
            "payment_intent.succeeded",
            &json!({ "object": "payment_intent", "id": "pi_1", "invoice": "in_123" }),
        );
        assert!(matches!(change, Change::Ignored));

        // Newer ones do not, and the intent is recognised as somebody else's
        // by carrying none of our metadata.
        let change = classify(
            "payment_intent.succeeded",
            &json!({
                "object": "payment_intent", "id": "pi_1",
                "amount_received": 2900, "description": "Subscription creation",
            }),
        );
        assert!(matches!(change, Change::Ignored));

        // A one-off bought through this app's checkout carries its
        // organisation, and is recorded.
        let change = classify(
            "payment_intent.succeeded",
            &json!({
                "object": "payment_intent", "id": "pi_1", "amount_received": 1200,
                "metadata": { ORG_METADATA_KEY: "org-1", PRICE_METADATA_KEY: "price-row-1" },
            }),
        );
        match change {
            Change::Payment(payment) => {
                assert_eq!(payment.amount, 1200);
                assert_eq!(payment.organization_id, "org-1");
                assert_eq!(payment.price_id, "price-row-1");
            }
            other => panic!("expected a payment, got {other:?}"),
        }
    }

    /// `invoice.paid` and `invoice.payment_succeeded` describe one payment.
    /// Recording both would say the customer paid twice.
    #[test]
    fn one_settled_invoice_produces_one_payment() {
        let invoice = json!({ "object": "invoice", "id": "in_1", "amount_paid": 2900 });

        assert!(matches!(
            classify("invoice.paid", &invoice),
            Change::Payment(_)
        ));
        assert!(matches!(
            classify("invoice.payment_succeeded", &invoice),
            Change::Ignored
        ));
        // A failure is still its own event, and still recorded.
        assert!(matches!(
            classify("invoice.payment_failed", &invoice),
            Change::Payment(_)
        ));
    }

    /// Every subscription event is the same write, so a type we have never
    /// seen still updates the row rather than being dropped.
    #[test]
    fn any_subscription_event_reports_the_subscription_state() {
        for kind in [
            "customer.subscription.created",
            "customer.subscription.updated",
            "customer.subscription.deleted",
            "customer.subscription.paused",
            "customer.subscription.resumed",
            "customer.subscription.trial_will_end",
            // One Stripe has not invented yet.
            "customer.subscription.rescheduled",
        ] {
            let change = classify(kind, &json!({ "object": "subscription", "id": "sub_1" }));
            assert!(matches!(change, Change::Subscription(_)), "{kind}");
        }
    }

    /// The whole reason this module reads JSON: the same subscription, in the
    /// shape two different API versions send it, has to produce the same row.
    #[test]
    fn a_subscription_reads_the_same_from_either_api_version() {
        let old = json!({
            "object": "subscription",
            "id": "sub_1",
            "customer": "cus_1",
            "status": "active",
            "current_period_end": 1_760_000_000_i64,
            "cancel_at_period_end": false,
            "metadata": { ORG_METADATA_KEY: "org-1" },
            "items": { "data": [{ "quantity": 2, "price": { "id": "price_1" } }] },
        });
        // 2025 and later: the period belongs to the item, and the customer
        // arrives expanded rather than as an id.
        let new = json!({
            "object": "subscription",
            "id": "sub_1",
            "customer": { "id": "cus_1", "object": "customer" },
            "status": "active",
            "cancel_at_period_end": false,
            "metadata": { ORG_METADATA_KEY: "org-1" },
            "items": { "data": [{
                "quantity": 2,
                "current_period_end": 1_760_000_000_i64,
                "price": { "id": "price_1" },
            }] },
        });

        for object in [old, new] {
            let state = subscription_state(&object);
            assert_eq!(state.stripe_subscription_id, "sub_1");
            assert_eq!(state.stripe_customer_id, "cus_1");
            assert_eq!(state.stripe_price_id, "price_1");
            assert_eq!(state.status, "active");
            assert_eq!(state.quantity, 2);
            assert_eq!(state.organization_id, "org-1");
            assert_eq!(state.current_period_end, Some(1_760_000_000));
        }
    }

    /// Likewise for an invoice: the subscription it renews and the tax it
    /// charged both moved, and both still have to be found.
    #[test]
    fn an_invoice_reads_the_same_from_either_api_version() {
        let old = json!({
            "object": "invoice",
            "id": "in_1",
            "customer": "cus_1",
            "subscription": "sub_1",
            "total": 2400, "amount_paid": 2400, "tax": 400,
            "currency": "eur",
            "metadata": { ORG_METADATA_KEY: "org-1" },
        });
        let new = json!({
            "object": "invoice",
            "id": "in_1",
            "customer": "cus_1",
            "total": 2400, "amount_paid": 2400,
            "total_taxes": [{ "amount": 300 }, { "amount": 100 }],
            "currency": "eur",
            "parent": { "subscription_details": {
                "subscription": "sub_1",
                "metadata": { ORG_METADATA_KEY: "org-1" },
            }},
        });

        for object in [old, new] {
            let payment = invoice_payment(&object, "succeeded");
            assert_eq!(payment.stripe_invoice_id, "in_1");
            assert_eq!(payment.stripe_subscription_id, "sub_1");
            assert_eq!(payment.organization_id, "org-1");
            assert_eq!(payment.amount, 2400);
            assert_eq!(payment.tax_amount, 400);
        }
    }

    /// A failed invoice is recorded at its total and a succeeded one at what
    /// was actually taken — the two are not the same number on a partial
    /// payment, and the ledger should say what happened.
    #[test]
    fn a_failed_charge_is_recorded_as_well_as_a_successful_one() {
        let invoice = json!({
            "object": "invoice", "id": "in_1",
            "total": 2400, "amount_paid": 2400, "tax": 400,
        });

        let paid = invoice_payment(&invoice, "succeeded");
        assert_eq!(paid.amount, 2400);
        assert_eq!(paid.tax_amount, 400);
        assert_eq!(paid.status, "succeeded");

        let mut failed_invoice = invoice.clone();
        failed_invoice["amount_paid"] = json!(0);
        let failed = invoice_payment(&failed_invoice, "failed");
        assert_eq!(failed.amount, 2400, "a failure records what was owed");
        assert_eq!(failed.status, "failed");
    }
}
