//! The two switches that change what an app *is*: `[auth] enabled` and
//! `[organization] enabled`.
//!
//! Both are exercised end to end rather than at the predicates, because that is
//! where they could go wrong: three different enforcement styles read the same
//! three primitives, and a test of the primitives would pass while any one of
//! the three quietly kept its own rules.

use super::*;

/// A resource nobody may reach without a role, so a passing request is a
/// permission check that was actually answered rather than one that was never
/// asked.
const NOTES_TOML: &str = r#"
[resource]
name = "note"
scope = "organization"

[permissions]
list   = "role:admin"
read   = "role:admin"
create = "role:admin"
update = "role:admin"
delete = "role:admin"

[fields.title]
type = "string"
required = true
"#;

/// A resource that is not there at all, for anybody.
const SECRET_TOML: &str = r#"
[resource]
name = "secret"
scope = "global"

[permissions]
list   = "private"
read   = "private"
create = "private"
update = "private"
delete = "private"

[fields.title]
type = "string"
"#;

fn main_toml(db_url: &str, extra: &str) -> String {
    format!("[server]\nbase_path = \"/api\"\n\n[database]\nurl = \"{db_url}\"\n\n{extra}")
}

/// With no accounts, an anonymous caller is the administrator: a `role:admin`
/// resource is fully readable and writable without a token anywhere.
///
/// And `private` is still `private` — it is not a permission somebody holds but
/// a statement that something is not reachable, so the one thing this mode does
/// not do is open it.
#[ntex::test]
async fn auth_disabled_lets_anybody_do_anything_except_reach_private() {
    let db = TempDatabase::create("auth_off").await;
    let root = temp_dir("auth_off");
    write_files(
        &root,
        &[
            (
                "main.toml",
                &main_toml(&db.url, "[auth]\nenabled = false\n"),
            ),
            ("resources/notes.toml", NOTES_TOML),
            ("resources/secrets.toml", SECRET_TOML),
        ],
    );

    let state = load_state(&root).await;
    let app = init_http_app!(state);

    // Create, with no credential of any kind.
    let created = test::call_service(
        &app,
        req_json("POST", "/api/note", json!({ "title": "hello" })),
    )
    .await;
    assert_eq!(
        created.status(),
        201,
        "anonymous create on a role:admin resource"
    );
    let note = read_json(created).await;
    let id = note["id"].as_str().unwrap().to_string();

    // The row belongs to the one organisation, which migrate seeded.
    assert_eq!(
        note["organization_id"].as_str(),
        Some(apiplant_core::SOLO_ORGANIZATION_ID),
        "rows land in the solo organisation"
    );

    let listed =
        test::call_service(&app, test::TestRequest::get().uri("/api/note").to_request()).await;
    assert_eq!(listed.status(), 200);

    let updated = test::call_service(
        &app,
        req_json(
            "PATCH",
            &format!("/api/note/{id}"),
            json!({ "title": "again" }),
        ),
    )
    .await;
    assert_eq!(updated.status(), 200);

    let deleted = test::call_service(
        &app,
        test::TestRequest::delete()
            .uri(&format!("/api/note/{id}"))
            .to_request(),
    )
    .await;
    assert_eq!(deleted.status(), 204);

    // `private` is not a lock this key opens.
    let secret = test::call_service(
        &app,
        test::TestRequest::get().uri("/api/secret").to_request(),
    )
    .await;
    assert_eq!(secret.status(), 404, "private stays private with auth off");

    db.cleanup().await;
}

/// The `/auth/*` endpoints are not mounted, and neither are the account tables.
#[ntex::test]
async fn auth_disabled_mounts_no_auth_endpoints_and_creates_no_account_tables() {
    let db = TempDatabase::create("auth_off_routes").await;
    let root = temp_dir("auth_off_routes");
    write_files(
        &root,
        &[(
            "main.toml",
            &main_toml(&db.url, "[auth]\nenabled = false\n"),
        )],
    );

    let app_config = App::load(&root).unwrap();
    for absent in [
        "user",
        "membership",
        "membership_role",
        "api_key",
        "oauth_connection",
        "invitation",
        "auth_token",
    ] {
        assert!(
            !app_config.resources.contains_key(absent),
            "`{absent}` should not exist in an app with no accounts"
        );
    }
    assert!(
        app_config.resources.contains_key("organization"),
        "the tenant is not an auth table and stays"
    );

    let state = load_state(&root).await;
    let app = init_http_app!(state);

    // Unmounted, so the path falls through to the generic `/{resource}/{id}`
    // routes and is refused there. Which refusal it is — a 404 for a resource
    // called `auth`, or a 405 because those routes take other methods — is not
    // the point; that no session comes back is.
    for path in ["/api/auth/login", "/api/auth/register"] {
        let response = test::call_service(
            &app,
            req_json(
                "POST",
                path,
                json!({ "email": "a@example.com", "password": "pw" }),
            ),
        )
        .await;
        assert!(
            response.status().is_client_error(),
            "{path} should not sign anybody in: {}",
            response.status()
        );
    }

    db.cleanup().await;
}

/// A resource that references `user` in an app that has no `user` is refused at
/// load, where the file and the field are still known — rather than at the
/// first `migrate`, as a Postgres error about a missing relation.
#[ntex::test]
async fn auth_disabled_refuses_a_resource_that_references_an_account() {
    let root = temp_dir("auth_off_refs");
    write_files(
        &root,
        &[
            (
                "main.toml",
                "[database]\nurl = \"postgres://postgres@127.0.0.1:5432/unused\"\n\n[auth]\nenabled = false\n",
            ),
            (
                "resources/posts.toml",
                "[resource]\nname = \"post\"\nscope = \"global\"\n\n[fields.author_id]\ntype = \"reference\"\nreferences = \"user\"\n",
            ),
        ],
    );

    let error = App::load(&root).unwrap_err().to_string();
    assert!(error.contains("author_id"), "{error}");
    assert!(error.contains("user"), "{error}");
    assert!(error.contains("[auth] enabled = false"), "{error}");
}

/// With one organisation, two unrelated accounts see the same rows: `member`
/// and `role:` stop being questions about where somebody stands, and the
/// `X-Organization` header stops selecting anything.
#[ntex::test]
async fn organizations_disabled_put_everybody_in_one_tenant() {
    let db = TempDatabase::create("orgs_off").await;
    let root = temp_dir("orgs_off");
    write_files(
        &root,
        &[
            (
                "main.toml",
                &main_toml(&db.url, "[organization]\nenabled = false\n"),
            ),
            ("resources/notes.toml", NOTES_TOML),
        ],
    );

    let state = load_state(&root).await;
    let app = init_http_app!(state);

    let register = |email: &'static str| {
        req_json(
            "POST",
            "/api/auth/register",
            json!({ "email": email, "password": "pw" }),
        )
    };
    let alice = read_json(test::call_service(&app, register("alice@example.com")).await).await;
    let alice_token = alice["token"].as_str().unwrap().to_string();
    let bob = read_json(test::call_service(&app, register("bob@example.com")).await).await;
    let bob_token = bob["token"].as_str().unwrap().to_string();

    // Alice writes without ever naming an organisation, on a `role:admin`
    // resource she was never granted a role in.
    let created = test::call_service(
        &app,
        bearer(
            test::TestRequest::post()
                .uri("/api/note")
                .header(CONTENT_TYPE, "application/json")
                .set_payload(json!({ "title": "shared" }).to_string()),
            &alice_token,
        )
        .to_request(),
    )
    .await;
    assert_eq!(
        created.status(),
        201,
        "no membership needed with one tenant"
    );
    let note = read_json(created).await;
    assert_eq!(
        note["organization_id"].as_str(),
        Some(apiplant_core::SOLO_ORGANIZATION_ID)
    );

    // Bob sees it, and an `X-Organization` naming somewhere else is ignored
    // rather than refused: there is nothing for it to select.
    let listed = read_json(
        test::call_service(
            &app,
            bearer(test::TestRequest::get().uri("/api/note"), &bob_token)
                .header("x-organization", Uuid::new_v4().to_string())
                .to_request(),
        )
        .await,
    )
    .await;
    let rows = listed["data"]
        .as_array()
        .unwrap_or_else(|| listed.as_array().expect("a list response"));
    assert_eq!(rows.len(), 1, "one tenant, one set of rows: {listed}");

    db.cleanup().await;
}

/// Auth off is tenancy off, whatever `[organization] enabled` says: an app with
/// no accounts has nobody to be a member of anything.
#[ntex::test]
async fn auth_disabled_forces_organizations_off() {
    let root = temp_dir("orgs_derived");
    write_files(
        &root,
        &[(
            "main.toml",
            "[database]\nurl = \"postgres://postgres@127.0.0.1:5432/unused\"\n\n[auth]\nenabled = false\n\n[organization]\nenabled = true\n",
        )],
    );

    let app = App::load(&root).unwrap();
    assert!(!app.config.auth_enabled());
    assert!(
        !app.config.organizations_enabled(),
        "tenancy cannot outlive the accounts it is made of"
    );
}
