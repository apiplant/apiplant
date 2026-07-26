//! Schema evolution: migrations against a table that already has rows, and
//! apps that override the built-in models with their own.

use super::*;

#[ntex::test]
async fn migrations_are_additive_for_existing_rows() {
    let db = TempDatabase::create("migrate").await;
    let root = temp_dir("migrate");
    write_files(
        &root,
        &[
            (
                "main.toml",
                &format!(
                    r#"
[database]
url = "{}"
"#,
                    db.url
                ),
            ),
            (
                "models/note.toml",
                r#"
[resource]
name = "note"
scope = "global"

[fields.title]
type = "string"
required = true
"#,
            ),
        ],
    );

    let app_v1 = App::load(&root).unwrap();
    let db_conn = Db::connect(&db.url, 4).await.unwrap();
    apiplant_db::migrate(db_conn.connection(), &app_v1)
        .await
        .unwrap();
    let note_v1 = app_v1.resources.get("note").unwrap();
    db_conn
        .create(
            note_v1,
            &serde_json::Map::from_iter([("title".to_string(), Value::String("first".into()))]),
        )
        .await
        .unwrap();

    write_files(
        &root,
        &[(
            "models/note.toml",
            r#"
[resource]
name = "note"
scope = "global"

[fields.title]
type = "string"
required = true

[fields.status]
type = "string"
required = true
default = "draft"
"#,
        )],
    );

    let app_v2 = App::load(&root).unwrap();
    apiplant_db::migrate(db_conn.connection(), &app_v2)
        .await
        .unwrap();
    apiplant_db::migrate(db_conn.connection(), &app_v2)
        .await
        .unwrap();

    let note_v2 = app_v2.resources.get("note").unwrap();
    let rows = db_conn.list(note_v2, &[], 10, 0).await.unwrap();
    let row = &rows.as_array().unwrap()[0];
    assert_eq!(row["title"], "first");
    assert_eq!(row["status"], "draft");

    fs::remove_dir_all(root).unwrap();
    db.cleanup().await;
}

#[ntex::test]
async fn complex_overrides_and_mixed_permission_models_work_together() {
    let db = TempDatabase::create("complex").await;
    let root = temp_dir("complex");
    write_files(
        &root,
        &[
            (
                "main.toml",
                &format!(
                    r#"
[server]
base_path = "/api"

[database]
url = "{}"

[docs]
path = "/reference"
title = "Complex App"
"#,
                    db.url
                ),
            ),
            (
                "models/users.toml",
                r#"
[resource]
name = "user"
scope = "global"

[permissions]
list = "authenticated"
read = "owner"
create = "public"
update = "owner"
delete = "private"

[auth]
identity_field = "username"
password_field = "secret_hash"

[fields.username]
type = "string"
required = true
unique = true

[fields.secret_hash]
type = "string"
hidden = true

[fields.display_name]
type = "string"
"#,
            ),
            (
                "models/organization.toml",
                r#"
[resource]
name = "organization"
scope = "global"

[permissions]
list = "member"
read = "member"
create = "authenticated"
update = "role:admin"
delete = "role:admin"

[fields.name]
type = "string"
required = true

[fields.slug]
type = "string"
required = true
unique = true

[fields.tier]
type = "string"
required = true
default = "standard"
"#,
            ),
            (
                "models/membership.toml",
                r#"
[resource]
name = "membership"
scope = "organization"

[permissions]
list = "member"
read = "member"
create = "role:admin"
update = "role:admin"
delete = "role:admin"

[fields.user_id]
type = "reference"
references = "user"
required = true

[fields.organization_id]
type = "reference"
references = "organization"
required = true

[fields.role]
type = "string"

[fields.team]
type = "string"
"#,
            ),
            (
                "models/api_key.toml",
                r#"
[resource]
name = "api_key"
scope = "global"

[permissions]
list = "owner"
read = "owner"
create = "authenticated"
update = "private"
delete = "owner"

[fields.name]
type = "string"

[fields.note]
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
"#,
            ),
            (
                "models/news.toml",
                r#"
[resource]
name = "news"
scope = "global"

[permissions]
list = "public"
read = "public"
create = "private"
update = "private"
delete = "private"

[fields.headline]
type = "string"
required = true
"#,
            ),
            (
                "models/bulletin.toml",
                r#"
[resource]
name = "bulletin"
scope = "global"

[permissions]
list = "authenticated"
read = "authenticated"
create = "private"
update = "private"
delete = "private"

[fields.title]
type = "string"
required = true
"#,
            ),
            (
                "models/preference.toml",
                r#"
[resource]
name = "preference"
scope = "global"

[permissions]
list = "owner"
read = "owner"
create = "authenticated"
update = "owner"
delete = "owner"

[fields.key]
type = "string"
required = true

[fields.value]
type = "string"

[fields.owner_id]
type = "reference"
references = "user"
required = true
"#,
            ),
            (
                "models/project.toml",
                r#"
[resource]
name = "project"

[permissions]
list = "member"
read = "member"
create = "member"
update = "owner"
delete = "role:admin"

[fields.name]
type = "string"
required = true

[fields.owner_id]
type = "reference"
references = "user"
"#,
            ),
            (
                "models/deployment.toml",
                r#"
[resource]
name = "deployment"

[permissions]
list = "member"
read = "member"
create = "role:ops"
update = "role:ops"
delete = "role:admin"

[fields.project_id]
type = "reference"
references = "project"
required = true

[fields.owner_id]
type = "reference"
references = "user"

[fields.status]
type = "string"
default = "queued"
"#,
            ),
        ],
    );

    let state = load_state(&root).await;
    let app = init_http_app!(state);

    let anon_news =
        test::call_service(&app, test::TestRequest::get().uri("/api/news").to_request()).await;
    assert_eq!(anon_news.status().as_u16(), 200);
    assert_eq!(read_json(anon_news).await, json!([]));

    let anon_bulletin = test::call_service(
        &app,
        test::TestRequest::get().uri("/api/bulletin").to_request(),
    )
    .await;
    assert_eq!(anon_bulletin.status().as_u16(), 401);

    let anon_news_create = test::call_service(
        &app,
        req_json("POST", "/api/news", json!({"headline":"launch"})),
    )
    .await;
    assert_eq!(anon_news_create.status().as_u16(), 404);

    let alice_reg = test::call_service(
        &app,
        req_json(
            "POST",
            "/api/auth/register",
            json!({"username":"alice","password":"pw","display_name":"Alice"}),
        ),
    )
    .await;
    let alice = read_json(alice_reg).await;
    let alice_token = alice["token"].as_str().unwrap().to_string();
    let alice_id = alice["user"]["id"].as_str().unwrap().to_string();

    let alice_login = test::call_service(
        &app,
        req_json(
            "POST",
            "/api/auth/login",
            json!({"username":"alice","password":"pw"}),
        ),
    )
    .await;
    assert_eq!(alice_login.status().as_u16(), 200);

    let org_resp = test::call_service(
        &app,
        bearer(
            test::TestRequest::post()
                .uri("/api/organization")
                .header(CONTENT_TYPE, "application/json")
                .set_payload(json!({"name":"Acme","slug":"acme","tier":"enterprise"}).to_string()),
            &alice_token,
        )
        .to_request(),
    )
    .await;
    assert_eq!(org_resp.status().as_u16(), 201);
    let organization = read_json(org_resp).await;
    let org_id = organization["id"].as_str().unwrap().to_string();
    assert_eq!(organization["tier"], "enterprise");

    let bulletin_auth = test::call_service(
        &app,
        bearer(test::TestRequest::get().uri("/api/bulletin"), &alice_token).to_request(),
    )
    .await;
    assert_eq!(bulletin_auth.status().as_u16(), 200);

    let pref_resp = test::call_service(
        &app,
        bearer(
            test::TestRequest::post()
                .uri("/api/preference")
                .header(CONTENT_TYPE, "application/json")
                .set_payload(
                    json!({"key":"theme","value":"dark","owner_id":Uuid::new_v4()}).to_string(),
                ),
            &alice_token,
        )
        .to_request(),
    )
    .await;
    assert_eq!(pref_resp.status().as_u16(), 201);
    let preference = read_json(pref_resp).await;
    let preference_id = preference["id"].as_str().unwrap().to_string();
    assert_eq!(preference["owner_id"], alice_id);

    let project_resp = test::call_service(
        &app,
        bearer(
            test::TestRequest::post()
                .uri("/api/project")
                .header(CONTENT_TYPE, "application/json")
                .set_payload(json!({"name":"alpha"}).to_string()),
            &alice_token,
        )
        .to_request(),
    )
    .await;
    assert_eq!(project_resp.status().as_u16(), 201);
    let project = read_json(project_resp).await;
    let project_id = project["id"].as_str().unwrap().to_string();

    let bob_reg = test::call_service(
        &app,
        req_json(
            "POST",
            "/api/auth/register",
            json!({"username":"bob","password":"pw","display_name":"Bob"}),
        ),
    )
    .await;
    let bob = read_json(bob_reg).await;
    let bob_token = bob["token"].as_str().unwrap().to_string();
    let bob_id = bob["user"]["id"].as_str().unwrap().to_string();

    let carol_reg = test::call_service(
        &app,
        req_json(
            "POST",
            "/api/auth/register",
            json!({"username":"carol","password":"pw","display_name":"Carol"}),
        ),
    )
    .await;
    let carol = read_json(carol_reg).await;
    let carol_token = carol["token"].as_str().unwrap().to_string();
    let carol_id = carol["user"]["id"].as_str().unwrap().to_string();

    let bob_membership = test::call_service(
        &app,
        bearer(
            test::TestRequest::post()
                .uri("/api/membership")
                .header(CONTENT_TYPE, "application/json")
                .set_payload(json!({"user_id":bob_id,"role":"ops","team":"platform"}).to_string()),
            &alice_token,
        )
        .to_request(),
    )
    .await;
    assert_eq!(bob_membership.status().as_u16(), 201);
    let bob_membership = read_json(bob_membership).await;
    assert_eq!(bob_membership["organization_id"], org_id);
    assert_eq!(bob_membership["team"], "platform");

    let carol_membership = test::call_service(
        &app,
        bearer(
            test::TestRequest::post()
                .uri("/api/membership")
                .header(CONTENT_TYPE, "application/json")
                .set_payload(
                    json!({"user_id":carol_id,"role":"member","team":"support"}).to_string(),
                ),
            &alice_token,
        )
        .to_request(),
    )
    .await;
    assert_eq!(carol_membership.status().as_u16(), 201);

    let bob_api_key = test::call_service(
        &app,
        bearer(
            test::TestRequest::post()
                .uri("/api/auth/apikeys")
                .header(CONTENT_TYPE, "application/json")
                .set_payload(json!({"name":"ops-bot"}).to_string()),
            &bob_token,
        )
        .to_request(),
    )
    .await;
    assert_eq!(bob_api_key.status().as_u16(), 201);
    let bob_api_key = read_json(bob_api_key).await;
    let bob_key = bob_api_key["api_key"].as_str().unwrap().to_string();

    let deployment_resp = test::call_service(
        &app,
        test::TestRequest::post()
            .uri("/api/deployment")
            .header("x-api-key", bob_key.as_str())
            .header("x-organization", org_id.as_str())
            .header(CONTENT_TYPE, "application/json")
            .set_payload(json!({"project_id":project_id}).to_string())
            .to_request(),
    )
    .await;
    assert_eq!(deployment_resp.status().as_u16(), 201);
    let deployment = read_json(deployment_resp).await;
    assert_eq!(deployment["owner_id"], bob_id);
    assert_eq!(deployment["status"], "queued");

    let carol_deployment = test::call_service(
        &app,
        bearer(
            test::TestRequest::post()
                .uri("/api/deployment")
                .header("x-organization", org_id.as_str())
                .header(CONTENT_TYPE, "application/json")
                .set_payload(json!({"project_id":project_id}).to_string()),
            &carol_token,
        )
        .to_request(),
    )
    .await;
    assert_eq!(carol_deployment.status().as_u16(), 403);

    let bob_delete_project = test::call_service(
        &app,
        test::TestRequest::delete()
            .uri(&format!("/api/project/{project_id}"))
            .header("authorization", format!("ApiKey {bob_key}"))
            .header("x-organization", org_id.as_str())
            .to_request(),
    )
    .await;
    assert_eq!(bob_delete_project.status().as_u16(), 403);

    let bob_membership_write = test::call_service(
        &app,
        bearer(
            test::TestRequest::post()
                .uri("/api/membership")
                .header("x-organization", org_id.as_str())
                .header(CONTENT_TYPE, "application/json")
                .set_payload(json!({"user_id":alice_id,"role":"member"}).to_string()),
            &bob_token,
        )
        .to_request(),
    )
    .await;
    assert_eq!(bob_membership_write.status().as_u16(), 403);

    let alice_prefs = test::call_service(
        &app,
        bearer(
            test::TestRequest::get().uri("/api/preference"),
            &alice_token,
        )
        .to_request(),
    )
    .await;
    assert_eq!(alice_prefs.status().as_u16(), 200);
    assert_eq!(read_json(alice_prefs).await.as_array().unwrap().len(), 1);

    let bob_prefs = test::call_service(
        &app,
        bearer(test::TestRequest::get().uri("/api/preference"), &bob_token).to_request(),
    )
    .await;
    assert_eq!(bob_prefs.status().as_u16(), 200);
    assert_eq!(read_json(bob_prefs).await, json!([]));

    let bob_pref_read = test::call_service(
        &app,
        bearer(
            test::TestRequest::get().uri(&format!("/api/preference/{preference_id}")),
            &bob_token,
        )
        .to_request(),
    )
    .await;
    assert_eq!(bob_pref_read.status().as_u16(), 404);

    let docs_resp = test::call_service(
        &app,
        test::TestRequest::get().uri("/api/reference").to_request(),
    )
    .await;
    assert_eq!(docs_resp.status().as_u16(), 200);

    let spec_resp = test::call_service(
        &app,
        test::TestRequest::get()
            .uri("/api/openapi.json")
            .to_request(),
    )
    .await;
    assert_eq!(spec_resp.status().as_u16(), 200);
    let spec = read_json(spec_resp).await;
    assert_eq!(spec["info"]["title"], "Complex App");
    assert_eq!(spec["paths"]["/news"]["get"].get("security"), None);
    assert!(spec["paths"]["/bulletin"]["get"]["security"].is_array());
    assert_eq!(
        spec["paths"]["/deployment"]["post"]["description"],
        "Requires the `ops` role in the active organisation."
    );

    fs::remove_dir_all(root).unwrap();
    db.cleanup().await;
}
