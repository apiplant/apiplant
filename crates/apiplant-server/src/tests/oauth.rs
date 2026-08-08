//! Signing in with somebody else's account.
//!
//! Every case here runs the real handshake against a mock provider on a local
//! port: the app builds an authorize URL, the test plays the browser, and the
//! callback redeems a code and reads a profile over an actual socket. What is
//! being checked is the half that is this crate's — whose account a completed
//! handshake belongs to, and what it takes to complete one at all — since the
//! provider-shaped half has its own tests in `apiplant-oauth`.

use super::*;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

/// What the mock provider will say about whoever signs in next.
#[derive(Clone)]
struct Identity {
    id: String,
    email: Option<String>,
    verified: bool,
    name: String,
}

impl Identity {
    fn new(id: &str, email: Option<&str>, verified: bool) -> Identity {
        Identity {
            id: id.to_string(),
            email: email.map(str::to_string),
            verified,
            name: "Octo Cat".to_string(),
        }
    }
}

/// A provider on a local port: `/token` redeems anything, `/user` and
/// `/user/emails` answer with whatever [`Identity`] is currently set.
///
/// Serving GitHub's shape rather than OIDC's is deliberate — it is the awkward
/// one (a numeric id, and an address that lives behind a second call), so a
/// test that passes through it is exercising the seams.
struct MockProvider {
    origin: String,
    identity: Arc<Mutex<Identity>>,
    /// Every token request the app made, so a test can assert what was sent.
    token_requests: Arc<Mutex<Vec<String>>>,
    handle: tokio::task::JoinHandle<()>,
}

impl MockProvider {
    async fn start(identity: Identity) -> MockProvider {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let origin = format!("http://{}", listener.local_addr().unwrap());
        let identity = Arc::new(Mutex::new(identity));
        let token_requests = Arc::new(Mutex::new(Vec::new()));

        let served = identity.clone();
        let seen = token_requests.clone();
        let handle = tokio::spawn(async move {
            loop {
                let Ok((mut stream, _)) = listener.accept().await else {
                    return;
                };
                let served = served.clone();
                let seen = seen.clone();
                tokio::spawn(async move {
                    let mut buffer = Vec::new();
                    let mut header_end = None;
                    loop {
                        let mut chunk = [0_u8; 1024];
                        let Ok(read) = stream.read(&mut chunk).await else {
                            return;
                        };
                        if read == 0 {
                            break;
                        }
                        buffer.extend_from_slice(&chunk[..read]);
                        if let Some(pos) =
                            buffer.windows(4).position(|window| window == b"\r\n\r\n")
                        {
                            header_end = Some(pos + 4);
                            break;
                        }
                    }
                    let Some(header_end) = header_end else { return };
                    let headers = String::from_utf8_lossy(&buffer[..header_end]).to_string();
                    let length = headers
                        .lines()
                        .find_map(|line| {
                            let lower = line.to_lowercase();
                            lower
                                .strip_prefix("content-length:")
                                .map(|value| value.trim().to_string())
                        })
                        .and_then(|value| value.parse::<usize>().ok())
                        .unwrap_or_default();
                    while buffer.len() < header_end + length {
                        let mut chunk = vec![0_u8; header_end + length - buffer.len()];
                        let Ok(read) = stream.read(&mut chunk).await else {
                            return;
                        };
                        if read == 0 {
                            break;
                        }
                        buffer.extend_from_slice(&chunk[..read]);
                    }
                    let body = String::from_utf8_lossy(&buffer[header_end..]).to_string();
                    let path = headers.split_whitespace().nth(1).unwrap_or("/").to_string();

                    let identity = served.lock().unwrap().clone();
                    let payload = if path.starts_with("/token") {
                        seen.lock().unwrap().push(body.clone());
                        json!({ "access_token": "mock-token", "token_type": "bearer" })
                    } else if path.starts_with("/user/emails") {
                        match &identity.email {
                            Some(email) => json!([{
                                "email": email, "primary": true, "verified": identity.verified
                            }]),
                            None => json!([]),
                        }
                    } else if path.starts_with("/user") {
                        json!({
                            "id": identity.id.parse::<i64>().unwrap_or(1),
                            "login": "octo",
                            "name": identity.name,
                            "avatar_url": "https://example.invalid/octo.png",
                            "email": Value::Null,
                        })
                    } else {
                        json!({ "error": "not found" })
                    };

                    let body = payload.to_string();
                    let response = format!(
                        "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\n\
                         content-length: {}\r\nconnection: close\r\n\r\n{body}",
                        body.len()
                    );
                    let _ = stream.write_all(response.as_bytes()).await;
                    let _ = stream.flush().await;
                });
            }
        });

        MockProvider {
            origin,
            identity,
            token_requests,
            handle,
        }
    }

    fn becomes(&self, identity: Identity) {
        *self.identity.lock().unwrap() = identity;
    }
}

impl Drop for MockProvider {
    fn drop(&mut self) {
        self.handle.abort();
    }
}

/// An app with one provider — `github`, pointed at the mock — plus whatever
/// extra config the test wants.
async fn app_with(
    label: &str,
    provider: &MockProvider,
    extra: &str,
) -> (AppState, PathBuf, TempDatabase) {
    let db = TempDatabase::create(label).await;
    let root = temp_dir(label);
    write_files(
        &root,
        &[(
            "main.toml",
            &format!(
                r#"
[server]
base_path = "/api"
public_url = "https://app.example"

[database]
url = "{url}"

[oauth.github]
client_id = "test-client"
client_secret = "test-secret"
authorize_url = "{origin}/authorize"
token_url = "{origin}/token"
userinfo_url = "{origin}/user"
{extra}
"#,
                url = db.url,
                origin = provider.origin,
            ),
        )],
    );
    let state = load_state(&root).await;
    (state, root, db)
}

/// Play the browser: start a flow, read the `state` out of the authorize URL,
/// and hand it back to the callback with a code.
async fn sign_in<S, E>(
    app: &ntex::service::Pipeline<S>,
    session: Option<&str>,
) -> ntex::web::WebResponse
where
    S: ntex::service::Service<ntex::http::Request, Response = ntex::web::WebResponse, Error = E>,
    E: std::fmt::Debug,
{
    let state = start(app, session).await;
    let request = test::TestRequest::post()
        .uri("/api/auth/oauth/github/callback")
        .header(CONTENT_TYPE, "application/json")
        .set_payload(json!({ "code": "the-code", "state": state }).to_string())
        .to_request();
    test::call_service(app, request).await
}

/// Start a flow and return its `state` parameter.
async fn start<S, E>(app: &ntex::service::Pipeline<S>, session: Option<&str>) -> String
where
    S: ntex::service::Service<ntex::http::Request, Response = ntex::web::WebResponse, Error = E>,
    E: std::fmt::Debug,
{
    let mut request = test::TestRequest::post().uri("/api/auth/oauth/github/start");
    if let Some(token) = session {
        request = request.header("authorization", format!("Bearer {token}"));
    }
    let response = test::call_service(app, request.to_request()).await;
    assert_eq!(response.status(), 200, "start should succeed");
    let body = read_json(response).await;
    body["state"].as_str().expect("a state").to_string()
}

// ---------------------------------------------------------------------------

/// The routes exist only where a provider does — the same bargain `[email]`
/// and `[payments]` make, and the reason an app that configures nothing has no
/// half-working sign-in to discover.
#[ntex::test]
async fn without_a_provider_there_are_no_oauth_routes_and_no_state_table() {
    let db = TempDatabase::create("oauth_off").await;
    let root = temp_dir("oauth_off");
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
    let state = load_state(&root).await;
    assert!(!state.oauth_enabled());
    assert!(
        !state.app.resources.contains_key("oauth_state"),
        "no provider, no machinery"
    );
    // `oauth_connection` is a built-in either way: what somebody's account is
    // known by is worth a shape before anybody uses it.
    assert!(state.app.resources.contains_key("oauth_connection"));

    let app = init_http_app!(state);
    let response = test::call_service(
        &app,
        test::TestRequest::get().uri("/api/auth/oauth").to_request(),
    )
    .await;
    assert_eq!(response.status(), 404);
}

#[ntex::test]
async fn two_credentials_are_the_whole_configuration() {
    let provider = MockProvider::start(Identity::new("4242", Some("octo@example.com"), true)).await;
    let (state, _root, _db) = app_with("oauth_min", &provider, "").await;
    assert!(state.oauth_enabled());
    assert!(state.app.resources.contains_key("oauth_state"));
    let app = init_http_app!(state);

    let response = test::call_service(
        &app,
        test::TestRequest::get().uri("/api/auth/oauth").to_request(),
    )
    .await;
    assert_eq!(response.status(), 200);
    let body = read_json(response).await;
    let listed = &body["providers"][0];
    assert_eq!(listed["provider"], "github");
    assert_eq!(
        listed["label"], "GitHub",
        "the label comes from the built-in"
    );
    assert_eq!(
        listed["start_url"], "https://app.example/api/auth/oauth/github/start",
        "the URL to link to is derived, not configured"
    );
}

/// The end-to-end case: a stranger arrives and leaves with a session the rest
/// of the API accepts.
#[ntex::test]
async fn a_first_sign_in_creates_an_account_and_a_usable_session() {
    let provider = MockProvider::start(Identity::new("4242", Some("octo@example.com"), true)).await;
    let (state, _root, _db) = app_with("oauth_first", &provider, "").await;
    let app = init_http_app!(state);

    let response = sign_in(&app, None).await;
    assert_eq!(response.status(), 200);
    let body = read_json(response).await;
    assert_eq!(body["created"], true);
    assert_eq!(body["linked"], false);
    assert_eq!(body["user"]["email"], "octo@example.com");
    assert_eq!(body["user"]["display_name"], "Octo Cat");

    // The token is the framework's own, so the framework's own endpoint takes
    // it. This is the assertion the whole design rests on.
    let token = body["token"].as_str().unwrap();
    let me = test::call_service(
        &app,
        test::TestRequest::get()
            .uri("/api/auth/me")
            .header("authorization", format!("Bearer {token}"))
            .to_request(),
    )
    .await;
    assert_eq!(me.status(), 200);

    // Registration ran the ordinary path, so the account has somewhere to work
    // — exactly as `POST /auth/register` leaves it.
    let orgs = test::call_service(
        &app,
        test::TestRequest::get()
            .uri("/api/organization")
            .header("authorization", format!("Bearer {token}"))
            .to_request(),
    )
    .await;
    let orgs = read_json(orgs).await;
    assert_eq!(orgs.as_array().map(Vec::len), Some(1), "a personal org");

    // The token request carried the secret and the redirect URI the app
    // registered — not one a caller could choose.
    let sent = provider.token_requests.lock().unwrap().clone();
    assert_eq!(sent.len(), 1);
    assert!(sent[0].contains("client_secret=test-secret"));
    assert!(sent[0].contains("code=the-code"));
    assert!(
        sent[0].contains("app.example"),
        "the stored redirect_uri: {}",
        sent[0]
    );
}

/// Signing in twice is signing in, not signing up.
#[ntex::test]
async fn signing_in_again_reaches_the_same_account() {
    let provider = MockProvider::start(Identity::new("4242", Some("octo@example.com"), true)).await;
    let (state, _root, _db) = app_with("oauth_again", &provider, "").await;
    let app = init_http_app!(state);

    let first = read_json(sign_in(&app, None).await).await;
    // Same provider account, different name and address than last time —
    // neither of which identifies anybody.
    provider.becomes(Identity {
        id: "4242".into(),
        email: Some("moved@example.com".into()),
        verified: true,
        name: "Octo Renamed".into(),
    });
    let second = read_json(sign_in(&app, None).await).await;

    assert_eq!(second["created"], false);
    assert_eq!(
        second["user"]["id"], first["user"]["id"],
        "the provider's id is what identifies somebody, not their address"
    );
}

/// A `state` is spent once. Everything else — never issued, expired, another
/// provider's — is refused the same way and with the same words.
#[ntex::test]
async fn a_state_cannot_be_replayed_or_invented() {
    let provider = MockProvider::start(Identity::new("4242", Some("octo@example.com"), true)).await;
    let (state, _root, _db) = app_with("oauth_state", &provider, "").await;
    let app = init_http_app!(state);

    let state_value = start(&app, None).await;
    let callback = |state: String| {
        test::TestRequest::post()
            .uri("/api/auth/oauth/github/callback")
            .header(CONTENT_TYPE, "application/json")
            .set_payload(json!({ "code": "the-code", "state": state }).to_string())
            .to_request()
    };

    let first = test::call_service(&app, callback(state_value.clone())).await;
    assert_eq!(first.status(), 200);

    let replay = test::call_service(&app, callback(state_value)).await;
    assert_eq!(replay.status(), 400);
    let invented = test::call_service(&app, callback("not-a-state".into())).await;
    assert_eq!(invented.status(), 400);
    assert_eq!(
        read_json(replay).await["error"],
        read_json(invented).await["error"],
        "a replay and a guess get the same answer: telling them apart answers \
         a question only an attacker is asking"
    );
}

/// The rule the whole feature turns on: a *verified* address may reach an
/// existing account, and an unverified one may never.
#[ntex::test]
async fn only_a_verified_address_reaches_an_existing_account() {
    let provider = MockProvider::start(Identity::new("1", Some("ann@example.com"), true)).await;
    let (state, _root, _db) = app_with("oauth_match", &provider, "").await;
    let app = init_http_app!(state);

    // Somebody who registered with a password.
    let registered = test::call_service(
        &app,
        req_json(
            "POST",
            "/api/auth/register",
            json!({ "email": "ann@example.com", "password": "hunter2222" }),
        ),
    )
    .await;
    assert_eq!(registered.status(), 201);
    let ann = read_json(registered).await["user"]["id"]
        .as_str()
        .unwrap()
        .to_string();

    // …coming back through a provider that verified the same address.
    let matched = read_json(sign_in(&app, None).await).await;
    assert_eq!(matched["created"], false);
    assert_eq!(matched["user"]["id"], ann, "same person, second door");

    // A different provider account claiming the same address, unverified.
    provider.becomes(Identity::new("999", Some("ann@example.com"), false));
    let response = sign_in(&app, None).await;
    assert_eq!(
        response.status(),
        409,
        "an unverified address must never reach somebody else's account"
    );
    let message = read_json(response).await["error"]
        .as_str()
        .unwrap()
        .to_string();
    assert!(
        message.contains("already uses") && message.contains("connect"),
        "and the refusal should say what to do instead: {message}"
    );

    // Ann still holds the account, and it still has her address.
    let still_hers = state
        .db
        .raw_json(
            "SELECT id::text AS id FROM apiplant_user WHERE email = $1",
            &[json!("ann@example.com")],
        )
        .await
        .unwrap();
    assert_eq!(still_hers.as_array().map(Vec::len), Some(1));
    assert_eq!(still_hers[0]["id"], ann);
}

/// Turning the convenience off means a match makes a second account instead —
/// inconvenient, and never wrong.
#[ntex::test]
async fn link_by_verified_email_can_be_switched_off() {
    let provider = MockProvider::start(Identity::new("1", Some("ann@example.com"), true)).await;
    let (state, _root, _db) = app_with(
        "oauth_nomatch",
        &provider,
        "\n[oauth]\nlink_by_verified_email = false\n",
    )
    .await;
    let app = init_http_app!(state);

    let registered = test::call_service(
        &app,
        req_json(
            "POST",
            "/api/auth/register",
            json!({ "email": "ann@example.com", "password": "hunter2222" }),
        ),
    )
    .await;
    let ann = read_json(registered).await["user"]["id"]
        .as_str()
        .unwrap()
        .to_string();

    // Off means never *automatically*. The address is taken, so the sign-in
    // refuses and says how to connect the two deliberately — which is the whole
    // point of switching it off.
    let response = sign_in(&app, None).await;
    assert_eq!(response.status(), 409);
    let _ = ann;

    // Deliberately is the `POST …/start` with a session: link, then in.
    let token = read_json(
        test::call_service(
            &app,
            req_json(
                "POST",
                "/api/auth/login",
                json!({ "email": "ann@example.com", "password": "hunter2222" }),
            ),
        )
        .await,
    )
    .await["token"]
        .as_str()
        .unwrap()
        .to_string();
    let linked = read_json(sign_in(&app, Some(&token)).await).await;
    assert_eq!(linked["linked"], true);
    let signed_in = read_json(sign_in(&app, None).await).await;
    assert_eq!(signed_in["user"]["email"], "ann@example.com");
}

/// A provider with no address to give still signs somebody in — the account
/// simply has no address, and says so rather than pretending.
///
/// The saying-so is `email_placeholder`, and it is in the built-in `user`
/// resource: this app has no `resources/users.toml` at all, because an app that has
/// never heard of the flag is exactly the one that would otherwise mail an
/// address apiplant made up.
#[ntex::test]
async fn a_provider_that_gives_no_address_still_works() {
    let provider = MockProvider::start(Identity::new("77", None, false)).await;
    let (state, _root, _db) = app_with("oauth_noemail", &provider, "").await;
    let app = init_http_app!(state);

    let body = read_json(sign_in(&app, None).await).await;
    assert_eq!(body["created"], true);
    let email = body["user"]["email"].as_str().unwrap();
    assert!(
        email.ends_with("@oauth.invalid"),
        "a placeholder at a domain that can never resolve, not a guess: {email}"
    );
    assert_eq!(
        body["user"]["email_placeholder"], true,
        "and the row says the address was invented"
    );
    assert!(body["token"].as_str().is_some_and(|t| !t.is_empty()));

    // A provider that *does* give an address leaves the flag alone.
    provider.becomes(Identity::new("78", Some("real@example.com"), true));
    let real = read_json(sign_in(&app, None).await).await;
    assert_eq!(real["user"]["email_placeholder"], false);
}

/// Starting a flow with a session links rather than signs in — and the
/// decision is taken from the session, not from anything the callback carries.
#[ntex::test]
async fn a_session_at_the_start_links_the_provider_to_that_account() {
    let provider =
        MockProvider::start(Identity::new("5", Some("someone@else.example"), true)).await;
    let (state, _root, _db) = app_with("oauth_link", &provider, "").await;
    let app = init_http_app!(state);

    let registered = test::call_service(
        &app,
        req_json(
            "POST",
            "/api/auth/register",
            json!({ "email": "eli@example.com", "password": "hunter2222" }),
        ),
    )
    .await;
    let registered = read_json(registered).await;
    let token = registered["token"].as_str().unwrap().to_string();
    let eli = registered["user"]["id"].as_str().unwrap().to_string();

    let linked = read_json(sign_in(&app, Some(&token)).await).await;
    assert_eq!(linked["linked"], true);
    assert_eq!(linked["created"], false);
    assert_eq!(linked["user"]["id"], eli);
    assert_eq!(
        linked["user"]["email"], "eli@example.com",
        "linking a provider does not rewrite the account's own address"
    );

    // And now that provider account signs in as Eli with no session at all.
    let anonymous = read_json(sign_in(&app, None).await).await;
    assert_eq!(anonymous["user"]["id"], eli);
    assert_eq!(anonymous["linked"], false);

    // The connection is visible to its owner, and to nobody else.
    let connections = test::call_service(
        &app,
        test::TestRequest::get()
            .uri("/api/oauth_connection")
            .header("authorization", format!("Bearer {token}"))
            .to_request(),
    )
    .await;
    let connections = read_json(connections).await;
    assert_eq!(connections.as_array().map(Vec::len), Some(1));
    assert_eq!(connections[0]["provider"], "github");
    assert_eq!(connections[0]["provider_key"], "github:5");
}

/// A provider account belongs to one account here. Somebody else's session
/// cannot take it over by linking.
#[ntex::test]
async fn a_linked_provider_account_cannot_be_claimed_by_somebody_else() {
    let provider = MockProvider::start(Identity::new("5", Some("shared@example.com"), true)).await;
    let (state, _root, _db) = app_with("oauth_steal", &provider, "").await;
    let app = init_http_app!(state);

    // It belongs to whoever signed in with it first.
    sign_in(&app, None).await;

    let other = read_json(
        test::call_service(
            &app,
            req_json(
                "POST",
                "/api/auth/register",
                json!({ "email": "mallory@example.com", "password": "hunter2222" }),
            ),
        )
        .await,
    )
    .await;
    let token = other["token"].as_str().unwrap().to_string();

    let response = sign_in(&app, Some(&token)).await;
    assert_eq!(response.status(), 409);
}

/// Unlinking is allowed until it would lock somebody out of their own account.
#[ntex::test]
async fn the_last_way_into_an_account_cannot_be_unlinked() {
    let provider = MockProvider::start(Identity::new("8", Some("solo@example.com"), true)).await;
    let (state, _root, _db) = app_with("oauth_unlink", &provider, "").await;
    let app = init_http_app!(state);

    // An account with no password: the connection is the only credential.
    let signed_in = read_json(sign_in(&app, None).await).await;
    let token = signed_in["token"].as_str().unwrap().to_string();

    let refused = test::call_service(
        &app,
        test::TestRequest::delete()
            .uri("/api/auth/oauth/github")
            .header("authorization", format!("Bearer {token}"))
            .to_request(),
    )
    .await;
    assert_eq!(refused.status(), 409);
    let message = read_json(refused).await["error"]
        .as_str()
        .unwrap()
        .to_string();
    assert!(message.contains("only way into this account"), "{message}");

    // An account that also has a password may unlink freely.
    let registered = read_json(
        test::call_service(
            &app,
            req_json(
                "POST",
                "/api/auth/register",
                json!({ "email": "both@example.com", "password": "hunter2222" }),
            ),
        )
        .await,
    )
    .await;
    let with_password = registered["token"].as_str().unwrap().to_string();
    provider.becomes(Identity::new("9", Some("both@example.com"), true));
    sign_in(&app, Some(&with_password)).await;

    let allowed = test::call_service(
        &app,
        test::TestRequest::delete()
            .uri("/api/auth/oauth/github")
            .header("authorization", format!("Bearer {with_password}"))
            .to_request(),
    )
    .await;
    assert_eq!(allowed.status(), 200);
    assert_eq!(read_json(allowed).await["removed"], 1);
}

/// An app with signup closed has closed it. A provider button is not a side
/// door — but it is still a way to *sign in* to an account that already exists.
#[ntex::test]
async fn registration_being_disabled_stops_new_accounts_but_not_returning_ones() {
    let provider = MockProvider::start(Identity::new("11", Some("known@example.com"), true)).await;
    let (state, _root, _db) = app_with(
        "oauth_closed",
        &provider,
        "\n[auth]\nallow_registration = false\n",
    )
    .await;
    let app = init_http_app!(state);

    let refused = sign_in(&app, None).await;
    assert_eq!(refused.status(), 403);

    // Now the account exists — added the way a closed app adds people, by an
    // admin rather than by a signup form. The same provider, the same verified
    // address, and this time it is a sign-in rather than a sign-up.
    let user_r = state.app.resources.get("user").unwrap();
    let mut row = serde_json::Map::new();
    row.insert("email".into(), json!("known@example.com"));
    row.insert(
        "email_verified_at".into(),
        json!(chrono::Utc::now().to_rfc3339()),
    );
    state.db.create(user_r, &row).await.unwrap();

    let allowed = sign_in(&app, None).await;
    assert_eq!(
        allowed.status(),
        200,
        "closing signup closes the door to strangers, not to the people already inside"
    );
    let body = read_json(allowed).await;
    assert_eq!(body["created"], false);
    assert_eq!(body["user"]["email"], "known@example.com");
}

/// The browser-facing pair: a link out, and a redirect back carrying the token
/// where `token_delivery` says to put it.
#[ntex::test]
async fn the_redirecting_endpoints_work_with_no_front_end_at_all() {
    let provider =
        MockProvider::start(Identity::new("12", Some("browser@example.com"), true)).await;
    let (state, _root, _db) = app_with("oauth_browser", &provider, "").await;
    let app = init_http_app!(state);

    let out = test::call_service(
        &app,
        test::TestRequest::get()
            .uri("/api/auth/oauth/github/start?return_to=/dashboard")
            .to_request(),
    )
    .await;
    assert_eq!(out.status(), 302);
    let location = out
        .headers()
        .get("location")
        .and_then(|v| v.to_str().ok())
        .unwrap()
        .to_string();
    assert!(location.contains("/authorize?"), "{location}");
    assert!(location.contains("client_id=test-client"));
    let state_value = location
        .split("state=")
        .nth(1)
        .and_then(|rest| rest.split('&').next())
        .unwrap()
        .to_string();

    let back = test::call_service(
        &app,
        test::TestRequest::get()
            .uri(&format!(
                "/api/auth/oauth/github/callback?code=the-code&state={state_value}"
            ))
            .to_request(),
    )
    .await;
    assert_eq!(back.status(), 302);
    let landing = back
        .headers()
        .get("location")
        .and_then(|v| v.to_str().ok())
        .unwrap()
        .to_string();
    assert!(
        landing.starts_with("/dashboard#token="),
        "the fragment keeps the token out of server logs: {landing}"
    );
}

/// A name and a picture land on the account, in the columns the built-in resource
/// declares — and in whichever columns an app says instead.
#[ntex::test]
async fn a_sign_in_fills_in_a_name_and_a_picture() {
    let provider = MockProvider::start(Identity::new("41", Some("pic@example.com"), true)).await;
    let (state, _root, _db) = app_with("oauth_profile", &provider, "").await;
    let app = init_http_app!(state);

    let user = read_json(sign_in(&app, None).await).await["user"].clone();
    assert_eq!(user["display_name"], "Octo Cat");
    assert_eq!(user["avatar_url"], "https://example.invalid/octo.png");

    // Both are refreshed on the way back in, because people change their name
    // and their picture and a copy that is only right on day one is worse than
    // none.
    provider.becomes(Identity {
        id: "41".into(),
        email: Some("pic@example.com".into()),
        verified: true,
        name: "Octo Renamed".into(),
    });
    let again = read_json(sign_in(&app, None).await).await;
    assert_eq!(again["created"], false);
    let connection = read_json(
        test::call_service(
            &app,
            test::TestRequest::get()
                .uri("/api/oauth_connection")
                .header(
                    "authorization",
                    format!("Bearer {}", again["token"].as_str().unwrap()),
                )
                .to_request(),
        )
        .await,
    )
    .await;
    assert_eq!(connection[0]["display_name"], "Octo Renamed");
}

/// An app that keeps its own name, and calls its picture something else.
#[ntex::test]
async fn where_the_profile_lands_is_the_apps_to_say() {
    let provider = MockProvider::start(Identity::new("42", Some("mine@example.com"), true)).await;
    let (state, _root, _db) = app_with(
        "oauth_fields",
        &provider,
        "\n[oauth]\nname_field = \"\"\navatar_field = \"picture\"\n",
    )
    .await;
    // The column has to exist for the value to land in it; `keep_declared`
    // drops what the resource does not declare, which is what lets an app opt out
    // by simply not having the column.
    let root = _root.clone();
    std::fs::write(
        root.join("resources/users.toml"),
        r#"
[resource]
name = "user"
scope = "global"

[auth]
identity_field = "email"
password_field = "password_hash"

[fields.email]
type = "string"
required = true
unique = true

[fields.password_hash]
type = "string"
hidden = true

[fields.display_name]
type = "string"

[fields.picture]
type = "string"
"#,
    )
    .unwrap();
    let state = load_state(&root).await;
    let _ = state.app.resources.get("user").expect("user");
    let app = init_http_app!(state);

    let user = read_json(sign_in(&app, None).await).await["user"].clone();
    assert_eq!(user["picture"], "https://example.invalid/octo.png");
    assert!(
        user["display_name"].is_null(),
        "an empty name_field writes no name: {user}"
    );
}

/// A provider apiplant draws no mark for can bring its own image, and it
/// reaches the client that has to draw the button.
#[ntex::test]
async fn an_unknown_provider_can_carry_its_own_icon() {
    let provider = MockProvider::start(Identity::new("43", Some("gl@example.com"), true)).await;
    let (state, _root, _db) = app_with(
        "oauth_icon",
        &provider,
        &format!(
            "\n[oauth.gitlab]\nclient_id = \"gl\"\nclient_secret = \"s\"\n\
             authorize_url = \"{origin}/authorize\"\ntoken_url = \"{origin}/token\"\n\
             userinfo_url = \"{origin}/user\"\nscopes = \"openid email\"\n\
             icon = \"/oauth/gitlab.svg\"\n",
            origin = provider.origin,
        ),
    )
    .await;
    let app = init_http_app!(state);

    let listed = read_json(
        test::call_service(
            &app,
            test::TestRequest::get().uri("/api/auth/oauth").to_request(),
        )
        .await,
    )
    .await;
    let providers = listed["providers"].as_array().unwrap().clone();
    let gitlab = providers
        .iter()
        .find(|p| p["provider"] == "gitlab")
        .expect("gitlab");
    assert_eq!(gitlab["icon"], "/oauth/gitlab.svg");
    assert_eq!(gitlab["label"], "Gitlab");
    // The four apiplant draws itself carry none, so a client knows to use its
    // own mark rather than an image.
    let github = providers
        .iter()
        .find(|p| p["provider"] == "github")
        .expect("github");
    assert_eq!(github["icon"], "");
}

/// A client that knows how it wants the token says so, and the app's default
/// stands for everybody else.
#[ntex::test]
async fn a_caller_can_choose_how_the_token_reaches_it() {
    let provider = MockProvider::start(Identity::new("21", Some("pick@example.com"), true)).await;
    let (state, _root, _db) = app_with(
        "oauth_delivery",
        &provider,
        "\n[oauth]\ntoken_delivery = \"query\"\n",
    )
    .await;
    let app = init_http_app!(state);

    // The app's own setting: a query parameter.
    let landing = browser_flow(&app, "").await;
    assert!(landing.starts_with("/?token="), "{landing}");

    // A client that would rather have a fragment — the admin dashboard does —
    // asks for one when it starts the flow, and the callback remembers.
    let landing = browser_flow(&app, "&token_delivery=fragment").await;
    assert!(landing.starts_with("/#token="), "{landing}");

    // Anything unrecognised leaves the app's setting in force.
    let landing = browser_flow(&app, "&token_delivery=carrier-pigeon").await;
    assert!(landing.starts_with("/?token="), "{landing}");
}

/// Walk the redirecting pair the way a browser would, returning where it lands.
async fn browser_flow<S, E>(app: &ntex::service::Pipeline<S>, extra: &str) -> String
where
    S: ntex::service::Service<ntex::http::Request, Response = ntex::web::WebResponse, Error = E>,
    E: std::fmt::Debug,
{
    let out = test::call_service(
        app,
        test::TestRequest::get()
            .uri(&format!("/api/auth/oauth/github/start?return_to=/{extra}"))
            .to_request(),
    )
    .await;
    let location = out
        .headers()
        .get("location")
        .and_then(|v| v.to_str().ok())
        .unwrap()
        .to_string();
    let state_value = location
        .split("state=")
        .nth(1)
        .and_then(|rest| rest.split('&').next())
        .unwrap()
        .to_string();
    let back = test::call_service(
        app,
        test::TestRequest::get()
            .uri(&format!(
                "/api/auth/oauth/github/callback?code=the-code&state={state_value}"
            ))
            .to_request(),
    )
    .await;
    back.headers()
        .get("location")
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_string()
}

/// An open redirect is how a sign-in page becomes a phishing hop, so
/// `return_to` is accepted as a shape rather than checked against a list.
#[ntex::test]
async fn return_to_cannot_leave_this_site() {
    let provider = MockProvider::start(Identity::new("13", Some("safe@example.com"), true)).await;
    let (state, _root, _db) = app_with("oauth_return", &provider, "").await;
    let app = init_http_app!(state);

    for hostile in ["//evil.example", "https://evil.example", "/\\evil.example"] {
        let out = test::call_service(
            &app,
            test::TestRequest::get()
                .uri(&format!(
                    "/api/auth/oauth/github/start?return_to={}",
                    urlencode(hostile)
                ))
                .to_request(),
        )
        .await;
        let location = out
            .headers()
            .get("location")
            .and_then(|v| v.to_str().ok())
            .unwrap()
            .to_string();
        let state_value = location
            .split("state=")
            .nth(1)
            .and_then(|rest| rest.split('&').next())
            .unwrap()
            .to_string();

        let back = test::call_service(
            &app,
            test::TestRequest::get()
                .uri(&format!(
                    "/api/auth/oauth/github/callback?code=the-code&state={state_value}"
                ))
                .to_request(),
        )
        .await;
        let landing = back
            .headers()
            .get("location")
            .and_then(|v| v.to_str().ok())
            .unwrap()
            .to_string();
        assert!(
            landing.starts_with("/#token="),
            "`{hostile}` should have fallen back to the configured landing page, got {landing}"
        );
    }
}

/// Somebody who pressed Cancel is not an error to debug.
#[ntex::test]
async fn a_refusal_at_the_provider_comes_back_as_a_refusal() {
    let provider = MockProvider::start(Identity::new("14", Some("nope@example.com"), true)).await;
    let (state, _root, _db) = app_with("oauth_cancel", &provider, "").await;
    let app = init_http_app!(state);

    let response = test::call_service(
        &app,
        test::TestRequest::post()
            .uri("/api/auth/oauth/github/callback")
            .header(CONTENT_TYPE, "application/json")
            .set_payload(
                json!({ "error": "access_denied", "error_description": "the user said no" })
                    .to_string(),
            )
            .to_request(),
    )
    .await;
    assert_eq!(response.status(), 400);
    let message = read_json(response).await["error"]
        .as_str()
        .unwrap()
        .to_string();
    assert!(message.contains("the user said no"), "{message}");
}

/// A provider nobody configured is not a 500 and not a redirect to nowhere.
#[ntex::test]
async fn an_unconfigured_provider_is_a_404() {
    let provider = MockProvider::start(Identity::new("15", Some("a@b.example"), true)).await;
    let (state, _root, _db) = app_with("oauth_unknown", &provider, "").await;
    let app = init_http_app!(state);

    let response = test::call_service(
        &app,
        test::TestRequest::post()
            .uri("/api/auth/oauth/google/start")
            .to_request(),
    )
    .await;
    assert_eq!(response.status(), 404);
}

fn urlencode(value: &str) -> String {
    value
        .replace('/', "%2F")
        .replace('\\', "%5C")
        .replace(':', "%3A")
}

/// The last-credential check cannot be walked around.
///
/// `oauth_connection` is an ordinary resource, so a `delete = "owner"` on it
/// would put a second door beside the guarded endpoint — one that deletes the
/// row without asking what else is left, and locks somebody out of their own
/// account for good. That is why the built-in resource makes `delete` private:
/// there is exactly one way to unlink, and it is the way that checks.
#[ntex::test]
async fn a_connection_cannot_be_deleted_around_the_check() {
    let provider = MockProvider::start(Identity::new("31", Some("only@example.com"), true)).await;
    let (state, _root, _db) = app_with("oauth_bypass", &provider, "").await;
    let app = init_http_app!(state);

    // An account whose only credential is this connection: no password, and
    // nothing else linked.
    let token = read_json(sign_in(&app, None).await).await["token"]
        .as_str()
        .unwrap()
        .to_string();
    let listed = read_json(
        test::call_service(
            &app,
            test::TestRequest::get()
                .uri("/api/oauth_connection")
                .header("authorization", format!("Bearer {token}"))
                .to_request(),
        )
        .await,
    )
    .await;
    // Reading them is the point of the resource, and still works.
    assert_eq!(listed.as_array().map(Vec::len), Some(1));
    let id = listed[0]["id"].as_str().unwrap().to_string();

    let guarded = test::call_service(
        &app,
        test::TestRequest::delete()
            .uri("/api/auth/oauth/github")
            .header("authorization", format!("Bearer {token}"))
            .to_request(),
    )
    .await;
    assert_eq!(guarded.status(), 409, "the endpoint that checks refuses");

    let crud = test::call_service(
        &app,
        test::TestRequest::delete()
            .uri(&format!("/api/oauth_connection/{id}"))
            .header("authorization", format!("Bearer {token}"))
            .to_request(),
    )
    .await;
    assert_eq!(
        crud.status(),
        404,
        "and the row cannot be deleted out from under it"
    );

    // Which means the account still has its way in.
    let me = test::call_service(
        &app,
        test::TestRequest::get()
            .uri("/api/auth/me")
            .header("authorization", format!("Bearer {token}"))
            .to_request(),
    )
    .await;
    assert_eq!(me.status(), 200);
}
