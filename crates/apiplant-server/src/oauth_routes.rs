//! Signing in with somebody else's account, mounted under
//! `<base>/auth/oauth`.
//!
//! | Method | Path | Purpose |
//! |--------|------|---------|
//! | `GET` | `<base>/auth/oauth` | which providers this deployment offers |
//! | `GET` | `<base>/auth/oauth/{provider}/start` | **redirects** the browser to the provider |
//! | `POST` | `<base>/auth/oauth/{provider}/start` | the same, as JSON — and, with a session, this is how an account *links* a provider |
//! | `GET` | `<base>/auth/oauth/{provider}/callback` | where the provider sends the browser back |
//! | `POST` | `<base>/auth/oauth/{provider}/callback` | the same, for a front end that would rather post the code itself |
//! | `DELETE` | `<base>/auth/oauth/{provider}` | unlink, unless it is the last way in |
//!
//! None of it exists without an `[oauth.<provider>]` block. An app that signs
//! nobody in this way has no routes here and no `oauth_state` table — the same
//! bargain `[payments]` and `[email]` make.
//!
//! ## Two doors, one handshake
//!
//! The `GET` pair is the whole flow with no front end at all: point a browser
//! at `/api/auth/oauth/github/start` and it comes back signed in, with the
//! token delivered as `[oauth] token_delivery` says. That is what makes a
//! server-rendered app or a first look from a browser work with nothing built.
//!
//! The `POST` pair is the same handshake for a single-page app, which wants
//! JSON and its own callback route. Both share every line that matters; they
//! differ in how the answer is shaped, which is the only thing they should
//! differ in.
//!
//! ## What the session is
//!
//! The same HS256 JWT `POST <base>/auth/login` issues, signed with the same
//! `[auth] jwt_secret`, carrying the same `sub`/`exp`. Not a parallel kind of
//! session with parallel bugs: every permission, hook and `owner`-scoped query
//! in the app accepts it without knowing OAuth exists.

use apiplant_auth::Authenticator;
use apiplant_core::AuthEvent;
use apiplant_oauth::{Profile, Provider};
use chrono::{Duration, Utc};
use ntex::web::types::{Json, Path, State};
use ntex::web::{HttpRequest, HttpResponse};
use serde_json::{json, Map, Value};
use uuid::Uuid;

use crate::auth_routes::{auth_spec, table, VERIFIED_AT_FIELD};
use crate::crud::parse_query;
use crate::hooks::{self, HookRequest};
use crate::response::{db_error, error};
use crate::state::AppState;

/// Profile columns on `oauth_connection` that are written when present.
const OPTIONAL_CONNECTION_FIELDS: &[&str] = &[
    "email",
    "email_verified",
    "display_name",
    "avatar_url",
    "last_login_at",
];

/// Columns `oauth_connection` must keep for any of this to work, checked at
/// boot by [`check_resources`].
const REQUIRED_CONNECTION_FIELDS: &[&str] =
    &["provider", "provider_user_id", "provider_key", "owner_id"];

/// A synthesised address is at `.invalid`, which RFC 2606 reserves so that it
/// can never resolve — nothing will ever try to deliver to it, which is the
/// entire point of using it rather than a domain somebody could register.
const PLACEHOLDER_DOMAIN: &str = "oauth.invalid";

/// Refuse to boot an app whose `oauth_connection` cannot hold a connection.
///
/// `oauth_connection` is a built-in, so this only fires for an app that
/// replaced it with a `resources/oauth_connection.toml` of its own and dropped
/// something. Failing here is the kind thing to do: the alternative is a 500
/// in front of the first person who presses a sign-in button.
pub fn check_resources(app: &apiplant_core::App) -> Result<(), String> {
    if !app.config.oauth.enabled() {
        return Ok(());
    }
    for (name, required) in [
        ("oauth_connection", REQUIRED_CONNECTION_FIELDS),
        (
            "oauth_state",
            &["provider", "state_hash", "expires_at", "used_at"][..],
        ),
    ] {
        let Some(resource) = app.resources.get(name) else {
            return Err(format!(
                "[oauth] is configured but the `{name}` resource is missing"
            ));
        };
        for field in required {
            if !resource.fields.contains_key(*field) {
                return Err(format!(
                    "[oauth] is configured, so `{name}` needs a `{field}` field — \
                     resources/{name}.toml replaces the built-in, which has one"
                ));
            }
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// GET <base>/auth/oauth — what this deployment offers
// ---------------------------------------------------------------------------

/// The providers, with the URL that starts each one.
///
/// Public and unauthenticated, because it is what a sign-in page reads before
/// anybody has signed in. It names no secret: a client id is not one, and this
/// does not return them either.
pub async fn providers(state: State<AppState>) -> HttpResponse {
    let Some(oauth) = &state.oauth else {
        return error(404, "this app has no OAuth providers configured");
    };
    HttpResponse::Ok().json(&json!({ "providers": oauth.describe(&callback_base(&state)) }))
}

// ---------------------------------------------------------------------------
// start
// ---------------------------------------------------------------------------

/// `GET <base>/auth/oauth/{provider}/start` — send the browser to the provider.
///
/// A 302 rather than JSON, because the caller is a browser following a link: a
/// `<a href="/api/auth/oauth/github/start">` is the whole client-side
/// integration, and an app that has not built a front end yet still has a
/// working sign-in.
pub async fn start_redirect(
    req: HttpRequest,
    state: State<AppState>,
    path: Path<String>,
) -> HttpResponse {
    match begin(&req, &state, &path.into_inner()).await {
        Ok(flow) => HttpResponse::Found()
            .header("location", flow.authorize_url)
            // A consent screen the browser cached would be a sign-in that
            // reuses a spent `state`.
            .header("cache-control", "no-store")
            .finish(),
        Err(response) => response,
    }
}

/// `POST <base>/auth/oauth/{provider}/start` — the same, as JSON.
///
/// What a single-page app calls: it gets `authorize_url` and navigates there
/// itself. It is also how **linking** works. Called with a session, the flow is
/// recorded as belonging to that account, and finishing it attaches a provider
/// to the account instead of signing anybody in — the decision is made here,
/// while the caller is holding the credential that proves whose account it is,
/// and never taken from the callback.
pub async fn start_json(
    req: HttpRequest,
    state: State<AppState>,
    path: Path<String>,
) -> HttpResponse {
    match begin(&req, &state, &path.into_inner()).await {
        Ok(flow) => HttpResponse::Ok().json(&json!({
            "provider": flow.provider,
            "authorize_url": flow.authorize_url,
            "state": flow.state,
            "expires_in": state.app.config.oauth.state_ttl(),
            "linking": flow.linking,
        })),
        Err(response) => response,
    }
}

/// A started flow, as both `start` endpoints need it.
struct Started {
    provider: String,
    authorize_url: String,
    state: String,
    linking: bool,
}

/// Mint the `state`, store what the callback will need, and build the URL.
async fn begin(
    req: &HttpRequest,
    state: &State<AppState>,
    provider_key: &str,
) -> Result<Started, HttpResponse> {
    let provider = resolve(state, provider_key)?;

    // An app that will not let anybody in without a confirmed address cannot
    // use a provider that has no addresses to give — the account it created
    // could never sign in. Saying so at the door beats creating one.
    if state.requires_email_verification() && !provider.provides_email {
        return Err(error(
            400,
            format!(
                "{} cannot be used here: it releases no email address, and this app \
                 requires a confirmed one",
                provider.label
            ),
        ));
    }

    let flow = provider.start().map_err(oauth_error)?;

    // Linking, when the caller brought a session. `resolve_principal` is the
    // same check every other endpoint makes, so an expired token links nothing
    // rather than half-linking something.
    let link_user_id = state.resolve_principal(req).await.map(|p| p.user_id);

    let Some(state_r) = state.app.resources.get("oauth_state") else {
        return Err(error(500, "no oauth_state resource"));
    };
    let mut row = Map::new();
    row.insert("provider".into(), json!(provider.key));
    row.insert("state_hash".into(), json!(flow.state_hash));
    if let Some(verifier) = &flow.verifier {
        row.insert("verifier".into(), json!(verifier));
    }
    row.insert("redirect_uri".into(), json!(flow.redirect_uri));
    if let Some(user_id) = link_user_id {
        row.insert("link_user_id".into(), json!(user_id.to_string()));
    }
    let params = parse_query(req.query_string());
    row.insert("return_to".into(), json!(return_to(state, &params)));
    if let Some(delivery) = requested_delivery(&params) {
        row.insert("token_delivery".into(), json!(delivery));
    }
    row.insert(
        "expires_at".into(),
        json!(
            (Utc::now() + Duration::seconds(state.app.config.oauth.state_ttl() as i64))
                .to_rfc3339()
        ),
    );
    state
        .db
        .create(state_r, &keep_declared(state_r, row))
        .await
        .map_err(db_error)?;

    // Housekeeping, here because it is the moment that reliably happens. A
    // day's grace leaves yesterday's rows to answer "why did that sign-in
    // fail?" with something better than an empty table.
    sweep_expired(state).await;

    Ok(Started {
        provider: provider.key.clone(),
        authorize_url: flow.authorize_url,
        state: flow.state,
        linking: link_user_id.is_some(),
    })
}

// ---------------------------------------------------------------------------
// callback
// ---------------------------------------------------------------------------

/// What the provider sends back, however it reaches us.
#[derive(Debug, Default, serde::Deserialize)]
pub struct Callback {
    #[serde(default)]
    code: String,
    #[serde(default)]
    state: String,
    /// A provider that refuses says so here and sends no code. The commonest
    /// value by far is `access_denied`: somebody pressed Cancel, which is an
    /// answer rather than an error.
    #[serde(default)]
    error: String,
    #[serde(default)]
    error_description: String,
}

/// `GET <base>/auth/oauth/{provider}/callback` — the redirect URI.
///
/// This is the URL registered with the provider, and it answers a *browser*: on
/// success it redirects to wherever the flow said, carrying the session token
/// as `[oauth] token_delivery` describes. `token_delivery = "json"` answers
/// with the body instead, which is what makes the whole flow inspectable from
/// curl.
pub async fn callback_redirect(
    req: HttpRequest,
    state: State<AppState>,
    path: Path<String>,
) -> HttpResponse {
    let params = parse_query(req.query_string());
    let incoming = Callback {
        code: params.get("code").cloned().unwrap_or_default(),
        state: params.get("state").cloned().unwrap_or_default(),
        error: params.get("error").cloned().unwrap_or_default(),
        error_description: params.get("error_description").cloned().unwrap_or_default(),
    };

    let provider = path.into_inner();
    match complete(&req, &state, &provider, incoming).await {
        Ok(success) => deliver(success),
        Err(response) => failure(&state, response),
    }
}

/// `POST <base>/auth/oauth/{provider}/callback` — the same, from a body.
///
/// For a front end with its own `/oauth/callback` route: it reads the query
/// string the provider put in front of it and posts the two values here.
/// Always JSON, never a redirect — a client that chose this door has its own
/// idea of where to go next.
pub async fn callback_json(
    req: HttpRequest,
    state: State<AppState>,
    path: Path<String>,
    body: Json<Callback>,
) -> HttpResponse {
    match complete(&req, &state, &path.into_inner(), body.into_inner()).await {
        Ok(success) => HttpResponse::Ok().json(&success.body),
        Err(response) => response,
    }
}

/// A completed sign-in, before it is shaped into a response.
struct Success {
    body: Value,
    token: String,
    return_to: String,
    /// How this flow asked to be answered — the app's configured default
    /// unless the caller that started it wanted otherwise.
    delivery: String,
}

/// Redeem the code, work out whose account this is, and issue a session.
async fn complete(
    req: &HttpRequest,
    state: &State<AppState>,
    provider_key: &str,
    incoming: Callback,
) -> Result<Success, HttpResponse> {
    let provider = resolve(state, provider_key)?;

    if !incoming.error.is_empty() {
        let detail = match incoming.error_description.trim() {
            "" => incoming.error.as_str(),
            described => described,
        };
        return Err(error(
            400,
            format!("{} did not sign you in: {detail}", provider.label),
        ));
    }
    if incoming.code.trim().is_empty() || incoming.state.trim().is_empty() {
        return Err(error(400, "that redirect carried no code and state"));
    }

    let flow = claim_state(state, &provider.key, incoming.state.trim()).await?;
    let redirect_uri = flow
        .get("redirect_uri")
        .and_then(|v| v.as_str())
        .unwrap_or(&provider.redirect_uri)
        .to_string();
    let verifier = flow.get("verifier").and_then(|v| v.as_str());
    let link_user_id = flow
        .get("link_user_id")
        .and_then(|v| v.as_str())
        .and_then(|s| Uuid::parse_str(s).ok());
    let delivery = flow
        .get("token_delivery")
        .and_then(|v| v.as_str())
        .filter(|value| !value.is_empty())
        .unwrap_or(&state.app.config.oauth.token_delivery)
        .trim()
        .to_lowercase();
    let return_to = flow
        .get("return_to")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .unwrap_or(&state.app.config.oauth.success_redirect)
        .to_string();

    let Some(oauth) = &state.oauth else {
        return Err(error(404, "this app has no OAuth providers configured"));
    };
    let access_token = oauth
        .exchange_code(provider, incoming.code.trim(), verifier, &redirect_uri)
        .await
        .map_err(oauth_error)?;
    let profile = oauth
        .fetch_profile(provider, &access_token)
        .await
        .map_err(oauth_error)?;

    let account = resolve_account(req, state, provider, &profile, link_user_id).await?;
    let owner_id = link_connection(state, provider, &profile, &account).await?;

    let user = read_user(state, owner_id).await?;

    // An app that confirms addresses has not finished here either: a provider
    // that verified the address *is* the confirmation, and one that did not
    // leaves the account exactly where a password registration would — created,
    // unconfirmed, and told so rather than handed a token login would refuse.
    if state.requires_email_verification() && !verified_in(state, &user) {
        return Err(HttpResponse::Ok().json(&json!({
            "user": user,
            "verification_required": true,
            "message": "Check your email to confirm your address, then sign in.",
        })));
    }

    let token = state
        .auth
        .issue_token(owner_id)
        .map_err(|_| error(500, "failed to issue token"))?;

    let mut body = json!({
        "token": token,
        "user": user,
        "provider": provider.key,
        "created": account.created,
        "linked": account.linking,
    });

    // `after_login` fires for an OAuth sign-in exactly as it does for a
    // password one, carrying `provider` so a hook can tell them apart. A
    // lockout counter, an audit trail or a "new device" mail should not have to
    // care which door somebody came through — and would silently miss half the
    // logins if this were left out.
    if let Some(user_r) = state.app.resources.get("user") {
        let hook_req = HookRequest::new(req, &parse_query(req.query_string()), None, None)
            .with_record(owner_id);
        let outcome = json!({
            "success": true,
            "method": "oauth",
            "provider": provider.key,
            "user_id": owner_id.to_string(),
            "identity": user.get(&auth_spec(state).identity_field).cloned().unwrap_or(Value::Null),
            "created": account.created,
            "reason": Value::Null,
        });
        match hooks::run_auth(state, user_r, AuthEvent::AfterLogin, &hook_req, outcome).await {
            Ok(Some(replacement)) => merge_beside(&mut body, replacement, "token"),
            Ok(None) => {}
            Err(response) => return Err(response),
        }
    }

    tracing::info!(
        provider = %provider.key,
        user = %owner_id,
        created = account.created,
        linked = account.linking,
        "oauth sign-in"
    );

    Ok(Success {
        body,
        token,
        return_to,
        delivery,
    })
}

/// Claim the state row, then read it.
///
/// The claim is the `UPDATE`, and it is what makes two callbacks carrying the
/// same `state` — a double-clicked link, or a replayed one — resolve to exactly
/// one sign-in: `used_at IS NULL` is part of the `WHERE`, evaluated under a row
/// lock, so the second affects nothing and is turned away. The same pattern
/// spends an emailed link in [`crate::email_auth`], for the same reason.
///
/// `provider` is in the `WHERE` too: a state issued for GitHub cannot be
/// redeemed as a Google code, whatever the URL says.
async fn claim_state(
    state: &AppState,
    provider: &str,
    presented: &str,
) -> Result<Value, HttpResponse> {
    let Some(state_tbl) = table(state, "oauth_state") else {
        return Err(error(500, "no oauth_state resource"));
    };
    let hash = Authenticator::hash_link_token(presented);
    let params = [Value::String(hash.clone()), Value::String(provider.into())];

    let claim = format!(
        "UPDATE {state_tbl} SET used_at = now() \
         WHERE state_hash = $1 AND provider = $2 AND used_at IS NULL AND expires_at > now()"
    );
    let claimed = state
        .db
        .raw_json(&claim, &params)
        .await
        .map_err(db_error)?
        .get("rows_affected")
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    if claimed == 0 {
        // One message for "never issued", "already used", "expired" and "wrong
        // provider". From the caller's side they are one situation — start
        // again — and telling them apart answers questions only an attacker is
        // asking.
        return Err(error(
            400,
            "that sign-in is no longer valid — please start again",
        ));
    }

    let lookup = format!(
        "SELECT verifier, redirect_uri, return_to, token_delivery, \
                link_user_id::text AS link_user_id \
         FROM {state_tbl} WHERE state_hash = $1 AND provider = $2 LIMIT 1"
    );
    state
        .db
        .raw_json(&lookup, &params)
        .await
        .map_err(db_error)?
        .as_array()
        .and_then(|rows| rows.first())
        .cloned()
        .ok_or_else(|| error(400, "that sign-in is no longer valid — please start again"))
}

/// The account a completed handshake belongs to.
struct Account {
    user_id: Uuid,
    created: bool,
    linking: bool,
}

/// Decide whose account this is.
///
/// The one genuinely interesting decision in an OAuth implementation, in four
/// steps:
///
/// 1. **A connection already exists** — somebody signing in again. Keyed on the
///    provider's id, so a changed address or username changes nothing.
/// 2. **The flow was started from a session** — "connect my GitHub". Decided in
///    [`begin`], while that session was in hand.
/// 3. **A verified address matches an existing account** — the convenience
///    case: registered with a password, came back through Google.
/// 4. **Otherwise, a new account**, created through the same path
///    `POST <base>/auth/register` uses, so it fires the same hooks and gets the
///    same personal organisation.
///
/// Step 3 is the dangerous one. It is safe *only* because the provider says it
/// verified the address: if an unverified one were enough, anybody could set
/// their address at a careless provider to somebody else's and sign in as them.
/// That is not hypothetical — it is how several real "sign in with" takeovers
/// worked — which is why `email_verified` is never assumed anywhere upstream of
/// here, and why `[oauth] link_by_verified_email` exists for an app that would
/// rather not take the risk at all.
async fn resolve_account(
    req: &HttpRequest,
    state: &State<AppState>,
    provider: &Provider,
    profile: &Profile,
    link_user_id: Option<Uuid>,
) -> Result<Account, HttpResponse> {
    let provider_key = format!("{}:{}", provider.key, profile.id);

    // 1. Returning.
    if let Some(existing) = find_connection(state, &provider_key).await? {
        let owner = existing
            .get("owner_id")
            .and_then(|v| v.as_str())
            .and_then(|s| Uuid::parse_str(s).ok())
            .ok_or_else(|| error(500, "oauth_connection row has no owner"))?;
        // Signed in as one person, finishing a link that belongs to another.
        if link_user_id.is_some_and(|id| id != owner) {
            return Err(error(
                409,
                format!(
                    "that {} account is already linked to a different user",
                    provider.label
                ),
            ));
        }
        return Ok(Account {
            user_id: owner,
            created: false,
            linking: link_user_id.is_some(),
        });
    }

    // 2. Linking.
    if let Some(user_id) = link_user_id {
        return Ok(Account {
            user_id,
            created: false,
            linking: true,
        });
    }

    // 3. A verified address this app already knows.
    if state.app.config.oauth.link_by_verified_email && profile.email_verified {
        if let Some(email) = profile.email.as_deref() {
            if let Some(user_id) = find_user_by_identity(state, email).await? {
                tracing::info!(
                    provider = %provider.key,
                    "matched a verified provider address to an existing account"
                );
                return Ok(Account {
                    user_id,
                    created: false,
                    linking: false,
                });
            }
        }
    }

    // 4. Somebody new.
    create_account(req, state, provider, profile)
        .await
        .map(|user_id| Account {
            user_id,
            created: true,
            linking: false,
        })
}

/// Create the account behind a first-time sign-in.
///
/// Through [`crate::auth_routes::create_account`], which is the same code
/// `POST <base>/auth/register` runs: the `user` resource's `before_register`,
/// `before_create`, `after_create` and `after_register` hooks all fire, and the
/// new account gets its own organisation to work in. An OAuth sign-up that
/// skipped those would be a second kind of registration with a second set of
/// rules, and every rule an app writes about who may sign up would have a hole
/// in it shaped like a Google button.
async fn create_account(
    req: &HttpRequest,
    state: &State<AppState>,
    provider: &Provider,
    profile: &Profile,
) -> Result<Uuid, HttpResponse> {
    let settings = &state.app.config.oauth;
    if !state.app.config.auth.allow_registration {
        // The same 403 registration gives, for the same reason: an app with
        // signup closed has closed it, and a provider button is not a side door.
        return Err(error(
            403,
            "registration is disabled — ask for an invitation, then link this provider",
        ));
    }
    let Some(user_r) = state.app.resources.get("user") else {
        return Err(error(500, "no user resource"));
    };
    let spec = auth_spec(state);

    // The identity column is required and unique, and X will not give an
    // address, so something has to go in it. A synthesised one at `.invalid`
    // can never receive mail — which is the honest way to say "we do not have
    // an address for this person" in a column that cannot be empty.
    let (identity, placeholder) = match profile.email.as_deref().map(str::trim) {
        Some(email) if !email.is_empty() => (email.to_lowercase(), false),
        _ => (
            format!("{}_{}@{PLACEHOLDER_DOMAIN}", provider.key, profile.id),
            true,
        ),
    };

    let mut data = Map::new();
    data.insert(spec.identity_field.clone(), json!(identity));
    // Where a name and a picture go is the app's to say — `[oauth] name_field`
    // and `avatar_field`, defaulting to the columns the built-in `user` resource
    // declares. An empty setting writes neither, for an app that keeps its own.
    for (field, value) in [
        (&settings.name_field, &profile.display_name),
        (&settings.avatar_field, &profile.avatar_url),
    ] {
        if let (false, Some(value)) = (field.trim().is_empty(), value) {
            data.insert(field.trim().to_string(), json!(value));
        }
    }
    if placeholder {
        data.insert("email_placeholder".into(), json!(true));
    }
    // A provider that verified the address has done the confirming, so the
    // account starts confirmed. One that did not leaves the column null, and an
    // app requiring confirmation treats it exactly as it treats an unconfirmed
    // password signup.
    if profile.email_verified && !placeholder {
        data.insert(VERIFIED_AT_FIELD.into(), json!(Utc::now().to_rfc3339()));
    }

    // Somebody already signs in with this address, and the sign-in got here
    // rather than matching them — so the provider did not vouch for the
    // address, or this app does not accept that kind of vouching. Either way
    // the answer is *not* to take the account, and it is not to invent a second
    // one under a made-up address that its owner would never guess. It is to
    // say what happened and what to do about it.
    //
    // Reaching for the unique constraint instead would produce the same refusal
    // as a bare 409 "a record with these values already exists", which is a
    // true sentence nobody can act on.
    if find_user_by_identity(state, &identity).await?.is_some() {
        return Err(error(
            409,
            format!(
                "an account already uses {identity} — sign in with it, then connect \
                 {} from your account settings",
                provider.label
            ),
        ));
    }

    match crate::auth_routes::create_account(state, req, keep_declared(user_r, data)).await {
        Ok((user_id, _)) => Ok(user_id),
        Err(response) => Err(response),
    }
}

/// Write the connection, or refresh the one already there, and answer with
/// whose account it settled on.
///
/// The insert can lose a race: two first-time sign-ins for the same provider
/// account can both reach it, and `provider_key` is unique, so one of them
/// fails. The loser reads the winner's row and uses that account — and, if it
/// had just created an account of its own, takes it back out again. Without
/// this, one person clicking twice ends up with two accounts and no way to know
/// which one their next sign-in will reach.
async fn link_connection(
    state: &State<AppState>,
    provider: &Provider,
    profile: &Profile,
    account: &Account,
) -> Result<Uuid, HttpResponse> {
    let Some(connection_r) = state.app.resources.get("oauth_connection") else {
        return Err(error(500, "no oauth_connection resource"));
    };
    let provider_key = format!("{}:{}", provider.key, profile.id);

    let mut row = Map::new();
    row.insert("provider".into(), json!(provider.key));
    row.insert("provider_user_id".into(), json!(profile.id));
    row.insert("provider_key".into(), json!(provider_key));
    row.insert("owner_id".into(), json!(account.user_id.to_string()));
    row.insert(
        "email".into(),
        profile
            .email
            .clone()
            .map(Value::from)
            .unwrap_or(Value::Null),
    );
    row.insert("email_verified".into(), json!(profile.email_verified));
    row.insert(
        "display_name".into(),
        profile
            .display_name
            .clone()
            .map(Value::from)
            .unwrap_or(Value::Null),
    );
    row.insert(
        "avatar_url".into(),
        profile
            .avatar_url
            .clone()
            .map(Value::from)
            .unwrap_or(Value::Null),
    );
    row.insert("last_login_at".into(), json!(Utc::now().to_rfc3339()));
    let row = keep_declared(connection_r, row);

    match find_connection(state, &provider_key).await? {
        // Known: refresh what the provider says about them, because people
        // change their name and their picture.
        Some(existing) => {
            let id = existing.get("id").and_then(|v| v.as_str()).unwrap_or("");
            let owner = existing
                .get("owner_id")
                .and_then(|v| v.as_str())
                .and_then(|s| Uuid::parse_str(s).ok())
                .ok_or_else(|| error(500, "oauth_connection row has no owner"))?;
            let update: Map<String, Value> = row
                .into_iter()
                .filter(|(key, _)| OPTIONAL_CONNECTION_FIELDS.contains(&key.as_str()))
                .collect();
            if !update.is_empty() {
                if let Ok(id) = Uuid::parse_str(id) {
                    if let Err(e) = state.db.update(connection_r, id, &update, &[]).await {
                        // A profile that failed to refresh is not a failed
                        // sign-in: the connection is what identifies them, and
                        // it is already right.
                        tracing::warn!(error = %e, "could not refresh an oauth connection");
                    }
                }
            }
            Ok(owner)
        }
        None => {
            match state.db.create(connection_r, &row).await {
                Ok(_) => Ok(account.user_id),
                Err(e) if is_conflict(&e) => {
                    // Lost the race. Whoever won holds the account.
                    let winner = find_connection(state, &provider_key)
                        .await?
                        .and_then(|row| {
                            row.get("owner_id")
                                .and_then(|v| v.as_str())
                                .and_then(|s| Uuid::parse_str(s).ok())
                        })
                        .ok_or_else(|| error(500, "the connection could not be stored"))?;
                    if account.created && winner != account.user_id {
                        tracing::warn!("a concurrent sign-in created this account first; dropping the duplicate");
                        if let Some(user_r) = state.app.resources.get("user") {
                            let _ = state.db.delete(user_r, account.user_id, &[]).await;
                        }
                    }
                    Ok(winner)
                }
                Err(e) => Err(db_error(e)),
            }
        }
    }
}

// ---------------------------------------------------------------------------
// DELETE <base>/auth/oauth/{provider} — unlink
// ---------------------------------------------------------------------------

/// Remove one provider from the caller's own account.
///
/// `DELETE <base>/oauth_connection/{id}` exists too and is `owner`-gated, so
/// this endpoint earns its place with the check CRUD cannot make: an account
/// whose only credential is the connection being deleted becomes unreachable
/// the moment it succeeds — no password to fall back on, no second provider,
/// and the row that mapped a person to the account gone. So the last one is
/// refused, and the answer says what to do instead.
pub async fn unlink(req: HttpRequest, state: State<AppState>, path: Path<String>) -> HttpResponse {
    let Some(principal) = state.resolve_principal(&req).await else {
        return error(401, "authentication required");
    };
    let provider = path.into_inner().trim().to_lowercase();

    let (Some(user_tbl), Some(connection_tbl)) =
        (table(&state, "user"), table(&state, "oauth_connection"))
    else {
        return error(500, "missing user or oauth_connection resource");
    };
    let spec = auth_spec(&state);
    let user_id = Value::String(principal.user_id.to_string());

    let sql = format!(
        "SELECT (u.{password} IS NOT NULL) AS has_password, \
                (SELECT count(*) FROM {connection_tbl} c WHERE c.owner_id = u.id) AS connections, \
                (SELECT count(*) FROM {connection_tbl} c \
                   WHERE c.owner_id = u.id AND c.provider = $2) AS matching \
         FROM {user_tbl} u WHERE u.id = $1::uuid",
        password = crate::auth_routes::quote(&spec.password_field),
    );
    let rows = match state
        .db
        .raw_json(&sql, &[user_id.clone(), Value::String(provider.clone())])
        .await
    {
        Ok(rows) => rows,
        Err(e) => return db_error(e),
    };
    let Some(row) = rows.as_array().and_then(|rows| rows.first()) else {
        return error(404, "no such account");
    };
    let has_password = row.get("has_password").and_then(|v| v.as_bool()) == Some(true);
    let connections = row.get("connections").and_then(|v| v.as_i64()).unwrap_or(0);
    let matching = row.get("matching").and_then(|v| v.as_i64()).unwrap_or(0);

    if matching == 0 {
        return error(404, format!("no {provider} account is linked here"));
    }
    let credentials = connections + i64::from(has_password);
    if credentials - matching < 1 {
        return error(
            409,
            format!(
                "{provider} is the only way into this account — set a password or link \
                 another provider before unlinking it"
            ),
        );
    }

    let deleted =
        format!("DELETE FROM {connection_tbl} WHERE owner_id = $1::uuid AND provider = $2");
    match state
        .db
        .raw_json(&deleted, &[user_id, Value::String(provider.clone())])
        .await
    {
        Ok(result) => {
            let removed = result
                .get("rows_affected")
                .and_then(|v| v.as_u64())
                .unwrap_or(0);
            HttpResponse::Ok().json(&json!({
                "provider": provider,
                "removed": removed,
                "credentials_left": credentials - removed as i64,
            }))
        }
        Err(e) => db_error(e),
    }
}

// ---------------------------------------------------------------------------
// shaping the answer
// ---------------------------------------------------------------------------

/// Turn a completed sign-in into what the browser gets.
fn deliver(success: Success) -> HttpResponse {
    let delivery = success.delivery.as_str();
    if delivery == "json" {
        return HttpResponse::Ok().json(&success.body);
    }

    // A fragment is never sent to a server: it stays out of access logs, out of
    // the `Referer` header on the next click, and out of anything in front of
    // this app that records URLs. In exchange only a script on the landing page
    // can read it — which is why `query` exists for a server-rendered app, and
    // why it is not the default.
    let separator = match (delivery, success.return_to.contains('?')) {
        ("query", true) => '&',
        ("query", false) => '?',
        _ => '#',
    };
    let target = format!(
        "{}{separator}token={}",
        success.return_to,
        urlencoding(&success.token),
    );
    HttpResponse::Found()
        .header("location", target)
        .header("cache-control", "no-store")
        .finish()
}

/// Turn a failure into what the browser gets.
///
/// With no `failure_redirect` this is the JSON error as it stands, which is
/// what you want while setting a provider up: the provider's own complaint,
/// legible, at the URL it happened on. With one, the browser is sent there —
/// and the message goes as a query parameter, because a page that says "that
/// did not work" and cannot say why is a support ticket.
fn failure(state: &AppState, response: HttpResponse) -> HttpResponse {
    let target = state.app.config.oauth.failure_redirect.trim();
    if target.is_empty() || response.status().is_success() {
        return response;
    }
    let separator = if target.contains('?') { '&' } else { '?' };
    HttpResponse::Found()
        .header(
            "location",
            format!(
                "{target}{separator}oauth_error={}",
                urlencoding(&format!("{}", response.status().as_u16()))
            ),
        )
        .header("cache-control", "no-store")
        .finish()
}

// ---------------------------------------------------------------------------
// small shared parts
// ---------------------------------------------------------------------------

/// The provider named in the path, or the response to send instead.
fn resolve<'a>(state: &'a State<AppState>, key: &str) -> Result<&'a Provider, HttpResponse> {
    let Some(oauth) = &state.oauth else {
        return Err(error(404, "this app has no OAuth providers configured"));
    };
    oauth
        .get(key)
        .ok_or_else(|| error(404, format!("no `{key}` sign-in is configured here")))
}

/// The delivery a caller asked for, if it asked for one this server offers.
///
/// A client knows how it wants to be handed a token — the admin dashboard reads
/// a fragment, a server-rendered page reads a query — and that is a
/// presentation choice, not a security boundary: `return_to` is a path on this
/// site either way, so the token reaches the same origin however it travels.
/// An unrecognised value is ignored rather than refused, leaving the app's own
/// setting in force.
fn requested_delivery(params: &std::collections::HashMap<String, String>) -> Option<String> {
    let requested = params.get("token_delivery")?.trim().to_lowercase();
    matches!(requested.as_str(), "fragment" | "query" | "json").then_some(requested)
}

/// Where the browser should land afterwards.
///
/// Only a single-slash-prefixed path survives. `//evil.example` is refused
/// because a browser reads it as a protocol-relative *URL*, and a redirect
/// target somebody else picks turns a sign-in into a phishing hop — the classic
/// open redirect, and the reason this accepts a shape rather than rejecting a
/// list of strings.
fn return_to(state: &AppState, params: &std::collections::HashMap<String, String>) -> String {
    let fallback = state.app.config.oauth.success_redirect.clone();
    let Some(requested) = params.get("return_to").map(|s| s.trim()) else {
        return fallback;
    };
    match requested.starts_with('/')
        && !requested.starts_with("//")
        && !requested.contains(['\\', '\n', '\r'])
    {
        true => requested.to_string(),
        false => fallback,
    }
}

/// Where providers send browsers back to: `<origin><base>/auth/oauth`.
///
/// Derived rather than configured, so the URL to register in four dashboards is
/// a consequence of `[server] public_url` — which is the value that has to be
/// right anyway.
pub fn callback_base(state: &AppState) -> String {
    format!(
        "{}{}/auth/oauth",
        state.app.config.server.public_origin(),
        state.app.config.server.base_path.trim_end_matches('/'),
    )
}

/// Drop anything the resource does not declare.
///
/// This is what lets the *app* decide how much of a profile it keeps. A
/// sign-in offers `avatar_url` and `email_placeholder` for the `user` table;
/// the default `user` resource declares neither, so they are dropped here and the
/// sign-in works exactly the same. Add either to `resources/users.toml` and it
/// starts being filled, with no other change anywhere.
///
/// A framework that added an `avatar_url` column to every app in case somebody
/// might one day sign in with Google would be a framework deciding what your
/// tables look like. This is the alternative: offer, and let the resource answer.
fn keep_declared(
    resource: &apiplant_core::schema::Resource,
    data: Map<String, Value>,
) -> Map<String, Value> {
    data.into_iter()
        .filter(|(key, _)| resource.fields.contains_key(key))
        .collect()
}

async fn find_connection(
    state: &AppState,
    provider_key: &str,
) -> Result<Option<Value>, HttpResponse> {
    let Some(connection_tbl) = table(state, "oauth_connection") else {
        return Err(error(500, "no oauth_connection resource"));
    };
    let sql = format!(
        "SELECT id::text AS id, owner_id::text AS owner_id FROM {connection_tbl} \
         WHERE provider_key = $1 LIMIT 1"
    );
    Ok(state
        .db
        .raw_json(&sql, &[Value::String(provider_key.to_string())])
        .await
        .map_err(db_error)?
        .as_array()
        .and_then(|rows| rows.first())
        .cloned())
}

/// Find an account by what it signs in as, case-insensitively — because an
/// address is not case-sensitive in the half of it that matters, and somebody
/// who registered as `Ann@example.com` is who Google means by
/// `ann@example.com`.
async fn find_user_by_identity(
    state: &AppState,
    identity: &str,
) -> Result<Option<Uuid>, HttpResponse> {
    let Some(user_tbl) = table(state, "user") else {
        return Err(error(500, "no user resource"));
    };
    let spec = auth_spec(state);
    let sql = format!(
        "SELECT id::text AS id FROM {user_tbl} WHERE lower({identity_field}) = lower($1) LIMIT 1",
        identity_field = crate::auth_routes::quote(&spec.identity_field),
    );
    Ok(state
        .db
        .raw_json(&sql, &[Value::String(identity.to_string())])
        .await
        .map_err(db_error)?
        .as_array()
        .and_then(|rows| rows.first())
        .and_then(|row| row.get("id"))
        .and_then(|v| v.as_str())
        .and_then(|s| Uuid::parse_str(s).ok()))
}

async fn read_user(state: &AppState, user_id: Uuid) -> Result<Value, HttpResponse> {
    let Some(user_r) = state.app.resources.get("user") else {
        return Err(error(500, "no user resource"));
    };
    state
        .db
        .get(user_r, user_id, &[])
        .await
        .map_err(db_error)?
        .ok_or_else(|| error(500, "the account disappeared mid-sign-in"))
}

/// Whether this account's address counts as confirmed. An app whose `user`
/// resource has no such column does not confirm addresses at all, so the question
/// does not arise.
fn verified_in(state: &AppState, user: &Value) -> bool {
    match state
        .app
        .resources
        .get("user")
        .is_some_and(|r| r.fields.contains_key(VERIFIED_AT_FIELD))
    {
        true => !matches!(user.get(VERIFIED_AT_FIELD), None | Some(Value::Null)),
        false => true,
    }
}

async fn sweep_expired(state: &AppState) {
    let Some(state_tbl) = table(state, "oauth_state") else {
        return;
    };
    let sql = format!("DELETE FROM {state_tbl} WHERE expires_at < now() - interval '1 day'");
    if let Err(error) = state.db.raw_json(&sql, &[]).await {
        tracing::warn!(%error, "could not sweep expired oauth states");
    }
}

fn is_conflict(error: &apiplant_db::Error) -> bool {
    let text = error.to_string().to_lowercase();
    text.contains("unique") || text.contains("duplicate key")
}

/// Map a provider-side failure to a response.
///
/// The detail is logged and not returned. A provider's complaint names *this
/// app's* registration — a redirect URI that does not match, a scope that was
/// never granted — which is exactly what the operator needs and nothing the
/// person signing in can act on.
fn oauth_error(error: apiplant_oauth::Error) -> HttpResponse {
    use apiplant_oauth::Error;
    match error {
        Error::NotConfigured(message) => crate::response::error(404, message),
        Error::Misconfigured(message) => {
            tracing::error!(detail = %message, "oauth provider is misconfigured");
            crate::response::error(500, "this sign-in is misconfigured")
        }
        Error::Refused {
            ref provider,
            stage,
            ref detail,
        } => {
            tracing::error!(%provider, stage, %detail, "oauth provider refused");
            crate::response::error(
                400,
                format!("{provider} did not complete the sign-in — please try again"),
            )
        }
        Error::Unreachable { ref provider, .. } => {
            let provider = provider.clone();
            crate::telemetry::record_error("oauth_unreachable", &error);
            tracing::error!(%error, "oauth provider could not be reached");
            crate::response::error(
                502,
                format!("{provider} could not be reached — please try again"),
            )
        }
        // The provider answered with something this build cannot read: a shape
        // that changed, or a URL pointed somewhere unexpected. Nothing the
        // caller can do about either.
        Error::Unreadable(_) => {
            tracing::error!(%error, "oauth provider sent an unreadable reply");
            crate::response::error(
                502,
                "that sign-in could not be completed — please try again",
            )
        }
    }
}

/// Fold a hook's replacement into `response`, leaving `reserved` alone.
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

/// Percent-encode a value for a URL. A JWT is base64url with dots, so this only
/// ever has work to do for a `return_to` somebody was creative with — but a
/// token pasted raw into a `location` header is a token that breaks the moment
/// the format changes.
fn urlencoding(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char)
            }
            other => out.push_str(&format!("%{other:02X}")),
        }
    }
    out
}
