//! Invitations, address confirmation and password reset.
//!
//! The tokens in these flows only ever exist in an email, and no test here has
//! a mailbox. So the rows are seeded with a token whose plaintext the test
//! already knows — the same way the endpoints will find them — and what gets
//! exercised is everything downstream of delivery: who a link admits, how many
//! times it works, and what it leaves behind.
//!
//! The one place delivery itself is tested is
//! [`an_invitation_that_could_not_be_sent_is_not_left_pending`], where the SMTP
//! host is a port nothing is listening on.

use super::*;
use apiplant_auth::Authenticator;

/// An app that can send mail — to nowhere. Building the transport does not
/// connect, so the routes mount and only an actual send fails.
fn main_toml(db_url: &str) -> String {
    format!(
        r#"
[server]
base_path = "/api"
public_url = "https://example.test"

[database]
url = "{db_url}"

[email]
provider = "smtp"
from = "no-reply@example.test"

[email.smtp]
host = "127.0.0.1"
port = 1
encryption = "none"
"#
    )
}

/// Fill the physical table names into a test query.
///
/// Tables are prefixed (`user` lives in `apiplant_user`) and an app may rename
/// them outright, so the SQL here names resources and this resolves them the
/// same way the server does.
fn sql(state: &AppState, query: &str) -> String {
    let mut out = query.to_string();
    for (placeholder, resource) in [
        ("{users}", "user"),
        ("{organizations}", "organization"),
        ("{memberships}", "membership"),
        ("{invitations}", "invitation"),
        ("{auth_tokens}", "auth_token"),
    ] {
        out = out.replace(placeholder, &state.table(resource).expect(resource));
    }
    out
}

/// Insert a row directly, returning it — the seeding equivalent of a `POST`,
/// without the permissions that would refuse most of what these tests set up.
async fn seed(state: &AppState, resource: &str, fields: Value) -> Value {
    let resource = state.app.resources.get(resource).expect(resource);
    let Value::Object(data) = fields else {
        panic!("seed takes an object");
    };
    state.db.create(resource, &data).await.unwrap()
}

/// An RFC 3339 timestamp `seconds` from now, for the columns that take one.
fn at(seconds: i64) -> Value {
    json!((chrono::Utc::now() + chrono::Duration::seconds(seconds)).to_rfc3339())
}

/// Register somebody and confirm them, returning their id.
async fn verified_user(state: &AppState, email: &str, password: &str) -> String {
    let hash = state.auth.hash_password(password).unwrap();
    let row = seed(
        state,
        "user",
        json!({ "email": email, "password_hash": hash, "email_verified_at": at(0) }),
    )
    .await;
    row["id"].as_str().unwrap().to_string()
}

/// An organisation with `owner` as its admin. Returns its id.
async fn organization(state: &AppState, name: &str, owner: &str) -> String {
    let row = seed(state, "organization", json!({ "name": name })).await;
    let org = row["id"].as_str().unwrap().to_string();
    seed(
        state,
        "membership",
        json!({ "user_id": owner, "organization_id": org, "role": "admin" }),
    )
    .await;
    org
}

/// Seed an invitation with a token we know the plaintext of, expiring
/// `expires_in` seconds from now — negative for one that already has.
async fn invitation(
    state: &AppState,
    org: &str,
    email: &str,
    role: &str,
    token: &str,
    expires_in: i64,
) {
    seed(
        state,
        "invitation",
        json!({
            "email": email,
            "role": role,
            "token_hash": Authenticator::hash_link_token(token),
            "organization_id": org,
            "expires_at": at(expires_in),
        }),
    )
    .await;
}

/// Seed a single-use token of `kind` for `user`.
async fn auth_token(state: &AppState, user: &str, kind: &str, token: &str) {
    seed(
        state,
        "auth_token",
        json!({
            "user_id": user,
            "kind": kind,
            "token_hash": Authenticator::hash_link_token(token),
            "expires_at": at(60 * 60 * 24),
        }),
    )
    .await;
}

async fn count(state: &AppState, query: &str, params: &[Value]) -> usize {
    let rows = state.db.raw_json(&sql(state, query), params).await.unwrap();
    rows.as_array().map(Vec::len).unwrap_or_default()
}

/// An app with no `[email]` provider does not answer these routes at all.
///
/// This is the whole shape of the feature: a deployment that cannot send mail
/// has no password reset, and says so with a 404 rather than by failing halfway
/// through one.
#[ntex::test]
async fn without_a_mailer_there_are_no_mailbox_routes() {
    let db = TempDatabase::create("nomail").await;
    let root = temp_dir("nomail");
    write_files(
        &root,
        &[(
            "main.toml",
            &format!(
                "[server]\nbase_path = \"/api\"\n\n[database]\nurl = \"{}\"\n",
                db.url
            ),
        )],
    );

    let state = load_state(&root).await;
    assert!(!state.email_enabled());
    assert!(!state.invitations_enabled());
    assert!(!state.password_reset_enabled());
    assert!(!state.requires_email_verification());

    let app = init_http_app!(state);
    for (method, path) in [
        ("POST", "/api/auth/invitations"),
        ("POST", "/api/auth/password/forgot"),
        ("POST", "/api/auth/password/reset"),
        ("POST", "/api/auth/verify-email"),
    ] {
        let resp = test::call_service(&app, req_json(method, path, json!({}))).await;
        // 405, not 404: with the route unregistered the path falls through to
        // the generic `/{resource}/{id}` matcher, which has no POST. Either way
        // nothing here is *handled*, which is the claim.
        assert!(
            matches!(resp.status().as_u16(), 404 | 405),
            "{path} was handled by a server with no mailer"
        );
    }

    // …and registration still hands out a session, because there is nothing to
    // confirm.
    let resp = test::call_service(
        &app,
        req_json(
            "POST",
            "/api/auth/register",
            json!({"email":"ann@example.test","password":"hunter2"}),
        ),
    )
    .await;
    assert_eq!(resp.status().as_u16(), 201);
    assert!(read_json(resp).await["token"].is_string());

    db.cleanup().await;
    fs::remove_dir_all(root).unwrap();
}

/// Configuring a provider switches all three flows on without another line of
/// configuration — and an explicit `false` still turns one off.
#[ntex::test]
async fn a_provider_switches_the_flows_on_and_a_flag_can_switch_one_back_off() {
    let db = TempDatabase::create("emailflags").await;
    let root = temp_dir("emailflags");
    write_files(&root, &[("main.toml", &main_toml(&db.url))]);

    let state = load_state(&root).await;
    assert!(state.email_enabled());
    assert!(state.invitations_enabled());
    assert!(state.password_reset_enabled());
    assert!(state.requires_email_verification());

    write_files(
        &root,
        &[(
            "main.toml",
            &format!(
                "{}\n[auth]\nrequire_email_verification = false\n",
                main_toml(&db.url)
            ),
        )],
    );
    let state = load_state(&root).await;
    assert!(!state.requires_email_verification());
    // The other two are untouched: the flags are independent.
    assert!(state.invitations_enabled());
    assert!(state.password_reset_enabled());

    let app = init_http_app!(state);
    let resp =
        test::call_service(&app, req_json("POST", "/api/auth/verify-email", json!({}))).await;
    assert!(matches!(resp.status().as_u16(), 404 | 405));

    db.cleanup().await;
    fs::remove_dir_all(root).unwrap();
}

/// Registering with confirmation on hands back no session, and signing in is
/// refused until the link is opened — after which it is not.
#[ntex::test]
async fn an_unconfirmed_account_cannot_sign_in_until_it_confirms() {
    let db = TempDatabase::create("verify").await;
    let root = temp_dir("verify");
    write_files(&root, &[("main.toml", &main_toml(&db.url))]);

    let state = load_state(&root).await;
    let app = init_http_app!(state.clone());

    // Registration creates the account but cannot deliver the confirmation
    // (nothing is listening on port 1), which is a 502 — the account exists and
    // is unusable, and saying so is better than a cheerful 201.
    let resp = test::call_service(
        &app,
        req_json(
            "POST",
            "/api/auth/register",
            json!({"email":"ann@example.test","password":"hunter2"}),
        ),
    )
    .await;
    assert_eq!(resp.status().as_u16(), 502);

    let rows = state
        .db
        .raw_json(
            &sql(
                &state,
                "SELECT id::text AS id FROM {users} WHERE email = $1",
            ),
            &[json!("ann@example.test")],
        )
        .await
        .unwrap();
    let user_id = rows[0]["id"].as_str().unwrap().to_string();

    // The right password, and still no session: the address is unconfirmed.
    let resp = test::call_service(
        &app,
        req_json(
            "POST",
            "/api/auth/login",
            json!({"email":"ann@example.test","password":"hunter2"}),
        ),
    )
    .await;
    assert_eq!(resp.status().as_u16(), 403);
    let refused = read_json(resp).await;
    assert_eq!(refused["reason"], "email_unverified");

    // A wrong password is still a flat 401 — the 403 above must not become a
    // way of testing passwords without one.
    let resp = test::call_service(
        &app,
        req_json(
            "POST",
            "/api/auth/login",
            json!({"email":"ann@example.test","password":"wrong"}),
        ),
    )
    .await;
    assert_eq!(resp.status().as_u16(), 401);

    auth_token(&state, &user_id, "email_verification", "verify_known").await;
    let resp = test::call_service(
        &app,
        req_json(
            "POST",
            "/api/auth/verify-email",
            json!({"token":"verify_known"}),
        ),
    )
    .await;
    assert_eq!(resp.status().as_u16(), 200);
    // Confirming signs you in: you have just proved you read the mailbox.
    assert!(read_json(resp).await["token"].is_string());

    // The copy of the link still sitting in the mailbox is now inert.
    let resp = test::call_service(
        &app,
        req_json(
            "POST",
            "/api/auth/verify-email",
            json!({"token":"verify_known"}),
        ),
    )
    .await;
    assert_eq!(resp.status().as_u16(), 404);

    let resp = test::call_service(
        &app,
        req_json(
            "POST",
            "/api/auth/login",
            json!({"email":"ann@example.test","password":"hunter2"}),
        ),
    )
    .await;
    assert_eq!(resp.status().as_u16(), 200);

    db.cleanup().await;
    fs::remove_dir_all(root).unwrap();
}

/// A reset link sets the password, retires every other outstanding one, and
/// cannot be used twice.
#[ntex::test]
async fn a_reset_link_changes_the_password_once() {
    let db = TempDatabase::create("reset").await;
    let root = temp_dir("reset");
    write_files(&root, &[("main.toml", &main_toml(&db.url))]);

    let state = load_state(&root).await;
    let app = init_http_app!(state.clone());
    let user_id = verified_user(&state, "ann@example.test", "old-password").await;

    // Asking is always accepted, and says nothing about who exists — the same
    // answer for an address with no account at all.
    for address in ["ann@example.test", "nobody@example.test"] {
        let resp = test::call_service(
            &app,
            req_json(
                "POST",
                "/api/auth/password/forgot",
                json!({"email": address}),
            ),
        )
        .await;
        assert_eq!(resp.status().as_u16(), 202);
    }

    // Two links asked for in a moment of confusion.
    auth_token(&state, &user_id, "password_reset", "reset_first").await;
    auth_token(&state, &user_id, "password_reset", "reset_second").await;

    let resp = test::call_service(
        &app,
        req_json(
            "POST",
            "/api/auth/password/reset",
            json!({"token":"reset_first","password":"new-password"}),
        ),
    )
    .await;
    assert_eq!(resp.status().as_u16(), 200);
    assert!(read_json(resp).await["token"].is_string());

    // Using the first must not leave the second working under the mat.
    for token in ["reset_first", "reset_second"] {
        let resp = test::call_service(
            &app,
            req_json(
                "POST",
                "/api/auth/password/reset",
                json!({"token": token, "password":"third-password"}),
            ),
        )
        .await;
        assert_eq!(resp.status().as_u16(), 404, "{token} still worked");
    }

    let resp = test::call_service(
        &app,
        req_json(
            "POST",
            "/api/auth/login",
            json!({"email":"ann@example.test","password":"new-password"}),
        ),
    )
    .await;
    assert_eq!(resp.status().as_u16(), 200);

    let resp = test::call_service(
        &app,
        req_json(
            "POST",
            "/api/auth/login",
            json!({"email":"ann@example.test","password":"old-password"}),
        ),
    )
    .await;
    assert_eq!(resp.status().as_u16(), 401);

    db.cleanup().await;
    fs::remove_dir_all(root).unwrap();
}

/// The point of invitations: somebody with no account gets one, and a
/// membership, from a link.
#[ntex::test]
async fn an_invitation_creates_the_account_and_the_membership() {
    let db = TempDatabase::create("invite").await;
    let root = temp_dir("invite");
    write_files(&root, &[("main.toml", &main_toml(&db.url))]);

    let state = load_state(&root).await;
    let app = init_http_app!(state.clone());
    let admin = verified_user(&state, "admin@example.test", "hunter2").await;
    let org = organization(&state, "Acme Ltd", &admin).await;
    invitation(
        &state,
        &org,
        "new@example.test",
        "member",
        "inv_known",
        60 * 60 * 24 * 7,
    )
    .await;

    // Anonymous preview: enough to render the page, and nothing else.
    let resp = test::call_service(
        &app,
        test::TestRequest::get()
            .uri("/api/auth/invitations/inv_known")
            .to_request(),
    )
    .await;
    assert_eq!(resp.status().as_u16(), 200);
    let preview = read_json(resp).await;
    assert_eq!(preview["organization"], "Acme Ltd");
    assert_eq!(preview["email"], "new@example.test");
    assert_eq!(preview["has_account"], false);
    assert!(preview.get("organization_id").is_none());

    let resp = test::call_service(
        &app,
        req_json(
            "POST",
            "/api/auth/invitations/inv_known/accept",
            json!({"password":"hunter2"}),
        ),
    )
    .await;
    assert_eq!(resp.status().as_u16(), 200);
    let accepted = read_json(resp).await;
    assert!(accepted["token"].is_string());
    assert_eq!(accepted["organization_id"], org);

    assert_eq!(
        count(
            &state,
            "SELECT 1 AS hit FROM {memberships} m JOIN {users} u ON u.id = m.user_id \
             WHERE u.email = $1 AND m.organization_id = $2::uuid AND m.role = 'member'",
            &[json!("new@example.test"), json!(org)],
        )
        .await,
        1
    );

    // Accepting the invitation is the confirmation, so the new account can sign
    // in straight away without a second email.
    let resp = test::call_service(
        &app,
        req_json(
            "POST",
            "/api/auth/login",
            json!({"email":"new@example.test","password":"hunter2"}),
        ),
    )
    .await;
    assert_eq!(resp.status().as_u16(), 200);

    // The link is spent, and the row is kept as history rather than deleted.
    let resp = test::call_service(
        &app,
        req_json(
            "POST",
            "/api/auth/invitations/inv_known/accept",
            json!({"password":"hunter2"}),
        ),
    )
    .await;
    assert_eq!(resp.status().as_u16(), 404);
    assert_eq!(
        count(
            &state,
            "SELECT 1 AS hit FROM {invitations} WHERE accepted_at IS NOT NULL",
            &[],
        )
        .await,
        1
    );

    db.cleanup().await;
    fs::remove_dir_all(root).unwrap();
}

/// Somebody who already has an account is not asked to make a second one, and
/// is not asked for a password they already have.
#[ntex::test]
async fn an_invitation_to_an_existing_account_just_joins_it() {
    let db = TempDatabase::create("invitejoin").await;
    let root = temp_dir("invitejoin");
    write_files(&root, &[("main.toml", &main_toml(&db.url))]);

    let state = load_state(&root).await;
    let app = init_http_app!(state.clone());
    let admin = verified_user(&state, "admin@example.test", "hunter2").await;
    let org = organization(&state, "Acme Ltd", &admin).await;
    verified_user(&state, "ann@example.test", "her-own-password").await;
    invitation(
        &state,
        &org,
        "ann@example.test",
        "billing",
        "inv_known",
        60 * 60 * 24 * 7,
    )
    .await;

    let resp = test::call_service(
        &app,
        test::TestRequest::get()
            .uri("/api/auth/invitations/inv_known")
            .to_request(),
    )
    .await;
    assert_eq!(read_json(resp).await["has_account"], true);

    let resp = test::call_service(
        &app,
        req_json("POST", "/api/auth/invitations/inv_known/accept", json!({})),
    )
    .await;
    assert_eq!(resp.status().as_u16(), 200);

    // One account, not two, and the role the invitation named.
    assert_eq!(
        count(
            &state,
            "SELECT 1 AS hit FROM {users} WHERE email = $1",
            &[json!("ann@example.test")],
        )
        .await,
        1
    );
    assert_eq!(
        count(
            &state,
            "SELECT 1 AS hit FROM {memberships} m JOIN {users} u ON u.id = m.user_id \
             WHERE u.email = $1 AND m.role = 'billing'",
            &[json!("ann@example.test")],
        )
        .await,
        1
    );

    db.cleanup().await;
    fs::remove_dir_all(root).unwrap();
}

/// An expired link does nothing, and says exactly what an unknown one says.
#[ntex::test]
async fn an_expired_invitation_is_indistinguishable_from_one_that_never_existed() {
    let db = TempDatabase::create("inviteold").await;
    let root = temp_dir("inviteold");
    write_files(&root, &[("main.toml", &main_toml(&db.url))]);

    let state = load_state(&root).await;
    let app = init_http_app!(state.clone());
    let admin = verified_user(&state, "admin@example.test", "hunter2").await;
    let org = organization(&state, "Acme Ltd", &admin).await;

    invitation(
        &state,
        &org,
        "late@example.test",
        "member",
        "inv_stale",
        -60 * 60 * 24,
    )
    .await;

    for token in ["inv_stale", "inv_never_existed"] {
        let resp = test::call_service(
            &app,
            test::TestRequest::get()
                .uri(&format!("/api/auth/invitations/{token}"))
                .to_request(),
        )
        .await;
        assert_eq!(resp.status().as_u16(), 404);
    }

    db.cleanup().await;
    fs::remove_dir_all(root).unwrap();
}

/// Issuing one takes the same standing as adding a member, and a send that
/// fails leaves nothing behind pretending to be pending.
#[ntex::test]
async fn an_invitation_that_could_not_be_sent_is_not_left_pending() {
    let db = TempDatabase::create("invitesend").await;
    let root = temp_dir("invitesend");
    write_files(&root, &[("main.toml", &main_toml(&db.url))]);

    let state = load_state(&root).await;
    let app = init_http_app!(state.clone());

    let admin = verified_user(&state, "admin@example.test", "hunter2").await;
    let org = organization(&state, "Acme Ltd", &admin).await;
    let admin_token = state
        .auth
        .issue_token(uuid::Uuid::parse_str(&admin).unwrap())
        .unwrap();

    // A plain member of the organisation may not let anybody else in.
    let member = verified_user(&state, "member@example.test", "hunter2").await;
    seed(
        &state,
        "membership",
        json!({ "user_id": member, "organization_id": org, "role": "member" }),
    )
    .await;
    let member_token = state
        .auth
        .issue_token(uuid::Uuid::parse_str(&member).unwrap())
        .unwrap();

    let invite = |token: &str, body: Value| {
        bearer(
            test::TestRequest::post()
                .uri("/api/auth/invitations")
                .header(CONTENT_TYPE, "application/json")
                .set_payload(body.to_string()),
            token,
        )
        .header("x-organization", org.clone())
        .to_request()
    };

    let resp = test::call_service(
        &app,
        invite(&member_token, json!({"email":"new@example.test"})),
    )
    .await;
    assert_eq!(resp.status().as_u16(), 403);

    // The admin may — but nothing is listening on the SMTP port, so the
    // invitation fails, and must not survive as a row nobody was told about.
    let resp = test::call_service(
        &app,
        invite(&admin_token, json!({"email":"new@example.test"})),
    )
    .await;
    assert_eq!(resp.status().as_u16(), 502);
    assert_eq!(
        count(&state, "SELECT 1 AS hit FROM {invitations}", &[]).await,
        0,
        "a row was left behind for an email that was never sent"
    );

    // Somebody already inside is refused before any of that.
    let resp = test::call_service(
        &app,
        invite(&admin_token, json!({"email":"member@example.test"})),
    )
    .await;
    assert_eq!(resp.status().as_u16(), 409);

    // And an anonymous caller is nobody.
    let resp = test::call_service(
        &app,
        req_json(
            "POST",
            "/api/auth/invitations",
            json!({"email":"new@example.test"}),
        ),
    )
    .await;
    assert_eq!(resp.status().as_u16(), 401);

    db.cleanup().await;
    fs::remove_dir_all(root).unwrap();
}

/// Confirming an address is the end of a detour, and the app can say where it
/// ends: `[auth] verify_email_redirect` comes back beside the session token.
#[ntex::test]
async fn a_confirmed_address_is_told_where_to_go_next() {
    let db = TempDatabase::create("verifyredirect").await;
    let root = temp_dir("verifyredirect");
    write_files(
        &root,
        &[(
            "main.toml",
            &format!(
                "{}\n[auth]\nverify_email_redirect = \"https://app.example.test/welcome\"\n",
                main_toml(&db.url)
            ),
        )],
    );

    let state = load_state(&root).await;
    let user_id = seed(
        &state,
        "user",
        json!({ "email": "bo@example.test", "password_hash": "x" }),
    )
    .await["id"]
        .as_str()
        .unwrap()
        .to_string();
    auth_token(&state, &user_id, "email_verification", "verify_bo").await;

    let app = init_http_app!(state);
    let body = read_json(
        test::call_service(
            &app,
            req_json(
                "POST",
                "/api/auth/verify-email",
                json!({ "token": "verify_bo" }),
            ),
        )
        .await,
    )
    .await;
    // Signed in *and* pointed somewhere: the session has to be in place before
    // the browser leaves, or the app is landed on signed out.
    assert!(body["token"].is_string());
    assert_eq!(body["redirect_to"], "https://app.example.test/welcome");

    db.cleanup().await;
    fs::remove_dir_all(root).unwrap();
}

/// Unset, there is no redirect at all — absent rather than empty, so a client
/// can tell "go here" from "nowhere in particular".
#[ntex::test]
async fn without_the_setting_a_confirmation_points_nowhere() {
    let db = TempDatabase::create("noredirect").await;
    let root = temp_dir("noredirect");
    write_files(&root, &[("main.toml", &main_toml(&db.url))]);

    let state = load_state(&root).await;
    let user_id = seed(
        &state,
        "user",
        json!({ "email": "cy@example.test", "password_hash": "x" }),
    )
    .await["id"]
        .as_str()
        .unwrap()
        .to_string();
    auth_token(&state, &user_id, "email_verification", "verify_cy").await;

    let app = init_http_app!(state);
    let body = read_json(
        test::call_service(
            &app,
            req_json(
                "POST",
                "/api/auth/verify-email",
                json!({ "token": "verify_cy" }),
            ),
        )
        .await,
    )
    .await;
    assert!(body["token"].is_string());
    assert!(body.get("redirect_to").is_none());

    db.cleanup().await;
    fs::remove_dir_all(root).unwrap();
}
