//! HTTP surface for dynamically-loaded functions, mounted at
//! `<base>/functions/{name}`.

use apiplant_abi::{FunctionAccess, HttpMethod};
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
    let (method, access) = match state.functions.get(&name) {
        Some(f) => (f.manifest.method, f.manifest.access()),
        None => return error(404, format!("unknown function `{name}`")),
    };

    if req.method() != expected_method(method) {
        return error(405, "method not allowed");
    }

    let principal = state.resolve_principal(&req).await;
    // The same vocabulary a resource's `[permissions]` uses, minus `owner`:
    // a function call has no row to own.
    match &access {
        FunctionAccess::Public => {}
        // A private function is not merely forbidden, it is not there — the
        // same answer an unknown name gets, so probing cannot enumerate it.
        FunctionAccess::Private => return error(404, "unknown function"),
        FunctionAccess::Authenticated => {
            if principal.is_none() {
                return error(401, "authentication required");
            }
        }
        // `member` and `role:` are both organisation-scoped: they need an
        // active organisation the caller actually belongs to.
        FunctionAccess::Member | FunctionAccess::Role(_) => {
            if principal.is_none() {
                return error(401, "authentication required");
            }
            let membership_role = state
                .active_org(&req, &principal)
                .zip(principal.as_ref())
                .and_then(|(org, principal)| principal.role_in(org));
            let ok = match (&access, membership_role) {
                (FunctionAccess::Member, role) => role.is_some(),
                (FunctionAccess::Role(required), Some(held)) => held == required,
                _ => false,
            };
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
        // The function faulted rather than faulting the caller. Log the detail and
        // answer generically — it names internals the caller shouldn't see.
        Ok(Err(msg)) => match msg.strip_prefix(apiplant_abi::INTERNAL_ERROR_PREFIX) {
            Some(detail) => {
                tracing::error!(function = %name, detail, "function faulted");
                error(500, "internal function error")
            }
            None => error(400, msg),
        },
        // Reached only if the *host* side of the blocking closure panicked;
        // panics inside the function are caught before they cross the ABI.
        Err(_) => {
            tracing::error!(function = %name, "function task panicked");
            error(500, "internal function error")
        }
    }
}
