//! Auth hooks: the extension points on the built-in auth endpoints, declared
//! in the `user` model's `[hooks]` section next to its CRUD hooks.

use super::*;

/// `before_register`: normalises the address and refuses a blocked domain.
fn signup_guard(
    _host: &HostApi_TO<'_, RBox<()>>,
    _hook: &str,
    input: &str,
) -> Result<String, String> {
    let mut data: Value = serde_json::from_str(input).map_err(|e| e.to_string())?;
    if !data["password"].is_null() {
        return Err("the plaintext password reached an auth hook".to_string());
    }
    let email = data["email"].as_str().unwrap_or_default().to_lowercase();
    if email.ends_with("@blocked.test") {
        return Ok(
            json!({ "error": { "status": 403, "message": "domain not allowed" } }).to_string(),
        );
    }
    data["email"] = json!(email);
    Ok(json!({ "data": data }).to_string())
}

/// `after_register`: annotates the account in the response.
fn signup_welcome(
    _host: &HostApi_TO<'_, RBox<()>>,
    hook: &str,
    input: &str,
) -> Result<String, String> {
    let context: Value = serde_json::from_str(hook).map_err(|e| e.to_string())?;
    let mut row: Value = serde_json::from_str(input).map_err(|e| e.to_string())?;
    if context["event"] != "after_register" || context["action"] != "register" {
        return Err(format!("unexpected context: {context}"));
    }
    // The row reaches an `after_*` hook in `row`, not `data`.
    if context["row"]["id"] != row["id"] || !context["data"].is_null() {
        return Err("the created account should arrive in `row`".to_string());
    }
    if context["record_id"] != row["id"] {
        return Err("record_id should name the created account".to_string());
    }
    row["welcomed"] = json!(true);
    Ok(json!({ "data": row }).to_string())
}

/// `before_login`: locks one address out, and folds the case of the rest.
fn login_guard(
    _host: &HostApi_TO<'_, RBox<()>>,
    hook: &str,
    input: &str,
) -> Result<String, String> {
    let context: Value = serde_json::from_str(hook).map_err(|e| e.to_string())?;
    let mut data: Value = serde_json::from_str(input).map_err(|e| e.to_string())?;
    if !data["password"].is_null() {
        return Err("the plaintext password reached an auth hook".to_string());
    }
    if context["authenticated"] != json!(false) {
        return Err("a login attempt is anonymous by definition".to_string());
    }
    let email = data["email"].as_str().unwrap_or_default().to_lowercase();
    if email == "locked@acme.test" {
        return Ok(json!({ "error": { "status": 423, "message": "account locked" } }).to_string());
    }
    data["email"] = json!(email);
    Ok(json!({ "data": data }).to_string())
}

/// `after_login`: sees every attempt. It records the failures, answers `429` to
/// an unknown address, and widens the response of a success without being able
/// to touch the token that success issued.
fn login_stamp(host: &HostApi_TO<'_, RBox<()>>, hook: &str, input: &str) -> Result<String, String> {
    let context: Value = serde_json::from_str(hook).map_err(|e| e.to_string())?;
    let payload: Value = serde_json::from_str(input).map_err(|e| e.to_string())?;
    // The outcome arrives in `data`; there is no row to speak of, and on a
    // failure there is no account either.
    if context["data"]["success"] != payload["success"] || !context["row"].is_null() {
        return Err("the outcome should arrive in `data`".to_string());
    }

    if payload["success"] == json!(true) {
        if context["record_id"] != payload["user_id"] || !payload["reason"].is_null() {
            return Err("a success names the account and has no reason".to_string());
        }
        return Ok(json!({ "data": {
            "user_id": payload["user_id"],
            "greeting": "welcome back",
            "token": "forged",
        }})
        .to_string());
    }

    if !payload["user_id"].is_null() || !context["record_id"].is_null() {
        return Err("a failed attempt names no account".to_string());
    }
    let request = json!({
        "sql": "INSERT INTO apiplant_audit (event, detail) VALUES ($1, $2)",
        "params": [payload["reason"], payload["identity"]],
    })
    .to_string();
    if let RResult::RErr(e) = host.query(RStr::from_str(&request)) {
        return Err(e.into_string());
    }
    if payload["reason"] == "unknown_identity" {
        return Ok(
            json!({ "error": { "status": 429, "message": "too many attempts" } }).to_string(),
        );
    }
    Ok(json!({}).to_string())
}

/// `before_api_key`: stamps a name onto every key the app issues.
fn key_guard(_host: &HostApi_TO<'_, RBox<()>>, hook: &str, input: &str) -> Result<String, String> {
    let context: Value = serde_json::from_str(hook).map_err(|e| e.to_string())?;
    let mut data: Value = serde_json::from_str(input).map_err(|e| e.to_string())?;
    // Key issuance runs as the caller, unlike register and login.
    if context["authenticated"] != json!(true) || context["principal_id"].is_null() {
        return Err("api key issuance should see the caller".to_string());
    }
    if context["resource"] != "api_key" {
        return Err("the hook runs against the api_key resource".to_string());
    }
    data["name"] = json!(format!(
        "{}-managed",
        data["name"].as_str().unwrap_or("key")
    ));
    Ok(json!({ "data": data }).to_string())
}

/// `after_api_key`: widens the response, and cannot touch the plaintext key.
fn key_stamp(_host: &HostApi_TO<'_, RBox<()>>, _hook: &str, input: &str) -> Result<String, String> {
    let row: Value = serde_json::from_str(input).map_err(|e| e.to_string())?;
    Ok(json!({ "data": { "label": row["name"], "api_key": "forged" } }).to_string())
}

const AUTH_HOOKED_USER_MODEL: &str = r#"
[resource]
name = "user"
scope = "global"
timestamps = true

[permissions]
list = "authenticated"
read = "owner"
create = "public"
update = "owner"
delete = "private"

[auth]
identity_field = "email"
password_field = "password_hash"

[hooks]
before_register = "signup_guard"
after_register = "signup_welcome"
before_login = "login_guard"
after_login = "login_stamp"
before_api_key = "key_guard"
after_api_key = "key_stamp"

[fields.email]
type = "string"
required = true
unique = true

[fields.password_hash]
type = "string"
hidden = true
"#;

const AUDIT_MODEL: &str = r#"
[resource]
name = "audit"
scope = "global"
timestamps = true

[permissions]
list = "authenticated"
read = "authenticated"
create = "private"
update = "private"
delete = "private"

[fields.event]
type = "string"

[fields.detail]
type = "string"
"#;

#[ntex::test]
async fn auth_hooks_shape_registration_login_and_key_issuance() {
    let db = TempDatabase::create("auth_hooks").await;
    let root = temp_dir("auth-hooks");
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
            ("models/audit.toml", AUDIT_MODEL),
            ("models/users.toml", AUTH_HOOKED_USER_MODEL),
        ],
    );

    let state = load_state_with(
        &root,
        vec![
            test_function("signup_guard", Visibility::Private, signup_guard),
            test_function("signup_welcome", Visibility::Private, signup_welcome),
            test_function("login_guard", Visibility::Private, login_guard),
            test_function("login_stamp", Visibility::Private, login_stamp),
            test_function("key_guard", Visibility::Private, key_guard),
            test_function("key_stamp", Visibility::Private, key_stamp),
        ],
    )
    .await;
    let app = init_http_app!(state);

    // `before_register` folds the case of the address; `after_register`
    // annotates the account without disturbing the issued token.
    let ana = read_json(
        test::call_service(
            &app,
            req_json(
                "POST",
                "/api/auth/register",
                json!({"email":"Ana@Acme.test","password":"pw"}),
            ),
        )
        .await,
    )
    .await;
    assert_eq!(ana["user"]["email"], "ana@acme.test");
    assert_eq!(ana["user"]["welcomed"], true);
    assert!(ana["token"].as_str().is_some_and(|t| !t.is_empty()));

    // A `before_register` rejection fails the signup and writes no user.
    let blocked = test::call_service(
        &app,
        req_json(
            "POST",
            "/api/auth/register",
            json!({"email":"mal@blocked.test","password":"pw"}),
        ),
    )
    .await;
    assert_eq!(blocked.status().as_u16(), 403);
    assert_eq!(read_json(blocked).await["error"], "domain not allowed");

    // `before_login` rewrites the identity that is looked up, so the address
    // that was stored lowercase still matches what the client typed.
    let logged_in = test::call_service(
        &app,
        req_json(
            "POST",
            "/api/auth/login",
            json!({"email":"ANA@acme.test","password":"pw"}),
        ),
    )
    .await;
    assert_eq!(logged_in.status().as_u16(), 200);
    let session = read_json(logged_in).await;
    // `after_login` widened the response; the token it tried to forge lost to
    // the one the endpoint issued.
    assert_eq!(session["greeting"], "welcome back");
    assert_eq!(session["user_id"], ana["user"]["id"]);
    assert_ne!(session["token"], json!("forged"));
    let token = session["token"].as_str().unwrap().to_string();

    // A `before_login` rejection replaces the endpoint's answer entirely.
    let locked = test::call_service(
        &app,
        req_json(
            "POST",
            "/api/auth/login",
            json!({"email":"locked@acme.test","password":"pw"}),
        ),
    )
    .await;
    assert_eq!(locked.status().as_u16(), 423);
    assert_eq!(read_json(locked).await["error"], "account locked");

    // A wrong password: `after_login` sees the failed attempt, records it, and
    // lets the flat 401 stand.
    let wrong = test::call_service(
        &app,
        req_json(
            "POST",
            "/api/auth/login",
            json!({"email":"ana@acme.test","password":"nope"}),
        ),
    )
    .await;
    assert_eq!(wrong.status().as_u16(), 401);
    assert_eq!(read_json(wrong).await["error"], "invalid credentials");

    // An unknown address: the same hook answers 429 instead.
    let unknown = test::call_service(
        &app,
        req_json(
            "POST",
            "/api/auth/login",
            json!({"email":"nobody@acme.test","password":"pw"}),
        ),
    )
    .await;
    assert_eq!(unknown.status().as_u16(), 429);
    assert_eq!(read_json(unknown).await["error"], "too many attempts");

    // Both failures reached the hook, with the reason that distinguishes them.
    let audit = read_json(
        test::call_service(
            &app,
            bearer(test::TestRequest::get().uri("/api/audit"), &token).to_request(),
        )
        .await,
    )
    .await;
    let logged: Vec<(&str, &str)> = audit
        .as_array()
        .unwrap()
        .iter()
        .map(|r| {
            (
                r["event"].as_str().unwrap_or_default(),
                r["detail"].as_str().unwrap_or_default(),
            )
        })
        .collect();
    assert!(logged.contains(&("bad_password", "ana@acme.test")));
    assert!(logged.contains(&("unknown_identity", "nobody@acme.test")));

    // Key issuance: `before_api_key` writes the stored row, `after_api_key`
    // widens the response but cannot reach the plaintext key.
    let issued = read_json(
        test::call_service(
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
        .await,
    )
    .await;
    assert_eq!(issued["label"], "ci-managed");
    let plaintext = issued["api_key"].as_str().unwrap().to_string();
    assert_ne!(plaintext, "forged");

    // The key the hook renamed is the key that works.
    let whoami = test::call_service(
        &app,
        test::TestRequest::get()
            .uri("/api/auth/me")
            .header("x-api-key", plaintext)
            .to_request(),
    )
    .await;
    assert_eq!(whoami.status().as_u16(), 200);

    fs::remove_dir_all(root).unwrap();
    db.cleanup().await;
}

/// A model with no `[auth.hooks]` at all behaves exactly as before.
#[ntex::test]
async fn auth_endpoints_are_unchanged_without_hooks() {
    let db = TempDatabase::create("auth_no_hooks").await;
    let root = temp_dir("auth-no-hooks");
    write_files(
        &root,
        &[(
            "main.toml",
            &format!(
                "\n[server]\nbase_path = \"/api\"\n\n[database]\nurl = \"{}\"\n",
                db.url
            ),
        )],
    );
    let state = load_state_with(&root, vec![]).await;
    let app = init_http_app!(state);

    let registered = read_json(
        test::call_service(
            &app,
            req_json(
                "POST",
                "/api/auth/register",
                json!({"email":"ana@acme.test","password":"pw"}),
            ),
        )
        .await,
    )
    .await;
    assert!(registered["token"].as_str().is_some_and(|t| !t.is_empty()));

    let session = read_json(
        test::call_service(
            &app,
            req_json(
                "POST",
                "/api/auth/login",
                json!({"email":"ana@acme.test","password":"pw"}),
            ),
        )
        .await,
    )
    .await;
    // Nothing to widen the response, so it is the token and nothing else.
    assert_eq!(session.as_object().unwrap().len(), 1);
    assert!(session["token"].as_str().is_some_and(|t| !t.is_empty()));

    let failed = test::call_service(
        &app,
        req_json(
            "POST",
            "/api/auth/login",
            json!({"email":"ana@acme.test","password":"nope"}),
        ),
    )
    .await;
    assert_eq!(failed.status().as_u16(), 401);

    fs::remove_dir_all(root).unwrap();
    db.cleanup().await;
}
