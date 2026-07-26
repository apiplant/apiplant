//! What the server serves besides the API: the docs, the dashboard, the
//! app's `public/` directory and its 404 page.

use super::*;

#[ntex::test]
async fn docs_and_host_filtering_work() {
    let db = TempDatabase::create("docs").await;
    let root = temp_dir("docs");
    write_files(
        &root,
        &[(
            "main.toml",
            &format!(
                r#"
[server]
base_path = "/api"
domain = "api.example.test"

[database]
url = "{}"

[docs]
title = "Doc Test"
"#,
                db.url
            ),
        )],
    );

    let state = load_state(&root).await;
    let app = init_http_app!(state);

    let resp = test::call_service(
        &app,
        test::TestRequest::get().uri("/api/_health").to_request(),
    )
    .await;
    assert_eq!(resp.status().as_u16(), 404);

    let resp = test::call_service(
        &app,
        test::TestRequest::get()
            .uri("/api/_health")
            .header("host", "api.example.test")
            .to_request(),
    )
    .await;
    assert_eq!(resp.status().as_u16(), 200);
    assert_eq!(read_json(resp).await["status"], "ok");

    let spec_resp = test::call_service(
        &app,
        test::TestRequest::get()
            .uri("/api/openapi.json")
            .header("host", "api.example.test")
            .to_request(),
    )
    .await;
    assert_eq!(spec_resp.status().as_u16(), 200);
    let spec = read_json(spec_resp).await;
    assert_eq!(spec["info"]["title"], "Doc Test");
    assert!(spec["paths"]["/auth/login"].is_object());

    let docs_resp = test::call_service(
        &app,
        test::TestRequest::get()
            .uri("/api/docs")
            .header("host", "api.example.test")
            .to_request(),
    )
    .await;
    assert_eq!(docs_resp.status().as_u16(), 200);
    let body = String::from_utf8(test::read_body(docs_resp).await.to_vec()).unwrap();
    assert!(body.contains("persistAuthorization"));
    assert!(body.contains("/api/openapi.json"));

    fs::remove_dir_all(root).unwrap();
    db.cleanup().await;
}

#[ntex::test]
async fn admin_is_served_from_the_binary_for_an_app_that_generated_nothing() {
    let db = TempDatabase::create("admin-embedded").await;
    let root = temp_dir("admin-embedded");
    write_files(
        &root,
        &[("main.toml", &format!("[database]\nurl = \"{}\"\n", db.url))],
    );

    let state = load_state(&root).await;
    let app = init_http_app!(state);

    // No `admin/` directory anywhere — the dashboard still loads.
    assert!(!root.join("admin").exists());
    let index =
        test::call_service(&app, test::TestRequest::get().uri("/admin/").to_request()).await;
    assert_eq!(index.status().as_u16(), 200);
    let body = String::from_utf8(test::read_body(index).await.to_vec()).unwrap();
    assert!(body.contains("<script"));

    // Its manifest describes this app, and points at this origin's API.
    let manifest = test::call_service(
        &app,
        test::TestRequest::get()
            .uri("/admin/apiplant-admin.json")
            .to_request(),
    )
    .await;
    assert_eq!(manifest.status().as_u16(), 200);
    let manifest = read_json(manifest).await;
    assert_eq!(manifest["api_base_url"], "");

    // The stylesheet's absolute asset URLs are rewritten, or the dashboard
    // would ask the API for `/head.png`.
    let css = test::call_service(
        &app,
        test::TestRequest::get().uri("/admin/app.css").to_request(),
    )
    .await;
    assert_eq!(css.status().as_u16(), 200);
    let css = String::from_utf8(test::read_body(css).await.to_vec()).unwrap();
    assert!(!css.contains("url(/head.png)"));

    // `/admin` resolves as a directory so relative asset URLs work.
    let bare = test::call_service(&app, test::TestRequest::get().uri("/admin").to_request()).await;
    assert_eq!(bare.status().as_u16(), 308);
    assert_eq!(bare.headers().get("location").unwrap(), "/admin/");

    fs::remove_dir_all(root).unwrap();
    db.cleanup().await;
}

#[ntex::test]
async fn a_directory_in_the_app_never_shadows_the_dashboard() {
    let db = TempDatabase::create("admin-stale").await;
    let root = temp_dir("admin-stale");
    write_files(
        &root,
        &[
            ("main.toml", &format!("[database]\nurl = \"{}\"\n", db.url)),
            // Left over from an older apiplant, or just someone's stray files.
            ("admin/index.html", "<!doctype html><title>Stale</title>"),
        ],
    );

    let state = load_state(&root).await;
    let app = init_http_app!(state);

    let index =
        test::call_service(&app, test::TestRequest::get().uri("/admin/").to_request()).await;
    assert_eq!(index.status().as_u16(), 200);
    let body = String::from_utf8(test::read_body(index).await.to_vec()).unwrap();
    assert!(
        !body.contains("Stale"),
        "a generated copy shadowed the live dashboard"
    );

    fs::remove_dir_all(root).unwrap();
    db.cleanup().await;
}

#[ntex::test]
async fn admin_can_be_switched_off() {
    let db = TempDatabase::create("admin-off").await;
    let root = temp_dir("admin-off");
    write_files(
        &root,
        &[(
            "main.toml",
            &format!(
                "[database]\nurl = \"{}\"\n\n[admin]\nenabled = false\n",
                db.url
            ),
        )],
    );

    let state = load_state(&root).await;
    let app = init_http_app!(state);

    let index =
        test::call_service(&app, test::TestRequest::get().uri("/admin/").to_request()).await;
    assert_eq!(index.status().as_u16(), 404);

    fs::remove_dir_all(root).unwrap();
    db.cleanup().await;
}

#[ntex::test]
async fn a_public_directory_is_served_at_the_root_alongside_the_api() {
    let db = TempDatabase::create("public").await;
    let root = temp_dir("public");
    write_files(
        &root,
        &[
            ("main.toml", &format!("[database]\nurl = \"{}\"\n", db.url)),
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
"#,
            ),
            ("public/index.html", "<h1>home</h1>"),
            ("public/style.css", "body { margin: 0 }"),
            ("public/guide/index.html", "<h1>guide</h1>"),
            ("public/404.html", "<h1>lost</h1>"),
        ],
    );

    let state = load_state(&root).await;
    let app = init_http_app!(state);

    for (uri, expected) in [
        ("/", "<h1>home</h1>"),
        ("/index.html", "<h1>home</h1>"),
        ("/style.css", "body { margin: 0 }"),
        ("/guide/", "<h1>guide</h1>"),
        ("/guide", "<h1>guide</h1>"),
    ] {
        let resp = test::call_service(&app, test::TestRequest::get().uri(uri).to_request()).await;
        assert_eq!(resp.status().as_u16(), 200, "{uri}");
        let body = String::from_utf8(test::read_body(resp).await.to_vec()).unwrap();
        assert_eq!(body, expected, "{uri}");
    }

    // The API still owns everything that isn't a file: a static site at the
    // root must not shadow `/{resource}`.
    let list = test::call_service(&app, test::TestRequest::get().uri("/note").to_request()).await;
    assert_eq!(list.status().as_u16(), 200);
    assert!(read_json(list).await.is_array());

    // Anything unrouted gets the app's 404 page, with a 404 status.
    // A path no route claims at all — two-segment paths still belong to the
    // API's `/{resource}/{id}`, and answer in JSON as they always did.
    let missing = test::call_service(
        &app,
        test::TestRequest::get()
            .uri("/no/such/page/here")
            .to_request(),
    )
    .await;
    assert_eq!(missing.status().as_u16(), 404);
    let body = String::from_utf8(test::read_body(missing).await.to_vec()).unwrap();
    assert_eq!(body, "<h1>lost</h1>");

    fs::remove_dir_all(root).unwrap();
    db.cleanup().await;
}

#[ntex::test]
async fn the_404_page_can_be_named_in_settings() {
    let db = TempDatabase::create("public-404").await;
    let root = temp_dir("public-404");
    write_files(
        &root,
        &[
            (
                "main.toml",
                &format!(
                    "[database]\nurl = \"{}\"\n\n[public]\nnot_found = \"missing.html\"\n",
                    db.url
                ),
            ),
            ("public/index.html", "<h1>home</h1>"),
            ("public/missing.html", "<h1>gone</h1>"),
            // Present, but not the configured page, so it must not be used.
            ("public/404.html", "<h1>unused</h1>"),
        ],
    );

    let state = load_state(&root).await;
    let app = init_http_app!(state);

    let missing = test::call_service(
        &app,
        test::TestRequest::get()
            .uri("/no/such/page/here")
            .to_request(),
    )
    .await;
    assert_eq!(missing.status().as_u16(), 404);
    let body = String::from_utf8(test::read_body(missing).await.to_vec()).unwrap();
    assert_eq!(body, "<h1>gone</h1>");

    fs::remove_dir_all(root).unwrap();
    db.cleanup().await;
}
