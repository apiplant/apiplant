//! `[rate_limit]`: the app-wide rule, and what a resource or a function says
//! instead of it.

use super::*;

/// A function that answers `{"pong": true}` and nothing else, so the only thing
/// under test is how often it may be called.
fn ping() -> BoxedFunction {
    test_function("ping", Visibility::Public, |_host, _hook, _input| {
        Ok(json!({ "pong": true }).to_string())
    })
}

#[ntex::test]
async fn a_rate_limit_refuses_the_request_after_the_allowance_and_says_when_to_return() {
    let db = TempDatabase::create("ratelimit").await;
    let root = temp_dir("ratelimit");
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

[rate_limit]
default = "3/1m"
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

[permissions]
list = "public"
read = "public"
create = "public"

[fields.title]
type = "string"
required = true

# Narrower than the app-wide rule for one action, and lifted entirely for
# another.
[rate_limit]
create = "1/1m"
list   = "off"
"#,
            ),
        ],
    );

    let state = load_state_configured(
        &root,
        // The function's own limit, from the file a deployment writes rather
        // than from the manifest compiled into it.
        vec![(ping(), json!({ "rate_limit": "1/1m" }).to_string())],
    )
    .await;
    let app = init_http_app!(state);

    let post_note = || req_json("POST", "/api/note", json!({ "title": "hello" }));
    let get = |uri: &'static str| test::TestRequest::get().uri(uri).to_request();
    let call_ping = || req_json("POST", "/api/functions/ping", json!({}));

    // 1. The resource's own `create` rule, not the app-wide one: the second
    //    call is refused even though the app allows three a minute.
    assert_eq!(
        test::call_service(&app, post_note())
            .await
            .status()
            .as_u16(),
        201
    );
    let refused = test::call_service(&app, post_note()).await;
    assert_eq!(refused.status().as_u16(), 429);
    // A client library retries on `Retry-After`; a person reads the rest.
    let headers = refused.headers().clone();
    assert_eq!(headers.get("x-ratelimit-limit").unwrap(), "1");
    assert_eq!(headers.get("x-ratelimit-remaining").unwrap(), "0");
    assert!(headers.contains_key("retry-after"));
    assert!(headers.contains_key("x-ratelimit-reset"));
    let body = read_json(refused).await;
    assert!(
        body["error"].as_str().unwrap().contains("rate limit"),
        "unhelpful body: {body}"
    );

    // 2. `off` lifts the app-wide rule rather than inheriting it, so listing
    //    stays open well past the three a minute everything else gets.
    for _ in 0..6 {
        let response = test::call_service(&app, get("/api/note")).await;
        assert_eq!(response.status().as_u16(), 200);
        // An endpoint with no limit has no allowance to report.
        assert!(!response.headers().contains_key("x-ratelimit-limit"));
    }

    // 3. A function is limited by its own file, not by its neighbours.
    assert_eq!(
        test::call_service(&app, call_ping())
            .await
            .status()
            .as_u16(),
        200
    );
    assert_eq!(
        test::call_service(&app, call_ping())
            .await
            .status()
            .as_u16(),
        429
    );

    // 4. Everything nobody spoke for — here the health check — falls to the
    //    app-wide rule, and shares one allowance with the rest of it.
    for remaining in [2, 1, 0] {
        let response = test::call_service(&app, get("/api/_health")).await;
        assert_eq!(response.status().as_u16(), 200);
        assert_eq!(
            response.headers().get("x-ratelimit-remaining").unwrap(),
            remaining.to_string().as_str()
        );
    }
    assert_eq!(
        test::call_service(&app, get("/api/_health"))
            .await
            .status()
            .as_u16(),
        429
    );
    // The endpoints that spoke for themselves are unaffected by that: they
    // count into their own buckets.
    assert_eq!(
        test::call_service(&app, get("/api/note"))
            .await
            .status()
            .as_u16(),
        200
    );

    fs::remove_dir_all(root).unwrap();
    db.cleanup().await;
}

#[ntex::test]
async fn an_app_that_sets_no_rate_limit_is_limited_nowhere() {
    let db = TempDatabase::create("ratelimitoff").await;
    let root = temp_dir("ratelimitoff");
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
    assert!(
        !state.rate_limit.is_active(),
        "a rate limit nobody asked for is one that starts refusing traffic on an upgrade"
    );
    let app = init_http_app!(state);

    for _ in 0..20 {
        let response = test::call_service(
            &app,
            test::TestRequest::get().uri("/api/_health").to_request(),
        )
        .await;
        assert_eq!(response.status().as_u16(), 200);
    }

    fs::remove_dir_all(root).unwrap();
    db.cleanup().await;
}

/// `enabled = false` is the switch to reach for during an incident: it drops
/// every limit, including the ones written on resources and functions, without
/// editing them.
#[ntex::test]
async fn switching_rate_limiting_off_drops_the_overrides_too() {
    let db = TempDatabase::create("ratelimitkill").await;
    let root = temp_dir("ratelimitkill");
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

[rate_limit]
enabled = false
default = "1/1h"
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

[permissions]
list = "public"

[fields.title]
type = "string"

[rate_limit]
list = "1/1h"
"#,
            ),
        ],
    );

    let state = load_state(&root).await;
    assert!(!state.rate_limit.is_active());
    let app = init_http_app!(state);

    for _ in 0..5 {
        let response =
            test::call_service(&app, test::TestRequest::get().uri("/api/note").to_request()).await;
        assert_eq!(response.status().as_u16(), 200);
    }

    fs::remove_dir_all(root).unwrap();
    db.cleanup().await;
}
