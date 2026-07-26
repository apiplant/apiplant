//! Registration, login, and API keys.

use super::*;

#[ntex::test]
async fn register_login_and_api_keys_work_with_custom_identity_fields() {
    let db = TempDatabase::create("auth").await;
    let root = temp_dir("auth");
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
password_field = "password_hash"

[fields.username]
type = "string"
required = true
unique = true

[fields.password_hash]
type = "string"
hidden = true

[fields.display_name]
type = "string"
"#,
            ),
        ],
    );

    let state = load_state(&root).await;
    let app = init_http_app!(state);

    let resp = test::call_service(
        &app,
        req_json(
            "POST",
            "/api/auth/register",
            json!({"username":"alice","password":"hunter2","display_name":"Alice"}),
        ),
    )
    .await;
    assert_eq!(resp.status().as_u16(), 201);
    let registered = read_json(resp).await;
    let token = registered["token"].as_str().unwrap().to_string();
    let user_id = registered["user"]["id"].as_str().unwrap().to_string();
    assert_eq!(registered["user"]["username"], "alice");
    assert!(registered["user"].get("password_hash").is_none());

    let resp = test::call_service(
        &app,
        req_json(
            "POST",
            "/api/auth/login",
            json!({"username":"alice","password":"hunter2"}),
        ),
    )
    .await;
    assert_eq!(resp.status().as_u16(), 200);
    let login = read_json(resp).await;
    assert!(login["token"].as_str().unwrap().len() > 10);

    let resp = test::call_service(
        &app,
        bearer(
            test::TestRequest::post()
                .uri("/api/auth/apikeys")
                .header(CONTENT_TYPE, "application/json")
                .set_payload(json!({"name":"ci"}).to_string()),
            &token,
        )
        .to_request(),
    )
    .await;
    assert_eq!(resp.status().as_u16(), 201);
    let api_key = read_json(resp).await;
    let plaintext = api_key["api_key"].as_str().unwrap();

    let resp = test::call_service(
        &app,
        test::TestRequest::get()
            .uri(&format!("/api/user/{user_id}"))
            .header("x-api-key", plaintext)
            .to_request(),
    )
    .await;
    assert_eq!(resp.status().as_u16(), 200);
    let via_x_header = read_json(resp).await;
    assert_eq!(via_x_header["id"], user_id);
    assert!(via_x_header.get("password_hash").is_none());

    let resp = test::call_service(
        &app,
        test::TestRequest::get()
            .uri(&format!("/api/user/{user_id}"))
            .header("authorization", format!("ApiKey {plaintext}"))
            .to_request(),
    )
    .await;
    assert_eq!(resp.status().as_u16(), 200);

    fs::remove_dir_all(root).unwrap();
    db.cleanup().await;
}

#[ntex::test]
async fn registration_can_be_disabled() {
    let db = TempDatabase::create("auth-disabled").await;
    let root = temp_dir("auth-disabled");
    write_files(
        &root,
        &[(
            "main.toml",
            &format!(
                r#"
[server]
base_path = "/api"

[database]
url = "{}"

[auth]
allow_registration = false
"#,
                db.url
            ),
        )],
    );

    let state = load_state(&root).await;
    let app = init_http_app!(state);

    let resp = test::call_service(
        &app,
        req_json(
            "POST",
            "/api/auth/register",
            json!({"email":"nobody@example.com","password":"pw"}),
        ),
    )
    .await;
    assert_eq!(resp.status().as_u16(), 403);

    // `user` still carries `create = "public"` for registration's sake, so the
    // generic endpoint has to close with it — otherwise signup just moves.
    let direct = test::call_service(
        &app,
        req_json("POST", "/api/user", json!({"email":"nobody@example.com"})),
    )
    .await;
    assert_eq!(direct.status().as_u16(), 403);

    fs::remove_dir_all(root).unwrap();
    db.cleanup().await;
}
