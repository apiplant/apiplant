//! The `<base>/billing` endpoints, and the webhook that keeps the
//! `billing_*` tables true.
//!
//! Four routes, mounted only in an app whose `[payments]` section names a
//! provider:
//!
//! | Route | Who | What it does |
//! |-------|-----|--------------|
//! | `GET  /billing/config` | anyone | the publishable key and what checkout will do — enough for a front end to render a pricing page |
//! | `POST /billing/checkout` | an org **admin** | starts a purchase, answers with Stripe's URL |
//! | `POST /billing/portal` | an org **admin** | a link to Stripe's self-service billing screens |
//! | `POST /billing/webhook` | Stripe | the only thing that writes what has been paid for |
//!
//! ## Why buying is `role:admin`
//!
//! Because paying is not a thing you do to yourself, it is a thing you do to
//! an organisation's card. A member who can start a subscription can commit
//! their employer to a recurring charge, and every billing system that allowed
//! that learned not to. The catalogue is public, the checkout is not.
//!
//! ## Why the webhook writes and nothing else does
//!
//! `billing_subscription` and `billing_payment` are `private` resources: no
//! CRUD endpoint writes them. What is paid for is Stripe's fact, and a row
//! claiming otherwise is worse than no row — it is an entitlement somebody
//! granted themselves. The webhook is the one path, and it is signed.

use apiplant_payments::{
    Change, CheckoutOutcome, CheckoutSpec, CustomerSpec, CustomerState, Delivery, PaymentRecord,
    SubscriptionState,
};
use ntex::web::types::{Json, State};
use ntex::web::{HttpRequest, HttpResponse};
use serde_json::{json, Map, Value};
use uuid::Uuid;

use crate::response::{error, ok};
use crate::state::AppState;

/// `GET <base>/billing/config` — what a front end needs to sell something.
///
/// Public on purpose. Everything here is either already in the browser
/// (the publishable key is designed to be) or a fact about the shape of the
/// checkout — the currency, whether tax is added, whether a VAT number is
/// asked for. A pricing page is read by people who have no account, which is
/// also why `billing_product` and `billing_price` are `public` to read.
pub async fn config(state: State<AppState>) -> HttpResponse {
    let Some(payments) = &state.payments else {
        return error(404, "this app does not take payments");
    };
    let config = payments.config();
    ok(&json!({
        "provider": payments.provider().as_str(),
        "publishable_key": payments.publishable_key(),
        "currency": config.default_currency(),
        "automatic_tax": config.automatic_tax,
        "tax_id_collection": config.collects_tax_ids(),
        // A front end that knows this is false knows why nothing it buys ever
        // shows up, which is otherwise a very confusing afternoon.
        "webhooks_configured": payments.webhooks_enabled(),
    }))
}

/// `POST <base>/billing/checkout` — start a purchase.
///
/// Body: `{ "price_id": "<billing_price row>", "quantity": 1 }`, plus optional
/// `success_url` / `cancel_url`. The organisation comes from `X-Organization`,
/// like every other org-scoped request, and the caller must administer it.
///
/// Answers `{ "url": … }`. Redirect the buyer there; everything after that
/// happens on Stripe's domain and comes back through the webhook.
pub async fn checkout(
    req: HttpRequest,
    state: State<AppState>,
    body: Json<Map<String, Value>>,
) -> HttpResponse {
    let Some(payments) = state.payments.clone() else {
        return error(404, "this app does not take payments");
    };
    let (principal, org) = match admin_of_active_org(&req, &state).await {
        Ok(caller) => caller,
        Err(response) => return response,
    };

    let body = body.into_inner();
    let Some(price_id) = string(&body, "price_id") else {
        return error(422, "`price_id` is required — the billing_price to buy");
    };

    let price = match load_price(&state, &price_id).await {
        Ok(Some(price)) => price,
        Ok(None) => return error(404, "no such price"),
        Err(response) => return response,
    };
    if !price.active {
        return error(409, "that price is no longer on sale");
    }
    let Some(stripe_price_id) = price.stripe_price_id.clone() else {
        // The row exists but was never mirrored — the hook failed, or the row
        // predates payments being switched on. Buying it would charge nothing.
        return error(
            409,
            "that price has not been created in Stripe yet; save it again to sync it",
        );
    };

    // Reuse the organisation's customer when it has one, so a second purchase
    // lands on the same card, the same invoices and the same tax status.
    let existing = match load_customer(&state, org).await {
        Ok(customer) => customer,
        Err(response) => return response,
    };
    let email = principal_email(&state, principal).await;

    let spec = CheckoutSpec {
        stripe_price_id,
        price_id: price.id.to_string(),
        recurring: price.recurring,
        quantity: body.get("quantity").and_then(Value::as_u64).unwrap_or(1),
        stripe_customer_id: existing
            .as_ref()
            .map(|c| c.stripe_customer_id.clone())
            .unwrap_or_default(),
        customer: CustomerSpec {
            email: email.clone(),
            organization_id: org.to_string(),
            ..CustomerSpec::default()
        },
        organization_id: org.to_string(),
        trial_days: price.trial_days,
        success_url: string(&body, "success_url").unwrap_or_default(),
        cancel_url: string(&body, "cancel_url").unwrap_or_default(),
        allow_promotion_codes: body
            .get("allow_promotion_codes")
            .and_then(Value::as_bool)
            .unwrap_or(false),
    };

    let started = match payments.checkout(spec).await {
        Ok(started) => started,
        Err(e) => return payment_error(e),
    };

    // Record the customer now rather than waiting for the webhook. The buyer
    // may abandon the page — and then come back — and an organisation that
    // gets a second Stripe customer on its second attempt has split its
    // billing history in two.
    if existing.is_none() {
        upsert_customer(
            &state,
            org,
            &CustomerState {
                stripe_customer_id: started.stripe_customer_id.clone(),
                organization_id: org.to_string(),
                email,
                ..CustomerState::default()
            },
        )
        .await;
    }

    ok(&json!({
        "url": started.url,
        "session_id": started.session_id,
        "mode": started.mode,
    }))
}

/// `POST <base>/billing/portal` — a link to Stripe's self-service billing.
///
/// This is where a customer changes their card, downloads invoices, updates
/// their VAT number or cancels — none of which the app has to implement, and
/// all of which comes back as webhooks.
pub async fn portal(
    req: HttpRequest,
    state: State<AppState>,
    body: Json<Map<String, Value>>,
) -> HttpResponse {
    let Some(payments) = state.payments.clone() else {
        return error(404, "this app does not take payments");
    };
    let (_, org) = match admin_of_active_org(&req, &state).await {
        Ok(caller) => caller,
        Err(response) => return response,
    };

    let customer = match load_customer(&state, org).await {
        Ok(Some(customer)) => customer,
        // Nothing has ever been bought, so there is no billing to manage.
        // A 404 here is the honest answer and a clearer one than an empty
        // Stripe portal would be.
        Ok(None) => return error(404, "this organization has no billing to manage yet"),
        Err(response) => return response,
    };

    let return_url = string(&body.into_inner(), "return_url").unwrap_or_default();
    match payments
        .portal(&customer.stripe_customer_id, &return_url)
        .await
    {
        Ok(url) => ok(&json!({ "url": url })),
        Err(e) => payment_error(e),
    }
}

/// `POST <base>/billing/webhook` — Stripe telling us what actually happened.
///
/// Unauthenticated in the ordinary sense and authenticated in the only sense
/// that matters here: the body carries a signature over its own bytes, made
/// with `[payments] webhook_secret`, and an unverified delivery is refused
/// before it is parsed.
///
/// Answers `200` for anything it has recorded, *including* an event it does
/// nothing with. A non-2xx tells Stripe to retry, and retrying an event we
/// will never handle achieves nothing but a backlog.
pub async fn webhook(
    req: HttpRequest,
    state: State<AppState>,
    body: ntex::util::Bytes,
) -> HttpResponse {
    let Some(payments) = state.payments.clone() else {
        return error(404, "this app does not take payments");
    };

    // The signature covers the bytes Stripe sent. Parsing and re-serialising
    // the body — even into identical-looking JSON — would break it, which is
    // why this handler takes raw bytes and not `Json`.
    let Ok(payload) = std::str::from_utf8(&body) else {
        return error(400, "the webhook body is not valid UTF-8");
    };
    let signature = req
        .headers()
        .get("stripe-signature")
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default();

    let delivery = match payments.verify_webhook(payload, signature) {
        Ok(delivery) => delivery,
        Err(e) => {
            // Deliberately terse. A verification failure is either a
            // misconfigured secret or somebody probing, and neither deserves
            // to be told which.
            tracing::warn!(error = %e, "refused an unverified stripe webhook");
            return error(400, "signature verification failed");
        }
    };

    match record_delivery(&state, &delivery).await {
        Outcome::Fresh => {}
        Outcome::AlreadySeen => {
            // Stripe retries until it gets a 2xx, and may deliver the same
            // event twice regardless. Doing the work once is what stops a
            // retried `invoice.paid` becoming a second row in the ledger.
            tracing::debug!(event = %delivery.id, "webhook already processed");
            return ok(&json!({ "received": true, "duplicate": true }));
        }
        Outcome::Unrecorded => {
            // The ledger table is missing or unwritable. Without it there is
            // no idempotency, so refuse rather than risk double-applying —
            // Stripe will retry, and by then the table may be back.
            return error(500, "could not record the event");
        }
    }

    let applied = apply(&state, &delivery.change).await;
    finish_delivery(&state, &delivery.id, applied.as_ref().err()).await;

    match applied {
        Ok(()) => ok(&json!({ "received": true })),
        Err(message) => {
            // Answering 500 asks Stripe to retry, which is what we want: the
            // event is real and we failed to write it down.
            tracing::error!(event = %delivery.id, kind = %delivery.kind, %message, "failed to apply a webhook");
            error(500, "could not apply the event")
        }
    }
}

/// Write the change a delivery describes into the app's own tables.
async fn apply(state: &AppState, change: &Change) -> Result<(), String> {
    match change {
        Change::CheckoutCompleted(outcome) => apply_checkout(state, outcome).await,
        Change::Subscription(subscription) => apply_subscription(state, subscription).await,
        Change::Payment(payment) => apply_payment(state, payment).await,
        Change::Customer(customer) => {
            let Some(org) = parse_org(&customer.organization_id) else {
                return Ok(());
            };
            upsert_customer(state, org, customer).await;
            Ok(())
        }
        Change::Ignored => Ok(()),
    }
}

/// A completed checkout: make sure the organisation's customer row exists and
/// points at the right Stripe customer.
///
/// The subscription and payment that came out of the session arrive as their
/// own events, so this deliberately does not write them — it establishes the
/// link those events need, which is the one thing only the session knows.
async fn apply_checkout(state: &AppState, outcome: &CheckoutOutcome) -> Result<(), String> {
    let Some(org) = parse_org(&outcome.organization_id) else {
        // A session started somewhere other than this app — a payment link, a
        // Stripe dashboard test. There is no tenant to attach it to.
        tracing::warn!(
            session = %outcome.session_id,
            "a checkout completed with no organisation in its metadata; ignoring"
        );
        return Ok(());
    };
    upsert_customer(
        state,
        org,
        &CustomerState {
            stripe_customer_id: outcome.stripe_customer_id.clone(),
            organization_id: outcome.organization_id.clone(),
            email: outcome.customer_email.clone(),
            ..CustomerState::default()
        },
    )
    .await;
    Ok(())
}

/// A subscription's state, as Stripe now has it.
async fn apply_subscription(state: &AppState, sub: &SubscriptionState) -> Result<(), String> {
    let Some(table) = state.table("billing_subscription") else {
        return Ok(());
    };
    let org = match resolve_org(state, &sub.organization_id, &sub.stripe_customer_id).await {
        Some(org) => org,
        None => {
            tracing::warn!(
                subscription = %sub.stripe_subscription_id,
                "a subscription event names no organisation we know; ignoring"
            );
            return Ok(());
        }
    };
    let customer = customer_row_id(state, &sub.stripe_customer_id).await;
    let price = price_row_id(state, &sub.stripe_price_id).await;

    // One statement, so a renewal that arrives twice cannot produce two rows:
    // `stripe_subscription_id` is unique, and the conflict is an update.
    let sql = format!(
        "INSERT INTO {table} \
           (organization_id, customer_id, price_id, status, quantity, \
            current_period_end, cancel_at_period_end, trial_ends_at, canceled_at, \
            stripe_subscription_id) \
         VALUES ($1::uuid, $2::uuid, $3::uuid, $4, $5, \
                 to_timestamp($6), $7, to_timestamp($8), to_timestamp($9), $10) \
         ON CONFLICT (stripe_subscription_id) DO UPDATE SET \
           organization_id = EXCLUDED.organization_id, \
           customer_id = COALESCE(EXCLUDED.customer_id, {table}.customer_id), \
           price_id = COALESCE(EXCLUDED.price_id, {table}.price_id), \
           status = EXCLUDED.status, \
           quantity = EXCLUDED.quantity, \
           current_period_end = EXCLUDED.current_period_end, \
           cancel_at_period_end = EXCLUDED.cancel_at_period_end, \
           trial_ends_at = EXCLUDED.trial_ends_at, \
           canceled_at = EXCLUDED.canceled_at"
    );
    let params = vec![
        json!(org.to_string()),
        optional_id(customer),
        optional_id(price),
        json!(sub.status),
        json!(sub.quantity),
        seconds(sub.current_period_end),
        json!(sub.cancel_at_period_end),
        seconds(sub.trial_end),
        seconds(sub.canceled_at),
        json!(sub.stripe_subscription_id),
    ];
    state
        .db
        .raw_json(&sql, &params)
        .await
        .map(|_| ())
        .map_err(|e| e.to_string())
}

/// One movement of money, successful or not.
async fn apply_payment(state: &AppState, payment: &PaymentRecord) -> Result<(), String> {
    let Some(table) = state.table("billing_payment") else {
        return Ok(());
    };
    let org = match resolve_org(state, &payment.organization_id, &payment.stripe_customer_id).await
    {
        Some(org) => org,
        None => {
            tracing::warn!(
                intent = %payment.stripe_payment_intent_id,
                invoice = %payment.stripe_invoice_id,
                "a payment event names no organisation we know; ignoring"
            );
            return Ok(());
        }
    };
    let customer = customer_row_id(state, &payment.stripe_customer_id).await;
    let subscription = subscription_row_id(state, &payment.stripe_subscription_id).await;

    // A payment with no intent id — some invoices have none until they are
    // paid — can't be deduplicated on it, so it is inserted plainly. The
    // event ledger is what stops the delivery being applied twice.
    let sql = if payment.stripe_payment_intent_id.is_empty() {
        format!(
            "INSERT INTO {table} \
               (organization_id, customer_id, subscription_id, amount, tax_amount, \
                currency, status, description, receipt_url, paid_at, stripe_invoice_id) \
             VALUES ($1::uuid, $2::uuid, $3::uuid, $4, $5, $6, $7, $8, $9, to_timestamp($10), $11)"
        )
    } else {
        format!(
            "INSERT INTO {table} \
               (organization_id, customer_id, subscription_id, amount, tax_amount, \
                currency, status, description, receipt_url, paid_at, stripe_invoice_id, \
                stripe_payment_intent_id) \
             VALUES ($1::uuid, $2::uuid, $3::uuid, $4, $5, $6, $7, $8, $9, to_timestamp($10), $11, $12) \
             ON CONFLICT (stripe_payment_intent_id) DO UPDATE SET \
               status = EXCLUDED.status, \
               amount = EXCLUDED.amount, \
               tax_amount = EXCLUDED.tax_amount, \
               receipt_url = EXCLUDED.receipt_url, \
               paid_at = EXCLUDED.paid_at"
        )
    };

    let mut params = vec![
        json!(org.to_string()),
        optional_id(customer),
        optional_id(subscription),
        json!(payment.amount),
        json!(payment.tax_amount),
        json!(payment.currency),
        json!(payment.status),
        json!(payment.description),
        json!(payment.receipt_url),
        seconds(payment.paid_at),
        json!(payment.stripe_invoice_id),
    ];
    if !payment.stripe_payment_intent_id.is_empty() {
        params.push(json!(payment.stripe_payment_intent_id));
    }

    state
        .db
        .raw_json(&sql, &params)
        .await
        .map(|_| ())
        .map_err(|e| e.to_string())
}

/// Create or update the organisation's `billing_customer` row.
///
/// Failures are logged rather than raised: this is a copy of a Stripe fact,
/// and losing the copy is not a reason to make Stripe redeliver a subscription
/// that was written correctly.
async fn upsert_customer(state: &AppState, org: Uuid, customer: &CustomerState) {
    let Some(table) = state.table("billing_customer") else {
        return;
    };
    if customer.stripe_customer_id.is_empty() {
        return;
    }
    let sql = format!(
        "INSERT INTO {table} \
           (organization_id, stripe_customer_id, email, name, tax_id, tax_country, details) \
         VALUES ($1::uuid, $2, $3, $4, $5, $6, $7::jsonb) \
         ON CONFLICT (stripe_customer_id) DO UPDATE SET \
           email = COALESCE(NULLIF(EXCLUDED.email, ''), {table}.email), \
           name = COALESCE(NULLIF(EXCLUDED.name, ''), {table}.name), \
           tax_id = COALESCE(NULLIF(EXCLUDED.tax_id, ''), {table}.tax_id), \
           tax_country = COALESCE(NULLIF(EXCLUDED.tax_country, ''), {table}.tax_country), \
           details = EXCLUDED.details"
    );
    let params = vec![
        json!(org.to_string()),
        json!(customer.stripe_customer_id),
        json!(customer.email),
        json!(customer.name),
        json!(customer.tax_id),
        json!(customer.tax_country),
        json!(customer.details.to_string()),
    ];
    if let Err(error) = state.db.raw_json(&sql, &params).await {
        tracing::warn!(%error, "could not record the billing customer");
    }
}

// --- the event ledger ---------------------------------------------------

/// Whether a delivery is ours to process.
enum Outcome {
    /// Newly recorded — do the work.
    Fresh,
    /// Seen before; the work is already done.
    AlreadySeen,
    /// We could not record it, so we must not act on it.
    Unrecorded,
}

/// Claim a delivery by inserting its id, and say whether we got it first.
///
/// The insert *is* the lock: `stripe_event_id` is unique, so two workers
/// handed the same retry race to the same row and exactly one of them wins.
async fn record_delivery(state: &AppState, delivery: &Delivery) -> Outcome {
    let Some(table) = state.table("billing_event") else {
        return Outcome::Unrecorded;
    };
    let sql = format!(
        "INSERT INTO {table} (stripe_event_id, kind, payload) \
         VALUES ($1, $2, $3::jsonb) \
         ON CONFLICT (stripe_event_id) DO NOTHING \
         RETURNING id::text AS id"
    );
    let params = vec![
        json!(delivery.id),
        json!(delivery.kind),
        json!(delivery.payload.to_string()),
    ];
    match state.db.raw_json(&sql, &params).await {
        // No returned row means the conflict fired: somebody else has it.
        Ok(rows) => match rows.as_array().is_some_and(|rows| rows.is_empty()) {
            true => Outcome::AlreadySeen,
            false => Outcome::Fresh,
        },
        Err(error) => {
            tracing::error!(%error, "could not record a webhook delivery");
            Outcome::Unrecorded
        }
    }
}

/// Close a delivery off: stamp it processed, or record why it wasn't.
///
/// A `billing_event` row with a null `processed_at` and an `error` is an event
/// that arrived and failed, which is exactly what to look at when a customer
/// says they paid and the app disagrees.
async fn finish_delivery(state: &AppState, event_id: &str, failure: Option<&String>) {
    let Some(table) = state.table("billing_event") else {
        return;
    };
    let (sql, params) = match failure {
        None => (
            format!(
                "UPDATE {table} SET processed_at = now(), error = NULL WHERE stripe_event_id = $1"
            ),
            vec![json!(event_id)],
        ),
        Some(message) => (
            // `processed_at` stays null so a retry is taken seriously — but
            // the id is already claimed, so the retry will find it and stop.
            // Clearing the claim on failure would be the other design, and it
            // trades a stuck event for a double-charge; this way round the
            // failure is visible and nothing is applied twice.
            format!("UPDATE {table} SET error = $2 WHERE stripe_event_id = $1"),
            vec![json!(event_id), json!(message)],
        ),
    };
    if let Err(error) = state.db.raw_json(&sql, &params).await {
        tracing::warn!(%error, "could not close off a webhook delivery");
    }
}

// --- lookups ------------------------------------------------------------

/// A `billing_price` row, in the terms a checkout needs.
struct PriceRow {
    id: Uuid,
    stripe_price_id: Option<String>,
    recurring: bool,
    trial_days: u32,
    active: bool,
}

async fn load_price(state: &AppState, id: &str) -> Result<Option<PriceRow>, HttpResponse> {
    let Some(table) = state.table("billing_price") else {
        return Err(error(404, "this app has no price list"));
    };
    let Ok(id) = Uuid::parse_str(id.trim()) else {
        return Err(error(422, "`price_id` must be the id of a billing_price"));
    };
    let sql = format!(
        "SELECT id::text AS id, stripe_price_id, interval, trial_days, active \
         FROM {table} WHERE id = $1::uuid LIMIT 1"
    );
    let rows = match state.db.raw_json(&sql, &[json!(id.to_string())]).await {
        Ok(rows) => rows,
        Err(e) => return Err(crate::response::db_error(e)),
    };
    let Some(row) = rows.as_array().and_then(|rows| rows.first()) else {
        return Ok(None);
    };
    Ok(Some(PriceRow {
        id,
        stripe_price_id: row
            .get("stripe_price_id")
            .and_then(Value::as_str)
            .map(str::to_string)
            .filter(|id| !id.is_empty()),
        recurring: apiplant_payments::Interval::parse(
            row.get("interval").and_then(Value::as_str).unwrap_or(""),
        )
        .is_recurring(),
        trial_days: row
            .get("trial_days")
            .and_then(Value::as_i64)
            .unwrap_or(0)
            .clamp(0, u32::MAX as i64) as u32,
        active: row.get("active").and_then(Value::as_bool).unwrap_or(true),
    }))
}

/// The organisation's Stripe customer, if it has ever bought anything.
async fn load_customer(state: &AppState, org: Uuid) -> Result<Option<CustomerState>, HttpResponse> {
    let Some(table) = state.table("billing_customer") else {
        return Ok(None);
    };
    let sql = format!(
        "SELECT stripe_customer_id, email, name FROM {table} \
         WHERE organization_id = $1::uuid LIMIT 1"
    );
    let rows = match state.db.raw_json(&sql, &[json!(org.to_string())]).await {
        Ok(rows) => rows,
        Err(e) => return Err(crate::response::db_error(e)),
    };
    Ok(rows
        .as_array()
        .and_then(|rows| rows.first())
        .map(|row| CustomerState {
            stripe_customer_id: text(row, "stripe_customer_id"),
            organization_id: org.to_string(),
            email: text(row, "email"),
            name: text(row, "name"),
            ..CustomerState::default()
        }))
}

/// The row id of a `billing_customer` by its Stripe id.
async fn customer_row_id(state: &AppState, stripe_customer_id: &str) -> Option<Uuid> {
    row_id_by(
        state,
        "billing_customer",
        "stripe_customer_id",
        stripe_customer_id,
    )
    .await
}

async fn price_row_id(state: &AppState, stripe_price_id: &str) -> Option<Uuid> {
    row_id_by(state, "billing_price", "stripe_price_id", stripe_price_id).await
}

async fn subscription_row_id(state: &AppState, stripe_subscription_id: &str) -> Option<Uuid> {
    row_id_by(
        state,
        "billing_subscription",
        "stripe_subscription_id",
        stripe_subscription_id,
    )
    .await
}

/// One row's id, looked up by a unique Stripe id.
async fn row_id_by(state: &AppState, resource: &str, column: &str, value: &str) -> Option<Uuid> {
    if value.trim().is_empty() {
        return None;
    }
    let table = state.table(resource)?;
    let sql = format!("SELECT id::text AS id FROM {table} WHERE {column} = $1 LIMIT 1");
    let rows = state.db.raw_json(&sql, &[json!(value)]).await.ok()?;
    rows.as_array()
        .and_then(|rows| rows.first())
        .and_then(|row| row.get("id"))
        .and_then(Value::as_str)
        .and_then(|id| Uuid::parse_str(id).ok())
}

/// Which organisation an event belongs to.
///
/// The metadata is the primary answer and the customer row is the fallback,
/// because an object created before this app was wired up — or by hand in the
/// Stripe dashboard — carries no metadata but may still be a customer we know.
async fn resolve_org(
    state: &AppState,
    organization_id: &str,
    stripe_customer_id: &str,
) -> Option<Uuid> {
    if let Some(org) = parse_org(organization_id) {
        return Some(org);
    }
    let table = state.table("billing_customer")?;
    let sql = format!(
        "SELECT organization_id::text AS org FROM {table} WHERE stripe_customer_id = $1 LIMIT 1"
    );
    let rows = state
        .db
        .raw_json(&sql, &[json!(stripe_customer_id)])
        .await
        .ok()?;
    rows.as_array()
        .and_then(|rows| rows.first())
        .and_then(|row| row.get("org"))
        .and_then(Value::as_str)
        .and_then(|id| Uuid::parse_str(id).ok())
}

/// The caller's own address, for a Stripe customer that has none yet.
async fn principal_email(state: &AppState, principal: Uuid) -> String {
    let Some(table) = state.table("user") else {
        return String::new();
    };
    let field = crate::auth_routes::quote(&crate::auth_routes::auth_spec(state).identity_field);
    let sql = format!("SELECT {field} AS identity FROM {table} WHERE id = $1::uuid LIMIT 1");
    state
        .db
        .raw_json(&sql, &[json!(principal.to_string())])
        .await
        .ok()
        .and_then(|rows| {
            rows.as_array()
                .and_then(|rows| rows.first())
                .and_then(|row| row.get("identity"))
                .and_then(Value::as_str)
                .map(str::to_string)
        })
        .unwrap_or_default()
}

// --- small shared pieces ------------------------------------------------

/// Resolve the caller and insist they administer the organisation they named.
///
/// Both halves matter. Without an organisation there is nothing to bill; and
/// a member who is not an admin can otherwise commit their employer to a
/// recurring charge, which is the failure mode every billing system has had.
async fn admin_of_active_org(
    req: &HttpRequest,
    state: &AppState,
) -> Result<(Uuid, Uuid), HttpResponse> {
    let principal = state.resolve_principal(req).await;
    let Some(caller) = principal.clone() else {
        return Err(error(401, "authentication required"));
    };
    let Some(org) = state.active_org(req, &principal) else {
        return Err(error(
            400,
            "name the organization being billed in the X-Organization header",
        ));
    };
    if !caller.has_role_in(org, "admin") {
        return Err(error(
            403,
            "only an admin of this organization can manage billing",
        ));
    }
    Ok((caller.user_id, org))
}

/// Turn a payments error into the response it deserves.
///
/// A bad request is the caller's (422); everything else is ours or Stripe's,
/// and the caller is told only that it failed — a provider message can name a
/// key, an account or an internal id.
fn payment_error(e: apiplant_payments::PaymentsError) -> HttpResponse {
    use apiplant_payments::PaymentsError::*;
    match e {
        Request(message) => error(422, message),
        other => {
            crate::telemetry::record_error("payments", &other);
            tracing::error!(error = %other, "a payments call failed");
            error(502, "the payment provider could not be reached")
        }
    }
}

fn string(body: &Map<String, Value>, key: &str) -> Option<String> {
    body.get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

fn text(row: &Value, key: &str) -> String {
    row.get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

fn parse_org(value: &str) -> Option<Uuid> {
    Uuid::parse_str(value.trim()).ok()
}

/// A nullable id parameter: `NULL` rather than an empty string, so the column
/// keeps its foreign key.
fn optional_id(id: Option<Uuid>) -> Value {
    match id {
        Some(id) => json!(id.to_string()),
        None => Value::Null,
    }
}

/// A Unix timestamp for `to_timestamp()`, or `NULL`.
fn seconds(value: Option<i64>) -> Value {
    match value {
        Some(seconds) => json!(seconds),
        None => Value::Null,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_missing_timestamp_is_null_rather_than_the_epoch() {
        assert_eq!(seconds(None), Value::Null);
        assert_eq!(seconds(Some(0)), json!(0));
        assert_eq!(optional_id(None), Value::Null);
    }

    #[test]
    fn blank_body_fields_read_as_absent() {
        let mut body = Map::new();
        body.insert("success_url".into(), json!("   "));
        body.insert("cancel_url".into(), json!("https://example.com"));
        assert_eq!(string(&body, "success_url"), None);
        assert_eq!(
            string(&body, "cancel_url").as_deref(),
            Some("https://example.com")
        );
        assert_eq!(string(&body, "missing"), None);
    }

    /// Only a request error is the caller's fault; a provider message can
    /// name a key or an account and must not be echoed back.
    #[test]
    fn provider_failures_are_not_relayed_to_the_caller() {
        use apiplant_payments::PaymentsError;
        assert_eq!(
            payment_error(PaymentsError::Request("no price".into()))
                .status()
                .as_u16(),
            422
        );
        assert_eq!(
            payment_error(PaymentsError::Provider("sk_live_abc is invalid".into()))
                .status()
                .as_u16(),
            502
        );
    }
}
