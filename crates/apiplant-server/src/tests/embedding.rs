//! apiplant as a library: an embedder's routes beside the generated API, with
//! the same caller resolution and permission grammar.

use super::*;
use crate::state::State;
use crate::Caller;
use ntex::web::{HttpRequest, HttpResponse};

/// A host route: who are you, and are you a member of where you stand?
async fn whoami(caller: Caller, state: State<AppState>, req: HttpRequest) -> HttpResponse {
    if let Err(refusal) = caller.require(&state, &req, "member").await {
        return refusal;
    }
    HttpResponse::Ok().json(&json!({
        "user": caller.user_id().map(|id| id.to_string()),
        "org": caller.active_org.map(|id| id.to_string()),
        "admin": caller.has_role("admin"),
    }))
}

#[ntex::test]
async fn host_routes_share_the_session_and_the_permission_grammar() {
    let db = TempDatabase::create("embedding").await;
    let root = temp_dir("embedding");
    write_files(
        &root,
        &[(
            "main.toml",
            &format!(
                "[server]\nbase_path = \"/api/platform\"\n\n[database]\nurl = \"{}\"\n",
                db.url
            ),
        )],
    );
    let state = load_state(&root).await;
    let lookup = state.clone();
    let app = test::init_service_st(
        crate::state::Shared(std::sync::Arc::new(state.clone())),
        build_app!(state, |cfg: &mut ntex::web::ServiceConfig<
            crate::state::Shared,
        >| {
            cfg.route("/host/whoami", ntex::web::get().to(whoami));
        }),
    )
    .await;

    // Anonymous: the grammar's 401.
    let resp = test::call_service(
        &app,
        test::TestRequest::get().uri("/host/whoami").to_request(),
    )
    .await;
    assert_eq!(resp.status().as_u16(), 401);

    let resp = test::call_service(
        &app,
        req_json(
            "POST",
            "/api/platform/auth/register",
            json!({"email":"ana@acme.test","password":"pw"}),
        ),
    )
    .await;
    assert_eq!(resp.status().as_u16(), 201);
    let body = read_json(resp).await;
    let token = body["token"].as_str().unwrap().to_string();

    // Signed in but standing nowhere: `member` needs an organisation.
    let resp = test::call_service(
        &app,
        bearer(test::TestRequest::get().uri("/host/whoami"), &token).to_request(),
    )
    .await;
    assert_eq!(resp.status().as_u16(), 403);

    // Standing in their personal organisation, of which they are admin.
    let user = body["user"]["id"].as_str().unwrap().to_string();
    let rows = lookup
        .db
        .raw_json(
            &format!(
                "SELECT organization_id::text AS org FROM {} WHERE user_id = $1::uuid",
                lookup.table("membership").unwrap()
            ),
            &[json!(user)],
        )
        .await
        .unwrap();
    let org = rows[0]["org"].as_str().unwrap().to_string();
    let resp = test::call_service(
        &app,
        bearer(test::TestRequest::get().uri("/host/whoami"), &token)
            .header("x-organization", org.as_str())
            .to_request(),
    )
    .await;
    assert_eq!(resp.status().as_u16(), 200);
    let who = read_json(resp).await;
    assert_eq!(who["org"], org);
    assert_eq!(who["admin"], true);

    // The generated API is still there, under its own base path.
    let resp = test::call_service(
        &app,
        test::TestRequest::get()
            .uri("/api/platform/_health")
            .to_request(),
    )
    .await;
    assert_eq!(resp.status().as_u16(), 200);

    db.cleanup().await;
}
