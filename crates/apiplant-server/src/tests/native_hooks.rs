//! Hooks registered in Rust by an embedding program (`Platform::on`).

use std::sync::{Arc, Mutex};

use apiplant_core::HookEvent;
use ntex::http::header::CONTENT_TYPE;
use ntex::web::test;
use serde_json::{json, Value};

use super::*;
use crate::hooks::{HookCall, HookOutcome};

const NOTE: &str = r#"
[resource]
name = "note"

[permissions]
list = "member"
read = "member"
create = "member"
update = "member"
delete = "member"

[fields.title]
type = "string"
required = true
"#;

fn add(state: &mut AppState, resource: &str, event: HookEvent, hook: crate::hooks::NativeHook) {
    Arc::make_mut(&mut state.native_hooks).add(resource, event, hook);
}

#[ntex::test]
async fn native_hooks_replace_refuse_and_observe() {
    let db = TempDatabase::create("native_hooks").await;
    let root = temp_dir("native_hooks");
    write_files(
        &root,
        &[
            (
                "main.toml",
                &format!(
                    "\n[server]\nbase_path = \"/api\"\n\n[database]\nurl = \"{}\"\n",
                    db.url
                ),
            ),
            ("resources/note.toml", NOTE),
        ],
    );
    let mut state = load_state(&root).await;

    let seen: Arc<Mutex<Vec<String>>> = Arc::default();
    // Two before_create hooks: the first refuses or rewrites, the second sees
    // the rewrite.
    add(
        &mut state,
        "note",
        HookEvent::BeforeCreate,
        Arc::new(|call: HookCall| {
            Box::pin(async move {
                let title = call.payload["title"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string();
                if title == "forbidden" {
                    return HookOutcome::Abort {
                        status: 422,
                        message: "not that title".into(),
                    };
                }
                let mut body = call.payload.clone();
                body["title"] = json!(title.to_uppercase());
                HookOutcome::Replace(body)
            })
        }),
    );
    let order = seen.clone();
    add(
        &mut state,
        "note",
        HookEvent::BeforeCreate,
        Arc::new(move |call: HookCall| {
            let order = order.clone();
            Box::pin(async move {
                order
                    .lock()
                    .unwrap()
                    .push(format!("second saw {}", call.payload["title"]));
                HookOutcome::Continue
            })
        }),
    );
    let observed = seen.clone();
    add(
        &mut state,
        "organization",
        HookEvent::AfterCreate,
        Arc::new(move |call: HookCall| {
            let observed = observed.clone();
            Box::pin(async move {
                assert_eq!(call.context["event"], "after_create");
                assert_eq!(call.context["resource"], "organization");
                observed
                    .lock()
                    .unwrap()
                    .push(format!("org {}", call.payload["slug"]));
                HookOutcome::Continue
            })
        }),
    );
    let app = init_http_app!(state);

    let registration = read_json(
        test::call_service(
            &app,
            req_json(
                "POST",
                "/api/auth/register",
                json!({"email":"ada@example.com","password":"pw"}),
            ),
        )
        .await,
    )
    .await;
    let token = registration["token"].as_str().unwrap().to_string();
    let org = read_json(
        test::call_service(
            &app,
            bearer(
                test::TestRequest::post()
                    .uri("/api/organization")
                    .header(CONTENT_TYPE, "application/json")
                    .payload(json!({"name":"Acme","slug":"acme"}).to_string()),
                &token,
            )
            .to_request(),
        )
        .await,
    )
    .await;
    let org_id = org["id"].as_str().unwrap().to_string();

    let create = |title: &str| {
        bearer(
            test::TestRequest::post()
                .uri("/api/note")
                .header(CONTENT_TYPE, "application/json")
                .payload(json!({ "title": title }).to_string()),
            &token,
        )
        .header("x-organization", org_id.as_str())
        .to_request()
    };

    let created = test::call_service(&app, create("hello")).await;
    assert_eq!(created.status().as_u16(), 201);
    let created: Value = read_json(created).await;
    assert_eq!(
        created["title"], "HELLO",
        "the first hook's replacement is stored"
    );

    let refused = test::call_service(&app, create("forbidden")).await;
    assert_eq!(refused.status().as_u16(), 422);
    assert_eq!(read_json(refused).await["error"], "not that title");

    let seen = seen.lock().unwrap().clone();
    assert!(
        seen.contains(&"second saw \"HELLO\"".to_string()),
        "{seen:?}"
    );
    assert!(seen.contains(&"org \"acme\"".to_string()), "{seen:?}");
    assert!(
        !seen.iter().any(|s| s.contains("forbidden")),
        "an abort stops the chain: {seen:?}"
    );
}
