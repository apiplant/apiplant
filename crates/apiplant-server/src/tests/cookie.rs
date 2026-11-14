//! `[auth] cookie`: the session JWT carried in a cookie as well as in the body.

use super::*;

/// The `name=value` part of every `Set-Cookie` a response carries, by name.
fn set_cookies(resp: &ntex::http::Response) -> Vec<(String, String)> {
    resp.headers()
        .get_all("set-cookie")
        .filter_map(|v| v.to_str().ok())
        .filter_map(|v| v.split(';').next())
        .filter_map(|pair| pair.split_once('='))
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect()
}

fn cookie_value<'a>(cookies: &'a [(String, String)], name: &str) -> &'a str {
    &cookies.iter().find(|(k, _)| k == name).unwrap().1
}

#[ntex::test]
async fn a_browser_session_lives_in_a_cookie_and_writes_need_the_csrf_token() {
    let db = TempDatabase::create("cookie").await;
    let root = temp_dir("cookie");
    write_files(
        &root,
        &[(
            "main.toml",
            &format!(
                "[server]\nbase_path = \"/api\"\n\n[database]\nurl = \"{}\"\n\n[auth]\ncookie = true\n",
                db.url
            ),
        )],
    );
    let state = load_state(&root).await;
    let app = init_http_app!(state);

    // Registering hands out the token in the body, as always, and the same
    // token as an HttpOnly cookie beside a readable CSRF token.
    let resp = test::call_service(
        &app,
        req_json(
            "POST",
            "/api/auth/register",
            json!({"email":"ana@acme.test","password":"pw"}),
        ),
    )
    .await;
    assert_eq!(resp.status().as_u16(), 201);
    let cookies = set_cookies(&resp);
    let body = read_json(resp).await;
    let session = cookie_value(&cookies, "apiplant_session").to_string();
    let csrf = cookie_value(&cookies, "apiplant_session_csrf").to_string();
    assert_eq!(body["token"].as_str().unwrap(), session);
    let jar = format!("apiplant_session={session}; apiplant_session_csrf={csrf}");

    // A read is authenticated by the cookie alone.
    let resp = test::call_service(
        &app,
        test::TestRequest::get()
            .uri("/api/auth/me")
            .header("cookie", jar.as_str())
            .to_request(),
    )
    .await;
    assert_eq!(resp.status().as_u16(), 200);

    // A write without the CSRF header is anonymous…
    let mint_key = |csrf: Option<&str>| {
        let mut req = test::TestRequest::post()
            .uri("/api/auth/apikeys")
            .header(CONTENT_TYPE, "application/json")
            .header("cookie", jar.as_str());
        if let Some(csrf) = csrf {
            req = req.header("x-csrf-token", csrf);
        }
        req.payload(json!({"name":"k"}).to_string()).to_request()
    };
    let resp = test::call_service(&app, mint_key(None)).await;
    assert_eq!(resp.status().as_u16(), 401);
    let resp = test::call_service(&app, mint_key(Some("not-it"))).await;
    assert_eq!(resp.status().as_u16(), 401);

    // …and with it, it is the signed-in user.
    let resp = test::call_service(&app, mint_key(Some(&csrf))).await;
    assert!(resp.status().is_success(), "{}", resp.status());

    // A bearer header wins over the cookie, even a bad one.
    let resp = test::call_service(
        &app,
        test::TestRequest::get()
            .uri("/api/auth/me")
            .header("cookie", jar.as_str())
            .header("authorization", "Bearer nonsense")
            .to_request(),
    )
    .await;
    assert_eq!(resp.status().as_u16(), 401);

    // Logging out expires both cookies.
    let resp = test::call_service(
        &app,
        test::TestRequest::post()
            .uri("/api/auth/logout")
            .to_request(),
    )
    .await;
    assert_eq!(resp.status().as_u16(), 204);
    let cleared = set_cookies(&resp);
    assert_eq!(cookie_value(&cleared, "apiplant_session"), "");
    assert_eq!(cookie_value(&cleared, "apiplant_session_csrf"), "");

    db.cleanup().await;
}

#[ntex::test]
async fn without_the_switch_there_is_no_cookie_and_no_logout() {
    let db = TempDatabase::create("cookie_off").await;
    let root = temp_dir("cookie_off");
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
    let app = init_http_app!(state);

    let resp = test::call_service(
        &app,
        req_json(
            "POST",
            "/api/auth/register",
            json!({"email":"ana@acme.test","password":"pw"}),
        ),
    )
    .await;
    assert_eq!(resp.status().as_u16(), 201);
    assert!(set_cookies(&resp).is_empty());
    let token = read_json(resp).await["token"].as_str().unwrap().to_string();

    // A cookie holding a perfectly good token is not read.
    let resp = test::call_service(
        &app,
        test::TestRequest::get()
            .uri("/api/auth/me")
            .header("cookie", format!("apiplant_session={token}"))
            .to_request(),
    )
    .await;
    assert_eq!(resp.status().as_u16(), 401);

    let resp = test::call_service(
        &app,
        test::TestRequest::post()
            .uri("/api/auth/logout")
            .to_request(),
    )
    .await;
    assert_ne!(resp.status().as_u16(), 204);

    db.cleanup().await;
}
