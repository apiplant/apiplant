//! Built-in authentication endpoints, mounted under `<base>/auth`.
//!
//! These operate on the `user` and `api_key` resources — which are just ordinary
//! resources — but understand the `[auth]` section on the user model
//! (configurable identity/password field names).
//!
//! Each endpoint is extensible through the `[auth.hooks]` section of that same
//! model, which binds a function to a point in the endpoint's lifecycle:
//!
//! ```toml
//! [auth.hooks]
//! before_login = "check_lockout"
//! after_login  = "record_session"
//! login_failed = "count_failure"
//! ```
//!
//! They follow the same protocol as [resource hooks](crate::hooks) — a returned
//! `{"error": …}` aborts, a returned `{"data": …}` replaces — over these events:
//!
//! | Event | Receives | A `data` replacement |
//! |-------|----------|----------------------|
//! | `before_register` | the submitted body, password already hashed | replaces what is inserted |
//! | `after_register` | the created row | replaces the response's `user` |
//! | `before_login` | the identity being claimed, never the password | replaces the credentials looked up |
//! | `after_login` | `{"user_id": …}` for the verified account | is merged into the response beside `token` |
//! | `login_failed` | `{"identity": …, "reason": …}` | ignored; only an `error` has an effect |
//! | `before_api_key` | the submitted body | replaces the key's stored fields |
//! | `after_api_key` | the created row | is merged into the response beside `api_key` |
//!
//! Registration additionally fires the `user` resource's own `before_create` /
//! `after_create`, since it *is* a create; the register hooks run outside those
//! and are the place for logic that should not fire on `POST <base>/user`.

use apiplant_core::schema::AuthSpec;
use apiplant_core::{AuthEvent, HookEvent};
use ntex::web::types::{Json, State};
use ntex::web::{HttpRequest, HttpResponse};
use serde_json::{json, Value};
use uuid::Uuid;

use crate::crud::parse_query;
use crate::hooks::{self, HookRequest};
use crate::response::{db_error, error};
use crate::state::AppState;

fn auth_spec(state: &AppState) -> AuthSpec {
    state
        .app
        .resources
        .get("user")
        .and_then(|r| r.auth.clone())
        .unwrap_or_default()
}

fn quote(ident: &str) -> String {
    // auth field names come from the developer's own model; still refuse
    // anything that isn't a plain identifier.
    if ident.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') && !ident.is_empty() {
        format!("\"{ident}\"")
    } else {
        "\"__invalid__\"".to_string()
    }
}

/// `POST <base>/auth/register` — create a user and return a session token.
///
/// Registration is a `create` on the `user` resource, so the `user` model's
/// `before_create` / `after_create` [hooks](crate::hooks) fire here exactly as
/// they do on `POST <base>/user` — the same function serves both doors into the
/// same table. Two differences follow from what registration is:
///
/// * the plaintext `password` is swapped for the hashed `password_field`
///   *before* `before_create` runs, so a hook never sees the secret;
/// * the caller is anonymous, so `after_create` identifies the new account
///   through the hook context's `record_id` (and the row it receives) rather
///   than through `principal_id`. A replacement it returns replaces the `user`
///   object in the response, leaving the issued `token` alone.
pub async fn register(
    req: HttpRequest,
    state: State<AppState>,
    body: Json<serde_json::Map<String, Value>>,
) -> HttpResponse {
    if !state.app.config.auth.allow_registration {
        return error(403, "registration is disabled");
    }
    let user_r = match state.app.resources.get("user") {
        Some(r) => r,
        None => return error(500, "no user resource"),
    };
    let spec = auth_spec(&state);

    let mut data = body.into_inner();
    let password = match data
        .remove("password")
        .and_then(|v| v.as_str().map(String::from))
    {
        Some(p) => p,
        None => return error(400, "`password` is required"),
    };
    let hash = match state.auth.hash_password(&password) {
        Ok(h) => h,
        Err(_) => return error(500, "failed to hash password"),
    };
    data.insert(spec.password_field.clone(), Value::String(hash));

    // Nobody is authenticated yet: whoever is registering has no principal and
    // no organisation, so the hook context carries the request alone.
    let hook_req = HookRequest::new(&req, &parse_query(req.query_string()), None, None);

    // `before_register` runs outside `before_create`, so a model can reject a
    // signup without also rejecting an administrative `POST <base>/user`.
    match hooks::run_auth(
        &state,
        user_r,
        AuthEvent::BeforeRegister,
        &hook_req,
        Value::Object(data.clone()),
    )
    .await
    {
        Ok(Some(replacement)) => {
            let hook = user_r
                .auth_hook(AuthEvent::BeforeRegister)
                .unwrap_or_default();
            match hooks::replacement_object(replacement, hook) {
                Ok(map) => data = map,
                Err(resp) => return resp,
            }
        }
        Ok(None) => {}
        Err(resp) => return resp,
    }

    match hooks::run(
        &state,
        user_r,
        HookEvent::BeforeCreate,
        &hook_req,
        Value::Object(data.clone()),
    )
    .await
    {
        Ok(Some(replacement)) => {
            let hook = user_r.hook(HookEvent::BeforeCreate).unwrap_or_default();
            match hooks::replacement_object(replacement, hook) {
                Ok(map) => data = map,
                Err(resp) => return resp,
            }
        }
        Ok(None) => {}
        Err(resp) => return resp,
    }

    let created = match state.db.create(user_r, &data).await {
        Ok(row) => row,
        Err(e) => return db_error(e),
    };
    let user_id = created
        .get("id")
        .and_then(|v| v.as_str())
        .and_then(|s| Uuid::parse_str(s).ok());
    let Some(user_id) = user_id else {
        return error(500, "created user missing id");
    };

    let hook_req = hook_req.with_record(user_id);
    let user = match hooks::run(
        &state,
        user_r,
        HookEvent::AfterCreate,
        &hook_req,
        created.clone(),
    )
    .await
    {
        Ok(Some(replacement)) => replacement,
        Ok(None) => created,
        Err(resp) => return resp,
    };

    // The account already exists by now, so an `after_register` rejection fails
    // the *response*, not the write — the same bargain `after_create` makes.
    let user = match hooks::run_auth(
        &state,
        user_r,
        AuthEvent::AfterRegister,
        &hook_req,
        user.clone(),
    )
    .await
    {
        Ok(Some(replacement)) => replacement,
        Ok(None) => user,
        Err(resp) => return resp,
    };

    match state.auth.issue_token(user_id) {
        Ok(token) => HttpResponse::Created().json(&json!({ "token": token, "user": user })),
        Err(_) => error(500, "failed to issue token"),
    }
}

/// `POST <base>/auth/login` — verify credentials, return a session token.
///
/// The `[auth.hooks]` events fire around the credential check: `before_login`
/// sees the claimed identity (never the password) and can reject the attempt or
/// rewrite the identity that is looked up; `login_failed` sees every rejection;
/// `after_login` sees the verified account and can widen the response.
pub async fn login(
    req: HttpRequest,
    state: State<AppState>,
    body: Json<serde_json::Map<String, Value>>,
) -> HttpResponse {
    let spec = auth_spec(&state);
    let data = body.into_inner();

    let mut identity = match data.get(&spec.identity_field).and_then(|v| v.as_str()) {
        Some(s) => s.to_string(),
        None => return error(400, format!("`{}` is required", spec.identity_field)),
    };
    let password = match data.get("password").and_then(|v| v.as_str()) {
        Some(s) => s.to_string(),
        None => return error(400, "`password` is required"),
    };

    let Some(user_r) = state.app.resources.get("user") else {
        return error(500, "missing user resource");
    };
    // Nobody is authenticated during a login attempt, by definition.
    let hook_req = HookRequest::new(&req, &parse_query(req.query_string()), None, None);

    // The password never reaches a hook, here or anywhere else: a hook that
    // wants to reject an attempt does it on the identity alone.
    match hooks::run_auth(
        &state,
        user_r,
        AuthEvent::BeforeLogin,
        &hook_req,
        json!({ spec.identity_field.clone(): identity }),
    )
    .await
    {
        Ok(Some(replacement)) => {
            match replacement
                .get(&spec.identity_field)
                .and_then(|v| v.as_str())
            {
                Some(rewritten) => identity = rewritten.to_string(),
                None => {
                    return error(
                        500,
                        format!(
                            "`before_login` replaced the credentials without `{}`",
                            spec.identity_field
                        ),
                    )
                }
            }
        }
        Ok(None) => {}
        Err(resp) => return resp,
    }

    let Some(user_tbl) = table(&state, "user") else {
        return error(500, "missing user resource");
    };
    let sql = format!(
        "SELECT u.id::text AS id, u.{pw}::text AS password_hash \
         FROM {user_tbl} u WHERE u.{ident} = $1 LIMIT 1",
        pw = quote(&spec.password_field),
        ident = quote(&spec.identity_field),
    );
    let rows = match state
        .db
        .raw_json(&sql, &[Value::String(identity.clone())])
        .await
    {
        Ok(v) => v,
        Err(e) => return db_error(e),
    };
    let Some(row) = rows.as_array().and_then(|a| a.first()) else {
        // Spend the argon2 time anyway: answering an unknown identity faster
        // than a wrong password is enough to enumerate accounts.
        let _ = state.auth.hash_password(&password);
        return rejected(&state, user_r, &hook_req, &identity, "unknown_identity").await;
    };
    let stored = row
        .get("password_hash")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    if !state.auth.verify_password(&password, stored) {
        return rejected(&state, user_r, &hook_req, &identity, "bad_password").await;
    }
    let user_id = row
        .get("id")
        .and_then(|v| v.as_str())
        .and_then(|s| Uuid::parse_str(s).ok());
    let Some(user_id) = user_id else {
        return error(500, "user missing id");
    };
    let token = match state.auth.issue_token(user_id) {
        Ok(token) => token,
        Err(_) => return error(500, "failed to issue token"),
    };

    // `after_login` can widen the response — attaching the user row, their
    // roles, whatever the client needs — but never rewrite the token it is
    // being handed alongside.
    let mut response = json!({ "token": token });
    match hooks::run_auth(
        &state,
        user_r,
        AuthEvent::AfterLogin,
        &hook_req.with_record(user_id),
        json!({ "user_id": user_id.to_string() }),
    )
    .await
    {
        Ok(Some(replacement)) => merge_beside(&mut response, replacement, "token"),
        Ok(None) => {}
        Err(resp) => return resp,
    }
    HttpResponse::Ok().json(&response)
}

/// Report a rejected login, giving `login_failed` its say first.
///
/// The hook's return value is ignored — there is nothing to replace — except an
/// `{"error": …}`, which lets a lockout hook answer `429` where the endpoint
/// would have answered `401`. The caller is told no more than "invalid
/// credentials" either way.
async fn rejected(
    state: &AppState,
    user_r: &apiplant_core::Resource,
    hook_req: &HookRequest,
    identity: &str,
    reason: &str,
) -> HttpResponse {
    match hooks::run_auth(
        state,
        user_r,
        AuthEvent::LoginFailed,
        hook_req,
        json!({ "identity": identity, "reason": reason }),
    )
    .await
    {
        Ok(_) => error(401, "invalid credentials"),
        Err(resp) => resp,
    }
}

/// Fold a hook's replacement object into `response`, leaving `reserved` — the
/// secret the endpoint issued — as the endpoint set it.
fn merge_beside(response: &mut Value, replacement: Value, reserved: &str) {
    let (Some(target), Value::Object(fields)) = (response.as_object_mut(), replacement) else {
        return;
    };
    for (key, value) in fields {
        if key != reserved {
            target.insert(key, value);
        }
    }
}

/// `GET <base>/auth/me` — answer whether this credential still means anything.
///
/// A token can verify against the secret and still be worthless: the account it
/// names may have been deleted since it was issued. Both halves are checked
/// here — the signature by [`AppState::resolve_principal`], the account by
/// looking the row up — so a client holding a token has one call to ask whether
/// to keep it. Anything short of a live user is a flat 401.
pub async fn me(req: HttpRequest, state: State<AppState>) -> HttpResponse {
    let Some(principal) = state.resolve_principal(&req).await else {
        return error(401, "authentication required");
    };
    let Some(user_tbl) = table(&state, "user") else {
        return error(500, "missing user resource");
    };
    let sql = format!("SELECT id::text AS id FROM {user_tbl} WHERE id = $1::uuid LIMIT 1");
    let rows = match state
        .db
        .raw_json(&sql, &[Value::String(principal.user_id.to_string())])
        .await
    {
        Ok(v) => v,
        Err(e) => return db_error(e),
    };
    if rows.as_array().is_none_or(|a| a.is_empty()) {
        return error(401, "user no longer exists");
    }
    HttpResponse::Ok().json(&json!({ "user_id": principal.user_id.to_string() }))
}

/// `POST <base>/auth/apikeys` — issue an API key for the authenticated caller.
/// The plaintext key is returned exactly once.
pub async fn create_api_key(
    req: HttpRequest,
    state: State<AppState>,
    body: Json<Value>,
) -> HttpResponse {
    let principal = match state.resolve_principal(&req).await {
        Some(p) => p,
        None => return error(401, "authentication required"),
    };
    let api_key_r = match state.app.resources.get("api_key") {
        Some(r) => r,
        None => return error(500, "no api_key resource"),
    };
    let (plaintext, hash) = state.auth.generate_api_key();

    let mut data = serde_json::Map::new();
    if let Some(name) = body.get("name").and_then(|v| v.as_str()) {
        data.insert("name".into(), Value::String(name.to_string()));
    }
    data.insert("token_hash".into(), Value::String(hash));
    data.insert(
        "owner_id".into(),
        Value::String(principal.user_id.to_string()),
    );

    let active_org = state.active_org(&req, &Some(principal.clone()));
    let hook_req = HookRequest::new(
        &req,
        &parse_query(req.query_string()),
        Some(&principal),
        active_org,
    );

    // A `before_api_key` replacement writes the row, so a model can stamp an
    // expiry or a scope onto every key it issues.
    match hooks::run_auth(
        &state,
        api_key_r,
        AuthEvent::BeforeApiKey,
        &hook_req,
        Value::Object(data.clone()),
    )
    .await
    {
        Ok(Some(replacement)) => {
            let hook = state
                .app
                .resources
                .get("user")
                .and_then(|u| u.auth_hook(AuthEvent::BeforeApiKey))
                .unwrap_or_default();
            match hooks::replacement_object(replacement, hook) {
                Ok(map) => data = map,
                Err(resp) => return resp,
            }
        }
        Ok(None) => {}
        Err(resp) => return resp,
    }

    let row = match state.db.create(api_key_r, &data).await {
        Ok(row) => row,
        Err(e) => return db_error(e),
    };

    let mut response = json!({
        "api_key": plaintext,
        "id": row.get("id").cloned().unwrap_or(Value::Null),
        "note": "store this key now; it will not be shown again",
    });
    let hook_req = match row.get("id").and_then(|v| v.as_str()) {
        Some(id) => match Uuid::parse_str(id) {
            Ok(id) => hook_req.with_record(id),
            Err(_) => hook_req,
        },
        None => hook_req,
    };
    match hooks::run_auth(
        &state,
        api_key_r,
        AuthEvent::AfterApiKey,
        &hook_req,
        row.clone(),
    )
    .await
    {
        Ok(Some(replacement)) => merge_beside(&mut response, replacement, "api_key"),
        Ok(None) => {}
        Err(resp) => return resp,
    }
    HttpResponse::Created().json(&response)
}

fn table(state: &AppState, name: &str) -> Option<String> {
    state
        .app
        .resources
        .get(name)
        .map(|r| format!("\"{}\"", r.table_name()))
}
