//! Built-in authentication endpoints, mounted under `<base>/auth`.
//!
//! These operate on the `user` and `api_key` resources — which are just ordinary
//! resources — but understand the `[auth]` section on the user model
//! (configurable identity/password field names).

use apiplant_core::schema::AuthSpec;
use apiplant_core::HookEvent;
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

    let user = match hooks::run(
        &state,
        user_r,
        HookEvent::AfterCreate,
        &hook_req.with_record(user_id),
        created.clone(),
    )
    .await
    {
        Ok(Some(replacement)) => replacement,
        Ok(None) => created,
        Err(resp) => return resp,
    };

    match state.auth.issue_token(user_id) {
        Ok(token) => HttpResponse::Created().json(&json!({ "token": token, "user": user })),
        Err(_) => error(500, "failed to issue token"),
    }
}

/// `POST <base>/auth/login` — verify credentials, return a session token.
pub async fn login(
    state: State<AppState>,
    body: Json<serde_json::Map<String, Value>>,
) -> HttpResponse {
    let spec = auth_spec(&state);
    let data = body.into_inner();

    let identity = match data.get(&spec.identity_field).and_then(|v| v.as_str()) {
        Some(s) => s.to_string(),
        None => return error(400, format!("`{}` is required", spec.identity_field)),
    };
    let password = match data.get("password").and_then(|v| v.as_str()) {
        Some(s) => s.to_string(),
        None => return error(400, "`password` is required"),
    };

    let Some(user_tbl) = table(&state, "user") else {
        return error(500, "missing user resource");
    };
    let sql = format!(
        "SELECT u.id::text AS id, u.{pw}::text AS password_hash \
         FROM {user_tbl} u WHERE u.{ident} = $1 LIMIT 1",
        pw = quote(&spec.password_field),
        ident = quote(&spec.identity_field),
    );
    let rows = match state.db.raw_json(&sql, &[Value::String(identity)]).await {
        Ok(v) => v,
        Err(e) => return db_error(e),
    };
    let Some(row) = rows.as_array().and_then(|a| a.first()) else {
        // Spend the argon2 time anyway: answering an unknown identity faster
        // than a wrong password is enough to enumerate accounts.
        let _ = state.auth.hash_password(&password);
        return error(401, "invalid credentials");
    };
    let stored = row
        .get("password_hash")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    if !state.auth.verify_password(&password, stored) {
        return error(401, "invalid credentials");
    }
    let user_id = row
        .get("id")
        .and_then(|v| v.as_str())
        .and_then(|s| Uuid::parse_str(s).ok());
    let Some(user_id) = user_id else {
        return error(500, "user missing id");
    };
    match state.auth.issue_token(user_id) {
        Ok(token) => HttpResponse::Ok().json(&json!({ "token": token })),
        Err(_) => error(500, "failed to issue token"),
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

    match state.db.create(api_key_r, &data).await {
        Ok(row) => HttpResponse::Created().json(&json!({
            "api_key": plaintext,
            "id": row.get("id").cloned().unwrap_or(Value::Null),
            "note": "store this key now; it will not be shown again",
        })),
        Err(e) => db_error(e),
    }
}

fn table(state: &AppState, name: &str) -> Option<String> {
    state
        .app
        .resources
        .get(name)
        .map(|r| format!("\"{}\"", r.table_name()))
}
