//! Payments: what exists, who may buy, and what the webhook will accept.
//!
//! Nothing here talks to Stripe. Every case is a decision the server makes
//! *before* it would — is this route mounted, is this caller an admin, is this
//! delivery signed — which is where the interesting failures are anyway: a
//! test that a checkout session gets created tests Stripe, and a test that a
//! non-admin cannot start one tests us.

use super::*;

/// An app that takes money. The key is a syntactically valid test key that
/// nothing is ever authenticated with, because no case here gets that far.
fn paying_app(db_url: &str) -> String {
    format!(
        r#"
[server]
base_path = "/api"
public_url = "https://example.test"

[database]
url = "{db_url}"

[payments]
provider = "stripe"
secret_key = "sk_test_never_used"
publishable_key = "pk_test_public"
webhook_secret = "whsec_test"
currency = "eur"
"#
    )
}

fn plain_app(db_url: &str) -> String {
    format!("[server]\nbase_path = \"/api\"\n\n[database]\nurl = \"{db_url}\"\n")
}

/// Register somebody, put them in an organisation with `role`, and return
/// their session token and the organisation's id.
/// An app that names no provider has neither the endpoints nor the tables.
///
/// This is the whole shape of the feature. Five tables and a price list is a
/// lot to hand an app that will never bill anybody, so it gets none of it.
#[ntex::test]
async fn without_a_provider_there_is_no_billing_at_all() {
    let db = TempDatabase::create("nopay").await;
    let root = temp_dir("nopay");
    write_files(&root, &[("main.toml", &plain_app(&db.url))]);

    let state = load_state(&root).await;
    assert!(!state.payments_enabled());
    for resource in [
        "billing_product",
        "billing_price",
        "billing_customer",
        "billing_subscription",
        "billing_payment",
        "billing_event",
    ] {
        assert!(
            !state.app.resources.contains_key(resource),
            "`{resource}` exists in an app that takes no money"
        );
    }

    let app = init_http_app!(state);
    for path in [
        "/api/billing/checkout",
        "/api/billing/portal",
        "/api/billing/webhook",
    ] {
        let resp = test::call_service(&app, req_json("POST", path, json!({}))).await;
        // 405 rather than 404 where the path falls through to the generic
        // `/{resource}/{id}` matcher, which has no POST — see the equivalent
        // case in `email_auth`. Either way nothing here is handled.
        assert!(
            matches!(resp.status().as_u16(), 404 | 405),
            "{path} was handled by a server that takes no payments"
        );
    }

    db.cleanup().await;
    fs::remove_dir_all(root).unwrap();
}

/// Turning payments on brings the whole catalogue with it, migrated and
/// queryable like any other resource.
#[ntex::test]
async fn a_provider_brings_the_billing_resources_and_routes() {
    let db = TempDatabase::create("pay").await;
    let root = temp_dir("pay");
    write_files(&root, &[("main.toml", &paying_app(&db.url))]);

    let state = load_state(&root).await;
    assert!(state.payments_enabled());
    for resource in ["billing_product", "billing_price", "billing_subscription"] {
        assert!(state.app.resources.contains_key(resource), "{resource}");
    }

    let app = init_http_app!(state);

    // The price list is public: a pricing page is read by people who have no
    // account, which is the point of selling anything.
    let resp = test::call_service(
        &app,
        test::TestRequest::get()
            .uri("/api/billing_price")
            .to_request(),
    )
    .await;
    assert_eq!(resp.status().as_u16(), 200);

    db.cleanup().await;
    fs::remove_dir_all(root).unwrap();
}

/// `GET /billing/config` is public, and says nothing a browser shouldn't have.
#[ntex::test]
async fn the_public_config_carries_the_publishable_key_and_not_the_secret() {
    let db = TempDatabase::create("payconf").await;
    let root = temp_dir("payconf");
    write_files(&root, &[("main.toml", &paying_app(&db.url))]);
    let app = init_http_app!(load_state(&root).await);

    let resp = test::call_service(
        &app,
        test::TestRequest::get()
            .uri("/api/billing/config")
            .to_request(),
    )
    .await;
    assert_eq!(resp.status().as_u16(), 200);
    let body = read_json(resp).await;

    assert_eq!(body["provider"], "stripe");
    assert_eq!(body["publishable_key"], "pk_test_public");
    assert_eq!(body["currency"], "eur");
    assert_eq!(body["webhooks_configured"], true);
    // The secret key must not appear anywhere in a public response.
    assert!(!body.to_string().contains("sk_test"), "{body}");

    db.cleanup().await;
    fs::remove_dir_all(root).unwrap();
}

/// Buying commits an organisation's card to a charge, so it takes an admin —
/// and it takes an organisation, which the caller has to name.
#[ntex::test]
async fn only_an_admin_of_a_named_organization_can_start_a_checkout() {
    let db = TempDatabase::create("paycheckout").await;
    let root = temp_dir("paycheckout");
    write_files(&root, &[("main.toml", &paying_app(&db.url))]);
    let state = load_state(&root).await;
    let (member_token, org) = member_with_role(&state, "member@example.test", "member").await;
    let app = init_http_app!(state);

    let body = json!({ "price_id": Uuid::new_v4().to_string() });

    // Anonymous.
    let resp = test::call_service(
        &app,
        req_json("POST", "/api/billing/checkout", body.clone()),
    )
    .await;
    assert_eq!(resp.status().as_u16(), 401);

    // Authenticated, but naming no organisation: there is nothing to bill.
    let resp = test::call_service(
        &app,
        bearer(
            test::TestRequest::post()
                .uri("/api/billing/checkout")
                .header(CONTENT_TYPE, "application/json")
                .set_payload(body.to_string()),
            &member_token,
        )
        .to_request(),
    )
    .await;
    assert_eq!(resp.status().as_u16(), 400);

    // In the organisation, but not an admin of it.
    let resp = test::call_service(
        &app,
        bearer(
            test::TestRequest::post()
                .uri("/api/billing/checkout")
                .header(CONTENT_TYPE, "application/json")
                .header("x-organization", org.as_str())
                .set_payload(body.to_string()),
            &member_token,
        )
        .to_request(),
    )
    .await;
    assert_eq!(resp.status().as_u16(), 403);

    db.cleanup().await;
    fs::remove_dir_all(root).unwrap();
}

/// An admin buying a price that doesn't exist is told so — and, crucially,
/// this happens before anything reaches Stripe.
#[ntex::test]
async fn an_admin_buying_an_unknown_price_gets_a_404_and_no_provider_call() {
    let db = TempDatabase::create("payprice").await;
    let root = temp_dir("payprice");
    write_files(&root, &[("main.toml", &paying_app(&db.url))]);
    let state = load_state(&root).await;
    let (token, org) = member_with_role(&state, "boss@example.test", "admin").await;
    let app = init_http_app!(state);

    for (price_id, expected) in [
        // Not a uuid at all.
        ("nonsense", 422),
        // Well-formed and absent.
        (Uuid::new_v4().to_string().as_str(), 404),
    ] {
        let resp = test::call_service(
            &app,
            bearer(
                test::TestRequest::post()
                    .uri("/api/billing/checkout")
                    .header(CONTENT_TYPE, "application/json")
                    .header("x-organization", org.as_str())
                    .set_payload(json!({ "price_id": price_id }).to_string()),
                &token,
            )
            .to_request(),
        )
        .await;
        assert_eq!(resp.status().as_u16(), expected, "price_id {price_id}");
    }

    db.cleanup().await;
    fs::remove_dir_all(root).unwrap();
}

/// The webhook endpoint is a public URL that edits subscriptions. The
/// signature is the only thing standing between it and somebody granting
/// themselves a plan, so an unsigned delivery is refused and nothing is
/// written.
#[ntex::test]
async fn an_unsigned_webhook_is_refused_and_records_nothing() {
    let db = TempDatabase::create("payhook").await;
    let root = temp_dir("payhook");
    write_files(&root, &[("main.toml", &paying_app(&db.url))]);
    let state = load_state(&root).await;
    let events = state.table("billing_event").unwrap();
    let app = init_http_app!(state.clone());

    let delivery = json!({
        "id": "evt_forged",
        "type": "customer.subscription.created",
        "data": { "object": { "id": "sub_forged", "status": "active" } },
    });

    for signature in ["", "t=1,v1=deadbeef"] {
        let mut request = test::TestRequest::post()
            .uri("/api/billing/webhook")
            .header(CONTENT_TYPE, "application/json")
            .set_payload(delivery.to_string());
        if !signature.is_empty() {
            request = request.header("stripe-signature", signature);
        }
        let resp = test::call_service(&app, request.to_request()).await;
        assert_eq!(resp.status().as_u16(), 400, "signature {signature:?}");
    }

    let rows = state
        .db
        .raw_json(&format!("SELECT id FROM {events}"), &[])
        .await
        .unwrap();
    assert_eq!(
        rows.as_array().map(Vec::len).unwrap_or_default(),
        0,
        "a refused delivery left a row behind"
    );

    db.cleanup().await;
    fs::remove_dir_all(root).unwrap();
}

/// The catalogue is admin-only to write, and public to read — the two halves
/// of selling something through an API.
#[ntex::test]
async fn the_catalogue_is_public_to_read_and_admin_to_change() {
    let db = TempDatabase::create("paycat").await;
    let root = temp_dir("paycat");
    write_files(&root, &[("main.toml", &paying_app(&db.url))]);
    let state = load_state(&root).await;
    let (member_token, org) = member_with_role(&state, "member@example.test", "member").await;
    let app = init_http_app!(state);

    // Anonymous reads are fine.
    let resp = test::call_service(
        &app,
        test::TestRequest::get()
            .uri("/api/billing_product")
            .to_request(),
    )
    .await;
    assert_eq!(resp.status().as_u16(), 200);

    // A non-admin cannot add a plan.
    let resp = test::call_service(
        &app,
        bearer(
            test::TestRequest::post()
                .uri("/api/billing_product")
                .header(CONTENT_TYPE, "application/json")
                .header("x-organization", org.as_str())
                .set_payload(json!({ "name": "Pro" }).to_string()),
            &member_token,
        )
        .to_request(),
    )
    .await;
    assert_eq!(resp.status().as_u16(), 403);

    db.cleanup().await;
    fs::remove_dir_all(root).unwrap();
}
