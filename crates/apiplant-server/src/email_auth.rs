//! The endpoints that reach somebody through their mailbox.
//!
//! Three flows, one premise: **holding a link sent to an address is proof of
//! controlling that address**, and that proof is worth as much as a password
//! for exactly one action.
//!
//! | Flow | Endpoints |
//! |------|-----------|
//! | Invite somebody into an organisation | `POST <base>/auth/invitations`, `GET <base>/auth/invitations/{token}`, `POST <base>/auth/invitations/{token}/accept` |
//! | Confirm an address | `POST <base>/auth/verify-email`, `POST <base>/auth/verify-email/resend` |
//! | Reset a forgotten password | `POST <base>/auth/password/forgot`, `POST <base>/auth/password/reset` |
//!
//! ## None of this is mounted without a mailer
//!
//! Every route here is registered only when the app has an `[email]` provider
//! *and* the matching `[auth]` flag is on — see
//! [`AppState::invitations_enabled`](crate::state::AppState) and its
//! neighbours. A deployment that cannot send mail does not answer 500 on a
//! password reset, it does not answer at all, and the dashboard and console are
//! told through the admin manifest so they never show the button. A door that
//! cannot open is worse than one that isn't there.
//!
//! ## What the tokens are
//!
//! Random 256-bit strings, mailed once, stored only as a SHA-256 hash (see
//! [`Authenticator::generate_link_token`]). Each is single-use and expires;
//! spending one stamps the row so the copy left in a mailbox is inert. A
//! password reset additionally invalidates every *other* outstanding reset for
//! that account, because "I asked twice and used the first" should not leave a
//! second key under the mat.
//!
//! ## What is deliberately not said out loud
//!
//! `POST /auth/password/forgot` and `/auth/verify-email/resend` answer `202`
//! whatever happens. Answering "no such account" would turn either endpoint
//! into a membership oracle for any address somebody cares to try. The person
//! who really owns the address learns the truth in the only place they should:
//! their inbox.

use apiplant_auth::Authenticator;
use chrono::{DateTime, Duration, Utc};
use ntex::web::types::{Json, Path, State};
use ntex::web::{HttpRequest, HttpResponse};
use serde_json::{json, Map, Value};
use uuid::Uuid;

use crate::auth_routes::{auth_spec, quote, table, VERIFIED_AT_FIELD};
use crate::emails::{self, Links};
use crate::response::{db_error, error};
use crate::state::AppState;

/// `auth_token.kind` for an address confirmation.
const KIND_VERIFICATION: &str = "email_verification";
/// `auth_token.kind` for a password reset.
const KIND_RESET: &str = "password_reset";

// --- invitations -----------------------------------------------------------

/// `POST <base>/auth/invitations` — invite an address into the active
/// organisation.
///
/// The person invited need not have an account; that is the whole reason this
/// exists beside `POST <base>/membership`, which can only add somebody who has
/// already registered.
///
/// Issued by anyone who may add members — `role:admin` in a default app, or
/// whatever the app's `membership` model says `create` takes. The check is
/// against `membership` rather than against `invitation` on purpose: an
/// invitation is a membership that has not happened yet, and having two
/// answers to "who may let people in" is how they end up disagreeing.
///
/// Inviting an address that already has a pending invitation **replaces** it,
/// which is what somebody clicking "invite" a second time means. The earlier
/// link stops working.
pub async fn create_invitation(
    req: HttpRequest,
    state: State<AppState>,
    body: Json<Value>,
) -> HttpResponse {
    let Some(principal) = state.resolve_principal(&req).await else {
        return error(401, "authentication required");
    };
    let Some(org) = state.active_org(&req, &Some(principal.clone())) else {
        return error(400, "no active organization — pick one with X-Organization");
    };
    if !may_invite(&state, &principal, org) {
        return error(403, "you may not add people to this organization");
    }

    let Some(invitation_r) = state.app.resources.get("invitation") else {
        return error(500, "no invitation resource");
    };
    let spec = auth_spec(&state);

    let address = body
        .get("email")
        .or_else(|| body.get(&spec.identity_field))
        .and_then(|v| v.as_str())
        .map(str::trim)
        .unwrap_or_default()
        .to_string();
    if address.is_empty() {
        return error(400, "`email` is required");
    }
    let role = body
        .get("role")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|role| !role.is_empty())
        .unwrap_or("member")
        .to_string();

    // Somebody already inside does not need letting in, and an invitation that
    // resolved to "you are already a member" would be a confusing email.
    match already_a_member(&state, &address, org).await {
        Ok(true) => return error(409, "they are already in this organization"),
        Ok(false) => {}
        Err(resp) => return resp,
    }

    // A second invitation supersedes the first rather than sitting beside it:
    // two live links to the same organisation is a fact nobody wants to reason
    // about, least of all when revoking one.
    if let Some(invitation_tbl) = table(&state, "invitation") {
        let sql = format!(
            "DELETE FROM {invitation_tbl} \
             WHERE organization_id = $1::uuid AND lower(email) = lower($2) \
               AND accepted_at IS NULL"
        );
        if let Err(e) = state
            .db
            .raw_json(
                &sql,
                &[
                    Value::String(org.to_string()),
                    Value::String(address.clone()),
                ],
            )
            .await
        {
            return db_error(e);
        }
    }

    let ttl = state.app.config.auth.invite_ttl_secs;
    let (plaintext, hash) = Authenticator::generate_link_token("inv");

    let mut data = Map::new();
    data.insert("email".into(), Value::String(address.clone()));
    data.insert("role".into(), Value::String(role.clone()));
    data.insert("token_hash".into(), Value::String(hash));
    data.insert("organization_id".into(), Value::String(org.to_string()));
    data.insert(
        "invited_by".into(),
        Value::String(principal.user_id.to_string()),
    );
    data.insert("expires_at".into(), Value::String(rfc3339_in(ttl as i64)));

    let row = match state.db.create(invitation_r, &data).await {
        Ok(row) => row,
        Err(e) => return db_error(e),
    };

    let organization = organization_name(&state, org).await;
    let inviter = inviter_name(&state, principal.user_id).await;
    let message = emails::invitation(
        &Links::from_app(&state.app),
        &organization,
        inviter.as_deref(),
        &plaintext,
        &emails::humanise(ttl),
    );

    // A row nobody was told about is not an invitation, so a send that fails
    // takes the row with it. Otherwise the admin sees "pending" for a link that
    // was never delivered and waits for somebody who was never asked.
    if let Err(resp) = send(&state, message.to(&address)).await {
        if let (Some(invitation_tbl), Some(id)) = (
            table(&state, "invitation"),
            row.get("id").and_then(|v| v.as_str()),
        ) {
            let sql = format!("DELETE FROM {invitation_tbl} WHERE id = $1::uuid");
            let _ = state
                .db
                .raw_json(&sql, &[Value::String(id.to_string())])
                .await;
        }
        return resp;
    }

    HttpResponse::Created().json(&json!({ "invitation": row }))
}

/// `GET <base>/auth/invitations/{token}` — what a link is for, before anyone
/// commits to it.
///
/// Anonymous by design: the token *is* the credential, and the person holding
/// it has no account yet. It answers with the organisation's name, the address
/// it was sent to, and whether that address already has an account — which is
/// what tells the page whether to ask for a new password or just a click.
///
/// Nothing here is secret to the holder of the link, and nothing else is
/// returned: no member list, no inviter's address, no organisation id.
pub async fn preview_invitation(state: State<AppState>, token: Path<String>) -> HttpResponse {
    let invitation = match live_invitation(&state, &token).await {
        Ok(row) => row,
        Err(resp) => return resp,
    };

    let org = invitation
        .get("organization_id")
        .and_then(|v| v.as_str())
        .and_then(|s| Uuid::parse_str(s).ok());
    let organization = match org {
        Some(org) => organization_name(&state, org).await,
        None => state.app.display_name(),
    };
    let address = invitation
        .get("email")
        .and_then(|v| v.as_str())
        .unwrap_or_default();

    let has_account = match find_user_by_identity(&state, address).await {
        Ok(found) => found.is_some(),
        Err(resp) => return resp,
    };

    HttpResponse::Ok().json(&json!({
        "email": address,
        "organization": organization,
        "role": invitation.get("role").cloned().unwrap_or(Value::Null),
        "expires_at": invitation.get("expires_at").cloned().unwrap_or(Value::Null),
        // False means "choose a password"; true means "you already have one".
        "has_account": has_account,
        "identity_field": auth_spec(&state).identity_field,
    }))
}

/// `POST <base>/auth/invitations/{token}/accept` — take the invitation up.
///
/// Two shapes, decided by whether the address already has an account:
///
/// * **No account** — the body carries `password` (plus whatever else the
///   `user` model asks a new person for) and the account is created here. It is
///   marked as having a confirmed address without a second email: opening this
///   link is the proof that a confirmation email would have been asking for.
/// * **An account exists** — nothing is created and no password is wanted. The
///   token proves control of the address the account is registered to, which is
///   the same thing a login proves.
///
/// Either way the membership is created, the invitation is stamped as accepted
/// rather than deleted (so "who let them in" survives), and a session token
/// comes back — nobody should have to sign in immediately after proving who
/// they are.
pub async fn accept_invitation(
    req: HttpRequest,
    state: State<AppState>,
    token: Path<String>,
    body: Json<Map<String, Value>>,
) -> HttpResponse {
    let invitation = match live_invitation(&state, &token).await {
        Ok(row) => row,
        Err(resp) => return resp,
    };
    let (Some(invitation_id), Some(org), address) = (
        invitation
            .get("id")
            .and_then(|v| v.as_str())
            .and_then(|s| Uuid::parse_str(s).ok()),
        invitation
            .get("organization_id")
            .and_then(|v| v.as_str())
            .and_then(|s| Uuid::parse_str(s).ok()),
        invitation
            .get("email")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string(),
    ) else {
        return error(500, "invitation is missing its organization");
    };

    let spec = auth_spec(&state);
    let user_id = match find_user_by_identity(&state, &address).await {
        Ok(Some(id)) => id,
        Err(resp) => return resp,
        Ok(None) => {
            // A brand-new account. The address is pinned to the one the
            // invitation was sent to: letting the body name a different one
            // would turn any invitation into a free account at any address.
            let mut data = body.into_inner();
            let password = match data
                .remove("password")
                .and_then(|v| v.as_str().map(String::from))
                .filter(|p| !p.is_empty())
            {
                Some(password) => password,
                None => return error(400, "`password` is required to create your account"),
            };
            let hash = match state.auth.hash_password(&password) {
                Ok(hash) => hash,
                Err(_) => return error(500, "failed to hash password"),
            };
            data.insert(spec.identity_field.clone(), Value::String(address.clone()));
            data.insert(spec.password_field.clone(), Value::String(hash));
            // Opening a link sent to this address *is* the confirmation, so an
            // app that requires one must not then send a second.
            if state
                .app
                .resources
                .get("user")
                .is_some_and(|user| user.fields.contains_key(VERIFIED_AT_FIELD))
            {
                data.insert(VERIFIED_AT_FIELD.into(), Value::String(rfc3339_in(0)));
            }

            match crate::auth_routes::create_account(&state, &req, data).await {
                Ok((id, _)) => id,
                Err(resp) => return resp,
            }
        }
    };

    // Being invited twice, or being added by hand while the invitation sat in a
    // mailbox, must not produce two memberships.
    if let Err(resp) = ensure_membership(
        &state,
        user_id,
        org,
        invitation.get("role").and_then(|v| v.as_str()),
    )
    .await
    {
        return resp;
    }

    if let Some(invitation_tbl) = table(&state, "invitation") {
        let sql = format!("UPDATE {invitation_tbl} SET accepted_at = now() WHERE id = $1::uuid");
        if let Err(e) = state
            .db
            .raw_json(&sql, &[Value::String(invitation_id.to_string())])
            .await
        {
            return db_error(e);
        }
    }

    match state.auth.issue_token(user_id) {
        Ok(session) => HttpResponse::Ok().json(&json!({
            "token": session,
            "organization_id": org.to_string(),
        })),
        Err(_) => error(500, "failed to issue token"),
    }
}

// --- confirming an address -------------------------------------------------

/// Mint a confirmation token for `user_id` and mail it to `address`.
///
/// Called from `POST <base>/auth/register` and from the resend endpoint. The
/// `Err` is a ready-made response: a registration whose confirmation could not
/// be sent has produced an account nobody can sign in to, so it is worth
/// failing loudly rather than leaving somebody waiting for a message that was
/// never going to arrive.
pub async fn send_verification(
    state: &AppState,
    user_id: Uuid,
    address: &str,
) -> Result<(), HttpResponse> {
    if !deliverable(address) {
        return Ok(());
    }
    let ttl = state.app.config.auth.verification_ttl_secs;
    let plaintext = mint_token(state, user_id, KIND_VERIFICATION, ttl).await?;
    let message = emails::verification(
        &Links::from_app(&state.app),
        &plaintext,
        &emails::humanise(ttl),
    );
    send(state, message.to(address)).await
}

/// `POST <base>/auth/verify-email` — spend a confirmation token.
///
/// Answers with a session token: somebody who has just proved they read the
/// mailbox an account is registered to should not then be asked to sign in.
pub async fn verify_email(state: State<AppState>, body: Json<Value>) -> HttpResponse {
    let Some(token) = body.get("token").and_then(|v| v.as_str()) else {
        return error(400, "`token` is required");
    };
    let user_id = match spend_token(&state, token, KIND_VERIFICATION).await {
        Ok(id) => id,
        Err(resp) => return resp,
    };

    let Some(user_tbl) = table(&state, "user") else {
        return error(500, "missing user resource");
    };
    // `coalesce` so that confirming twice does not rewrite the date somebody
    // first confirmed on.
    let sql = format!(
        "UPDATE {user_tbl} SET {VERIFIED_AT_FIELD} = coalesce({VERIFIED_AT_FIELD}, now()) \
         WHERE id = $1::uuid"
    );
    if let Err(e) = state
        .db
        .raw_json(&sql, &[Value::String(user_id.to_string())])
        .await
    {
        return db_error(e);
    }

    match state.auth.issue_token(user_id) {
        Ok(session) => HttpResponse::Ok().json(&json!({ "token": session, "verified": true })),
        Err(_) => error(500, "failed to issue token"),
    }
}

/// `POST <base>/auth/verify-email/resend` — send the confirmation again.
///
/// Always `202`, whether or not the address has an account and whether or not
/// it was already confirmed. See the module docs: an endpoint that answers
/// truthfully here tells anybody who asks which addresses are registered.
pub async fn resend_verification(state: State<AppState>, body: Json<Value>) -> HttpResponse {
    let spec = auth_spec(&state);
    let address = body
        .get("email")
        .or_else(|| body.get(&spec.identity_field))
        .and_then(|v| v.as_str())
        .map(str::trim)
        .unwrap_or_default()
        .to_string();

    if !address.is_empty() {
        if let Ok(Some(user_id)) = find_unverified_user(&state, &address).await {
            // A failure to send is logged, not reported: the answer is the same
            // either way, and it has to be.
            if let Err(_resp) = send_verification(&state, user_id, &address).await {
                tracing::warn!("could not send a verification email");
            }
        }
    }

    accepted("If that address needs confirming, a new link is on its way.")
}

// --- resetting a password --------------------------------------------------

/// `POST <base>/auth/password/forgot` — mail a reset link.
///
/// Always `202`. See the module docs.
pub async fn forgot_password(state: State<AppState>, body: Json<Value>) -> HttpResponse {
    let spec = auth_spec(&state);
    let address = body
        .get("email")
        .or_else(|| body.get(&spec.identity_field))
        .and_then(|v| v.as_str())
        .map(str::trim)
        .unwrap_or_default()
        .to_string();

    if !address.is_empty() && deliverable(&address) {
        if let Ok(Some(user_id)) = find_user_by_identity(&state, &address).await {
            let ttl = state.app.config.auth.password_reset_ttl_secs;
            match mint_token(&state, user_id, KIND_RESET, ttl).await {
                Ok(plaintext) => {
                    let message = emails::password_reset(
                        &Links::from_app(&state.app),
                        &plaintext,
                        &emails::humanise(ttl),
                    );
                    if send(&state, message.to(&address)).await.is_err() {
                        tracing::warn!("could not send a password reset email");
                    }
                }
                Err(_) => tracing::warn!("could not mint a password reset token"),
            }
        }
    }

    accepted("If that address has an account, a reset link is on its way.")
}

/// `POST <base>/auth/password/reset` — spend a reset token and set the password.
///
/// Every other outstanding reset for the account is spent at the same time: two
/// links asked for in a moment of confusion should not leave the second one
/// working after the first has been used.
///
/// The address is marked confirmed as a side effect, because it now has been —
/// the link only reached somebody who reads it.
pub async fn reset_password(state: State<AppState>, body: Json<Value>) -> HttpResponse {
    let Some(token) = body.get("token").and_then(|v| v.as_str()) else {
        return error(400, "`token` is required");
    };
    let Some(password) = body
        .get("password")
        .and_then(|v| v.as_str())
        .filter(|p| !p.is_empty())
    else {
        return error(400, "`password` is required");
    };

    let user_id = match spend_token(&state, token, KIND_RESET).await {
        Ok(id) => id,
        Err(resp) => return resp,
    };
    let hash = match state.auth.hash_password(password) {
        Ok(hash) => hash,
        Err(_) => return error(500, "failed to hash password"),
    };

    let spec = auth_spec(&state);
    let Some(user_tbl) = table(&state, "user") else {
        return error(500, "missing user resource");
    };
    let sql = format!(
        "UPDATE {user_tbl} \
         SET {pw} = $1, {VERIFIED_AT_FIELD} = coalesce({VERIFIED_AT_FIELD}, now()) \
         WHERE id = $2::uuid",
        pw = quote(&spec.password_field),
    );
    if let Err(e) = state
        .db
        .raw_json(
            &sql,
            &[Value::String(hash), Value::String(user_id.to_string())],
        )
        .await
    {
        return db_error(e);
    }

    if let Some(token_tbl) = table(&state, "auth_token") {
        let sql = format!(
            "UPDATE {token_tbl} SET used_at = now() \
             WHERE user_id = $1::uuid AND kind = $2 AND used_at IS NULL"
        );
        let _ = state
            .db
            .raw_json(
                &sql,
                &[
                    Value::String(user_id.to_string()),
                    Value::String(KIND_RESET.into()),
                ],
            )
            .await;
    }

    match state.auth.issue_token(user_id) {
        Ok(session) => HttpResponse::Ok().json(&json!({ "token": session })),
        Err(_) => error(500, "failed to issue token"),
    }
}

// --- shared machinery ------------------------------------------------------

/// Whether `principal` may add people to `org`.
///
/// Read from the `membership` model's `create` policy so that an app which has
/// changed who manages its team gets invitations that agree with it. A policy
/// this code cannot express as a role check — `public`, say — falls back to
/// requiring `admin`, because handing out organisation membership is not
/// something to open up by accident.
fn may_invite(state: &AppState, principal: &apiplant_auth::Principal, org: Uuid) -> bool {
    invite_policy(
        state
            .app
            .resources
            .get("membership")
            .map(|membership| &membership.permissions.create),
        principal,
        org,
    )
}

/// [`may_invite`] with the policy handed in, so the rule can be checked without
/// a database behind it.
fn invite_policy(
    create: Option<&apiplant_core::Policy>,
    principal: &apiplant_auth::Principal,
    org: Uuid,
) -> bool {
    use apiplant_core::Access;
    // A class-qualified policy is only satisfied inside an organisation of that
    // class, invitations included.
    if let Some(class) = create.and_then(|p| p.org_class.as_deref()) {
        if principal.org_class_of(org) != Some(class) {
            return false;
        }
    }
    match create.map(|p| &p.level) {
        Some(Access::Role(role)) => principal.has_role_in(org, role),
        Some(Access::Member | Access::Owner | Access::Authenticated) => principal.is_member(org),
        _ => principal.is_admin_of(org),
    }
}

/// The invitation a token names, if it is still good for something.
///
/// Expired, already accepted and never existed are one answer — `404` — on
/// purpose: they are all "this link does nothing", and distinguishing them
/// would let somebody probe for which tokens once existed.
async fn live_invitation(state: &AppState, token: &str) -> Result<Value, HttpResponse> {
    let Some(invitation_tbl) = table(state, "invitation") else {
        return Err(error(500, "no invitation resource"));
    };
    let hash = Authenticator::hash_link_token(token.trim());
    let sql = format!(
        "SELECT id::text AS id, email, role, organization_id::text AS organization_id, \
                expires_at::text AS expires_at \
         FROM {invitation_tbl} \
         WHERE token_hash = $1 AND accepted_at IS NULL AND expires_at > now() \
         LIMIT 1"
    );
    let rows = state
        .db
        .raw_json(&sql, &[Value::String(hash)])
        .await
        .map_err(db_error)?;
    match rows.as_array().and_then(|rows| rows.first()) {
        Some(row) => Ok(row.clone()),
        None => Err(error(404, "this invitation is no longer valid")),
    }
}

/// Store a fresh single-use token for `user_id` and return its plaintext.
async fn mint_token(
    state: &AppState,
    user_id: Uuid,
    kind: &str,
    ttl_secs: u64,
) -> Result<String, HttpResponse> {
    let Some(token_r) = state.app.resources.get("auth_token") else {
        return Err(error(500, "no auth_token resource"));
    };
    let prefix = if kind == KIND_RESET {
        "reset"
    } else {
        "verify"
    };
    let (plaintext, hash) = Authenticator::generate_link_token(prefix);

    let mut data = Map::new();
    data.insert("user_id".into(), Value::String(user_id.to_string()));
    data.insert("kind".into(), Value::String(kind.to_string()));
    data.insert("token_hash".into(), Value::String(hash));
    data.insert(
        "expires_at".into(),
        Value::String(rfc3339_in(ttl_secs as i64)),
    );

    state
        .db
        .create(token_r, &data)
        .await
        .map_err(db_error)
        .map(|_| plaintext)
}

/// Spend a token: claim it, then read whose it was.
///
/// The claim is the `UPDATE`, and it is what makes this safe against two clicks
/// arriving together: `used_at IS NULL` is part of the `WHERE`, so exactly one
/// of them affects a row and the other affects none. Only the winner goes on to
/// look the account up, so the second click gets the same 404 as a link that
/// was never issued.
async fn spend_token(state: &AppState, token: &str, kind: &str) -> Result<Uuid, HttpResponse> {
    let Some(token_tbl) = table(state, "auth_token") else {
        return Err(error(500, "no auth_token resource"));
    };
    let hash = Authenticator::hash_link_token(token.trim());
    let params = [Value::String(hash), Value::String(kind.to_string())];

    let claim = format!(
        "UPDATE {token_tbl} SET used_at = now() \
         WHERE token_hash = $1 AND kind = $2 AND used_at IS NULL AND expires_at > now()"
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
        return Err(error(404, "this link is no longer valid"));
    }

    let lookup = format!(
        "SELECT user_id::text AS user_id FROM {token_tbl} \
         WHERE token_hash = $1 AND kind = $2 LIMIT 1"
    );
    let rows = state
        .db
        .raw_json(&lookup, &params)
        .await
        .map_err(db_error)?;

    rows.as_array()
        .and_then(|rows| rows.first())
        .and_then(|row| row.get("user_id"))
        .and_then(|v| v.as_str())
        .and_then(|s| Uuid::parse_str(s).ok())
        .ok_or_else(|| error(404, "this link is no longer valid"))
}

/// The id of the account registered at `address`, if there is one.
///
/// Case-insensitively: nobody types their own address the same way twice, and
/// an invitation to `Ann@example.com` that misses the account at
/// `ann@example.com` would create a second account for the same person.
async fn find_user_by_identity(
    state: &AppState,
    address: &str,
) -> Result<Option<Uuid>, HttpResponse> {
    let spec = auth_spec(state);
    let Some(user_tbl) = table(state, "user") else {
        return Err(error(500, "missing user resource"));
    };
    let sql = format!(
        "SELECT id::text AS id FROM {user_tbl} WHERE lower({ident}) = lower($1) LIMIT 1",
        ident = quote(&spec.identity_field),
    );
    let rows = state
        .db
        .raw_json(&sql, &[Value::String(address.to_string())])
        .await
        .map_err(db_error)?;
    Ok(rows
        .as_array()
        .and_then(|rows| rows.first())
        .and_then(|row| row.get("id"))
        .and_then(|v| v.as_str())
        .and_then(|s| Uuid::parse_str(s).ok()))
}

/// The same lookup, restricted to accounts that have *not* confirmed — so a
/// resend for an already-confirmed address quietly does nothing rather than
/// mailing a link that would do nothing.
async fn find_unverified_user(
    state: &AppState,
    address: &str,
) -> Result<Option<Uuid>, HttpResponse> {
    let spec = auth_spec(state);
    let Some(user_tbl) = table(state, "user") else {
        return Err(error(500, "missing user resource"));
    };
    let sql = format!(
        "SELECT id::text AS id FROM {user_tbl} \
         WHERE lower({ident}) = lower($1) AND {VERIFIED_AT_FIELD} IS NULL LIMIT 1",
        ident = quote(&spec.identity_field),
    );
    let rows = state
        .db
        .raw_json(&sql, &[Value::String(address.to_string())])
        .await
        .map_err(db_error)?;
    Ok(rows
        .as_array()
        .and_then(|rows| rows.first())
        .and_then(|row| row.get("id"))
        .and_then(|v| v.as_str())
        .and_then(|s| Uuid::parse_str(s).ok()))
}

/// Whether the account at `address` is already in `org`.
async fn already_a_member(
    state: &AppState,
    address: &str,
    org: Uuid,
) -> Result<bool, HttpResponse> {
    let Some(user_id) = find_user_by_identity(state, address).await? else {
        return Ok(false);
    };
    let Some(membership_tbl) = table(state, "membership") else {
        return Ok(false);
    };
    let sql = format!(
        "SELECT 1 AS hit FROM {membership_tbl} \
         WHERE user_id = $1::uuid AND organization_id = $2::uuid LIMIT 1"
    );
    let rows = state
        .db
        .raw_json(
            &sql,
            &[
                Value::String(user_id.to_string()),
                Value::String(org.to_string()),
            ],
        )
        .await
        .map_err(db_error)?;
    Ok(rows.as_array().is_some_and(|rows| !rows.is_empty()))
}

/// Put `user_id` in `org` with `role`, unless they are in it already.
///
/// Written straight rather than through `POST <base>/membership`, which the
/// person accepting could not call: they are not a member yet, which is the
/// whole point.
async fn ensure_membership(
    state: &AppState,
    user_id: Uuid,
    org: Uuid,
    role: Option<&str>,
) -> Result<(), HttpResponse> {
    let Some(membership_r) = state.app.resources.get("membership") else {
        return Err(error(500, "no membership resource"));
    };
    let Some(membership_tbl) = table(state, "membership") else {
        return Err(error(500, "no membership resource"));
    };

    let sql = format!(
        "SELECT 1 AS hit FROM {membership_tbl} \
         WHERE user_id = $1::uuid AND organization_id = $2::uuid LIMIT 1"
    );
    let rows = state
        .db
        .raw_json(
            &sql,
            &[
                Value::String(user_id.to_string()),
                Value::String(org.to_string()),
            ],
        )
        .await
        .map_err(db_error)?;
    if rows.as_array().is_some_and(|rows| !rows.is_empty()) {
        return Ok(());
    }

    let mut data = Map::new();
    data.insert("user_id".into(), Value::String(user_id.to_string()));
    data.insert("organization_id".into(), Value::String(org.to_string()));
    if let Some(role) = role.filter(|role| !role.is_empty()) {
        data.insert("role".into(), Value::String(role.to_string()));
    }
    state
        .db
        .create(membership_r, &data)
        .await
        .map_err(db_error)
        .map(|_| ())
}

/// The organisation's own name, for the sentence in the email. Falls back to
/// the app's name rather than to an id, which would mean nothing to a reader.
async fn organization_name(state: &AppState, org: Uuid) -> String {
    let fallback = || state.app.display_name();
    let Some(org_tbl) = table(state, "organization") else {
        return fallback();
    };
    let sql = format!("SELECT name FROM {org_tbl} WHERE id = $1::uuid LIMIT 1");
    let rows = match state
        .db
        .raw_json(&sql, &[Value::String(org.to_string())])
        .await
    {
        Ok(rows) => rows,
        Err(_) => return fallback(),
    };
    rows.as_array()
        .and_then(|rows| rows.first())
        .and_then(|row| row.get("name"))
        .and_then(|v| v.as_str())
        .filter(|name| !name.is_empty())
        .map(str::to_owned)
        .unwrap_or_else(fallback)
}

/// How to refer to the person who sent an invitation: their display name if the
/// app keeps one, otherwise the address they signed in with, otherwise nobody.
async fn inviter_name(state: &AppState, user_id: Uuid) -> Option<String> {
    let spec = auth_spec(state);
    let user_r = state.app.resources.get("user")?;
    let user_tbl = table(state, "user")?;

    let mut candidates: Vec<String> = Vec::new();
    for name in ["display_name", "name"] {
        if user_r.fields.contains_key(name) {
            candidates.push(quote(name));
        }
    }
    candidates.push(quote(&spec.identity_field));

    let sql = format!(
        "SELECT coalesce({}) AS who FROM {user_tbl} WHERE id = $1::uuid LIMIT 1",
        candidates.join(", "),
    );
    let rows = state
        .db
        .raw_json(&sql, &[Value::String(user_id.to_string())])
        .await
        .ok()?;
    rows.as_array()?
        .first()?
        .get("who")?
        .as_str()
        .filter(|who| !who.is_empty())
        .map(str::to_owned)
}

/// Hand a composed message to the app's mailer.
///
/// The `Err` is a `502`, not a `500`: the request was fine and this server is
/// fine — somebody else's service said no, and that is worth distinguishing in
/// whatever is reading the logs.
async fn send(state: &AppState, message: apiplant_email::Message) -> Result<(), HttpResponse> {
    let Some(mailer) = &state.mailer else {
        // Unreachable through a route, which is only mounted with a mailer —
        // but a wrong 502 beats a panic if that ever stops being true.
        return Err(error(502, "this server cannot send email"));
    };
    match mailer.send(&message).await {
        Ok(_) => Ok(()),
        Err(e) => {
            tracing::error!(error = %e, "could not send an email");
            Err(error(502, "could not send the email — try again shortly"))
        }
    }
}

/// Whether it is worth handing this address to a mailer.
///
/// `.invalid` is reserved by RFC 2606 precisely so that it can never resolve,
/// which makes delivery to it not unlikely but impossible — it is the domain
/// apiplant synthesises an address at when a provider gives none (see the
/// `user` model's `email_placeholder`). Asking a provider to deliver there
/// wastes a call and, in bulk, spends the sender's reputation on mail that
/// cannot arrive.
///
/// Deliberately a fact about the address rather than a lookup of the flag: the
/// two places this is asked have an address in hand and not a row, and "that
/// domain cannot exist" is true however the address got there.
fn deliverable(address: &str) -> bool {
    let deliverable = !address
        .rsplit_once('@')
        .map(|(_, domain)| {
            let domain = domain.trim().trim_end_matches('.').to_lowercase();
            domain == "invalid" || domain.ends_with(".invalid")
        })
        .unwrap_or(false);
    if !deliverable {
        tracing::debug!("not mailing an address at a reserved `.invalid` domain");
    }
    deliverable
}

/// `202` with a message that says nothing about who exists.
fn accepted(message: &str) -> HttpResponse {
    HttpResponse::Accepted().json(&json!({ "message": message }))
}

/// An RFC 3339 timestamp `secs` from now, in the form
/// [`json_to_sql`](apiplant_db) wants for a `timestamp` column.
fn rfc3339_in(secs: i64) -> String {
    let at: DateTime<Utc> = Utc::now() + Duration::seconds(secs);
    at.to_rfc3339()
}

#[cfg(test)]
mod tests {
    use super::*;
    use apiplant_core::Access;

    fn principal(org: Uuid, role: &str) -> apiplant_auth::Principal {
        apiplant_auth::Principal {
            user_id: Uuid::new_v4(),
            organizations: vec![apiplant_auth::OrgMembership::new(
                org,
                Some(role.to_string()),
                [],
            )],
        }
    }

    #[test]
    fn who_may_invite_follows_who_may_add_a_member() {
        let org = Uuid::new_v4();

        // The default: admins of the organisation, and nobody else in it.
        let admins: apiplant_core::Policy = Access::Role("admin".into()).into();
        assert!(invite_policy(Some(&admins), &principal(org, "admin"), org));
        assert!(!invite_policy(
            Some(&admins),
            &principal(org, "member"),
            org
        ));

        // An app that lets any member add people gets invitations to match,
        // rather than a second, stricter answer to the same question.
        let members: apiplant_core::Policy = Access::Member.into();
        assert!(invite_policy(
            Some(&members),
            &principal(org, "member"),
            org
        ));

        // …but never for an organisation you are not in.
        assert!(!invite_policy(
            Some(&members),
            &principal(org, "member"),
            Uuid::new_v4()
        ));
    }

    #[test]
    fn a_policy_that_is_not_a_role_check_falls_back_to_admin() {
        let org = Uuid::new_v4();
        // Handing out membership of an organisation is not something to open
        // up because a permission happened to say `public`, and a model with
        // no `membership` at all is not an invitation to improvise.
        let public: apiplant_core::Policy = Access::Public.into();
        for policy in [Some(&public), None] {
            assert!(!invite_policy(policy, &principal(org, "member"), org));
            assert!(invite_policy(policy, &principal(org, "admin"), org));
        }
    }
}
