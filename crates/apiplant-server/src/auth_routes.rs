//! Built-in authentication endpoints, mounted under `<base>/auth`.
//!
//! These operate on the `user` and `api_key` resources — which are just ordinary
//! resources — but understand the `[auth]` section on the user resource
//! (configurable identity/password field names).
//!
//! Each endpoint is extensible through the `user` resource's ordinary `[hooks]`
//! section, which carries these events alongside the CRUD ones:
//!
//! ```toml
//! [hooks]
//! after_create = "index_user"    # the table's own lifecycle
//! before_login = "check_lockout" # and the endpoints in front of it
//! after_login  = "record_attempt"
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
//! | `after_login` | the attempt's outcome — see [`login`] | is merged into the response beside `token` |
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

/// The `user` column recording when an address was confirmed, as
/// [`USER_TOML`](apiplant_core::defaults::USER_TOML) declares it.
pub(crate) const VERIFIED_AT_FIELD: &str = "email_verified_at";

pub(crate) fn auth_spec(state: &AppState) -> AuthSpec {
    state
        .app
        .resources
        .get("user")
        .and_then(|r| r.auth.clone())
        .unwrap_or_default()
}

pub(crate) fn quote(ident: &str) -> String {
    // auth field names come from the developer's own resource; still refuse
    // anything that isn't a plain identifier.
    if ident.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') && !ident.is_empty() {
        format!("\"{ident}\"")
    } else {
        "\"__invalid__\"".to_string()
    }
}

/// `POST <base>/auth/register` — create a user and return a session token.
///
/// Registration is a `create` on the `user` resource, so the `user` resource's
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

    let (user_id, user) = match create_account(&state, &req, data).await {
        Ok(created) => created,
        Err(resp) => return resp,
    };

    // An app that confirms addresses has not finished registering anybody yet:
    // the account exists, but until the link in their mailbox is opened there
    // is no session to hand out. Saying so — rather than returning a token that
    // login would then refuse — is what lets a client show the right screen.
    if state.requires_email_verification() {
        let address = user
            .get(&spec.identity_field)
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string();
        if let Err(resp) = crate::email_auth::send_verification(&state, user_id, &address).await {
            return resp;
        }
        return HttpResponse::Created().json(&json!({
            "user": user,
            "verification_required": true,
            "message": "Check your email to confirm your address, then sign in.",
        }));
    }

    match state.auth.issue_token(user_id) {
        Ok(token) => HttpResponse::Created().json(&json!({ "token": token, "user": user })),
        Err(_) => error(500, "failed to issue token"),
    }
}

/// Create a user row, running every hook a registration fires, and answer with
/// `(id, the row as the response should show it)`.
///
/// Shared by `POST <base>/auth/register` and by
/// [accepting an invitation](crate::email_auth::accept_invitation), because the
/// two are the same event seen from different doors: somebody who did not have
/// an account now has one. A resource's `before_register` fires for both, which is
/// the point — a rule about who may sign up should not be bypassable by getting
/// invited.
///
/// `data` arrives with the plaintext password **already swapped** for the
/// hashed `password_field`, so no hook on either path can see a secret.
pub(crate) async fn create_account(
    state: &AppState,
    req: &HttpRequest,
    mut data: serde_json::Map<String, Value>,
) -> Result<(Uuid, Value), HttpResponse> {
    let Some(user_r) = state.app.resources.get("user") else {
        return Err(error(500, "no user resource"));
    };

    // Nobody is authenticated yet: whoever is registering has no principal and
    // no organisation, so the hook context carries the request alone.
    let hook_req = HookRequest::new(req, &parse_query(req.query_string()), None, None);

    // `before_register` runs outside `before_create`, so a resource can reject a
    // signup without also rejecting an administrative `POST <base>/user`.
    match hooks::run_auth(
        state,
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
                Err(resp) => return Err(resp),
            }
        }
        Ok(None) => {}
        Err(resp) => return Err(resp),
    }

    match hooks::run(
        state,
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
                Err(resp) => return Err(resp),
            }
        }
        Ok(None) => {}
        Err(resp) => return Err(resp),
    }

    let created = match state.db.create(user_r, &data).await {
        Ok(row) => row,
        Err(e) => return Err(db_error(e)),
    };
    let user_id = created
        .get("id")
        .and_then(|v| v.as_str())
        .and_then(|s| Uuid::parse_str(s).ok());
    let Some(user_id) = user_id else {
        return Err(error(500, "created user missing id"));
    };

    let hook_req = hook_req.with_record(user_id);
    let user = match hooks::run(
        state,
        user_r,
        HookEvent::AfterCreate,
        &hook_req,
        created.clone(),
    )
    .await
    {
        Ok(Some(replacement)) => replacement,
        Ok(None) => created,
        Err(resp) => return Err(resp),
    };

    // The account already exists by now, so an `after_register` rejection fails
    // the *response*, not the write — the same bargain `after_create` makes.
    let user = match hooks::run_auth(
        state,
        user_r,
        AuthEvent::AfterRegister,
        &hook_req,
        user.clone(),
    )
    .await
    {
        Ok(Some(replacement)) => replacement,
        Ok(None) => user,
        Err(resp) => return Err(resp),
    };

    // Every account gets somewhere to work from the moment it exists, so nobody
    // ever lands in an interface where every write is a 400. It is an ordinary
    // organisation — renameable, and no bar to creating others.
    create_personal_organization(state, user_id, &user).await;

    Ok((user_id, user))
}

/// Give a freshly created user their own organisation, with them as its admin.
///
/// Best-effort: a failure here is logged and swallowed rather than failing a
/// registration that has already written the account. An app that has removed
/// the `organization` or `membership` resources simply gets nothing.
pub(crate) async fn create_personal_organization(state: &AppState, user_id: Uuid, user: &Value) {
    let (Some(org_r), Some(membership_r)) = (
        state.app.resources.get("organization"),
        state.app.resources.get("membership"),
    ) else {
        return;
    };

    let name = personal_org_name(state, user);
    let mut org = serde_json::Map::new();
    org.insert("name".into(), Value::String(name.clone()));
    if org_r.fields.contains_key("slug") {
        // `slug` is unique, so it carries the user id: two people called Sam
        // both get one, and neither collides with an app's own naming.
        org.insert(
            "slug".into(),
            Value::String(personal_org_slug(&name, user_id)),
        );
    }

    // The same default a `POST /organization` gets: a personal organisation is
    // an ordinary organisation, and a deployment whose tenants are all of one
    // class means this one too.
    crate::crud::stamp_default_org_class(state, org_r, &mut org);

    let created = match state.db.create(org_r, &org).await {
        Ok(row) => row,
        Err(e) => {
            tracing::error!(error = %e, "failed to create the personal organization");
            return;
        }
    };
    let Some(org_id) = created.get("id").and_then(|v| v.as_str()) else {
        tracing::error!("created personal organization is missing an id");
        return;
    };

    let mut m = serde_json::Map::new();
    m.insert("user_id".into(), Value::String(user_id.to_string()));
    m.insert("organization_id".into(), Value::String(org_id.to_string()));
    m.insert("role".into(), Value::String("admin".into()));
    if let Err(e) = state.db.create(membership_r, &m).await {
        tracing::error!(error = %e, "failed to create the personal membership");
    }
}

/// What to call it: whatever the account is known by, falling back to the local
/// part of the identity they signed up with.
fn personal_org_name(state: &AppState, user: &Value) -> String {
    let spec = auth_spec(state);
    for field in ["name", "full_name", "display_name"] {
        if let Some(value) = user.get(field).and_then(|v| v.as_str()) {
            let value = value.trim();
            if !value.is_empty() {
                return value.to_string();
            }
        }
    }
    let identity = user
        .get(&spec.identity_field)
        .and_then(|v| v.as_str())
        .unwrap_or_default();
    let local = identity.split('@').next().unwrap_or("").trim();
    if local.is_empty() {
        "Personal".to_string()
    } else {
        local.to_string()
    }
}

fn personal_org_slug(name: &str, user_id: Uuid) -> String {
    let base: String = name
        .to_lowercase()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    let base = base.trim_matches('-').replace("--", "-");
    let base: String = base.chars().take(32).collect();
    let base = base.trim_matches('-');
    let short = &user_id.simple().to_string()[..8];
    if base.is_empty() {
        format!("personal-{short}")
    } else {
        format!("{base}-{short}")
    }
}

/// `POST <base>/auth/login` — verify credentials, return a session token.
///
/// Two hooks fire around the credential check. `before_login` sees the claimed
/// identity (never the password) and can reject the attempt or rewrite the
/// identity that is looked up. `after_login` sees the *outcome* — every attempt
/// reaches it, successful or not, distinguished by `success` and `reason` — so
/// one hook can both widen a successful response and count the failures that a
/// lockout is built on.
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
    // `email_verified_at` is a built-in column, but the `user` resource is
    // replaceable — an app that dropped it is an app that does not confirm
    // addresses, and asking for the column would be a 500 rather than a login.
    let verification_required =
        state.requires_email_verification() && user_r.fields.contains_key(VERIFIED_AT_FIELD);
    let verified_column = if verification_required {
        format!(", u.{VERIFIED_AT_FIELD} IS NOT NULL AS verified")
    } else {
        ", true AS verified".to_string()
    };
    let sql = format!(
        "SELECT u.id::text AS id, u.{pw}::text AS password_hash{verified_column} \
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
    // Whether the right password was given for an account that has not yet
    // confirmed its address. Tracked apart from `verified` because it is not a
    // failed login — the credentials were correct — and answering it as one
    // would leave somebody retyping a password that was never the problem.
    let mut unconfirmed = false;
    let verified = match rows.as_array().and_then(|a| a.first()) {
        None => {
            // Spend the argon2 time anyway: answering an unknown identity
            // faster than a wrong password is enough to enumerate accounts.
            let _ = state.auth.hash_password(&password);
            None
        }
        Some(row) => {
            let stored = row
                .get("password_hash")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            if state.auth.verify_password(&password, stored) {
                unconfirmed = row.get("verified").and_then(|v| v.as_bool()) == Some(false);
                match row
                    .get("id")
                    .and_then(|v| v.as_str())
                    .and_then(|s| Uuid::parse_str(s).ok())
                {
                    Some(id) if !unconfirmed => Some(id),
                    Some(_) => None,
                    None => return error(500, "user missing id"),
                }
            } else {
                None
            }
        }
    };

    // The token is issued before the hook runs, so `after_login` can be told
    // what actually happened — and a hook that aborts still costs the caller
    // nothing, because a token nobody receives is a token nobody can use.
    let token = match verified {
        Some(user_id) => match state.auth.issue_token(user_id) {
            Ok(token) => Some(token),
            Err(_) => return error(500, "failed to issue token"),
        },
        None => None,
    };

    let outcome = json!({
        "success": verified.is_some(),
        "user_id": verified.map(|id| id.to_string()),
        "identity": identity,
        // What the caller is never told apart, a hook is: an address nobody
        // holds and a password nobody guessed are different problems.
        "reason": match (verified.is_some(), unconfirmed, rows.as_array().is_some_and(|a| a.is_empty())) {
            (true, _, _) => Value::Null,
            // The password was right and the address is not confirmed — worth
            // telling apart from a wrong one, since a lockout counter should
            // not be counting these.
            (false, true, _) => json!("email_unverified"),
            (false, _, true) => json!("unknown_identity"),
            (false, _, false) => json!("bad_password"),
        },
    });
    let hook_req = match verified {
        Some(user_id) => hook_req.with_record(user_id),
        None => hook_req,
    };

    // On the way out, `after_login` can widen the response — the user row,
    // their roles, whatever the client needs — but never rewrite the token it
    // is being handed alongside. On a failure there is nothing to widen: only
    // an `{"error": …}` matters, and it is how a lockout answers 429 where the
    // endpoint would have answered 401.
    let mut response = match &token {
        Some(token) => json!({ "token": token }),
        None => Value::Null,
    };
    match hooks::run_auth(&state, user_r, AuthEvent::AfterLogin, &hook_req, outcome).await {
        Ok(Some(replacement)) if token.is_some() => {
            merge_beside(&mut response, replacement, "token")
        }
        Ok(_) => {}
        Err(resp) => return resp,
    }
    match token {
        Some(_) => HttpResponse::Ok().json(&response),
        // Telling somebody who typed their own password correctly that their
        // address is unconfirmed reveals nothing they did not already know,
        // and is the difference between a dead end and a resend button.
        None if unconfirmed => HttpResponse::Forbidden().json(&json!({
            "error": "confirm your email address before signing in",
            "reason": "email_unverified",
        })),
        None => error(401, "invalid credentials"),
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

    // A `before_api_key` replacement writes the row, so a resource can stamp an
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

pub(crate) fn table(state: &AppState, name: &str) -> Option<String> {
    state
        .app
        .resources
        .get(name)
        .map(|r| format!("\"{}\"", r.table_name()))
}
