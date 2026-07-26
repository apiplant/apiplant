//! End-to-end tests for the server: the shared harness lives here, the cases
//! themselves in the submodules below. Every one of them boots a real app
//! against a throwaway Postgres database and drives it through the same
//! `build_app!` route table `run` uses, so what is exercised is what ships.

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
use ntex::web::test;
use serde_json::{json, Value};
use uuid::Uuid;

use super::functions::FunctionRegistry;
use super::openapi;
use super::state::AppState;

const ADMIN_DB_URL: &str = "postgres://postgres@127.0.0.1:55432/postgres";

macro_rules! init_http_app {
    ($state:expr) => {
        test::init_service(build_app!($state)).await
    };
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
            permission: RString::new(),
            admin: RString::new(),
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
fn observer(_host: &HostApi_TO<'_, RBox<()>>, hook: &str, _input: &str) -> Result<String, String> {
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

    let admin_manifest = crate::admin::manifest_json(&app, &functions, String::new())
        .expect("build the admin manifest");
    let statics = crate::state::Statics::resolve(&app);

    AppState {
        app: Arc::new(app),
        db,
        auth: Authenticator::new(b"test-secret".to_vec(), 3600),
        functions: Arc::new(functions),
        statics: Arc::new(statics),
        admin_manifest: Arc::new(admin_manifest),
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

mod auth;
mod functions;
mod hooks;
mod permissions;
mod resources;
mod schema;
mod serving;
