use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use abi_stable::sabi_trait::TD_Opaque;
use abi_stable::std_types::{RBox, RResult, RStr, RString};
use apiplant_abi::{
    BoxedFunction, Function, FunctionManifest, Function_TO, HostApi_TO, HttpMethod, Visibility,
};
use apiplant_auth::Authenticator;
use apiplant_core::App;
use apiplant_db::Db;
use ntex::http::header::CONTENT_TYPE;
use ntex::web::{self, guard, test, App as WebApp};
use serde_json::{json, Value};
use uuid::Uuid;

use super::auth_routes;
use super::crud;
use super::docs_page;
use super::functions::FunctionRegistry;
use super::health;
use super::openapi;
use super::openapi_spec;
use super::state::AppState;

const ADMIN_DB_URL: &str = "postgres://postgres@127.0.0.1:55432/postgres";

macro_rules! init_http_app {
    ($state:expr) => {{
        let state = $state.clone();
        let base_path = state.app.config.server.base_path.clone();
        let docs_enabled = state.app.config.docs.enabled;
        let docs_path = state.app.config.docs.path.clone();
        let domain = state.app.config.server.domain.clone();

        test::init_service(WebApp::new().state(state.clone()).service({
            let mut scope = web::scope(base_path.as_str());
            if let Some(d) = &domain {
                scope = scope.guard(guard::Host(d.clone()));
            }
            if docs_enabled {
                scope = scope
                    .route("/openapi.json", web::get().to(openapi_spec))
                    .route(&docs_path, web::get().to(docs_page));
            }
            scope
                .route("/_health", web::get().to(health))
                .route("/auth/register", web::post().to(auth_routes::register))
                .route("/auth/login", web::post().to(auth_routes::login))
                .route("/auth/apikeys", web::post().to(auth_routes::create_api_key))
                .route("/functions/{name}", web::route().to(super::function_routes::invoke))
                .service(
                    web::resource("/{resource}")
                        .route(web::get().to(crud::list))
                        .route(web::post().to(crud::create)),
                )
                .service(
                    web::resource("/{resource}/{id}")
                        .route(web::get().to(crud::get))
                        .route(web::patch().to(crud::update))
                        .route(web::put().to(crud::update))
                        .route(web::delete().to(crud::delete)),
                )
                .route(
                    "/{parent}/{id}/{child}",
                    web::get().to(crud::nested_list),
                )
        }))
        .await
    }};
}

fn temp_dir(label: &str) -> PathBuf {
    let mut dir = std::env::temp_dir();
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    dir.push(format!(
        "apiplant-server-test-{label}-{}-{stamp}",
        std::process::id()
    ));
    fs::create_dir_all(dir.join("models")).unwrap();
    dir
}

fn write_files(root: &Path, files: &[(&str, &str)]) {
    for (relative, contents) in files {
        let path = root.join(relative);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(path, contents).unwrap();
    }
}

struct TempDatabase {
    name: String,
    url: String,
}

impl TempDatabase {
    async fn create(label: &str) -> Self {
        let safe_label = label.replace('-', "_");
        let name = format!("apiplant_{}_{}", safe_label, Uuid::new_v4().simple());
        let admin = Db::connect(ADMIN_DB_URL, 4).await.unwrap();
        admin
            .raw_json(&format!("CREATE DATABASE {name}"), &[])
            .await
            .unwrap();

        let url = format!("postgres://postgres@127.0.0.1:55432/{name}");
        let db = Db::connect(&url, 4).await.unwrap();
        db.raw_json("CREATE EXTENSION IF NOT EXISTS pgcrypto", &[])
            .await
            .unwrap();

        TempDatabase { name, url }
    }

    async fn cleanup(&self) {
        let admin = Db::connect(ADMIN_DB_URL, 4).await.unwrap();
        let _ = admin
            .raw_json(
                &format!("DROP DATABASE IF EXISTS {} WITH (FORCE)", self.name),
                &[],
            )
            .await;
    }
}

/// A function defined inline for tests, so hook behaviour can be exercised
/// without compiling and dynamically loading a `.so`.
///
/// `handler` receives the host (for database access), the hook context JSON
/// (empty for a plain HTTP call) and the input body, and returns the JSON reply.
type TestHandler = fn(&HostApi_TO<'_, RBox<()>>, &str, &str) -> Result<String, String>;

struct TestFunction {
    name: String,
    visibility: Visibility,
    handler: TestHandler,
}

impl Function for TestFunction {
    fn manifest(&self) -> FunctionManifest {
        FunctionManifest {
            name: RString::from(self.name.as_str()),
            version: RString::from("0.0.0"),
            description: RString::from("test function"),
            visibility: self.visibility,
            role: RString::new(),
            method: HttpMethod::Post,
            config_schema: RString::new(),
            input_schema: RString::new(),
            output_schema: RString::new(),
        }
    }

    fn invoke(&self, host: HostApi_TO<'_, RBox<()>>, input: RStr<'_>) -> RResult<RString, RString> {
        match (self.handler)(&host, host.hook().as_str(), input.as_str()) {
            Ok(reply) => RResult::ROk(RString::from(reply)),
            Err(message) => RResult::RErr(RString::from(message)),
        }
    }
}

fn test_function(name: &str, visibility: Visibility, handler: TestHandler) -> BoxedFunction {
    Function_TO::from_value(
        TestFunction {
            name: name.to_string(),
            visibility,
            handler,
        },
        TD_Opaque,
    )
}

/// A hook function that only records the context it saw and lets the request
/// continue untouched.
fn observer(
    _host: &HostApi_TO<'_, RBox<()>>,
    hook: &str,
    _input: &str,
) -> Result<String, String> {
    record(hook);
    Ok(json!({}).to_string())
}

/// Hook contexts observed during the lifecycle test, in firing order.
static HOOK_LOG: Mutex<Vec<Value>> = Mutex::new(Vec::new());

fn record(hook: &str) {
    let context: Value = serde_json::from_str(hook).expect("hooks always receive a context");
    HOOK_LOG.lock().unwrap().push(context);
}

/// The first recorded context for an event.
fn recorded(event: &str) -> Value {
    let found = HOOK_LOG
        .lock()
        .unwrap()
        .iter()
        .find(|c| c["event"] == event)
        .cloned();
    found.unwrap_or_else(|| panic!("`{event}` never fired; saw {:?}", events()))
}

/// How many times an event fired.
fn fired(event: &str) -> usize {
    HOOK_LOG
        .lock()
        .unwrap()
        .iter()
        .filter(|c| c["event"] == event)
        .count()
}

fn events() -> Vec<String> {
    HOOK_LOG
        .lock()
        .unwrap()
        .iter()
        .map(|c| c["event"].as_str().unwrap_or_default().to_string())
        .collect()
}

async fn load_state(root: &Path) -> AppState {
    load_state_with(root, Vec::new()).await
}

async fn load_state_with(root: &Path, functions: Vec<BoxedFunction>) -> AppState {
    let app = App::load(root).unwrap();
    let db = Db::connect(&app.config.database.resolved_url(), 8)
        .await
        .unwrap();
    apiplant_db::migrate(db.connection(), &app).await.unwrap();

    let mut functions_registry = FunctionRegistry::load_dir(&app.functions_dir);
    for function in functions {
        functions_registry.register(function, "{}".to_string());
    }
    let functions = functions_registry;
    let spec = openapi::build(&app, &functions);
    let spec_url = format!("{}/openapi.json", app.config.server.base_path);

    AppState {
        app: Arc::new(app),
        db,
        auth: Authenticator::new(b"test-secret".to_vec(), 3600),
        functions: Arc::new(functions),
        openapi_json: Arc::new(serde_json::to_string(&spec).unwrap()),
        docs_html: Arc::new(openapi::swagger_ui_html(
            &spec_url,
            spec["info"]["title"].as_str().unwrap_or("apiplant API"),
        )),
    }
}

async fn read_json(resp: ntex::web::WebResponse) -> Value {
    serde_json::from_slice(&test::read_body(resp).await).unwrap()
}

fn req_json(method: &str, uri: &str, body: Value) -> ntex::http::Request {
    let req = match method {
        "GET" => test::TestRequest::get(),
        "POST" => test::TestRequest::post(),
        "PATCH" => test::TestRequest::patch(),
        "PUT" => test::TestRequest::put(),
        "DELETE" => test::TestRequest::delete(),
        other => panic!("unsupported method {other}"),
    };
    req.uri(uri)
        .header(CONTENT_TYPE, "application/json")
        .set_payload(body.to_string())
        .to_request()
}

fn bearer(req: test::TestRequest, token: &str) -> test::TestRequest {
    req.header("authorization", format!("Bearer {token}"))
}

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

    fs::remove_dir_all(root).unwrap();
    db.cleanup().await;
}

#[ntex::test]
async fn multitenancy_relationships_permissions_and_constraints_work_end_to_end() {
    let db = TempDatabase::create("multi").await;
    let root = temp_dir("multi");
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
                "models/post.toml",
                r#"
[resource]
name = "post"

[permissions]
list = "member"
read = "member"
create = "member"
update = "owner"
delete = "role:admin"

[fields.title]
type = "string"
required = true
unique = true

[fields.owner_id]
type = "reference"
references = "user"
"#,
            ),
            (
                "models/comment.toml",
                r#"
[resource]
name = "comment"

[permissions]
list = "member"
read = "member"
create = "member"
update = "owner"
delete = "role:admin"

[fields.body]
type = "text"
required = true

[fields.post_id]
type = "reference"
references = "post"
required = true
on_delete = "cascade"

[fields.owner_id]
type = "reference"
references = "user"
"#,
            ),
            (
                "models/plan.toml",
                r#"
[resource]
name = "plan"
scope = "global"

[permissions]
list = "public"
read = "public"
create = "private"
update = "private"
delete = "private"

[fields.name]
type = "string"
"#,
            ),
        ],
    );

    let state = load_state(&root).await;
    let app = init_http_app!(state);

    let alice_reg = test::call_service(
        &app,
        req_json(
            "POST",
            "/api/auth/register",
            json!({"email":"alice@example.com","password":"pw"}),
        ),
    )
    .await;
    let alice = read_json(alice_reg).await;
    let alice_token = alice["token"].as_str().unwrap().to_string();
    let alice_id = alice["user"]["id"].as_str().unwrap().to_string();

    let org_a_resp = test::call_service(
        &app,
        bearer(
            test::TestRequest::post()
                .uri("/api/organization")
                .header(CONTENT_TYPE, "application/json")
                .set_payload(json!({"name":"Acme","slug":"acme"}).to_string()),
            &alice_token,
        )
        .to_request(),
    )
    .await;
    assert_eq!(org_a_resp.status().as_u16(), 201);
    let org_a = read_json(org_a_resp).await;
    let org_a_id = org_a["id"].as_str().unwrap().to_string();

    let post_resp = test::call_service(
        &app,
        bearer(
            test::TestRequest::post()
                .uri("/api/post")
                .header(CONTENT_TYPE, "application/json")
                .set_payload(
                    json!({
                        "title":"hello",
                        "owner_id": Uuid::new_v4(),
                        "organization_id": Uuid::new_v4()
                    })
                    .to_string(),
                ),
            &alice_token,
        )
        .to_request(),
    )
    .await;
    assert_eq!(post_resp.status().as_u16(), 201);
    let post = read_json(post_resp).await;
    let post_id = post["id"].as_str().unwrap().to_string();
    assert_eq!(post["owner_id"], alice_id);
    assert_eq!(post["organization_id"], org_a_id);

    let second_post_resp = test::call_service(
        &app,
        bearer(
            test::TestRequest::post()
                .uri("/api/post")
                .header(CONTENT_TYPE, "application/json")
                .set_payload(json!({"title":"later"}).to_string()),
            &alice_token,
        )
        .to_request(),
    )
    .await;
    assert_eq!(second_post_resp.status().as_u16(), 201);
    let second_post = read_json(second_post_resp).await;
    let second_post_id = second_post["id"].as_str().unwrap().to_string();

    let duplicate_resp = test::call_service(
        &app,
        bearer(
            test::TestRequest::post()
                .uri("/api/post")
                .header(CONTENT_TYPE, "application/json")
                .set_payload(json!({"title":"hello"}).to_string()),
            &alice_token,
        )
        .to_request(),
    )
    .await;
    assert_eq!(duplicate_resp.status().as_u16(), 409);

    let comment_resp = test::call_service(
        &app,
        bearer(
            test::TestRequest::post()
                .uri("/api/comment")
                .header(CONTENT_TYPE, "application/json")
                .set_payload(json!({"body":"nice","post_id":post_id}).to_string()),
            &alice_token,
        )
        .to_request(),
    )
    .await;
    assert_eq!(comment_resp.status().as_u16(), 201);

    let bad_comment_resp = test::call_service(
        &app,
        bearer(
            test::TestRequest::post()
                .uri("/api/comment")
                .header(CONTENT_TYPE, "application/json")
                .set_payload(json!({"body":"oops","post_id":Uuid::new_v4()}).to_string()),
            &alice_token,
        )
        .to_request(),
    )
    .await;
    assert_eq!(bad_comment_resp.status().as_u16(), 400);

    let paged_resp = test::call_service(
        &app,
        bearer(
            test::TestRequest::get().uri("/api/post?limit=1&offset=1"),
            &alice_token,
        )
        .to_request(),
    )
    .await;
    assert_eq!(paged_resp.status().as_u16(), 200);
    let paged = read_json(paged_resp).await;
    let page = paged.as_array().unwrap();
    assert_eq!(page.len(), 1);
    assert_eq!(page[0]["id"], post_id);

    let update_resp = test::call_service(
        &app,
        bearer(
            test::TestRequest::put()
                .uri(&format!("/api/post/{post_id}"))
                .header(CONTENT_TYPE, "application/json")
                .set_payload(json!({"title":"hello-updated"}).to_string()),
            &alice_token,
        )
        .to_request(),
    )
    .await;
    assert_eq!(update_resp.status().as_u16(), 200);
    let updated = read_json(update_resp).await;
    assert_eq!(updated["title"], "hello-updated");

    let bob_reg = test::call_service(
        &app,
        req_json(
            "POST",
            "/api/auth/register",
            json!({"email":"bob@example.com","password":"pw"}),
        ),
    )
    .await;
    let bob = read_json(bob_reg).await;
    let bob_token = bob["token"].as_str().unwrap().to_string();
    let bob_id = bob["user"]["id"].as_str().unwrap().to_string();

    let org_b_resp = test::call_service(
        &app,
        bearer(
            test::TestRequest::post()
                .uri("/api/organization")
                .header(CONTENT_TYPE, "application/json")
                .set_payload(json!({"name":"Beta","slug":"beta"}).to_string()),
            &bob_token,
        )
        .to_request(),
    )
    .await;
    assert_eq!(org_b_resp.status().as_u16(), 201);
    let org_b = read_json(org_b_resp).await;
    let org_b_id = org_b["id"].as_str().unwrap().to_string();

    let isolated_resp = test::call_service(
        &app,
        bearer(test::TestRequest::get().uri("/api/post"), &bob_token).to_request(),
    )
    .await;
    assert_eq!(isolated_resp.status().as_u16(), 200);
    assert_eq!(read_json(isolated_resp).await, json!([]));

    let plan_resp = test::call_service(
        &app,
        test::TestRequest::get().uri("/api/plan").to_request(),
    )
    .await;
    assert_eq!(plan_resp.status().as_u16(), 200);
    assert_eq!(read_json(plan_resp).await, json!([]));

    let membership_resp = test::call_service(
        &app,
        bearer(
            test::TestRequest::post()
                .uri("/api/membership")
                .header(CONTENT_TYPE, "application/json")
                .set_payload(json!({"user_id":bob_id,"role":"member"}).to_string()),
            &alice_token,
        )
        .to_request(),
    )
    .await;
    assert_eq!(membership_resp.status().as_u16(), 201);
    let membership = read_json(membership_resp).await;
    assert_eq!(membership["organization_id"], org_a_id);

    let multi_org_resp = test::call_service(
        &app,
        bearer(test::TestRequest::get().uri("/api/post"), &bob_token).to_request(),
    )
    .await;
    assert_eq!(multi_org_resp.status().as_u16(), 403);
    assert_eq!(
        read_json(multi_org_resp).await["error"],
        "select an organisation with the X-Organization header"
    );

    let org_a_posts_resp = test::call_service(
        &app,
        bearer(
            test::TestRequest::get()
                .uri("/api/post")
                .header("x-organization", org_a_id.as_str()),
            &bob_token,
        )
        .to_request(),
    )
    .await;
    assert_eq!(org_a_posts_resp.status().as_u16(), 200);
    let org_a_posts = read_json(org_a_posts_resp).await;
    assert_eq!(org_a_posts.as_array().unwrap().len(), 2);

    let bob_update_resp = test::call_service(
        &app,
        bearer(
            test::TestRequest::patch()
                .uri(&format!("/api/post/{post_id}"))
                .header("x-organization", org_a_id.as_str())
                .header(CONTENT_TYPE, "application/json")
                .set_payload(json!({"title":"nope"}).to_string()),
            &bob_token,
        )
        .to_request(),
    )
    .await;
    assert_eq!(bob_update_resp.status().as_u16(), 404);

    let bob_membership_write_resp = test::call_service(
        &app,
        bearer(
            test::TestRequest::post()
                .uri("/api/membership")
                .header("x-organization", org_a_id.as_str())
                .header(CONTENT_TYPE, "application/json")
                .set_payload(json!({"user_id":alice_id,"role":"member"}).to_string()),
            &bob_token,
        )
        .to_request(),
    )
    .await;
    assert_eq!(bob_membership_write_resp.status().as_u16(), 403);

    let expand_resp = test::call_service(
        &app,
        bearer(
            test::TestRequest::get()
                .uri(&format!("/api/comment?expand=post,owner&post_id={post_id}")),
            &alice_token,
        )
        .to_request(),
    )
    .await;
    assert_eq!(expand_resp.status().as_u16(), 200);
    let expanded = read_json(expand_resp).await;
    let comment = &expanded.as_array().unwrap()[0];
    assert_eq!(comment["post"]["id"], post_id);
    assert_eq!(comment["owner"]["id"], alice_id);
    assert!(comment["owner"].get("password_hash").is_none());

    let nested_resp = test::call_service(
        &app,
        bearer(
            test::TestRequest::get().uri(&format!("/api/post/{post_id}/comment")),
            &alice_token,
        )
        .to_request(),
    )
    .await;
    assert_eq!(nested_resp.status().as_u16(), 200);
    assert_eq!(read_json(nested_resp).await.as_array().unwrap().len(), 1);

    let org_b_posts_resp = test::call_service(
        &app,
        bearer(
            test::TestRequest::get()
                .uri("/api/post")
                .header("x-organization", org_b_id.as_str()),
            &bob_token,
        )
        .to_request(),
    )
    .await;
    assert_eq!(org_b_posts_resp.status().as_u16(), 200);
    assert_eq!(read_json(org_b_posts_resp).await, json!([]));

    assert_ne!(second_post_id, post_id);

    fs::remove_dir_all(root).unwrap();
    db.cleanup().await;
}

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
async fn migrations_are_additive_for_existing_rows() {
    let db = TempDatabase::create("migrate").await;
    let root = temp_dir("migrate");
    write_files(
        &root,
        &[
            (
                "main.toml",
                &format!(
                    r#"
[database]
url = "{}"
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

[fields.title]
type = "string"
required = true
"#,
            ),
        ],
    );

    let app_v1 = App::load(&root).unwrap();
    let db_conn = Db::connect(&db.url, 4).await.unwrap();
    apiplant_db::migrate(db_conn.connection(), &app_v1)
        .await
        .unwrap();
    let note_v1 = app_v1.resources.get("note").unwrap();
    db_conn
        .create(note_v1, &serde_json::Map::from_iter([(
            "title".to_string(),
            Value::String("first".into()),
        )]))
        .await
        .unwrap();

    write_files(
        &root,
        &[(
            "models/note.toml",
            r#"
[resource]
name = "note"
scope = "global"

[fields.title]
type = "string"
required = true

[fields.status]
type = "string"
required = true
default = "draft"
"#,
        )],
    );

    let app_v2 = App::load(&root).unwrap();
    apiplant_db::migrate(db_conn.connection(), &app_v2)
        .await
        .unwrap();
    apiplant_db::migrate(db_conn.connection(), &app_v2)
        .await
        .unwrap();

    let note_v2 = app_v2.resources.get("note").unwrap();
    let rows = db_conn.list(note_v2, &[], 10, 0).await.unwrap();
    let row = &rows.as_array().unwrap()[0];
    assert_eq!(row["title"], "first");
    assert_eq!(row["status"], "draft");

    fs::remove_dir_all(root).unwrap();
    db.cleanup().await;
}

#[ntex::test]
async fn complex_overrides_and_mixed_permission_models_work_together() {
    let db = TempDatabase::create("complex").await;
    let root = temp_dir("complex");
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

[docs]
path = "/reference"
title = "Complex App"
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
password_field = "secret_hash"

[fields.username]
type = "string"
required = true
unique = true

[fields.secret_hash]
type = "string"
hidden = true

[fields.display_name]
type = "string"
"#,
            ),
            (
                "models/organization.toml",
                r#"
[resource]
name = "organization"
scope = "global"

[permissions]
list = "member"
read = "member"
create = "authenticated"
update = "role:admin"
delete = "role:admin"

[fields.name]
type = "string"
required = true

[fields.slug]
type = "string"
required = true
unique = true

[fields.tier]
type = "string"
required = true
default = "standard"
"#,
            ),
            (
                "models/membership.toml",
                r#"
[resource]
name = "membership"
scope = "organization"

[permissions]
list = "member"
read = "member"
create = "role:admin"
update = "role:admin"
delete = "role:admin"

[fields.user_id]
type = "reference"
references = "user"
required = true

[fields.organization_id]
type = "reference"
references = "organization"
required = true

[fields.role]
type = "string"

[fields.team]
type = "string"
"#,
            ),
            (
                "models/api_key.toml",
                r#"
[resource]
name = "api_key"
scope = "global"

[permissions]
list = "owner"
read = "owner"
create = "authenticated"
update = "private"
delete = "owner"

[fields.name]
type = "string"

[fields.note]
type = "string"

[fields.token_hash]
type = "string"
required = true
unique = true
hidden = true

[fields.owner_id]
type = "reference"
references = "user"
required = true
"#,
            ),
            (
                "models/news.toml",
                r#"
[resource]
name = "news"
scope = "global"

[permissions]
list = "public"
read = "public"
create = "private"
update = "private"
delete = "private"

[fields.headline]
type = "string"
required = true
"#,
            ),
            (
                "models/bulletin.toml",
                r#"
[resource]
name = "bulletin"
scope = "global"

[permissions]
list = "authenticated"
read = "authenticated"
create = "private"
update = "private"
delete = "private"

[fields.title]
type = "string"
required = true
"#,
            ),
            (
                "models/preference.toml",
                r#"
[resource]
name = "preference"
scope = "global"

[permissions]
list = "owner"
read = "owner"
create = "authenticated"
update = "owner"
delete = "owner"

[fields.key]
type = "string"
required = true

[fields.value]
type = "string"

[fields.owner_id]
type = "reference"
references = "user"
required = true
"#,
            ),
            (
                "models/project.toml",
                r#"
[resource]
name = "project"

[permissions]
list = "member"
read = "member"
create = "member"
update = "owner"
delete = "role:admin"

[fields.name]
type = "string"
required = true

[fields.owner_id]
type = "reference"
references = "user"
"#,
            ),
            (
                "models/deployment.toml",
                r#"
[resource]
name = "deployment"

[permissions]
list = "member"
read = "member"
create = "role:ops"
update = "role:ops"
delete = "role:admin"

[fields.project_id]
type = "reference"
references = "project"
required = true

[fields.owner_id]
type = "reference"
references = "user"

[fields.status]
type = "string"
default = "queued"
"#,
            ),
        ],
    );

    let state = load_state(&root).await;
    let app = init_http_app!(state);

    let anon_news = test::call_service(&app, test::TestRequest::get().uri("/api/news").to_request()).await;
    assert_eq!(anon_news.status().as_u16(), 200);
    assert_eq!(read_json(anon_news).await, json!([]));

    let anon_bulletin = test::call_service(&app, test::TestRequest::get().uri("/api/bulletin").to_request()).await;
    assert_eq!(anon_bulletin.status().as_u16(), 401);

    let anon_news_create = test::call_service(
        &app,
        req_json("POST", "/api/news", json!({"headline":"launch"})),
    )
    .await;
    assert_eq!(anon_news_create.status().as_u16(), 404);

    let alice_reg = test::call_service(
        &app,
        req_json(
            "POST",
            "/api/auth/register",
            json!({"username":"alice","password":"pw","display_name":"Alice"}),
        ),
    )
    .await;
    let alice = read_json(alice_reg).await;
    let alice_token = alice["token"].as_str().unwrap().to_string();
    let alice_id = alice["user"]["id"].as_str().unwrap().to_string();

    let alice_login = test::call_service(
        &app,
        req_json(
            "POST",
            "/api/auth/login",
            json!({"username":"alice","password":"pw"}),
        ),
    )
    .await;
    assert_eq!(alice_login.status().as_u16(), 200);

    let org_resp = test::call_service(
        &app,
        bearer(
            test::TestRequest::post()
                .uri("/api/organization")
                .header(CONTENT_TYPE, "application/json")
                .set_payload(json!({"name":"Acme","slug":"acme","tier":"enterprise"}).to_string()),
            &alice_token,
        )
        .to_request(),
    )
    .await;
    assert_eq!(org_resp.status().as_u16(), 201);
    let organization = read_json(org_resp).await;
    let org_id = organization["id"].as_str().unwrap().to_string();
    assert_eq!(organization["tier"], "enterprise");

    let bulletin_auth = test::call_service(
        &app,
        bearer(test::TestRequest::get().uri("/api/bulletin"), &alice_token).to_request(),
    )
    .await;
    assert_eq!(bulletin_auth.status().as_u16(), 200);

    let pref_resp = test::call_service(
        &app,
        bearer(
            test::TestRequest::post()
                .uri("/api/preference")
                .header(CONTENT_TYPE, "application/json")
                .set_payload(
                    json!({"key":"theme","value":"dark","owner_id":Uuid::new_v4()}).to_string(),
                ),
            &alice_token,
        )
        .to_request(),
    )
    .await;
    assert_eq!(pref_resp.status().as_u16(), 201);
    let preference = read_json(pref_resp).await;
    let preference_id = preference["id"].as_str().unwrap().to_string();
    assert_eq!(preference["owner_id"], alice_id);

    let project_resp = test::call_service(
        &app,
        bearer(
            test::TestRequest::post()
                .uri("/api/project")
                .header(CONTENT_TYPE, "application/json")
                .set_payload(json!({"name":"alpha"}).to_string()),
            &alice_token,
        )
        .to_request(),
    )
    .await;
    assert_eq!(project_resp.status().as_u16(), 201);
    let project = read_json(project_resp).await;
    let project_id = project["id"].as_str().unwrap().to_string();

    let bob_reg = test::call_service(
        &app,
        req_json(
            "POST",
            "/api/auth/register",
            json!({"username":"bob","password":"pw","display_name":"Bob"}),
        ),
    )
    .await;
    let bob = read_json(bob_reg).await;
    let bob_token = bob["token"].as_str().unwrap().to_string();
    let bob_id = bob["user"]["id"].as_str().unwrap().to_string();

    let carol_reg = test::call_service(
        &app,
        req_json(
            "POST",
            "/api/auth/register",
            json!({"username":"carol","password":"pw","display_name":"Carol"}),
        ),
    )
    .await;
    let carol = read_json(carol_reg).await;
    let carol_token = carol["token"].as_str().unwrap().to_string();
    let carol_id = carol["user"]["id"].as_str().unwrap().to_string();

    let bob_membership = test::call_service(
        &app,
        bearer(
            test::TestRequest::post()
                .uri("/api/membership")
                .header(CONTENT_TYPE, "application/json")
                .set_payload(json!({"user_id":bob_id,"role":"ops","team":"platform"}).to_string()),
            &alice_token,
        )
        .to_request(),
    )
    .await;
    assert_eq!(bob_membership.status().as_u16(), 201);
    let bob_membership = read_json(bob_membership).await;
    assert_eq!(bob_membership["organization_id"], org_id);
    assert_eq!(bob_membership["team"], "platform");

    let carol_membership = test::call_service(
        &app,
        bearer(
            test::TestRequest::post()
                .uri("/api/membership")
                .header(CONTENT_TYPE, "application/json")
                .set_payload(json!({"user_id":carol_id,"role":"member","team":"support"}).to_string()),
            &alice_token,
        )
        .to_request(),
    )
    .await;
    assert_eq!(carol_membership.status().as_u16(), 201);

    let bob_api_key = test::call_service(
        &app,
        bearer(
            test::TestRequest::post()
                .uri("/api/auth/apikeys")
                .header(CONTENT_TYPE, "application/json")
                .set_payload(json!({"name":"ops-bot"}).to_string()),
            &bob_token,
        )
        .to_request(),
    )
    .await;
    assert_eq!(bob_api_key.status().as_u16(), 201);
    let bob_api_key = read_json(bob_api_key).await;
    let bob_key = bob_api_key["api_key"].as_str().unwrap().to_string();

    let deployment_resp = test::call_service(
        &app,
        test::TestRequest::post()
            .uri("/api/deployment")
            .header("x-api-key", bob_key.as_str())
            .header("x-organization", org_id.as_str())
            .header(CONTENT_TYPE, "application/json")
            .set_payload(json!({"project_id":project_id}).to_string())
            .to_request(),
    )
    .await;
    assert_eq!(deployment_resp.status().as_u16(), 201);
    let deployment = read_json(deployment_resp).await;
    assert_eq!(deployment["owner_id"], bob_id);
    assert_eq!(deployment["status"], "queued");

    let carol_deployment = test::call_service(
        &app,
        bearer(
            test::TestRequest::post()
                .uri("/api/deployment")
                .header("x-organization", org_id.as_str())
                .header(CONTENT_TYPE, "application/json")
                .set_payload(json!({"project_id":project_id}).to_string()),
            &carol_token,
        )
        .to_request(),
    )
    .await;
    assert_eq!(carol_deployment.status().as_u16(), 403);

    let bob_delete_project = test::call_service(
        &app,
        test::TestRequest::delete()
            .uri(&format!("/api/project/{project_id}"))
            .header("authorization", format!("ApiKey {bob_key}"))
            .header("x-organization", org_id.as_str())
            .to_request(),
    )
    .await;
    assert_eq!(bob_delete_project.status().as_u16(), 403);

    let bob_membership_write = test::call_service(
        &app,
        bearer(
            test::TestRequest::post()
                .uri("/api/membership")
                .header("x-organization", org_id.as_str())
                .header(CONTENT_TYPE, "application/json")
                .set_payload(json!({"user_id":alice_id,"role":"member"}).to_string()),
            &bob_token,
        )
        .to_request(),
    )
    .await;
    assert_eq!(bob_membership_write.status().as_u16(), 403);

    let alice_prefs = test::call_service(
        &app,
        bearer(test::TestRequest::get().uri("/api/preference"), &alice_token).to_request(),
    )
    .await;
    assert_eq!(alice_prefs.status().as_u16(), 200);
    assert_eq!(read_json(alice_prefs).await.as_array().unwrap().len(), 1);

    let bob_prefs = test::call_service(
        &app,
        bearer(test::TestRequest::get().uri("/api/preference"), &bob_token).to_request(),
    )
    .await;
    assert_eq!(bob_prefs.status().as_u16(), 200);
    assert_eq!(read_json(bob_prefs).await, json!([]));

    let bob_pref_read = test::call_service(
        &app,
        bearer(
            test::TestRequest::get().uri(&format!("/api/preference/{preference_id}")),
            &bob_token,
        )
        .to_request(),
    )
    .await;
    assert_eq!(bob_pref_read.status().as_u16(), 404);

    let docs_resp = test::call_service(
        &app,
        test::TestRequest::get().uri("/api/reference").to_request(),
    )
    .await;
    assert_eq!(docs_resp.status().as_u16(), 200);

    let spec_resp = test::call_service(
        &app,
        test::TestRequest::get().uri("/api/openapi.json").to_request(),
    )
    .await;
    assert_eq!(spec_resp.status().as_u16(), 200);
    let spec = read_json(spec_resp).await;
    assert_eq!(spec["info"]["title"], "Complex App");
    assert_eq!(spec["paths"]["/news"]["get"].get("security"), None);
    assert!(spec["paths"]["/bulletin"]["get"]["security"].is_array());
    assert_eq!(
        spec["paths"]["/deployment"]["post"]["description"],
        "Requires the `ops` role in the active organisation."
    );

    fs::remove_dir_all(root).unwrap();
    db.cleanup().await;
}

// --- lifecycle hooks --------------------------------------------------------

/// `before_create`: rejects a blank title, and normalises the one it accepts.
fn post_guard(_host: &HostApi_TO<'_, RBox<()>>, hook: &str, input: &str) -> Result<String, String> {
    record(hook);
    let mut data: Value = serde_json::from_str(input).map_err(|e| e.to_string())?;
    let title = data["title"].as_str().unwrap_or_default().to_string();
    if title.trim().is_empty() {
        return Ok(json!({ "error": { "status": 422, "message": "title is required" } }).to_string());
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
fn read_guard(_host: &HostApi_TO<'_, RBox<()>>, hook: &str, _input: &str) -> Result<String, String> {
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
fn update_guard(_host: &HostApi_TO<'_, RBox<()>>, hook: &str, input: &str) -> Result<String, String> {
    record(hook);
    let mut data: Value = serde_json::from_str(input).map_err(|e| e.to_string())?;
    data["body"] = json!("normalised by hook");
    Ok(json!({ "data": data }).to_string())
}

/// `before_delete`: protects locked rows, using the row the host pre-fetched.
fn delete_guard(_host: &HostApi_TO<'_, RBox<()>>, hook: &str, input: &str) -> Result<String, String> {
    record(hook);
    let row: Value = serde_json::from_str(input).map_err(|e| e.to_string())?;
    if row["locked"] == json!(true) {
        return Ok(json!({ "error": { "status": 409, "message": "post is locked" } }).to_string());
    }
    Ok(json!({}).to_string())
}

/// `after_delete`: answers with the row that was removed instead of a bare 204.
fn delete_echo(_host: &HostApi_TO<'_, RBox<()>>, hook: &str, input: &str) -> Result<String, String> {
    record(hook);
    let row: Value = serde_json::from_str(input).map_err(|e| e.to_string())?;
    Ok(json!({ "data": { "deleted": row["title"] } }).to_string())
}

const HOOKED_POST_MODEL: &str = r#"
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

const AUDIT_MODEL: &str = r#"
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
                &format!("\n[server]\nbase_path = \"/api\"\n\n[database]\nurl = \"{}\"\n", db.url),
            ),
            ("models/post.toml", HOOKED_POST_MODEL),
            ("models/audit.toml", AUDIT_MODEL),
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
            bearer(test::TestRequest::get().uri("/api/audit"), &token).to_request(),
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
        .to_request(),
    )
    .await;
    assert_eq!(rejected.status().as_u16(), 422);
    assert_eq!(read_json(rejected).await["error"], "title is required");

    // --- list: the after hook replaces the response body wholesale.
    let listed_resp = test::call_service(
        &app,
        bearer(test::TestRequest::get().uri("/api/post?limit=10"), &token).to_request(),
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
    assert_eq!(create["data"]["title"], "hello", "before_create sees the submitted body");
    assert!(create["row"].is_null());

    let audit = recorded("after_create");
    assert_eq!(audit["row"]["title"], "HELLO", "after_create sees the stored row");
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
                &format!("\n[server]\nbase_path = \"/api\"\n\n[database]\nurl = \"{}\"\n", db.url),
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
        test::call_service(&app, test::TestRequest::get().uri("/api/note").to_request())
            .await,
    )
    .await;
    assert_eq!(listed.as_array().unwrap().len(), 0);

    fs::remove_dir_all(root).unwrap();
    db.cleanup().await;
}
