//! HTTP surface for dynamically-loaded functions, mounted at
//! `<base>/functions/{name}`.

use apiplant_abi::{HttpMethod, Visibility};
use ntex::http::Method;
use ntex::web::types::{Path, State};
use ntex::web::{HttpRequest, HttpResponse};

use crate::functions::HostBridge;
use crate::response::error;
use crate::state::AppState;

fn expected_method(m: HttpMethod) -> Method {
    match m {
        HttpMethod::Get => Method::GET,
        HttpMethod::Post => Method::POST,
        HttpMethod::Put => Method::PUT,
        HttpMethod::Delete => Method::DELETE,
    }
}

/// Dispatch a request to a loaded function. Enforces the manifest's method and
/// visibility, then runs `invoke` on a blocking worker (the host bridge blocks
/// on the async database from there).
pub async fn invoke(
    req: HttpRequest,
    state: State<AppState>,
    path: Path<String>,
    body: String,
) -> HttpResponse {
    let name = path.into_inner();

    // Read manifest fields we need before crossing into the blocking task.
    let (method, visibility, role) = match state.functions.get(&name) {
        Some(f) => (
            f.manifest.method,
            f.manifest.visibility,
            f.manifest.role.to_string(),
        ),
        None => return error(404, format!("unknown function `{name}`")),
    };

    if req.method() != expected_method(method) {
        return error(405, "method not allowed");
    }

    let principal = state.resolve_principal(&req).await;
    match visibility {
        Visibility::Public => {}
        Visibility::Private => return error(404, "unknown function"),
        Visibility::Authenticated => {
            if principal.is_none() {
                return error(401, "authentication required");
            }
        }
        Visibility::RoleGated => {
            let ok = principal
                .as_ref()
                .and_then(|p| p.role.as_deref())
                .map(|r| r == role)
                .unwrap_or(false);
            if !ok {
                return error(403, "forbidden");
            }
        }
    }

    let input = if body.trim().is_empty() {
        "{}".to_string()
    } else {
        body
    };
    let principal_id = principal
        .as_ref()
        .map(|p| p.user_id.to_string())
        .unwrap_or_default();

    // Move owned handles into the blocking worker.
    let functions = state.functions.clone();
    let db = state.db.clone();
    let handle = tokio::runtime::Handle::current();
    let name2 = name.clone();

    let result = tokio::task::spawn_blocking(move || {
        let f = functions.get(&name2).expect("checked above");
        let bridge = HostBridge::new(db, handle, f.config_json.clone(), principal_id);
        f.invoke(bridge, &input)
    })
    .await;

    match result {
        Ok(Ok(json)) => HttpResponse::Ok()
            .content_type("application/json")
            .body(json),
        Ok(Err(msg)) => error(400, msg),
        Err(_) => error(500, "function panicked"),
    }
}
