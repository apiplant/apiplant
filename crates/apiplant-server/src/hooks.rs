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
//! | `{"data": …}` | replace the payload (`before_create`/`before_update`), answer the request without touching the database (`before_read`/`before_list`), or replace the response body (`after_*`) |
//! | `{"error": {"status": 422, "message": "…"}}` | abort the request with that status |
//! | `Err(msg)` from the handler | abort with `400` and `msg` |
//! | a panic in the handler | abort with `500`; the detail is logged, not returned |
//!
//! Hooks are called regardless of a function's `visibility`, so a
//! `Private` function — invisible over HTTP — is the natural way to write one.

use std::collections::HashMap;

use apiplant_auth::Principal;
use apiplant_core::{AuthEvent, HookEvent, Resource};
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
    /// The caller's primary role in the active organisation.
    role: Option<String>,
    /// Every role they hold there — what a `role:` permission is checked
    /// against, since one member can hold several and an admin holds all.
    roles: Vec<String>,
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
            // `role` is the caller's primary one and stays exactly what it was;
            // `roles` is every role they hold, which is what a permission is
            // actually checked against.
            roles: principal
                .zip(active_org)
                .map(|(p, org)| p.roles_in(org).to_vec())
                .unwrap_or_default(),
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
    describe(
        &resource.meta.name,
        event.as_str(),
        event.action(),
        event.phase(),
        request,
        slot,
        payload,
    )
}

/// Build the JSON context handed to an auth hook.
///
/// Same shape as a resource hook's context, so a function can be written once
/// and bound to either: a row the endpoint produced arrives in `row`, and
/// anything else — a submitted body, a login's outcome — in `data`.
fn auth_context_json(
    resource: &Resource,
    event: AuthEvent,
    request: &HookRequest,
    payload: &Value,
) -> String {
    let slot = match event {
        // `after_login` reports on an attempt rather than handing back a row,
        // and a failed attempt has no row to hand back at all.
        AuthEvent::AfterRegister | AuthEvent::AfterApiKey => "row",
        _ => "data",
    };
    describe(
        &resource.meta.name,
        event.as_str(),
        event.action(),
        event.phase(),
        request,
        slot,
        payload,
    )
}

/// The context object shared by both hook families, with `payload` dropped into
/// `slot`.
fn describe(
    resource: &str,
    event: &str,
    action: &str,
    phase: &str,
    request: &HookRequest,
    slot: &str,
    payload: &Value,
) -> String {
    let mut context = json!({
        "event": event,
        "action": action,
        "phase": phase,
        "resource": resource,
        "url": request.url,
        "method": request.method,
        "query": request.query,
        "authenticated": request.authenticated,
        "principal_id": request.principal_id,
        "organization_id": request.organization_id,
        "role": request.role,
        "roles": request.roles,
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
    let context = context_json(resource, event, request, &payload);
    invoke(
        state,
        resource,
        event.as_str(),
        name,
        request,
        context,
        payload,
    )
    .await
}

/// Publish the topic a resource declares for `event`, if it declares one.
///
/// The row is the message. A subscriber to `order.placed` gets the order,
/// exactly as the API would have returned it — so the handler is an ordinary
/// function over an ordinary object, and can be tested by posting one to it.
///
/// **Never fails the request.** By the time this runs the row is committed and
/// the caller's response is decided; turning "the message could not be queued"
/// into a 500 would tell them their write failed when it did not. It is logged
/// instead, at `error`, because a dropped announcement is a real problem — just
/// not theirs.
///
/// Deliberately after the `after_*` hook rather than before it: a hook may still
/// replace or reject the response, and announcing a create that the hook then
/// turned into a 409 would be announcing something that did not happen.
pub async fn announce(
    state: &AppState,
    resource: &Resource,
    event: HookEvent,
    request: &HookRequest,
    row: &Value,
) {
    let Some(topic) = resource.publish.get(event) else {
        return;
    };
    let published_by = request.principal_id.clone().unwrap_or_default();

    if let Err(error) = state.queue.publish(topic, row, &published_by).await {
        tracing::error!(
            resource = %resource.meta.name,
            event = event.as_str(),
            topic,
            %error,
            "could not publish the message this write declares — the write itself succeeded"
        );
    }
}

/// Run the auth hook bound to `event`, if the `user` resource declares one.
///
/// Same contract as [`run`]: `Ok(None)` carries on, `Ok(Some(value))` is a
/// replacement, `Err(response)` aborts the request. `resource` is the resource
/// the endpoint operates on — `user` for register/login, `api_key` for key
/// issuance — but the hook is always looked up on the `user` model, which is
/// the resource the auth endpoints belong to.
pub async fn run_auth(
    state: &AppState,
    resource: &Resource,
    event: AuthEvent,
    request: &HookRequest,
    payload: Value,
) -> Result<Option<Value>, HttpResponse> {
    let Some(user) = state.app.resources.get("user") else {
        return Ok(None);
    };
    let Some(name) = user.auth_hook(event) else {
        return Ok(None);
    };
    let context = auth_context_json(resource, event, request, &payload);
    invoke(
        state,
        resource,
        event.as_str(),
        name,
        request,
        context,
        payload,
    )
    .await
}

/// Call `name` with `payload` and `context`, and interpret what comes back.
async fn invoke(
    state: &AppState,
    resource: &Resource,
    event: &str,
    name: &str,
    request: &HookRequest,
    context: String,
    payload: Value,
) -> Result<Option<Value>, HttpResponse> {
    if state.functions.get(name).is_none() {
        tracing::error!(
            resource = %resource.meta.name,
            hook = event,
            function = name,
            "hook function is not loaded"
        );
        return Err(error(
            500,
            format!(
                "`{}` declares a `{event}` hook on a function `{name}` that is not loaded",
                resource.meta.name,
            ),
        ));
    }

    let input = payload.to_string();
    let principal_id = request.principal_id.clone().unwrap_or_default();

    // Move owned handles into the blocking worker; functions block on the DB.
    let functions = state.functions.clone();
    let db = state.db.clone();
    let mailer = state.mailer.clone();
    let cache = state.cache.clone();
    let payments = state.payments.clone();
    let ai = state.ai.clone();
    let queue = state.queue.clone();
    let handle = tokio::runtime::Handle::current();
    let name = name.to_string();
    let hook_name = name.clone();

    let result = tokio::task::spawn_blocking(move || {
        let f = functions.get(&name).expect("checked above");
        let bridge = HostBridge::new(db, handle, f.config_json.clone(), principal_id)
            .with_services(mailer, cache, payments, ai)
            .with_queue(queue)
            .with_hook(context);
        f.invoke(bridge, &input)
    })
    .await;

    match result {
        Ok(Ok(raw)) => outcome(&raw, &hook_name),
        // A hook that faulted must not abort the request with the caller's `400`;
        // it is the hook that is broken, not the request.
        Ok(Err(message)) => match message.strip_prefix(apiplant_abi::INTERNAL_ERROR_PREFIX) {
            Some(detail) => {
                crate::telemetry::record_error("hook_fault", detail);
                tracing::error!(hook = %hook_name, detail, "hook faulted");
                Err(error(500, "hook failed"))
            }
            None => Err(error(400, message)),
        },
        // Reached only if the *host* side of the blocking closure panicked;
        // panics inside the hook are caught before they cross the ABI.
        Err(_) => {
            crate::telemetry::record_error("hook_panic", &hook_name);
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
            role: Some("support".into()),
            roles: vec!["support".into(), "billing".into()],
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
        // `role` is the caller's primary one, unchanged; `roles` is every role
        // they hold, which is what a `role:` permission is checked against.
        assert_eq!(context["role"], "support");
        assert_eq!(context["roles"][0], "support");
        assert_eq!(context["roles"][1], "billing");
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
