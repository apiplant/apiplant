//! OpenAPI 3.0 document generation and the Swagger UI page.
//!
//! The spec is derived entirely from the loaded app — resources become CRUD
//! paths with schemas, the built-in auth routes are described, and every loaded
//! function gets a path. Two security schemes are declared so Swagger UI's
//! **Authorize** button works out of the box:
//!
//! * `bearerAuth` — a session JWT (`Authorization: Bearer <token>`),
//! * `apiKeyAuth` — an API key in the `X-Api-Key` header.
//!
//! An operation references those schemes whenever its resource/function policy
//! requires authentication; public operations carry no security requirement.

use apiplant_abi::{FunctionAccess, HttpMethod};
use apiplant_core::schema::{Access, Field, FieldType, Policy};
use apiplant_core::{Agent, App, Resource, Scope};
use serde_json::{json, Map, Value};

use crate::functions::FunctionRegistry;

/// Build the full OpenAPI document for an app + its loaded functions.
///
/// `email_enabled` is whether the app has a working `[email]` provider. It
/// cannot be read off the `App` — a provider is only *known* to work once the
/// mailer is built — and it decides whether the invitation, confirmation and
/// password-reset paths are described at all. Documenting an endpoint the
/// server does not mount would send every reader of these docs to a 404.
pub fn build(app: &App, functions: &FunctionRegistry, email_enabled: bool) -> Value {
    let base = &app.config.server.base_path;
    let server_url = if base.is_empty() { "/" } else { base.as_str() };

    let mut paths = Map::new();
    let mut schemas = Map::new();

    // Resources → schemas + CRUD paths.
    //
    // A resource whose every action is `private` — `auth_token`, `oauth_state`:
    // machinery the framework writes and nobody calls — contributes no
    // operations, and a path entry with none under it is a heading in the docs
    // page with nothing beneath it. Its schemas stay, since a hook's payload can
    // still refer to one.
    for r in app.resources.values() {
        schemas.insert(read_schema_name(r), resource_read_schema(r));
        schemas.insert(input_schema_name(r), resource_input_schema(r));

        let collection = collection_path(r);
        if has_operations(&collection) {
            paths.insert(format!("/{}", r.meta.name), collection);
        }
        let item = item_path(r);
        if has_operations(&item) {
            paths.insert(format!("/{}/{{id}}", r.meta.name), item);
        }
    }

    // Nested has_many collections: /parent/{id}/child for each reverse relation.
    for parent in app.resources.values() {
        for child in app.resources.values() {
            let related: Vec<_> = child
                .references()
                .into_iter()
                .filter(|rf| rf.target == parent.meta.name)
                .collect();
            // A nested collection is a *list* of the child, so it answers to the
            // child's `list` policy — and a private one has no endpoint here
            // any more than it does at `/{child}`. Documenting it would promise
            // a route that answers 404.
            if related.is_empty() || child.permissions.list.level == Access::Private {
                continue;
            }
            paths.insert(
                format!("/{}/{{id}}/{}", parent.meta.name, child.meta.name),
                nested_path(parent, child, &related),
            );
        }
    }

    // Built-in auth endpoints.
    paths.insert("/auth/register".into(), auth_register_path());
    paths.insert("/auth/login".into(), auth_login_path(app));
    paths.insert("/auth/me".into(), auth_me_path());
    paths.insert("/auth/apikeys".into(), auth_apikeys_path());

    // The mailbox flows, described only where they are mounted — the same three
    // conditions `build_app!` registers the routes under.
    let auth = &app.config.auth;
    if auth.invitations_enabled(email_enabled) {
        paths.insert("/auth/invitations".into(), auth_invitations_path());
        paths.insert(
            "/auth/invitations/{token}".into(),
            auth_invitation_preview_path(),
        );
        paths.insert(
            "/auth/invitations/{token}/accept".into(),
            auth_invitation_accept_path(),
        );
    }
    if auth.requires_email_verification(email_enabled) {
        paths.insert("/auth/verify-email".into(), auth_verify_email_path());
        paths.insert(
            "/auth/verify-email/resend".into(),
            auth_resend_verification_path(),
        );
    }
    if auth.password_reset_enabled(email_enabled) {
        paths.insert("/auth/password/forgot".into(), auth_forgot_password_path());
        paths.insert("/auth/password/reset".into(), auth_reset_password_path());
    }

    // Signing in with somebody else's account, described only where a
    // provider is configured — the same condition `build_app!` mounts the
    // routes under.
    if app.config.oauth.enabled() {
        paths.insert("/auth/oauth".into(), oauth_providers_path(app));
        paths.insert("/auth/oauth/{provider}/start".into(), oauth_start_path(app));
        paths.insert(
            "/auth/oauth/{provider}/callback".into(),
            oauth_callback_path(app),
        );
        paths.insert("/auth/oauth/{provider}".into(), oauth_unlink_path(app));
    }

    // Billing, described only where a provider is configured — the same
    // condition `build_app!` mounts the routes under.
    if app.config.payments.enabled() {
        paths.insert("/billing/config".into(), billing_config_path());
        paths.insert("/billing/checkout".into(), billing_checkout_path());
        paths.insert("/billing/portal".into(), billing_portal_path());
        paths.insert("/billing/webhook".into(), billing_webhook_path());
    }

    if app.config.ai.enabled() {
        paths.insert("/ai/config".into(), ai_config_path(app));
        paths.insert("/ai/chat".into(), ai_chat_path(app));
        for agent in app.agents.values() {
            paths.insert(
                format!("/ai/agents/{}/chat", agent.meta.name),
                ai_agent_chat_path(agent),
            );
        }
    }

    // Function endpoints. Typed input/output schemas (from the function's
    // manifest, generated by the `function!` macro) are registered as components
    // and referenced, so function bodies are typed in the docs.
    for f in functions.iter() {
        let m = &f.manifest;
        let access = m.access();
        if access == FunctionAccess::Private {
            continue;
        }
        let input_ref = ingest_fn_schema(
            &mut schemas,
            m.name.as_str(),
            "Input",
            m.input_schema.as_str(),
        );
        let output_ref = ingest_fn_schema(
            &mut schemas,
            m.name.as_str(),
            "Output",
            m.output_schema.as_str(),
        );
        paths.insert(
            format!("/functions/{}", m.name),
            function_path(
                m.method,
                &access,
                m.name.as_str(),
                m.description.as_str(),
                input_ref,
                output_ref,
            ),
        );
    }

    json!({
        "openapi": "3.0.3",
        "info": {
            "title": app.docs_title(),
            "version": env!("CARGO_PKG_VERSION"),
            "description": "API generated by apiplant from resource, auth and function definitions.",
        },
        "servers": [{ "url": server_url }],
        "components": {
            "securitySchemes": {
                "bearerAuth": {
                    "type": "http",
                    "scheme": "bearer",
                    "bearerFormat": "JWT",
                    "description": "A session token from POST /auth/login or /auth/register.",
                },
                "apiKeyAuth": {
                    "type": "apiKey",
                    "in": "header",
                    "name": "X-Api-Key",
                    "description": "An API key from POST /auth/apikeys. Acts as its owning user.",
                },
            },
            "schemas": schemas,
        },
        "paths": paths,
    })
}

// --- schema generation ----------------------------------------------------

fn read_schema_name(r: &Resource) -> String {
    pascal(&r.meta.name)
}
fn input_schema_name(r: &Resource) -> String {
    format!("{}Input", pascal(&r.meta.name))
}

fn field_schema(f: &Field) -> Value {
    let mut base = match f.ty {
        FieldType::String | FieldType::Text => json!({ "type": "string" }),
        FieldType::File => json!({ "type": "string", "format": "uri-reference" }),
        FieldType::Integer | FieldType::BigInt => json!({ "type": "integer" }),
        FieldType::Float => json!({ "type": "number" }),
        FieldType::Boolean => json!({ "type": "boolean" }),
        FieldType::Uuid | FieldType::Reference => json!({ "type": "string", "format": "uuid" }),
        FieldType::Timestamp => json!({ "type": "string", "format": "date-time" }),
        FieldType::Json => json!({}),
    };
    if let (Some(max), Value::Object(map)) = (f.max_length, &mut base) {
        map.insert("maxLength".into(), json!(max));
    }
    base
}

/// Read representation: id + non-hidden fields + timestamps, all read-only where
/// server-managed.
fn resource_read_schema(r: &Resource) -> Value {
    let mut props = Map::new();
    props.insert(
        "id".into(),
        json!({ "type": "string", "format": "uuid", "readOnly": true }),
    );
    for (name, field) in &r.fields {
        if field.hidden {
            continue;
        }
        props.insert(name.clone(), field_schema(field));
    }
    if r.meta.timestamps {
        props.insert(
            "created_at".into(),
            json!({ "type": "string", "format": "date-time", "readOnly": true }),
        );
        props.insert(
            "updated_at".into(),
            json!({ "type": "string", "format": "date-time", "readOnly": true }),
        );
    }
    json!({ "type": "object", "properties": props })
}

/// Write representation for create/update: writable fields only (hidden and the
/// auto-stamped owner column are excluded).
fn resource_input_schema(r: &Resource) -> Value {
    let mut props = Map::new();
    let mut required = Vec::new();
    for (name, field) in &r.fields {
        // Hidden, the auto-stamped owner, and the auto-stamped organisation are
        // never client-supplied.
        if field.hidden || name == &r.meta.owner_field || name == "organization_id" {
            continue;
        }
        props.insert(name.clone(), field_schema(field));
        if field.required {
            required.push(json!(name));
        }
    }
    let mut obj = json!({ "type": "object", "properties": props });
    if !required.is_empty() {
        obj["required"] = json!(required);
    }
    obj
}

// --- security -------------------------------------------------------------

/// The security requirement for an action, or `None` when public.
///
/// A class-qualified `public` is not public: it needs an organisation, and an
/// organisation needs credentials.
fn security_for(policy: &Policy) -> Option<Value> {
    match (&policy.level, &policy.org_class) {
        (Access::Public, None) => None,
        _ => Some(json!([{ "bearerAuth": [] }, { "apiKeyAuth": [] }])),
    }
}

fn access_note(policy: &Policy) -> String {
    let note = access_level_note(&policy.level);
    match &policy.org_class {
        Some(class) => format!("{note} Only in organisations of class `{class}`."),
        None => note,
    }
}

fn access_level_note(access: &Access) -> String {
    match access {
        Access::Public => "Public — no authentication required.".into(),
        Access::Authenticated => "Requires authentication.".into(),
        Access::Member => "Requires membership of the active organisation.".into(),
        Access::Owner => "Requires authentication; scoped to records you own.".into(),
        Access::Role(role) => format!("Requires the `{role}` role in the active organisation."),
        Access::Private => "Not exposed.".into(),
    }
}

/// Attach `security` to an operation object when the access policy demands it.
fn with_security(mut op: Value, access: &Policy) -> Value {
    if let Some(sec) = security_for(access) {
        op["security"] = sec;
    }
    op
}

fn with_agent_security(mut op: Value, agent: &Agent) -> Value {
    let secure = if agent.meta.scope == Scope::Organization {
        Some(json!([{ "bearerAuth": [] }, { "apiKeyAuth": [] }]))
    } else {
        match (
            &agent.permissions.chat.level,
            &agent.permissions.chat.org_class,
        ) {
            (Access::Public, None) => None,
            _ => Some(json!([{ "bearerAuth": [] }, { "apiKeyAuth": [] }])),
        }
    };
    if let Some(sec) = secure {
        op["security"] = sec;
    }
    op
}

fn json_body(schema_ref: Value) -> Value {
    json!({ "required": true, "content": { "application/json": { "schema": schema_ref } } })
}

fn ref_to(name: &str) -> Value {
    json!({ "$ref": format!("#/components/schemas/{name}") })
}

// --- resource paths -------------------------------------------------------

/// Whether a path object carries any operation at all, as against only the
/// `parameters` every operation on it would share.
fn has_operations(path: &Value) -> bool {
    path.as_object()
        .is_some_and(|entries| entries.keys().any(|key| key != "parameters"))
}

fn collection_path(r: &Resource) -> Value {
    let name = &r.meta.name;
    let read_ref = read_schema_name(r);
    let input_ref = input_schema_name(r);
    let mut path = Map::new();

    if r.permissions.list.level != Access::Private {
        let op = with_security(
            json!({
                "tags": [name],
                "operationId": format!("list_{name}"),
                "summary": format!("List {name}"),
                "description": access_note(&r.permissions.list),
                "parameters": list_parameters(r),
                "responses": {
                    "200": {
                        "description": "A page of records",
                        "content": { "application/json": {
                            "schema": { "type": "array", "items": ref_to(&read_ref) }
                        } }
                    }
                }
            }),
            &r.permissions.list,
        );
        path.insert("get".into(), op);
    }

    if r.permissions.create.level != Access::Private {
        let op = with_security(
            json!({
                "tags": [name],
                "operationId": format!("create_{name}"),
                "summary": format!("Create {name}"),
                "description": access_note(&r.permissions.create),
                "requestBody": json_body(ref_to(&input_ref)),
                "responses": {
                    "201": { "description": "Created", "content": { "application/json": { "schema": ref_to(&read_ref) } } },
                    "400": { "description": "Invalid input" },
                    "401": { "description": "Authentication required" },
                }
            }),
            &r.permissions.create,
        );
        path.insert("post".into(), op);
    }

    Value::Object(path)
}

fn item_path(r: &Resource) -> Value {
    let name = &r.meta.name;
    let read_ref = read_schema_name(r);
    let input_ref = input_schema_name(r);
    let id_param = json!([{
        "name": "id", "in": "path", "required": true,
        "schema": { "type": "string", "format": "uuid" }
    }]);
    let mut path = Map::new();
    path.insert("parameters".into(), id_param);

    if r.permissions.read.level != Access::Private {
        path.insert(
            "get".into(),
            with_security(
                json!({
                    "tags": [name],
                    "operationId": format!("get_{name}"),
                    "summary": format!("Fetch a {name} by id"),
                    "description": access_note(&r.permissions.read),
                    "parameters": [expand_parameter(r)],
                    "responses": {
                        "200": { "description": "The record", "content": { "application/json": { "schema": ref_to(&read_ref) } } },
                        "404": { "description": "Not found" },
                    }
                }),
                &r.permissions.read,
            ),
        );
    }

    if r.permissions.update.level != Access::Private {
        let update_op = with_security(
            json!({
                "tags": [name],
                "operationId": format!("update_{name}"),
                "summary": format!("Update a {name}"),
                "description": access_note(&r.permissions.update),
                "requestBody": json_body(ref_to(&input_ref)),
                "responses": {
                    "200": { "description": "Updated", "content": { "application/json": { "schema": ref_to(&read_ref) } } },
                    "404": { "description": "Not found" },
                }
            }),
            &r.permissions.update,
        );
        path.insert("patch".into(), update_op.clone());
        path.insert("put".into(), update_op);
    }

    if r.permissions.delete.level != Access::Private {
        path.insert(
            "delete".into(),
            with_security(
                json!({
                    "tags": [name],
                    "operationId": format!("delete_{name}"),
                    "summary": format!("Delete a {name}"),
                    "description": access_note(&r.permissions.delete),
                    "responses": {
                        "204": { "description": "Deleted" },
                        "404": { "description": "Not found" },
                    }
                }),
                &r.permissions.delete,
            ),
        );
    }

    Value::Object(path)
}

// --- list parameters & nested relationship paths --------------------------

fn relation_names(r: &Resource) -> Vec<String> {
    r.references().into_iter().map(|rf| rf.relation).collect()
}

/// The `?expand=` query parameter, documenting the relations available to inline.
fn expand_parameter(r: &Resource) -> Value {
    let rels = relation_names(r);
    let desc = if rels.is_empty() {
        "Comma-separated relations to inline (this resource has no references).".to_string()
    } else {
        format!(
            "Comma-separated relations to inline. Available: {}.",
            rels.join(", ")
        )
    };
    json!({
        "name": "expand", "in": "query", "required": false,
        "schema": { "type": "string" }, "description": desc,
    })
}

/// The `?order=` query parameter, naming the columns a caller may sort by.
fn order_parameter(r: &Resource) -> Value {
    let mut columns = vec!["id".to_string()];
    if r.meta.timestamps {
        columns.push("created_at".to_string());
        columns.push("updated_at".to_string());
    }
    columns.extend(
        r.fields
            .iter()
            .filter(|(_, field)| !field.hidden)
            .map(|(name, _)| name.clone()),
    );
    json!({
        "name": "order", "in": "query", "required": false,
        "schema": { "type": "string" },
        "description": format!(
            "Sort key(s), comma-separated; prefix with `-` (or suffix `:desc`) for descending. Available: {}.",
            columns.join(", ")
        ),
    })
}

/// The `?search=`/`?search_fields=` pair, listed only where something is
/// searchable — a documented parameter that always answers `400` is worse than
/// no parameter at all.
fn search_parameters(r: &Resource) -> Option<Vec<Value>> {
    let configured = r.admin_search_fields();
    if configured.is_empty() {
        return None;
    }
    let searchable: Vec<String> = r
        .fields
        .iter()
        .filter(|(_, field)| {
            !field.hidden && matches!(field.ty, FieldType::String | FieldType::Text)
        })
        .map(|(name, _)| name.clone())
        .collect();
    Some(vec![
        json!({
            "name": "search", "in": "query", "required": false,
            "schema": { "type": "string" },
            "description": format!(
                "Case-insensitive substring search across {} (unless `search_fields` narrows it).",
                configured.join(", ")
            ),
        }),
        json!({
            "name": "search_fields", "in": "query", "required": false,
            "schema": { "type": "string" },
            "description": format!(
                "Comma-separated columns for `search` to look in instead of the configured set. Available: {}.",
                searchable.join(", ")
            ),
        }),
    ])
}

/// Query parameters for a list operation: paging, expansion, ordering, search,
/// and one exact-match filter per column.
fn list_parameters(r: &Resource) -> Value {
    let mut params = vec![
        json!({ "name": "limit", "in": "query", "schema": { "type": "integer", "default": 50, "maximum": 500 } }),
        json!({ "name": "offset", "in": "query", "schema": { "type": "integer", "default": 0 } }),
        expand_parameter(r),
        order_parameter(r),
    ];
    if let Some(search) = search_parameters(r) {
        params.extend(search);
    }
    for (name, field) in &r.fields {
        if field.hidden {
            continue;
        }
        params.push(json!({
            "name": name, "in": "query", "required": false,
            "schema": field_schema(field),
            "description": format!("Filter by exact `{name}`."),
        }));
    }
    Value::Array(params)
}

/// A `GET /parent/{id}/child` nested-collection path (reverse `has_many`).
fn nested_path(parent: &Resource, child: &Resource, related: &[apiplant_core::Reference]) -> Value {
    let child_name = &child.meta.name;
    let parent_name = &parent.meta.name;
    let via_note = if related.len() > 1 {
        let fields = related
            .iter()
            .map(|rf| format!("`{}`", rf.field))
            .collect::<Vec<_>>()
            .join(", ");
        format!(" `{child_name}` references `{parent_name}` via {fields}; add `?via=<field>` to disambiguate.")
    } else {
        String::new()
    };
    let op = with_security(
        json!({
            "tags": [child_name],
            "operationId": format!("list_{child_name}_by_{parent_name}"),
            "summary": format!("List {child_name} belonging to a {parent_name}"),
            "description": format!("{}{}", access_note(&child.permissions.list), via_note),
            "parameters": [
                { "name": "id", "in": "path", "required": true, "schema": { "type": "string", "format": "uuid" } },
                { "name": "limit", "in": "query", "schema": { "type": "integer", "default": 50, "maximum": 500 } },
                { "name": "offset", "in": "query", "schema": { "type": "integer", "default": 0 } },
                order_parameter(child),
            ],
            "responses": {
                "200": {
                    "description": format!("A page of {child_name}"),
                    "content": { "application/json": { "schema": { "type": "array", "items": ref_to(&read_schema_name(child)) } } }
                }
            }
        }),
        &child.permissions.list,
    );
    json!({ "get": op })
}

// --- auth paths -----------------------------------------------------------

fn token_response() -> Value {
    json!({
        "type": "object",
        "properties": { "token": { "type": "string" } }
    })
}

// --- ai paths -------------------------------------------------------------

fn ai_config_path(_app: &App) -> Value {
    json!({
        "get": {
            "tags": ["ai"],
            "operationId": "ai_config",
            "summary": "Describe the configured assistant",
            "description": "Public metadata a front end needs before rendering a chat box: provider, default model, access, and the configured agents.",
            "responses": {
                "200": {
                    "description": "Assistant configuration",
                    "content": { "application/json": { "schema": {
                        "type": "object",
                        "properties": {
                            "provider": { "type": "string" },
                            "model": { "type": "string" },
                            "access": { "type": "string" },
                            "streaming": { "type": "boolean" },
                            "agents": {
                                "type": "array",
                                "items": {
                                    "type": "object",
                                    "properties": {
                                        "name": { "type": "string" },
                                        "description": { "type": "string" },
                                        "access": { "type": "string" },
                                        "scope": { "type": "string", "enum": ["global", "organization"] },
                                        "storage": { "type": "boolean" }
                                    }
                                }
                            }
                        }
                    } } }
                }
            }
        }
    })
}

fn ai_chat_path(app: &App) -> Value {
    let access = Policy::parse(&app.config.ai.access);
    json!({
        "post": with_security(
            json!({
                "tags": ["ai"],
                "operationId": "ai_chat",
                "summary": "Send a conversation to the assistant",
                "description": "Streams `delta`/`reasoning`/`done` events by default; `\"stream\": false` asks for one JSON reply instead.",
                "requestBody": json_body(json!({
                    "type": "object",
                    "properties": {
                        "messages": {
                            "type": "array",
                            "items": {
                                "type": "object",
                                "properties": {
                                    "role": { "type": "string", "enum": ["system", "user", "assistant"] },
                                    "content": { "type": "string" }
                                },
                                "required": ["role", "content"]
                            }
                        },
                        "model": { "type": "string" },
                        "system": { "type": "string" },
                        "temperature": { "type": "number" },
                        "max_tokens": { "type": "integer" },
                        "stream": { "type": "boolean", "default": true }
                    },
                    "required": ["messages"]
                })),
                "responses": {
                    "200": {
                        "description": "A streaming SSE response, or one JSON reply when `stream` is false.",
                        "content": {
                            "text/event-stream": { "schema": { "type": "string" } },
                            "application/json": { "schema": {
                                "type": "object",
                                "properties": {
                                    "text": { "type": "string" },
                                    "provider": { "type": "string" },
                                    "model": { "type": "string" },
                                    "finish_reason": { "type": "string" },
                                    "input_tokens": { "type": "integer" },
                                    "output_tokens": { "type": "integer" }
                                }
                            } }
                        }
                    }
                }
            }),
            &access,
        )
    })
}

fn ai_agent_chat_path(agent: &Agent) -> Value {
    let description = match agent.meta.scope {
        Scope::Global => format!(
            "{} Messages stream by default. When storage is enabled, a missing `thread_id` starts a new conversation and a present one continues it.",
            access_note(&agent.permissions.chat)
        ),
        Scope::Organization => format!(
            "Requires the active organisation in `X-Organization`. {} Messages stream by default. When storage is enabled, a missing `thread_id` starts a new conversation and a present one continues it.",
            match &agent.permissions.chat.level {
                Access::Role(role) => format!("Requires the `{role}` role in that organisation."),
                _ => "Any member who may use the agent may chat with it.".to_string(),
            }
        ),
    };
    json!({
        "post": with_agent_security(
            json!({
                "tags": ["ai"],
                "operationId": format!("ai_agent_{}_chat", agent.meta.name),
                "summary": format!("Chat with the `{}` agent", agent.meta.name),
                "description": description,
                "requestBody": json_body(json!({
                    "type": "object",
                    "properties": {
                        "message": { "type": "string" },
                        "messages": {
                            "type": "array",
                            "items": {
                                "type": "object",
                                "properties": {
                                    "role": { "type": "string", "enum": ["user", "assistant"] },
                                    "content": { "type": "string" }
                                },
                                "required": ["role", "content"]
                            }
                        },
                        "thread_id": { "type": "string", "format": "uuid" },
                        "title": { "type": "string" },
                        "stream": { "type": "boolean", "default": true }
                    },
                    "required": ["message"]
                })),
                "responses": {
                    "200": {
                        "description": "A streaming SSE response, or one JSON reply when `stream` is false.",
                        "content": {
                            "text/event-stream": { "schema": { "type": "string" } },
                            "application/json": { "schema": {
                                "type": "object",
                                "properties": {
                                    "thread_id": { "type": "string", "format": "uuid" },
                                    "text": { "type": "string" },
                                    "provider": { "type": "string" },
                                    "model": { "type": "string" },
                                    "finish_reason": { "type": "string" },
                                    "input_tokens": { "type": "integer" },
                                    "output_tokens": { "type": "integer" }
                                }
                            } }
                        }
                    }
                }
            }),
            agent,
        )
    })
}

fn auth_register_path() -> Value {
    json!({
        "post": {
            "tags": ["auth"],
            "operationId": "register",
            "summary": "Register a new user",
            "description": "Creates a user and returns a session token. Requires a `password`; other properties map to the user resource's fields.",
            "requestBody": json_body(json!({
                "type": "object",
                "properties": {
                    "email": { "type": "string", "format": "email" },
                    "password": { "type": "string", "format": "password" },
                },
                "required": ["password"],
                "additionalProperties": true,
            })),
            "responses": {
                "201": { "description": "Created", "content": { "application/json": { "schema": token_response() } } },
                "403": { "description": "Registration disabled" },
            }
        }
    })
}

fn auth_login_path(app: &App) -> Value {
    let identity = app
        .resources
        .get("user")
        .and_then(|r| r.auth.as_ref())
        .map(|a| a.identity_field.clone())
        .unwrap_or_else(|| "email".to_string());
    let identity_key = identity.clone();
    json!({
        "post": {
            "tags": ["auth"],
            "operationId": "login",
            "summary": "Log in",
            "description": "Exchanges credentials for a session token. Paste the returned token into **Authorize → bearerAuth**.",
            "requestBody": json_body(json!({
                "type": "object",
                "properties": {
                    identity_key: { "type": "string" },
                    "password": { "type": "string", "format": "password" },
                },
                "required": [identity, "password"],
            })),
            "responses": {
                "200": { "description": "Authenticated", "content": { "application/json": { "schema": token_response() } } },
                "401": { "description": "Invalid credentials" },
            }
        }
    })
}

fn auth_me_path() -> Value {
    json!({
        "get": {
            "tags": ["auth"],
            "operationId": "me",
            "summary": "Check the current credential",
            "description": "Verifies the caller's token or API key and that the account it names still exists. Returns 401 if either is no longer true.",
            "security": [{ "bearerAuth": [] }, { "apiKeyAuth": [] }],
            "responses": {
                "200": {
                    "description": "Credential is valid",
                    "content": { "application/json": { "schema": json!({
                        "type": "object",
                        "properties": { "user_id": { "type": "string", "format": "uuid" } },
                    }) } }
                },
                "401": { "description": "Invalid credential, or the user no longer exists" },
            }
        }
    })
}

fn auth_apikeys_path() -> Value {
    json!({
        "post": {
            "tags": ["auth"],
            "operationId": "createApiKey",
            "summary": "Issue an API key",
            "description": "Creates an API key for the authenticated caller. The plaintext key is returned once — use it via the `X-Api-Key` header (Authorize → apiKeyAuth).",
            "security": [{ "bearerAuth": [] }, { "apiKeyAuth": [] }],
            "requestBody": json_body(json!({
                "type": "object",
                "properties": { "name": { "type": "string" } },
            })),
            "responses": {
                "201": {
                    "description": "Key created",
                    "content": { "application/json": { "schema": json!({
                        "type": "object",
                        "properties": {
                            "api_key": { "type": "string" },
                            "id": { "type": "string", "format": "uuid" },
                        }
                    }) } }
                },
                "401": { "description": "Authentication required" },
            }
        }
    })
}

// --- signing in with somebody else's account ------------------------------
//
// Only described when `[oauth]` names a provider; see `build`.

/// The path parameter every per-provider operation takes, listing the
/// providers this deployment actually has — so the docs page offers a dropdown
/// of what works here rather than a free-text box.
fn provider_parameter(app: &App) -> Value {
    json!([{
        "name": "provider",
        "in": "path",
        "required": true,
        "schema": {
            "type": "string",
            "enum": app.config.oauth.active_providers(),
        },
    }])
}

fn oauth_providers_path(_app: &App) -> Value {
    json!({
        "get": {
            "tags": ["auth"],
            "operationId": "listOAuthProviders",
            "summary": "Which providers this deployment offers",
            "description": "Anonymous: it is what a sign-in page reads before anybody has signed in. Each entry carries the URL that starts that provider's flow, so adding a provider is a config change and not a front-end change.",
            "responses": {
                "200": {
                    "description": "The configured providers",
                    "content": { "application/json": { "schema": json!({
                        "type": "object",
                        "properties": { "providers": {
                            "type": "array",
                            "items": {
                                "type": "object",
                                "properties": {
                                    "provider": { "type": "string" },
                                    "label": { "type": "string" },
                                    "provides_email": { "type": "boolean", "description": "False for a provider that releases no email address — X. An account created through one has a placeholder address at `oauth.invalid`." },
                                    "start_url": { "type": "string" },
                                }
                            }
                        } }
                    }) } }
                }
            }
        }
    })
}

fn oauth_start_path(app: &App) -> Value {
    let mut redirect_parameters = provider_parameter(app);
    if let Some(list) = redirect_parameters.as_array_mut() {
        list.push(json!({
            "name": "return_to", "in": "query", "required": false,
            "schema": { "type": "string" },
            "description": "A path on this site to land on afterwards. Anything else falls back to `[oauth] success_redirect`.",
        }));
    }
    json!({
        "get": {
            "tags": ["auth"],
            "operationId": "startOAuthRedirect",
            "summary": "Begin a sign-in (redirects the browser)",
            "description": "Answers `302` to the provider's consent screen. This is the URL to put in a link: `<a href=\"…/auth/oauth/github/start\">Sign in with GitHub</a>` is the entire client-side integration. `?return_to=/path` chooses where the browser lands afterwards, and is accepted only as a path on this site.",
            "parameters": redirect_parameters,
            "responses": {
                "302": { "description": "Go and consent" },
                "400": { "description": "This provider cannot be used here" },
                "404": { "description": "No such provider is configured" },
            }
        },
        "post": {
            "tags": ["auth"],
            "operationId": "startOAuth",
            "summary": "Begin a sign-in (returns the URL)",
            "description": "The same handshake for a client that holds the browser itself: navigate to `authorize_url`. **Called with a session, this links** — the provider is attached to the caller's account instead of signing anybody in, and that decision is recorded server-side rather than taken from the callback.",
            "security": [{}, { "bearerAuth": [] }],
            "parameters": provider_parameter(app),
            "responses": {
                "200": {
                    "description": "Where to send the browser",
                    "content": { "application/json": { "schema": json!({
                        "type": "object",
                        "properties": {
                            "provider": { "type": "string" },
                            "authorize_url": { "type": "string" },
                            "state": { "type": "string" },
                            "expires_in": { "type": "integer" },
                            "linking": { "type": "boolean", "description": "True when a session started this flow, so finishing it links rather than signs in." },
                        }
                    }) } }
                },
                "400": { "description": "This provider cannot be used here" },
                "404": { "description": "No such provider is configured" },
            }
        }
    })
}

fn oauth_callback_path(app: &App) -> Value {
    let mut redirect_parameters = provider_parameter(app);
    if let Some(list) = redirect_parameters.as_array_mut() {
        list.extend(
            json!([
                { "name": "code", "in": "query", "required": false, "schema": { "type": "string" } },
                { "name": "state", "in": "query", "required": false, "schema": { "type": "string" } },
                { "name": "error", "in": "query", "required": false, "schema": { "type": "string" },
                  "description": "What the provider sends instead of a code when it refuses — `access_denied` when somebody pressed Cancel." },
            ])
            .as_array()
            .cloned()
            .unwrap_or_default(),
        );
    }
    json!({
        "get": {
            "tags": ["auth"],
            "operationId": "completeOAuthRedirect",
            "summary": "Finish a sign-in (the registered redirect URI)",
            "description": "This is the URL to register with the provider. It redeems the code, resolves the account, and answers `302` to the flow's landing page with the session token delivered as `[oauth] token_delivery` says — in the fragment by default, so it stays out of server logs. With `token_delivery = \"json\"` it answers with the body below instead.",
            "parameters": redirect_parameters,
            "responses": {
                "302": { "description": "Signed in; go to the landing page" },
                "400": { "description": "The provider refused, or that sign-in is no longer valid" },
                "403": { "description": "Registration is disabled and this is a new account" },
                "409": { "description": "That address or provider account belongs to somebody else" },
                "502": { "description": "The provider could not be reached" },
            }
        },
        "post": {
            "tags": ["auth"],
            "operationId": "completeOAuth",
            "summary": "Finish a sign-in (from a body)",
            "description": "For a front end with its own callback route: read `code` and `state` out of the query string the provider left you, and post them here. Always JSON.",
            "requestBody": json_body(json!({
                "type": "object",
                "properties": {
                    "code": { "type": "string" },
                    "state": { "type": "string" },
                    "error": { "type": "string" },
                    "error_description": { "type": "string" },
                },
            })),
            "parameters": provider_parameter(app),
            "responses": {
                "200": {
                    "description": "Signed in",
                    "content": { "application/json": { "schema": json!({
                        "type": "object",
                        "properties": {
                            "token": { "type": "string", "description": "A session token, identical in kind to the one POST /auth/login returns." },
                            "user": { "type": "object" },
                            "provider": { "type": "string" },
                            "created": { "type": "boolean", "description": "True when this sign-in created the account." },
                            "linked": { "type": "boolean", "description": "True when it attached a provider to an account that was already signed in." },
                        }
                    }) } }
                },
                "400": { "description": "The provider refused, or that sign-in is no longer valid" },
                "403": { "description": "Registration is disabled and this is a new account" },
                "409": { "description": "That address or provider account belongs to somebody else" },
                "502": { "description": "The provider could not be reached" },
            }
        }
    })
}

fn oauth_unlink_path(app: &App) -> Value {
    json!({
        "delete": {
            "tags": ["auth"],
            "operationId": "unlinkOAuthProvider",
            "summary": "Unlink a provider from your account",
            "description": "Refuses the last one: an account with no password and no other provider would become permanently unreachable. Set a password or link a second provider first.",
            "security": [{ "bearerAuth": [] }, { "apiKeyAuth": [] }],
            "parameters": provider_parameter(app),
            "responses": {
                "200": { "description": "Unlinked" },
                "401": { "description": "Authentication required" },
                "404": { "description": "No such provider is linked here" },
                "409": { "description": "It is the only way into this account" },
            }
        }
    })
}

// --- the mailbox flows ----------------------------------------------------
//
// Only described when the app can send email; see `build`.

/// The path parameter every invitation-by-token operation takes.
fn token_parameter() -> Value {
    json!([{
        "name": "token",
        "in": "path",
        "required": true,
        "schema": { "type": "string" },
        "description": "The token from the invitation email.",
    }])
}

fn auth_invitations_path() -> Value {
    json!({
        "post": {
            "tags": ["auth"],
            "operationId": "createInvitation",
            "summary": "Invite somebody into the active organization",
            "description": "Emails an invitation link to an address that need not have an account yet. Requires whatever `membership` requires to create a member — `role:admin` by default — in the organization named by `X-Organization`. Inviting the same address again replaces the pending invitation, and the earlier link stops working.",
            "security": [{ "bearerAuth": [] }, { "apiKeyAuth": [] }],
            "requestBody": json_body(json!({
                "type": "object",
                "properties": {
                    "email": { "type": "string", "format": "email" },
                    "role": { "type": "string", "default": "member", "description": "The role they hold once they accept." },
                },
                "required": ["email"],
            })),
            "responses": {
                "201": { "description": "Invitation sent" },
                "400": { "description": "No active organization, or no address given" },
                "403": { "description": "Not allowed to add people to this organization" },
                "409": { "description": "They are already in this organization" },
                "502": { "description": "The email could not be sent; no invitation was created" },
            }
        }
    })
}

fn auth_invitation_preview_path() -> Value {
    json!({
        "get": {
            "tags": ["auth"],
            "operationId": "previewInvitation",
            "summary": "What an invitation link is for",
            "description": "Anonymous: the token is the credential. Returns the organization's name, the invited address, and whether that address already has an account — which is what decides whether accepting needs a password.",
            "parameters": token_parameter(),
            "responses": {
                "200": {
                    "description": "The invitation",
                    "content": { "application/json": { "schema": json!({
                        "type": "object",
                        "properties": {
                            "email": { "type": "string" },
                            "organization": { "type": "string" },
                            "role": { "type": "string" },
                            "expires_at": { "type": "string", "format": "date-time" },
                            "has_account": { "type": "boolean" },
                        }
                    }) } }
                },
                "404": { "description": "Unknown, expired or already accepted" },
            }
        }
    })
}

fn auth_invitation_accept_path() -> Value {
    json!({
        "post": {
            "tags": ["auth"],
            "operationId": "acceptInvitation",
            "summary": "Accept an invitation",
            "description": "Joins the organization and returns a session token. When the invited address has no account yet, `password` is required and one is created — with the address already confirmed, since opening the link proved it. When it does have an account, send an empty body.",
            "requestBody": json_body(json!({
                "type": "object",
                "properties": {
                    "password": { "type": "string", "format": "password", "description": "Required only when creating an account." },
                },
                "additionalProperties": true,
            })),
            "parameters": token_parameter(),
            "responses": {
                "200": { "description": "Joined", "content": { "application/json": { "schema": token_response() } } },
                "400": { "description": "A password is needed to create the account" },
                "404": { "description": "Unknown, expired or already accepted" },
            }
        }
    })
}

fn auth_verify_email_path() -> Value {
    json!({
        "post": {
            "tags": ["auth"],
            "operationId": "verifyEmail",
            "summary": "Confirm an email address",
            "description": "Spends the single-use token from a confirmation email and returns a session token — somebody who has just proved they read the account's mailbox should not then have to sign in.",
            "requestBody": json_body(json!({
                "type": "object",
                "properties": { "token": { "type": "string" } },
                "required": ["token"],
            })),
            "responses": {
                "200": { "description": "Confirmed", "content": { "application/json": { "schema": token_response() } } },
                "404": { "description": "Unknown, expired or already used" },
            }
        }
    })
}

fn auth_resend_verification_path() -> Value {
    json!({
        "post": {
            "tags": ["auth"],
            "operationId": "resendVerification",
            "summary": "Send the confirmation email again",
            "description": "Always answers 202, whether or not the address has an account and whether or not it is already confirmed. Answering truthfully would turn this into a way of testing which addresses are registered.",
            "requestBody": json_body(json!({
                "type": "object",
                "properties": { "email": { "type": "string", "format": "email" } },
                "required": ["email"],
            })),
            "responses": { "202": { "description": "Accepted" } }
        }
    })
}

fn auth_forgot_password_path() -> Value {
    json!({
        "post": {
            "tags": ["auth"],
            "operationId": "forgotPassword",
            "summary": "Email a password reset link",
            "description": "Always answers 202, for the same reason as `/auth/verify-email/resend`. The existing password keeps working until the link is actually used.",
            "requestBody": json_body(json!({
                "type": "object",
                "properties": { "email": { "type": "string", "format": "email" } },
                "required": ["email"],
            })),
            "responses": { "202": { "description": "Accepted" } }
        }
    })
}

fn auth_reset_password_path() -> Value {
    json!({
        "post": {
            "tags": ["auth"],
            "operationId": "resetPassword",
            "summary": "Set a new password from a reset link",
            "description": "Spends the token, sets the password, and invalidates every other outstanding reset for that account. Returns a session token.",
            "requestBody": json_body(json!({
                "type": "object",
                "properties": {
                    "token": { "type": "string" },
                    "password": { "type": "string", "format": "password" },
                },
                "required": ["token", "password"],
            })),
            "responses": {
                "200": { "description": "Password changed", "content": { "application/json": { "schema": token_response() } } },
                "404": { "description": "Unknown, expired or already used" },
            }
        }
    })
}

// --- billing paths --------------------------------------------------------

fn billing_config_path() -> Value {
    json!({
        "get": {
            "tags": ["billing"],
            "operationId": "billingConfig",
            "summary": "What this app's checkout will do",
            "description": "Public. The publishable key is designed to be in a browser, and the rest describes the shape of the checkout — currency, whether tax is added, whether a VAT number is collected — which a pricing page needs before anyone has an account.",
            "responses": {
                "200": {
                    "description": "Payment configuration",
                    "content": { "application/json": { "schema": json!({
                        "type": "object",
                        "properties": {
                            "provider": { "type": "string", "example": "stripe" },
                            "publishable_key": { "type": "string" },
                            "currency": { "type": "string", "example": "eur" },
                            "automatic_tax": { "type": "boolean" },
                            "tax_id_collection": { "type": "boolean" },
                            "webhooks_configured": { "type": "boolean" },
                        },
                    }) } }
                }
            }
        }
    })
}

fn billing_checkout_path() -> Value {
    json!({
        "post": {
            "tags": ["billing"],
            "operationId": "startCheckout",
            "summary": "Start a purchase",
            "description": "Answers with the provider's hosted payment page; redirect the buyer there. Requires the `admin` role in the organization named by `X-Organization` — a member who can start a subscription can commit their employer to a recurring charge. Whether it starts a subscription or takes a single payment is decided by the price's `interval`.",
            "requestBody": json_body(json!({
                "type": "object",
                "properties": {
                    "price_id": { "type": "string", "format": "uuid", "description": "The billing_price to buy" },
                    "quantity": { "type": "integer", "minimum": 1, "default": 1 },
                    "success_url": { "type": "string", "description": "Overrides [payments] success_url" },
                    "cancel_url": { "type": "string" },
                    "allow_promotion_codes": { "type": "boolean", "default": false },
                },
                "required": ["price_id"],
            })),
            "responses": {
                "200": {
                    "description": "Checkout started",
                    "content": { "application/json": { "schema": json!({
                        "type": "object",
                        "properties": {
                            "url": { "type": "string", "description": "Send the buyer here" },
                            "session_id": { "type": "string" },
                            "mode": { "type": "string", "enum": ["subscription", "payment"] },
                        },
                    }) } }
                },
                "400": { "description": "No organization named in X-Organization" },
                "401": { "description": "Authentication required" },
                "403": { "description": "Not an admin of this organization" },
                "404": { "description": "No such price" },
                "409": { "description": "The price is archived, or has never been synced to the provider" },
                "502": { "description": "The provider could not be reached" },
            }
        }
    })
}

fn billing_portal_path() -> Value {
    json!({
        "post": {
            "tags": ["billing"],
            "operationId": "billingPortal",
            "summary": "A link to the provider's self-service billing",
            "description": "Where a customer changes their card, downloads invoices, updates their tax number or cancels — none of which this app implements, and all of which comes back as webhooks. Requires the `admin` role in `X-Organization`.",
            "requestBody": json_body(json!({
                "type": "object",
                "properties": { "return_url": { "type": "string" } },
            })),
            "responses": {
                "200": { "description": "Portal session", "content": { "application/json": { "schema": json!({
                    "type": "object",
                    "properties": { "url": { "type": "string" } },
                }) } } },
                "403": { "description": "Not an admin of this organization" },
                "404": { "description": "This organization has never bought anything" },
            }
        }
    })
}

fn billing_webhook_path() -> Value {
    json!({
        "post": {
            "tags": ["billing"],
            "operationId": "billingWebhook",
            "summary": "Deliveries from the payment provider",
            "description": "Not for you to call. The body is verified against `[payments] webhook_secret` using a signature over its raw bytes, and this is the only path that writes `billing_subscription` and `billing_payment` — what has been paid for is the provider's fact, not the caller's. Answers 200 for anything recorded, including events it does nothing with, so the provider stops retrying them.",
            "requestBody": {
                "required": true,
                "content": { "application/json": { "schema": { "type": "object" } } }
            },
            "responses": {
                "200": { "description": "Recorded" },
                "400": { "description": "Signature verification failed" },
                "500": { "description": "Could not record or apply the event — the provider should retry" },
            }
        }
    })
}

// --- function paths -------------------------------------------------------

#[allow(clippy::too_many_arguments)]
fn function_path(
    method: HttpMethod,
    access: &FunctionAccess,
    name: &str,
    description: &str,
    input_ref: Option<String>,
    output_ref: Option<String>,
) -> Value {
    let verb = match method {
        HttpMethod::Get => "get",
        HttpMethod::Post => "post",
        HttpMethod::Put => "put",
        HttpMethod::Delete => "delete",
    };
    let note = match access {
        FunctionAccess::Public => "Public — no authentication required.".to_string(),
        FunctionAccess::Authenticated => "Requires authentication.".to_string(),
        FunctionAccess::Member => "Requires membership of the active organization.".to_string(),
        FunctionAccess::Role(role) => {
            format!("Requires the `{role}` role in the active organization.")
        }
        FunctionAccess::Private => "Not exposed.".to_string(),
    };
    let untyped = || json!({ "type": "object" });
    let response_schema = output_ref.map(|r| ref_to(&r)).unwrap_or_else(untyped);

    let mut op = json!({
        "tags": ["functions"],
        "operationId": format!("fn_{name}"),
        "summary": if description.is_empty() { format!("Invoke {name}") } else { description.to_string() },
        "description": note,
        "responses": {
            "200": { "description": "Function result", "content": { "application/json": { "schema": response_schema } } },
            "400": { "description": "Invalid input" },
        }
    });
    if matches!(method, HttpMethod::Post | HttpMethod::Put) {
        let request_schema = input_ref.map(|r| ref_to(&r)).unwrap_or_else(untyped);
        op["requestBody"] = json_body(request_schema);
    }
    if !access.is_public() {
        op["security"] = json!([{ "bearerAuth": [] }, { "apiKeyAuth": [] }]);
    }

    json!({ verb: op })
}

/// Ingest a function's JSON Schema (as produced by schemars) into the shared
/// `components.schemas` map and return the component name to `$ref`.
///
/// schemars emits a root object plus a `$defs`/`definitions` block with
/// `#/$defs/…` refs; we relocate those under `components.schemas`, namespaced by
/// function so two functions can each have an `Input`, and rewrite the refs
/// accordingly. Returns `None` for an empty/unparseable schema (⇒ untyped body).
fn ingest_fn_schema(
    schemas: &mut Map<String, Value>,
    func: &str,
    kind: &str,
    raw: &str,
) -> Option<String> {
    if raw.trim().is_empty() {
        return None;
    }
    let mut root: Value = serde_json::from_str(raw).ok()?;
    let component = format!("Fn{}{}", pascal(func), kind);
    let prefix = format!("Fn{}_", pascal(func));

    if let Some(obj) = root.as_object_mut() {
        for defs_key in ["$defs", "definitions"] {
            if let Some(Value::Object(defs)) = obj.remove(defs_key) {
                for (def_name, mut def) in defs {
                    rewrite_refs(&mut def, &prefix);
                    schemas.insert(format!("{prefix}{def_name}"), def);
                }
            }
        }
        obj.remove("$schema");
        obj.remove("title");
    }
    rewrite_refs(&mut root, &prefix);
    schemas.insert(component.clone(), root);
    Some(component)
}

/// Rewrite `#/$defs/X` and `#/definitions/X` refs to
/// `#/components/schemas/<prefix>X`, recursively.
fn rewrite_refs(value: &mut Value, prefix: &str) {
    match value {
        Value::Object(map) => {
            if let Some(Value::String(r)) = map.get_mut("$ref") {
                for p in ["#/$defs/", "#/definitions/"] {
                    if let Some(rest) = r.strip_prefix(p) {
                        *r = format!("#/components/schemas/{prefix}{rest}");
                        break;
                    }
                }
            }
            for v in map.values_mut() {
                rewrite_refs(v, prefix);
            }
        }
        Value::Array(arr) => {
            for v in arr {
                rewrite_refs(v, prefix);
            }
        }
        _ => {}
    }
}

// --- Swagger UI page ------------------------------------------------------

/// A self-contained Swagger UI page pointing at `spec_url`. `persistAuthorization`
/// keeps the entered token across reloads so the Authorize flow sticks.
pub fn swagger_ui_html(spec_url: &str, title: &str) -> String {
    format!(
        r#"<!DOCTYPE html>
<html lang="en">
<head>
  <meta charset="utf-8"/>
  <meta name="viewport" content="width=device-width, initial-scale=1"/>
  <title>{title}</title>
  <link rel="stylesheet" href="https://cdn.jsdelivr.net/npm/swagger-ui-dist@5/swagger-ui.css">
  <style>body {{ margin: 0; }}</style>
</head>
<body>
  <div id="swagger-ui"></div>
  <script src="https://cdn.jsdelivr.net/npm/swagger-ui-dist@5/swagger-ui-bundle.js" crossorigin></script>
  <script>
    window.ui = SwaggerUIBundle({{
      url: {spec_url},
      dom_id: '#swagger-ui',
      deepLinking: true,
      persistAuthorization: true,
      presets: [SwaggerUIBundle.presets.apis, SwaggerUIBundle.SwaggerUIStandalonePreset],
    }});
  </script>
</body>
</html>"#,
        title = html_escape(title),
        spec_url = serde_json::to_string(spec_url).unwrap_or_else(|_| "\"openapi.json\"".into()),
    )
}

fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// Convert a snake_case resource name to a PascalCase schema name.
fn pascal(s: &str) -> String {
    s.split('_')
        .filter(|p| !p.is_empty())
        .map(|p| {
            let mut c = p.chars();
            match c.next() {
                Some(first) => first.to_uppercase().collect::<String>() + c.as_str(),
                None => String::new(),
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_app_dir(label: &str) -> std::path::PathBuf {
        let mut dir = std::env::temp_dir();
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        dir.push(format!(
            "apiplant-openapi-{label}-{}-{stamp}",
            std::process::id()
        ));
        fs::create_dir_all(dir.join("models")).unwrap();
        dir
    }

    #[test]
    fn input_schema_excludes_hidden_owner_and_organization_fields() {
        let resource: Resource = toml::from_str(
            r#"
[resource]
name = "post"

[fields.title]
type = "string"
required = true

[fields.owner_id]
type = "reference"
references = "user"
required = true

[fields.organization_id]
type = "reference"
references = "organization"
required = true

[fields.secret]
type = "string"
hidden = true
"#,
        )
        .unwrap();

        let schema = resource_input_schema(&resource);
        let props = schema.get("properties").unwrap().as_object().unwrap();
        assert!(props.contains_key("title"));
        assert!(!props.contains_key("owner_id"));
        assert!(!props.contains_key("organization_id"));
        assert!(!props.contains_key("secret"));
        assert_eq!(schema["required"], json!(["title"]));
    }

    #[test]
    fn build_emits_nested_paths_auth_routes_and_security() {
        let dir = temp_app_dir("build");
        fs::write(
            dir.join("main.toml"),
            r#"
[server]
base_path = "/api"

[docs]
title = "Test API"
"#,
        )
        .unwrap();
        fs::write(
            dir.join("models/post.toml"),
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

[fields.owner_id]
type = "reference"
references = "user"
required = true
"#,
        )
        .unwrap();
        fs::write(
            dir.join("models/comment.toml"),
            r#"
[resource]
name = "comment"

[fields.body]
type = "text"
required = true

[fields.post_id]
type = "reference"
references = "post"
required = true
"#,
        )
        .unwrap();
        fs::write(
            dir.join("models/plan.toml"),
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
        )
        .unwrap();

        let app = App::load(&dir).unwrap();
        let spec = build(&app, &FunctionRegistry::default(), false);

        assert_eq!(spec["info"]["title"], "Test API");
        assert_eq!(spec["servers"][0]["url"], "/api");
        assert!(spec["paths"]["/post"].get("get").is_some());
        assert!(spec["paths"]["/post/{id}/comment"].get("get").is_some());
        assert!(spec["paths"]["/auth/register"].get("post").is_some());
        assert!(spec["components"]["securitySchemes"]["bearerAuth"].is_object());

        assert!(spec["paths"]["/post"]["get"]["security"].is_array());
        assert!(spec["paths"]["/plan"]["get"].get("security").is_none());
        assert_eq!(
            spec["paths"]["/post/{id}"]["delete"]["description"],
            "Requires the `admin` role in the active organisation."
        );

        // Nothing that needs a mailbox is documented for an app with no mailer.
        for path in [
            "/auth/invitations",
            "/auth/verify-email",
            "/auth/password/forgot",
        ] {
            assert!(
                spec["paths"].get(path).is_none(),
                "{path} is documented but would not be mounted"
            );
        }

        fs::remove_dir_all(dir).unwrap();
    }

    /// Configuring a provider is all it takes: the three flows turn themselves
    /// on, and the docs describe exactly what the server will answer.
    #[test]
    fn the_mailbox_flows_are_documented_once_the_app_can_send_email() {
        let dir =
            std::env::temp_dir().join(format!("apiplant-openapi-email-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            dir.join("main.toml"),
            r#"
[email]
provider = "smtp"
from = "no-reply@example.com"
"#,
        )
        .unwrap();

        let app = App::load(&dir).unwrap();
        let spec = build(&app, &FunctionRegistry::default(), true);

        for path in [
            "/auth/invitations",
            "/auth/invitations/{token}",
            "/auth/invitations/{token}/accept",
            "/auth/verify-email",
            "/auth/verify-email/resend",
            "/auth/password/forgot",
            "/auth/password/reset",
        ] {
            assert!(spec["paths"].get(path).is_some(), "{path} is missing");
        }

        // …and an app that wants one of them off gets it off, mailer or not.
        let mut app = App::load(&dir).unwrap();
        app.config.auth.allow_password_reset = Some(false);
        let spec = build(&app, &FunctionRegistry::default(), true);
        assert!(spec["paths"].get("/auth/password/forgot").is_none());
        assert!(spec["paths"].get("/auth/invitations").is_some());

        fs::remove_dir_all(dir).unwrap();
    }
}
