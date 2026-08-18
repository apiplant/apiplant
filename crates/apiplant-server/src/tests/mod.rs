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

const ADMIN_DB_URL: &str = "postgres://postgres@127.0.0.1:5432/postgres";

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
    fs::create_dir_all(dir.join("resources")).unwrap();
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

        let url = format!("postgres://postgres@127.0.0.1:5432/{name}");
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

/// A signed-in person holding `role` in a brand-new organisation.
///
/// Returns their token and that organisation's id — the pair every request
/// under an org-scoped or `role:` policy needs.
pub(super) async fn member_with_role(
    state: &AppState,
    email: &str,
    role: &str,
) -> (String, String) {
    let hash = state.auth.hash_password("hunter2").unwrap();
    let user = state
        .db
        .create(
            state.app.resources.get("user").unwrap(),
            &json!({ "email": email, "password_hash": hash })
                .as_object()
                .unwrap()
                .clone(),
        )
        .await
        .unwrap();
    let user_id = user["id"].as_str().unwrap().to_string();

    let org = state
        .db
        .create(
            state.app.resources.get("organization").unwrap(),
            &json!({ "name": "Acme" }).as_object().unwrap().clone(),
        )
        .await
        .unwrap();
    let org_id = org["id"].as_str().unwrap().to_string();

    state
        .db
        .create(
            state.app.resources.get("membership").unwrap(),
            &json!({ "user_id": user_id, "organization_id": org_id, "role": role })
                .as_object()
                .unwrap()
                .clone(),
        )
        .await
        .unwrap();

    let token = state
        .auth
        .issue_token(Uuid::parse_str(&user_id).unwrap())
        .unwrap();
    (token, org_id)
}

async fn load_state(root: &Path) -> AppState {
    load_state_with(root, Vec::new()).await
}

async fn load_state_with(root: &Path, functions: Vec<BoxedFunction>) -> AppState {
    load_state_configured(
        root,
        functions
            .into_iter()
            .map(|f| (f, "{}".to_string()))
            .collect(),
    )
    .await
}

/// The same, for a test that needs a function's `functions/<name>.toml` — its
/// per-deployment config, which is also where a function's own rate limit is
/// declared.
async fn load_state_configured(root: &Path, functions: Vec<(BoxedFunction, String)>) -> AppState {
    let app = App::load(root).unwrap();
    let db = Db::connect(&app.config.database.resolved_url(), 8)
        .await
        .unwrap();
    apiplant_db::migrate(db.connection(), &app).await.unwrap();

    let mut functions_registry = FunctionRegistry::load(&app);
    for (function, config_json) in functions {
        functions_registry.register(function, config_json);
    }
    let functions = functions_registry;
    // Built from the app's own `[email]` section, exactly as `run` does, so a
    // test app that configures a provider gets the mailbox routes mounted and
    // one that doesn't gets a server with no such endpoints — which is the
    // difference these tests are largely about.
    let mailer = apiplant_email::Mailer::from_config(&app.config.email).expect("valid [email]");
    let payments = apiplant_payments::Payments::from_config(
        &app.config.payments,
        "https://test.example/admin/#/billing",
    )
    .expect("valid [payments]");
    // Same again for the assistant: a test app that names an `[ai]` provider
    // gets `/ai/chat`, and one that doesn't gets no such route.
    let ai = apiplant_ai::Ai::from_config(&app.config.ai).expect("valid [ai]");
    // And once more for sign-in providers: a test app with an `[oauth.…]`
    // block gets the `/auth/oauth` routes and the `oauth_state` table, and one
    // without gets neither.
    crate::oauth_routes::check_resources(&app).expect("oauth resources are intact");
    let oauth = apiplant_oauth::Providers::from_config(
        &app.config.oauth,
        &format!(
            "{}{}/auth/oauth",
            app.config.server.public_origin(),
            app.config.server.base_path.trim_end_matches('/')
        ),
    )
    .expect("valid [oauth]");
    let agent_ais = app
        .agents
        .values()
        .filter_map(|agent| {
            agent.ai.as_ref().map(|_| {
                apiplant_ai::Ai::from_config(&agent.merged_ai_config(&app.config.ai))
                    .map(|ai| (agent.meta.name.clone(), ai))
            })
        })
        .collect::<Result<Vec<_>, _>>()
        .expect("valid agent [ai] overrides")
        .into_iter()
        .filter_map(|(name, ai)| ai.map(|ai| (name, ai)))
        .collect();
    let spec = openapi::build(&app, &functions, mailer.is_some());
    let spec_url = format!("{}/openapi.json", app.config.server.base_path);

    let admin_manifest =
        crate::admin::manifest_json(&app, &functions, String::new(), mailer.is_some())
            .expect("build the admin manifest");
    let statics = crate::state::Statics::resolve(&app);
    // Real, and rooted in the test app's own temporary directory: a test that
    // uploads writes into a directory that is deleted with the rest of it.
    let storage = apiplant_storage::Storage::connect(&app.config.storage, &app.root)
        .expect("valid [storage]");
    // Real, like the storage above: `queue_message` is a built-in, so the test
    // database has the table and a test that publishes actually writes a row.
    let queue = apiplant_queue::Queue::new(&db, &app);
    // Real, from the app's own `[rate_limit]` and whatever its resources and
    // functions say beside it: a test app that sets no limit gets a policy that
    // refuses nothing, and one that does gets 429s.
    let rate_limit = crate::rate_limit::RateLimitPolicy::build(&app, &functions);
    // Also from the app's own config, so a test can assert on `X-Trace-Id`.
    // Nothing is exported: no test installs a subscriber with an OTLP layer
    // behind it, so the spans are built and dropped.
    let telemetry = crate::telemetry::TelemetryPolicy::build(
        &app.config.observability,
        &app.config.server.base_path,
    );

    AppState {
        app: Arc::new(app),
        db,
        auth: Authenticator::new(b"test-secret".to_vec(), 3600),
        functions: Arc::new(functions),
        mailer,
        // Compiled from the app being loaded, so a test that writes an
        // `emails/` directory gets its templates the way a real boot would.
        email_templates: Arc::new(
            crate::email_templates::EmailTemplates::load(root).expect("email templates"),
        ),
        // No test configures a cache, so a function that reaches for one gets
        // the same "not configured" error a real app would.
        cache: None,
        storage,
        // Built from the app's own `[payments]` section, like the mailer
        // above: a test app that names a provider gets the `/billing` routes
        // and the `billing_*` resources, and one that doesn't gets neither.
        payments,
        ai,
        telemetry: Arc::new(telemetry),
        oauth: oauth.map(Arc::new),
        queue,
        agent_ais: Arc::new(agent_ais),
        rate_limit: Arc::new(rate_limit),
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

mod ai;
mod auth;
mod auth_hooks;
mod billing;
mod email_auth;
mod functions;
mod hooks;
mod oauth;
mod permissions;
mod queues;
mod rate_limit;
mod resources;
mod schema;
mod serving;
mod storage;
