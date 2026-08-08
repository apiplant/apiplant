//! Uploading a file, reading it back, and writing the link into a row.

use super::*;

/// The whole round trip against a `local` backend: upload, serve, store in a
/// `file` field, and the refusals on either side of it.
#[ntex::test]
async fn files_upload_serve_and_land_in_a_file_field() {
    let db = TempDatabase::create("storage").await;
    let root = temp_dir("storage");
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

[storage]
backend = "local"
dir = "uploads"
max_size_mb = 1
allowed_types = ["image/*", "application/pdf"]
"#,
                    db.url
                ),
            ),
            (
                "resources/brand.toml",
                r#"
[resource]
name = "brand"
scope = "global"

[permissions]
list = "public"
read = "public"
create = "authenticated"
update = "authenticated"

[fields.name]
type = "string"
required = true

[fields.logo]
type = "file"
"#,
            ),
        ],
    );

    let state = load_state(&root).await;
    let app = init_http_app!(state);

    let registration = test::call_service(
        &app,
        req_json(
            "POST",
            "/api/auth/register",
            json!({"email":"alice@example.com","password":"pw"}),
        ),
    )
    .await;
    let token = read_json(registration).await["token"]
        .as_str()
        .unwrap()
        .to_string();

    // An anonymous caller cannot spend the app's disk.
    let anonymous = test::call_service(
        &app,
        test::TestRequest::post()
            .uri("/api/uploads?filename=logo.png")
            .header(CONTENT_TYPE, "image/png")
            .set_payload("PNG")
            .to_request(),
    )
    .await;
    assert_eq!(anonymous.status().as_u16(), 401);

    // Nor can a signed-in one upload a type this app did not allow.
    let wrong_type = test::call_service(
        &app,
        bearer(
            test::TestRequest::post()
                .uri("/api/uploads?filename=payload.html")
                .header(CONTENT_TYPE, "text/html")
                .set_payload("<script>"),
            &token,
        )
        .to_request(),
    )
    .await;
    assert_eq!(wrong_type.status().as_u16(), 415);

    let upload = test::call_service(
        &app,
        bearer(
            test::TestRequest::post()
                .uri("/api/uploads?filename=Logo%20Final%20(2).PNG")
                .header(CONTENT_TYPE, "image/png")
                .set_payload("PNGDATA"),
            &token,
        )
        .to_request(),
    )
    .await;
    assert_eq!(upload.status().as_u16(), 200);
    let stored = read_json(upload).await;
    let url = stored["url"].as_str().unwrap().to_string();

    // Relative, under the configured prefix, and carrying the original name so
    // the link is readable — but not *only* the original name, or two people
    // uploading a logo would overwrite each other.
    assert!(url.starts_with("/files/"), "{url} should be relative");
    assert!(url.ends_with("-logo-final-2.png"), "{url} keeps the name");
    assert_eq!(stored["content_type"], "image/png");

    // It landed in the configured directory, not in `public/`.
    let key = stored["key"].as_str().unwrap();
    assert_eq!(
        fs::read(root.join("uploads").join(key)).unwrap(),
        b"PNGDATA"
    );

    // And it reads back, unauthenticated, as the type its extension implies.
    let served = test::call_service(&app, test::TestRequest::get().uri(&url).to_request()).await;
    assert_eq!(served.status().as_u16(), 200);
    assert_eq!(served.headers().get("content-type").unwrap(), "image/png");
    assert_eq!(
        served.headers().get("x-content-type-options").unwrap(),
        "nosniff"
    );
    assert_eq!(test::read_body(served).await, "PNGDATA");

    // Nothing under the prefix can reach outside the store.
    let escape = test::call_service(
        &app,
        test::TestRequest::get()
            .uri("/files/../../main.toml")
            .to_request(),
    )
    .await;
    assert_ne!(escape.status().as_u16(), 200);

    let missing = test::call_service(
        &app,
        test::TestRequest::get()
            .uri("/files/2026/01/nothing.png")
            .to_request(),
    )
    .await;
    assert_eq!(missing.status().as_u16(), 404);

    // The link is an ordinary string to the API, which is the point of storing
    // it rather than a bucket reference.
    let created = test::call_service(
        &app,
        bearer(
            test::TestRequest::post()
                .uri("/api/brand")
                .header(CONTENT_TYPE, "application/json")
                .set_payload(json!({"name": "Acme", "logo": url}).to_string()),
            &token,
        )
        .to_request(),
    )
    .await;
    assert_eq!(created.status().as_u16(), 201);
    assert_eq!(read_json(created).await["logo"], url.as_str());
}

/// A `file` field is a `varchar` in Postgres and a plain string everywhere a
/// client can see, so nothing outside the dashboard has to know it exists.
#[ntex::test]
async fn a_file_field_is_a_string_to_every_client() {
    let db = TempDatabase::create("filefield").await;
    let root = temp_dir("filefield");
    write_files(
        &root,
        &[
            (
                "main.toml",
                &format!(
                    "[server]\nbase_path = \"/api\"\n\n[database]\nurl = \"{}\"\n",
                    db.url
                ),
            ),
            (
                "resources/doc.toml",
                r#"
[resource]
name = "doc"
scope = "global"

[permissions]
list = "public"
read = "public"
create = "public"

[fields.attachment]
type = "file"
max_length = 512
"#,
            ),
        ],
    );

    let state = load_state(&root).await;
    let app = init_http_app!(state);

    let spec = read_json(
        test::call_service(
            &app,
            test::TestRequest::get()
                .uri("/api/openapi.json")
                .to_request(),
        )
        .await,
    )
    .await;
    let schema = &spec["components"]["schemas"]["Doc"]["properties"]["attachment"];
    assert_eq!(schema["type"], "string");
    assert_eq!(schema["format"], "uri-reference");

    // Including a URL the app did not store: the column takes any string, so a
    // picture already hosted elsewhere is a perfectly good value.
    let created = test::call_service(
        &app,
        req_json(
            "POST",
            "/api/doc",
            json!({"attachment": "https://cdn.example.invalid/a.pdf"}),
        ),
    )
    .await;
    assert_eq!(created.status().as_u16(), 201);
    assert_eq!(
        read_json(created).await["attachment"],
        "https://cdn.example.invalid/a.pdf"
    );

    // The dashboard is the only thing that treats it differently.
    let manifest: Value = serde_json::from_str(&state.admin_manifest).unwrap();
    let field = manifest["resources"]
        .as_array()
        .unwrap()
        .iter()
        .find(|resource| resource["name"] == "doc")
        .unwrap()["fields"]
        .as_array()
        .unwrap()
        .iter()
        .find(|field| field["name"] == "attachment")
        .unwrap()
        .clone();
    assert_eq!(field["type"], "file");
    assert_eq!(field["widget"], "file");
}

/// With no store configured there is no upload endpoint and no `/files`, rather
/// than an endpoint that fails at the last moment.
#[ntex::test]
async fn an_app_that_stores_nothing_has_no_upload_routes() {
    let db = TempDatabase::create("nostorage").await;
    let root = temp_dir("nostorage");
    write_files(
        &root,
        &[(
            "main.toml",
            &format!(
                "[server]\nbase_path = \"/api\"\n\n[database]\nurl = \"{}\"\n\n[storage]\nbackend = \"none\"\n",
                db.url
            ),
        )],
    );

    let state = load_state(&root).await;
    assert!(state.storage.is_none());
    let app = init_http_app!(state);

    // The route is not mounted at all, the way the mailbox routes are not
    // mounted without an email provider, so the request falls through to the
    // generic `/{resource}` create and is refused there. Not a 404, because
    // the body is not JSON and never reaches the "no such resource" check —
    // what matters is that nothing stores anything.
    let upload = test::call_service(
        &app,
        test::TestRequest::post()
            .uri("/api/uploads?filename=a.png")
            .header(CONTENT_TYPE, "image/png")
            .set_payload("x")
            .to_request(),
    )
    .await;
    assert!(!upload.status().is_success(), "{}", upload.status());

    let serve = test::call_service(
        &app,
        test::TestRequest::get().uri("/files/a.png").to_request(),
    )
    .await;
    assert_eq!(serve.status().as_u16(), 404);
}
