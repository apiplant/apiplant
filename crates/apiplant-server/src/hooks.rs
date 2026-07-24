//! Resource lifecycle hooks: running a named function around a CRUD operation.
//!
//! A resource declares hooks in its `[hooks]` section, one function name per
//! [`HookEvent`]:
//!
//! ```toml
//! [hooks]
//! before_create = "validate_post"
//! after_create  = "notify_slack"
//! ```
//!
//! `before_*` hooks run after the permission check but before the database is
//! touched; `after_*` hooks run once the operation succeeded. Both receive the
//! operation's *payload* as their input (the submitted body, the row, or the
//! list of rows) plus a [context object](context_json) describing the event,
//! the request URL, the caller's auth status and the row(s) in play — reachable
//! from a function through `ctx.hook()`.
//!
//! What a hook returns decides what happens next:
//!
//! | Return value | Effect |
//! |--------------|--------|
//! | `{}` / `null` / anything else | continue unchanged |
//! | `{"data": …}` | replace the payload (`before_create`/`before_update`) or the response body (`after_*`) |
//! | `{"error": {"status": 422, "message": "…"}}` | abort the request with that status |
//! | `Err(msg)` from the handler | abort with `400` and `msg` |
//! | a panic in the handler | abort with `500`; the detail is logged, not returned |
//!
//! Hooks are called regardless of a function's `visibility`, so a
//! `Private` function — invisible over HTTP — is the natural way to write one.

use std::collections::HashMap;

use apiplant_auth::Principal;
use apiplant_core::{HookEvent, Resource};
use ntex::web::{HttpRequest, HttpResponse};
use serde_json::{json, Value};
use uuid::Uuid;

use crate::functions::HostBridge;
use crate::response::error;
use crate::state::AppState;

/// The request-scoped facts every hook sees, independent of the event.
///
/// Built once per handler and reused by that handler's before/after hooks.
#[derive(Debug, Clone)]
pub struct HookRequest {
    url: String,
    method: String,
    query: HashMap<String, String>,
    authenticated: bool,
    principal_id: Option<String>,
    organization_id: Option<String>,
    role: Option<String>,
    record_id: Option<String>,
}

impl HookRequest {
    /// Capture the current request and the caller's resolved identity.
    pub fn new(
        req: &HttpRequest,
        query: &HashMap<String, String>,
        principal: Option<&Principal>,
        active_org: Option<Uuid>,
    ) -> Self {
        HookRequest {
            url: req.uri().to_string(),
            method: req.method().to_string(),
            query: query.clone(),
            authenticated: principal.is_some(),
            principal_id: principal.map(|p| p.user_id.to_string()),
            organization_id: active_org.map(|org| org.to_string()),
            role: principal
                .zip(active_org)
                .and_then(|(p, org)| p.role_in(org))
                .map(str::to_string),
            record_id: None,
        }
    }

    /// Attach the record id from the URL, for single-record operations.
    pub fn with_record(mut self, id: Uuid) -> Self {
        self.record_id = Some(id.to_string());
        self
    }
}

/// Build the JSON context handed to the hook function.
///
/// The payload is mirrored into the slot the event implies — `data` for the
/// `before_create`/`before_update` body, `rows` for `after_list`, `row`
/// everywhere else — so a hook can read it without knowing how it was invoked.
fn context_json(
    resource: &Resource,
    event: HookEvent,
    request: &HookRequest,
    payload: &Value,
) -> String {
    let slot = match event {
        HookEvent::BeforeCreate | HookEvent::BeforeUpdate => "data",
        HookEvent::AfterList => "rows",
        _ => "row",
    };
    let mut context = json!({
        "event": event.as_str(),
        "action": event.action(),
        "phase": event.phase(),
        "resource": resource.meta.name,
        "url": request.url,
        "method": request.method,
        "query": request.query,
        "authenticated": request.authenticated,
        "principal_id": request.principal_id,
        "organization_id": request.organization_id,
        "role": request.role,
        "record_id": request.record_id,
        "data": Value::Null,
        "row": Value::Null,
        "rows": Value::Null,
    });
    context[slot] = payload.clone();
    context.to_string()
}

/// Run the hook bound to `event`, if the resource declares one.
///
/// Returns `Ok(None)` to carry on unchanged, `Ok(Some(value))` when the hook
/// replaced the payload/response, and `Err(response)` when it aborted the
/// request. A declared hook whose function isn't loaded fails closed with a
/// `500` — silently skipping it would bypass validation.
pub async fn run(
    state: &AppState,
    resource: &Resource,
    event: HookEvent,
    request: &HookRequest,
    payload: Value,
) -> Result<Option<Value>, HttpResponse> {
    let Some(name) = resource.hook(event) else {
        return Ok(None);
    };
    if state.functions.get(name).is_none() {
        tracing::error!(
            resource = %resource.meta.name,
            hook = event.as_str(),
            function = name,
            "hook function is not loaded"
        );
        return Err(error(
            500,
            format!(
                "`{}` declares a `{}` hook on a function `{name}` that is not loaded",
                resource.meta.name,
                event.as_str()
            ),
        ));
    }

    let context = context_json(resource, event, request, &payload);
    let input = payload.to_string();
    let principal_id = request.principal_id.clone().unwrap_or_default();

    // Move owned handles into the blocking worker; functions block on the DB.
    let functions = state.functions.clone();
    let db = state.db.clone();
    let handle = tokio::runtime::Handle::current();
    let name = name.to_string();
    let hook_name = name.clone();

    let result = tokio::task::spawn_blocking(move || {
        let f = functions.get(&name).expect("checked above");
        let bridge =
            HostBridge::new(db, handle, f.config_json.clone(), principal_id).with_hook(context);
        f.invoke(bridge, &input)
    })
    .await;

    match result {
        Ok(Ok(raw)) => outcome(&raw, &hook_name),
        // A hook that faulted must not abort the request with the caller's `400`;
        // it is the hook that is broken, not the request.
        Ok(Err(message)) => match message.strip_prefix(apiplant_abi::INTERNAL_ERROR_PREFIX) {
            Some(detail) => {
                tracing::error!(hook = %hook_name, detail, "hook faulted");
                Err(error(500, "hook failed"))
            }
            None => Err(error(400, message)),
        },
        // Reached only if the *host* side of the blocking closure panicked;
        // panics inside the hook are caught before they cross the ABI.
        Err(_) => {
            tracing::error!(hook = %hook_name, "hook task panicked");
            Err(error(500, "hook failed"))
        }
    }
}

/// Interpret what a hook returned. See the module docs for the protocol.
fn outcome(raw: &str, hook_name: &str) -> Result<Option<Value>, HttpResponse> {
    let value: Value = match serde_json::from_str(raw) {
        Ok(v) => v,
        Err(e) => {
            tracing::error!(hook = %hook_name, error = %e, "hook returned invalid JSON");
            return Err(error(
                500,
                format!("hook `{hook_name}` returned invalid JSON"),
            ));
        }
    };
    let Some(object) = value.as_object() else {
        return Ok(None);
    };
    if let Some(rejection) = object.get("error") {
        let (status, message) = match rejection {
            Value::String(message) => (400, message.clone()),
            Value::Object(details) => (
                details
                    .get("status")
                    .and_then(Value::as_u64)
                    .and_then(|s| u16::try_from(s).ok())
                    .filter(|s| (400..=599).contains(s))
                    .unwrap_or(400),
                details
                    .get("message")
                    .and_then(Value::as_str)
                    .unwrap_or("rejected by hook")
                    .to_string(),
            ),
            other => (400, other.to_string()),
        };
        return Err(error(status, message));
    }
    Ok(object.get("data").cloned())
}

/// A hook's replacement payload, which must stay a JSON object for the
/// operations that write columns.
pub fn replacement_object(
    replacement: Value,
    hook_name: &str,
) -> Result<serde_json::Map<String, Value>, HttpResponse> {
    match replacement {
        Value::Object(map) => Ok(map),
        _ => Err(error(
            500,
            format!("hook `{hook_name}` replaced the body with a non-object value"),
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_resource(src: &str) -> Resource {
        let resource: Resource = toml::from_str(src).unwrap();
        resource.validate().unwrap();
        resource
    }

    fn request() -> HookRequest {
        HookRequest {
            url: "/api/post?draft=true".into(),
            method: "POST".into(),
            query: HashMap::from([("draft".to_string(), "true".to_string())]),
            authenticated: true,
            principal_id: Some("11111111-1111-1111-1111-111111111111".into()),
            organization_id: Some("22222222-2222-2222-2222-222222222222".into()),
            role: Some("admin".into()),
            record_id: None,
        }
    }

    #[test]
    fn context_describes_the_event_and_the_caller() {
        let resource = parse_resource("[resource]\nname = \"post\"\n");
        let raw = context_json(
            &resource,
            HookEvent::BeforeCreate,
            &request(),
            &json!({ "title": "Draft" }),
        );
        let context: Value = serde_json::from_str(&raw).unwrap();

        assert_eq!(context["event"], "before_create");
        assert_eq!(context["action"], "create");
        assert_eq!(context["phase"], "before");
        assert_eq!(context["resource"], "post");
        assert_eq!(context["url"], "/api/post?draft=true");
        assert_eq!(context["method"], "POST");
        assert_eq!(context["query"]["draft"], "true");
        assert_eq!(context["authenticated"], true);
        assert_eq!(context["role"], "admin");
        assert!(context["record_id"].is_null());
    }

    #[test]
    fn payload_lands_in_the_slot_the_event_implies() {
        let resource = parse_resource("[resource]\nname = \"post\"\n");
        let row = json!({ "id": "abc", "title": "Hi" });

        let created: Value = serde_json::from_str(&context_json(
            &resource,
            HookEvent::AfterCreate,
            &request(),
            &row,
        ))
        .unwrap();
        assert_eq!(created["row"], row);
        assert!(created["data"].is_null());
        assert!(created["rows"].is_null());

        let listed: Value = serde_json::from_str(&context_json(
            &resource,
            HookEvent::AfterList,
            &request(),
            &json!([row]),
        ))
        .unwrap();
        assert_eq!(listed["rows"].as_array().unwrap().len(), 1);
        assert!(listed["row"].is_null());

        let submitted: Value = serde_json::from_str(&context_json(
            &resource,
            HookEvent::BeforeUpdate,
            &request(),
            &json!({ "title": "Edited" }),
        ))
        .unwrap();
        assert_eq!(submitted["data"]["title"], "Edited");
        assert!(submitted["row"].is_null());
    }

    #[test]
    fn record_id_is_carried_for_single_record_operations() {
        let resource = parse_resource("[resource]\nname = \"post\"\n");
        let id = Uuid::new_v4();
        let raw = context_json(
            &resource,
            HookEvent::BeforeDelete,
            &request().with_record(id),
            &json!({}),
        );
        let context: Value = serde_json::from_str(&raw).unwrap();
        assert_eq!(context["record_id"], id.to_string());
    }

    #[test]
    fn outcome_continues_on_empty_or_unrecognised_replies() {
        assert!(outcome("{}", "h").unwrap().is_none());
        assert!(outcome("null", "h").unwrap().is_none());
        assert!(outcome("\"ok\"", "h").unwrap().is_none());
        assert!(outcome(r#"{"logged":true}"#, "h").unwrap().is_none());
    }

    #[test]
    fn outcome_extracts_replacement_data() {
        let replacement = outcome(r#"{"data":{"title":"clean"}}"#, "h")
            .unwrap()
            .unwrap();
        assert_eq!(replacement["title"], "clean");

        let rows = outcome(r#"{"data":[{"id":"a"}]}"#, "h").unwrap().unwrap();
        assert_eq!(rows.as_array().unwrap().len(), 1);
    }

    #[test]
    fn outcome_maps_rejections_to_http_statuses() {
        let err = outcome(
            r#"{"error":{"status":422,"message":"title required"}}"#,
            "h",
        )
        .unwrap_err();
        assert_eq!(err.status().as_u16(), 422);

        let plain = outcome(r#"{"error":"nope"}"#, "h").unwrap_err();
        assert_eq!(plain.status().as_u16(), 400);

        // A status outside 4xx/5xx (or a missing one) falls back to 400.
        let odd = outcome(r#"{"error":{"status":200,"message":"x"}}"#, "h").unwrap_err();
        assert_eq!(odd.status().as_u16(), 400);
        let bare = outcome(r#"{"error":{}}"#, "h").unwrap_err();
        assert_eq!(bare.status().as_u16(), 400);
    }

    #[test]
    fn outcome_rejects_malformed_json_with_a_500() {
        let err = outcome("{not json", "h").unwrap_err();
        assert_eq!(err.status().as_u16(), 500);
    }

    #[test]
    fn replacement_must_be_an_object_for_writes() {
        let map = replacement_object(json!({ "title": "x" }), "h").unwrap();
        assert_eq!(map["title"], "x");

        let err = replacement_object(json!([1, 2]), "h").unwrap_err();
        assert_eq!(err.status().as_u16(), 500);
    }
}
