//! Resource lifecycle hooks.

use super::*;

use std::collections::HashMap;

/// `before_create`: rejects a blank title, and normalises the one it accepts.
fn post_guard(_host: &HostApi_TO<'_, RBox<()>>, hook: &str, input: &str) -> Result<String, String> {
    record(hook);
    let mut data: Value = serde_json::from_str(input).map_err(|e| e.to_string())?;
    let title = data["title"].as_str().unwrap_or_default().to_string();
    if title.trim().is_empty() {
        return Ok(
            json!({ "error": { "status": 422, "message": "title is required" } }).to_string(),
        );
    }
    data["title"] = json!(title.to_uppercase());
    Ok(json!({ "data": data }).to_string())
}

/// `after_create`: writes an audit row through the host's database bridge.
fn post_audit(host: &HostApi_TO<'_, RBox<()>>, hook: &str, input: &str) -> Result<String, String> {
    record(hook);
    let context: Value = serde_json::from_str(hook).map_err(|e| e.to_string())?;
    let row: Value = serde_json::from_str(input).map_err(|e| e.to_string())?;
    let request = json!({
        "sql": "INSERT INTO apiplant_audit (event, detail) VALUES ($1, $2)",
        "params": [context["event"], row["title"]],
    })
    .to_string();
    match host.query(RStr::from_str(&request)) {
        RResult::ROk(_) => Ok(json!({}).to_string()),
        RResult::RErr(e) => Err(e.into_string()),
    }
}

/// `after_list`: wraps the rows in an envelope, replacing the response body.
fn list_wrap(_host: &HostApi_TO<'_, RBox<()>>, hook: &str, input: &str) -> Result<String, String> {
    record(hook);
    let rows: Value = serde_json::from_str(input).map_err(|e| e.to_string())?;
    let count = rows.as_array().map(Vec::len).unwrap_or(0);
    Ok(json!({ "data": { "count": count, "rows": rows } }).to_string())
}

/// `before_read`: refuses the request when the caller asks for `?blocked=1`.
fn read_guard(
    _host: &HostApi_TO<'_, RBox<()>>,
    hook: &str,
    _input: &str,
) -> Result<String, String> {
    record(hook);
    let context: Value = serde_json::from_str(hook).map_err(|e| e.to_string())?;
    if context["query"]["blocked"] == "1" {
        return Ok(json!({ "error": { "status": 403, "message": "read blocked" } }).to_string());
    }
    Ok(json!({}).to_string())
}

/// `after_read`: annotates the row that was fetched.
fn read_stamp(_host: &HostApi_TO<'_, RBox<()>>, hook: &str, input: &str) -> Result<String, String> {
    record(hook);
    let context: Value = serde_json::from_str(hook).map_err(|e| e.to_string())?;
    let mut row: Value = serde_json::from_str(input).map_err(|e| e.to_string())?;
    row["hooked_url"] = context["url"].clone();
    Ok(json!({ "data": row }).to_string())
}

/// `before_update`: rewrites the submitted body.
fn update_guard(
    _host: &HostApi_TO<'_, RBox<()>>,
    hook: &str,
    input: &str,
) -> Result<String, String> {
    record(hook);
    let mut data: Value = serde_json::from_str(input).map_err(|e| e.to_string())?;
    data["body"] = json!("normalised by hook");
    Ok(json!({ "data": data }).to_string())
}

/// `before_delete`: protects locked rows, using the row the host pre-fetched.
fn delete_guard(
    _host: &HostApi_TO<'_, RBox<()>>,
    hook: &str,
    input: &str,
) -> Result<String, String> {
    record(hook);
    let row: Value = serde_json::from_str(input).map_err(|e| e.to_string())?;
    if row["locked"] == json!(true) {
        return Ok(json!({ "error": { "status": 409, "message": "post is locked" } }).to_string());
    }
    Ok(json!({}).to_string())
}

/// `after_delete`: answers with the row that was removed instead of a bare 204.
fn delete_echo(
    _host: &HostApi_TO<'_, RBox<()>>,
    hook: &str,
    input: &str,
) -> Result<String, String> {
    record(hook);
    let row: Value = serde_json::from_str(input).map_err(|e| e.to_string())?;
    Ok(json!({ "data": { "deleted": row["title"] } }).to_string())
}

const HOOKED_POST_RESOURCE: &str = r#"
[resource]
name = "post"

[permissions]
list = "member"
read = "member"
create = "member"
update = "member"
delete = "member"

[hooks]
before_list = "list_watch"
after_list = "list_wrap"
before_read = "read_guard"
after_read = "read_stamp"
before_create = "post_guard"
after_create = "post_audit"
before_update = "update_guard"
after_update = "update_stamp"
before_delete = "delete_guard"
after_delete = "delete_echo"

[fields.title]
type = "string"
required = true

[fields.body]
type = "text"

[fields.locked]
type = "boolean"
"#;

const AUDIT_RESOURCE: &str = r#"
[resource]
name = "audit"
scope = "global"

[permissions]
list = "authenticated"
read = "authenticated"
create = "private"
update = "private"
delete = "private"

[fields.event]
type = "string"

[fields.detail]
type = "text"
"#;

#[ntex::test]
async fn lifecycle_hooks_validate_transform_and_observe_every_crud_operation() {
    let db = TempDatabase::create("hooks").await;
    let root = temp_dir("hooks");
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
            ("resources/post.toml", HOOKED_POST_RESOURCE),
            ("resources/audit.toml", AUDIT_RESOURCE),
        ],
    );

    let state = load_state_with(
        &root,
        vec![
            // Private functions are unreachable over HTTP but callable as hooks.
            test_function("list_watch", Visibility::Private, observer),
            test_function("list_wrap", Visibility::Private, list_wrap),
            test_function("read_guard", Visibility::Private, read_guard),
            test_function("read_stamp", Visibility::Private, read_stamp),
            test_function("post_guard", Visibility::Private, post_guard),
            test_function("post_audit", Visibility::Private, post_audit),
            test_function("update_guard", Visibility::Private, update_guard),
            test_function("update_stamp", Visibility::Private, observer),
            test_function("delete_guard", Visibility::Private, delete_guard),
            test_function("delete_echo", Visibility::Private, delete_echo),
        ],
    )
    .await;
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
    let user_id = registration["user"]["id"].as_str().unwrap().to_string();

    let org = read_json(
        test::call_service(
            &app,
            bearer(
                test::TestRequest::post()
                    .uri("/api/organization")
                    .header(CONTENT_TYPE, "application/json")
                    .set_payload(json!({"name":"Acme","slug":"acme"}).to_string()),
                &token,
            )
            .to_request(),
        )
        .await,
    )
    .await;
    let org_id = org["id"].as_str().unwrap().to_string();

    // --- create: the before hook rewrites the body, the after hook audits it.
    let created_resp = test::call_service(
        &app,
        bearer(
            test::TestRequest::post()
                .uri("/api/post")
                .header(CONTENT_TYPE, "application/json")
                .set_payload(json!({"title":"hello","body":"first"}).to_string()),
            &token,
        )
        .header("x-organization", org_id.as_str())
        .to_request(),
    )
    .await;
    assert_eq!(created_resp.status().as_u16(), 201);
    let created = read_json(created_resp).await;
    assert_eq!(created["title"], "HELLO", "before_create should normalise");
    let post_id = created["id"].as_str().unwrap().to_string();

    // The after_create hook reached the database through the host bridge.
    let audits = read_json(
        test::call_service(
            &app,
            bearer(test::TestRequest::get().uri("/api/audit"), &token)
                .header("x-organization", org_id.as_str())
                .to_request(),
        )
        .await,
    )
    .await;
    assert_eq!(audits.as_array().unwrap().len(), 1);
    assert_eq!(audits[0]["event"], "after_create");
    assert_eq!(audits[0]["detail"], "HELLO");

    // --- create: the before hook can reject outright.
    let rejected = test::call_service(
        &app,
        bearer(
            test::TestRequest::post()
                .uri("/api/post")
                .header(CONTENT_TYPE, "application/json")
                .set_payload(json!({"title":"   ","body":"nope"}).to_string()),
            &token,
        )
        .header("x-organization", org_id.as_str())
        .to_request(),
    )
    .await;
    assert_eq!(rejected.status().as_u16(), 422);
    assert_eq!(read_json(rejected).await["error"], "title is required");

    // --- list: the after hook replaces the response body wholesale.
    let listed_resp = test::call_service(
        &app,
        bearer(test::TestRequest::get().uri("/api/post?limit=10"), &token)
            .header("x-organization", org_id.as_str())
            .to_request(),
    )
    .await;
    assert_eq!(listed_resp.status().as_u16(), 200);
    let listed = read_json(listed_resp).await;
    assert_eq!(listed["count"], 1);
    assert_eq!(listed["rows"][0]["title"], "HELLO");

    // --- read: the after hook annotates the fetched row.
    let read_resp = test::call_service(
        &app,
        bearer(
            test::TestRequest::get().uri(&format!("/api/post/{post_id}")),
            &token,
        )
        .header("x-organization", org_id.as_str())
        .to_request(),
    )
    .await;
    assert_eq!(read_resp.status().as_u16(), 200);
    let fetched = read_json(read_resp).await;
    assert_eq!(fetched["title"], "HELLO");
    assert_eq!(fetched["hooked_url"], format!("/api/post/{post_id}"));

    // --- read: the before hook can veto on request context alone.
    let blocked = test::call_service(
        &app,
        bearer(
            test::TestRequest::get().uri(&format!("/api/post/{post_id}?blocked=1")),
            &token,
        )
        .header("x-organization", org_id.as_str())
        .to_request(),
    )
    .await;
    assert_eq!(blocked.status().as_u16(), 403);
    assert_eq!(read_json(blocked).await["error"], "read blocked");

    // --- update: the before hook rewrites the submitted body.
    let updated_resp = test::call_service(
        &app,
        bearer(
            test::TestRequest::patch()
                .uri(&format!("/api/post/{post_id}"))
                .header(CONTENT_TYPE, "application/json")
                .set_payload(json!({"body":"typed by hand"}).to_string()),
            &token,
        )
        .header("x-organization", org_id.as_str())
        .to_request(),
    )
    .await;
    assert_eq!(updated_resp.status().as_u16(), 200);
    let updated = read_json(updated_resp).await;
    assert_eq!(updated["body"], "normalised by hook");

    // --- delete: the before hook sees the row it is about to lose.
    let lock_resp = test::call_service(
        &app,
        bearer(
            test::TestRequest::patch()
                .uri(&format!("/api/post/{post_id}"))
                .header(CONTENT_TYPE, "application/json")
                .set_payload(json!({"locked": true}).to_string()),
            &token,
        )
        .header("x-organization", org_id.as_str())
        .to_request(),
    )
    .await;
    assert_eq!(lock_resp.status().as_u16(), 200);

    let locked_delete = test::call_service(
        &app,
        bearer(
            test::TestRequest::delete().uri(&format!("/api/post/{post_id}")),
            &token,
        )
        .header("x-organization", org_id.as_str())
        .to_request(),
    )
    .await;
    assert_eq!(locked_delete.status().as_u16(), 409);
    assert_eq!(read_json(locked_delete).await["error"], "post is locked");

    let unlock_resp = test::call_service(
        &app,
        bearer(
            test::TestRequest::patch()
                .uri(&format!("/api/post/{post_id}"))
                .header(CONTENT_TYPE, "application/json")
                .set_payload(json!({"locked": false}).to_string()),
            &token,
        )
        .header("x-organization", org_id.as_str())
        .to_request(),
    )
    .await;
    assert_eq!(unlock_resp.status().as_u16(), 200);

    // --- delete: the after hook turns the usual 204 into a body.
    let deleted_resp = test::call_service(
        &app,
        bearer(
            test::TestRequest::delete().uri(&format!("/api/post/{post_id}")),
            &token,
        )
        .header("x-organization", org_id.as_str())
        .to_request(),
    )
    .await;
    assert_eq!(deleted_resp.status().as_u16(), 200);
    assert_eq!(read_json(deleted_resp).await["deleted"], "HELLO");

    // --- every event fired, carrying the caller and the row in play.
    let seen = events();
    for event in [
        "before_list",
        "after_list",
        "before_read",
        "after_read",
        "before_create",
        "after_create",
        "before_update",
        "after_update",
        "before_delete",
        "after_delete",
    ] {
        assert!(seen.contains(&event.to_string()), "`{event}` never fired");
    }

    // A vetoing `before_*` hook stops the operation, so its `after_*` twin
    // never runs: two reads and two deletes were attempted, one of each denied.
    assert_eq!(fired("before_read"), 2);
    assert_eq!(fired("after_read"), 1);
    assert_eq!(fired("before_delete"), 2);
    assert_eq!(fired("after_delete"), 1);
    // The rejected create never reached the database or the after hook.
    assert_eq!(fired("before_create"), 2);
    assert_eq!(fired("after_create"), 1);

    let create = recorded("before_create");
    assert_eq!(create["resource"], "post");
    assert_eq!(create["action"], "create");
    assert_eq!(create["phase"], "before");
    assert_eq!(create["method"], "POST");
    assert_eq!(create["url"], "/api/post");
    assert_eq!(create["authenticated"], true);
    assert_eq!(create["principal_id"], user_id);
    assert_eq!(create["organization_id"], org_id);
    assert_eq!(create["role"], "admin");
    assert!(create["record_id"].is_null(), "create has no record id yet");
    assert_eq!(
        create["data"]["title"], "hello",
        "before_create sees the submitted body"
    );
    assert!(create["row"].is_null());

    let audit = recorded("after_create");
    assert_eq!(
        audit["row"]["title"], "HELLO",
        "after_create sees the stored row"
    );
    assert_eq!(audit["row"]["organization_id"], org_id);
    assert!(audit["data"].is_null());

    let watched = recorded("before_list");
    assert_eq!(watched["url"], "/api/post?limit=10");
    assert_eq!(watched["query"]["limit"], "10");
    assert!(watched["rows"].is_null());

    let wrapped = recorded("after_list");
    assert_eq!(wrapped["rows"].as_array().unwrap().len(), 1);

    let read = recorded("after_read");
    assert_eq!(read["record_id"], post_id);
    assert_eq!(read["row"]["id"], post_id);

    let update = recorded("before_update");
    assert_eq!(update["record_id"], post_id);
    assert_eq!(update["data"]["body"], "typed by hand");

    let removal = recorded("after_delete");
    assert_eq!(removal["record_id"], post_id);
    assert_eq!(removal["row"]["title"], "HELLO");

    // Hook functions stay invisible over HTTP.
    let direct = test::call_service(
        &app,
        bearer(
            test::TestRequest::post()
                .uri("/api/functions/post_guard")
                .header(CONTENT_TYPE, "application/json")
                .set_payload(json!({"title":"x"}).to_string()),
            &token,
        )
        .header("x-organization", org_id.as_str())
        .to_request(),
    )
    .await;
    assert_eq!(direct.status().as_u16(), 404);

    HOOK_LOG.lock().unwrap().clear();
    fs::remove_dir_all(root).unwrap();
    db.cleanup().await;
}

/// A function called over HTTP gets no hook context, and a resource pointing at
/// a function that isn't loaded fails closed rather than skipping the hook.
#[ntex::test]
async fn functions_see_no_hook_over_http_and_missing_hooks_fail_closed() {
    fn echo_hook(
        _host: &HostApi_TO<'_, RBox<()>>,
        hook: &str,
        input: &str,
    ) -> Result<String, String> {
        Ok(json!({ "hook_was_empty": hook.is_empty(), "echoed": input }).to_string())
    }

    let db = TempDatabase::create("hookless").await;
    let root = temp_dir("hookless");
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
            (
                "resources/note.toml",
                r#"
[resource]
name = "note"
scope = "global"

[permissions]
list = "public"
read = "public"
create = "public"
update = "public"
delete = "public"

[hooks]
before_create = "nowhere"

[fields.title]
type = "string"
"#,
            ),
        ],
    );

    let state = load_state_with(
        &root,
        vec![test_function("echo", Visibility::Public, echo_hook)],
    )
    .await;
    let app = init_http_app!(state);

    let called = read_json(
        test::call_service(
            &app,
            req_json("POST", "/api/functions/echo", json!({"hello":"world"})),
        )
        .await,
    )
    .await;
    assert_eq!(called["hook_was_empty"], true);
    assert_eq!(called["echoed"], json!({"hello":"world"}).to_string());

    let blocked = test::call_service(
        &app,
        req_json("POST", "/api/note", json!({"title":"unreachable hook"})),
    )
    .await;
    assert_eq!(blocked.status().as_u16(), 500);
    assert!(read_json(blocked).await["error"]
        .as_str()
        .unwrap()
        .contains("not loaded"));

    // The row must not have been written despite the hook being unavailable.
    let listed = read_json(
        test::call_service(&app, test::TestRequest::get().uri("/api/note").to_request()).await,
    )
    .await;
    assert_eq!(listed.as_array().unwrap().len(), 0);

    fs::remove_dir_all(root).unwrap();
    db.cleanup().await;
}

// --- registration as a create on `user` -------------------------------------

/// `before_create` on `user`: the hook must never see a plaintext password, and
/// what it returns is what gets inserted.
fn user_guard(
    _host: &HostApi_TO<'_, RBox<()>>,
    _hook: &str,
    input: &str,
) -> Result<String, String> {
    let mut data: Value = serde_json::from_str(input).map_err(|e| e.to_string())?;
    if !data["password"].is_null() {
        return Err("the plaintext password reached a hook".to_string());
    }
    let email = data["email"].as_str().unwrap_or_default().to_string();
    if email.ends_with("@blocked.test") {
        return Ok(
            json!({ "error": { "status": 403, "message": "domain not allowed" } }).to_string(),
        );
    }
    data["display_name"] = json!(email.split('@').next().unwrap_or_default());
    Ok(json!({ "data": data }).to_string())
}

/// `after_create` on `user`: join the organisation owning the address's domain,
/// the way `examples/14-email-domains` does, and report it in the response.
fn user_join(host: &HostApi_TO<'_, RBox<()>>, hook: &str, input: &str) -> Result<String, String> {
    let context: Value = serde_json::from_str(hook).map_err(|e| e.to_string())?;
    let mut row: Value = serde_json::from_str(input).map_err(|e| e.to_string())?;

    // Registration leaves the caller anonymous; the new account is the row, and
    // its id is echoed in `record_id`.
    if context["authenticated"] != json!(false) || !context["principal_id"].is_null() {
        return Err("registration should reach hooks anonymously".to_string());
    }
    let user_id = row["id"].as_str().unwrap_or_default().to_string();
    if context["record_id"] != json!(user_id) {
        return Err("record_id should name the created account".to_string());
    }

    let email = row["email"].as_str().unwrap_or_default();
    let domain = email
        .rsplit_once('@')
        .map(|(_, d)| d.to_string())
        .unwrap_or_default();
    let query = |sql: &str, params: Value| -> Result<Value, String> {
        let request = json!({ "sql": sql, "params": params }).to_string();
        match host.query(RStr::from_str(&request)) {
            RResult::ROk(s) => serde_json::from_str(s.as_str()).map_err(|e| e.to_string()),
            RResult::RErr(e) => Err(e.into_string()),
        }
    };

    let found = query(
        "SELECT id::text AS id FROM apiplant_organization WHERE domain = $1",
        json!([domain]),
    )?;
    if let Some(org_id) = found[0]["id"].as_str().map(str::to_string) {
        query(
            "INSERT INTO apiplant_membership (user_id, organization_id, role) \
             VALUES ($1::uuid, $2::uuid, $3)",
            json!([user_id, org_id, "member"]),
        )?;
        row["joined"] = json!(org_id);
    }
    Ok(json!({ "data": row }).to_string())
}

const DOMAIN_ORG_RESOURCE: &str = r#"
[resource]
name = "organization"
scope = "global"
timestamps = true

[permissions]
list = "member"
read = "member"
create = "authenticated"
update = "role:admin"
delete = "role:admin"

[fields.name]
type = "string"
required = true

[fields.domain]
type = "string"
unique = true
"#;

const HOOKED_USER_RESOURCE: &str = r#"
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
before_create = "user_guard"
after_create = "user_join"

[fields.email]
type = "string"
required = true
unique = true

[fields.password_hash]
type = "string"
hidden = true

[fields.display_name]
type = "string"
"#;

#[ntex::test]
async fn registering_runs_the_user_resources_create_hooks() {
    let db = TempDatabase::create("register_hooks").await;
    let root = temp_dir("register-hooks");
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
            ("resources/organization.toml", DOMAIN_ORG_RESOURCE),
            ("resources/users.toml", HOOKED_USER_RESOURCE),
        ],
    );

    let state = load_state_with(
        &root,
        vec![
            test_function("user_guard", Visibility::Private, user_guard),
            test_function("user_join", Visibility::Private, user_join),
        ],
    )
    .await;
    let app = init_http_app!(state);

    // Ana registers before any organisation exists: `before_create` still runs
    // (her display name is derived), `after_create` finds nothing to join.
    let ana = read_json(
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
    assert_eq!(ana["user"]["display_name"], "ana");
    assert!(ana["user"]["joined"].is_null());
    let ana_token = ana["token"].as_str().unwrap().to_string();

    let org = read_json(
        test::call_service(
            &app,
            bearer(
                test::TestRequest::post()
                    .uri("/api/organization")
                    .header(CONTENT_TYPE, "application/json")
                    .set_payload(json!({"name":"Acme","domain":"acme.test"}).to_string()),
                &ana_token,
            )
            .to_request(),
        )
        .await,
    )
    .await;
    let org_id = org["id"].as_str().unwrap().to_string();

    // Ben registers into a claimed domain: the hook makes him a member, and its
    // replacement lands in the response's `user` — the token is untouched.
    let ben = read_json(
        test::call_service(
            &app,
            req_json(
                "POST",
                "/api/auth/register",
                json!({"email":"ben@acme.test","password":"pw"}),
            ),
        )
        .await,
    )
    .await;
    assert_eq!(ben["user"]["joined"], org_id);
    assert_eq!(ben["user"]["display_name"], "ben");
    assert!(ben["token"].as_str().is_some_and(|t| !t.is_empty()));
    // Hidden fields stay hidden even when a hook hands the row back.
    assert!(ben["user"]["password_hash"].is_null());

    // The membership is real: Ben sees Acme without ever having been invited —
    // alongside the personal organisation every account is created with.
    let ben_token = ben["token"].as_str().unwrap().to_string();
    let visible = read_json(
        test::call_service(
            &app,
            bearer(
                test::TestRequest::get().uri("/api/organization"),
                &ben_token,
            )
            .to_request(),
        )
        .await,
    )
    .await;
    let visible = visible.as_array().unwrap();
    assert_eq!(visible.len(), 2);
    assert!(visible.iter().any(|org| org["id"] == org_id));

    // A `before_create` abort fails the registration and writes no user.
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

    let users = read_json(
        test::call_service(
            &app,
            bearer(test::TestRequest::get().uri("/api/user"), &ana_token).to_request(),
        )
        .await,
    )
    .await;
    assert_eq!(users.as_array().unwrap().len(), 2);

    fs::remove_dir_all(root).unwrap();
    db.cleanup().await;
}

// ---------------------------------------------------------------------------
// A read-through cache written entirely in hooks — the pattern
// `examples/16-caching` runs against Redis, here against a `HashMap` so the
// test needs no cache server.
// ---------------------------------------------------------------------------

/// Stands in for Redis. Keyed by record id, as the example keys by id.
static FAKE_CACHE: Mutex<Option<HashMap<String, Value>>> = Mutex::new(None);

fn cache() -> std::sync::MutexGuard<'static, Option<HashMap<String, Value>>> {
    let mut guard = FAKE_CACHE.lock().unwrap();
    guard.get_or_insert_with(HashMap::new);
    guard
}

/// `before_read`: a hit *is* the response, and the database is never queried.
fn cached_read(
    _host: &HostApi_TO<'_, RBox<()>>,
    hook: &str,
    _input: &str,
) -> Result<String, String> {
    let context: Value = serde_json::from_str(hook).map_err(|e| e.to_string())?;
    let id = context["record_id"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    match cache().as_ref().and_then(|c| c.get(&id)).cloned() {
        Some(row) => Ok(json!({ "data": row }).to_string()),
        None => Ok(json!({}).to_string()),
    }
}

/// `after_read`: only reached on a miss — a hit never got this far.
fn fill_cache(
    _host: &HostApi_TO<'_, RBox<()>>,
    _hook: &str,
    input: &str,
) -> Result<String, String> {
    let row: Value = serde_json::from_str(input).map_err(|e| e.to_string())?;
    if let Some(id) = row["id"].as_str() {
        cache()
            .as_mut()
            .unwrap()
            .insert(id.to_string(), row.clone());
    }
    Ok(json!({}).to_string())
}

/// `after_update` / `after_delete`: the write is what makes the entry stale.
fn evict(_host: &HostApi_TO<'_, RBox<()>>, _hook: &str, input: &str) -> Result<String, String> {
    let row: Value = serde_json::from_str(input).map_err(|e| e.to_string())?;
    if let Some(id) = row["id"].as_str() {
        cache().as_mut().unwrap().remove(id);
    }
    Ok(json!({}).to_string())
}

const CACHED_NOTE_RESOURCE: &str = r#"
[resource]
name = "note"
scope = "global"

[permissions]
list = "public"
read = "public"
create = "public"
update = "public"
delete = "public"

[hooks]
before_read = "cached_read"
after_read = "fill_cache"
after_update = "evict"
after_delete = "evict"

[fields.title]
type = "string"
required = true
"#;

#[ntex::test]
async fn a_before_read_hook_answers_from_cache_without_querying_the_database() {
    let db = TempDatabase::create("cached_read").await;
    let root = temp_dir("cached-read");
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
            ("resources/note.toml", CACHED_NOTE_RESOURCE),
        ],
    );

    let state = load_state_with(
        &root,
        vec![
            test_function("cached_read", Visibility::Private, cached_read),
            test_function("fill_cache", Visibility::Private, fill_cache),
            test_function("evict", Visibility::Private, evict),
        ],
    )
    .await;
    let app = init_http_app!(state);
    cache().as_mut().unwrap().clear();

    let created = read_json(
        test::call_service(
            &app,
            req_json("POST", "/api/note", json!({ "title": "first" })),
        )
        .await,
    )
    .await;
    let id = created["id"].as_str().unwrap().to_string();

    // First read: a miss, so the query runs and `after_read` fills the cache.
    let first = read_json(
        test::call_service(&app, req_json("GET", &format!("/api/note/{id}"), json!({}))).await,
    )
    .await;
    assert_eq!(first["title"], "first");
    assert!(cache().as_ref().unwrap().contains_key(&id));

    // Prove the second read never reaches Postgres: doctor the cached entry to
    // something the database does not contain. Only a short-circuited read can
    // return it.
    cache()
        .as_mut()
        .unwrap()
        .get_mut(&id)
        .unwrap()
        .as_object_mut()
        .unwrap()
        .insert("title".into(), json!("from cache, not the database"));

    let second = read_json(
        test::call_service(&app, req_json("GET", &format!("/api/note/{id}"), json!({}))).await,
    )
    .await;
    assert_eq!(second["title"], "from cache, not the database");

    // The row itself is untouched — the doctored value only ever lived in the
    // cache, which is what makes the assertion above mean what it says.
    let listed =
        read_json(test::call_service(&app, req_json("GET", "/api/note", json!({}))).await).await;
    assert_eq!(listed[0]["title"], "first");

    // A write invalidates: the next read misses, queries, and sees the update.
    let updated = test::call_service(
        &app,
        req_json(
            "PATCH",
            &format!("/api/note/{id}"),
            json!({ "title": "second" }),
        ),
    )
    .await;
    assert_eq!(updated.status().as_u16(), 200);
    assert!(!cache().as_ref().unwrap().contains_key(&id));

    let after_write = read_json(
        test::call_service(&app, req_json("GET", &format!("/api/note/{id}"), json!({}))).await,
    )
    .await;
    assert_eq!(after_write["title"], "second");
    assert_eq!(cache().as_ref().unwrap()[&id]["title"], "second");

    // A delete evicts too, and the stale entry can't outlive the row.
    let deleted = test::call_service(
        &app,
        req_json("DELETE", &format!("/api/note/{id}"), json!({})),
    )
    .await;
    assert_eq!(deleted.status().as_u16(), 204);
    assert!(!cache().as_ref().unwrap().contains_key(&id));
    let gone =
        test::call_service(&app, req_json("GET", &format!("/api/note/{id}"), json!({}))).await;
    assert_eq!(gone.status().as_u16(), 404);

    fs::remove_dir_all(root).unwrap();
    db.cleanup().await;
}
