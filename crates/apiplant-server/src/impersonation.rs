//! Acting as somebody else, and stopping.
//!
//! Two endpoints, mounted beside the rest of auth:
//!
//! | Route | What it does |
//! |-------|--------------|
//! | `POST <base>/auth/impersonate` | hands back a token that acts as another account |
//! | `POST <base>/auth/impersonate/stop` | hands back your own token again |
//!
//! There are two doors into the first, and they are deliberately different
//! sizes:
//!
//! * **An organisation's admin**, when `[auth] allow_impersonation` is on — the
//!   default. They may borrow a member of the organisation they administer, and
//!   the token they get is *pinned* to it: no `X-Organization` header moves it,
//!   and none of the borrowed account's other memberships are loaded at all. A
//!   member of two organisations cannot have one admin see the other, which is
//!   the property that makes this safe to leave on.
//!
//! * **The back office**, whoever `[organization] global_admin_role` names —
//!   nobody, unless an app says so. They may borrow anybody, in any
//!   organisation, and their session is not pinned, because moving around the
//!   account's organisations is what support access is for.
//!
//! Both mint an ordinary session JWT carrying two extra claims — who is really
//! behind it, and the pin — so the fact that a request is an impersonation
//! travels with the credential rather than living in a table the next request
//! would have to consult. Nothing here is nestable: a borrowed session cannot
//! borrow again, so `act` always names a real person.

use apiplant_auth::{Principal, Session};
use ntex::web::types::{Json, State};
use ntex::web::{HttpRequest, HttpResponse};
use serde_json::{json, Value};
use uuid::Uuid;

use crate::auth_routes::table;
use crate::response::{db_error, error};
use crate::state::AppState;

/// What the two endpoints answer with: the credential, and enough about it for
/// a client to draw the banner that says whose account this is.
fn session_response(session: Session) -> Value {
    json!({
        "user_id": session.user_id.to_string(),
        "impersonator": session.impersonator.map(|id| id.to_string()),
        "organization_id": session.org_lock.map(|id| id.to_string()),
    })
}

/// `POST <base>/auth/impersonate` — take a token that acts as `user_id`.
pub async fn start(req: HttpRequest, state: State<AppState>, body: Json<Value>) -> HttpResponse {
    let Some(principal) = state.resolve_principal(&req).await else {
        return error(401, "authentication required");
    };
    // Not nestable, and refused rather than ignored: somebody already wearing
    // an account is not in a position to judge whose it should be next, and an
    // `act` claim that could name another impersonation would make "who did
    // this" a chain to walk rather than a name to read.
    if principal.is_impersonating() {
        return error(409, "you are already acting as somebody else — stop first");
    }
    let Some(target) = body.get("user_id").and_then(|v| v.as_str()) else {
        return error(400, "`user_id` is required");
    };
    let Ok(target) = Uuid::parse_str(target.trim()) else {
        return error(400, "`user_id` is not a valid id");
    };
    if target == principal.user_id {
        return error(400, "you are already yourself");
    }

    let session = match authorize(&state, &req, &principal, target).await {
        Ok(session) => session,
        Err(resp) => return resp,
    };
    // Checked after the permission, not before: whether an account exists is
    // not something an unauthorised caller gets to learn by asking.
    match user_exists(&state, target).await {
        Ok(true) => {}
        Ok(false) => return error(404, "no such user"),
        Err(resp) => return resp,
    }

    let Ok(token) = state.auth.issue_session(session) else {
        return error(500, "could not issue a token");
    };
    tracing::info!(
        actor = %principal.user_id,
        subject = %target,
        organization = ?session.org_lock,
        "impersonation started"
    );
    let mut response = session_response(session);
    response["token"] = Value::String(token);
    HttpResponse::Ok().json(&response)
}

/// `POST <base>/auth/impersonate/stop` — go back to being yourself.
///
/// The token is minted for whoever the current one names as its actor, so the
/// way back needs no stored state and no second credential kept in a browser:
/// the borrowed session carries the identity it was borrowed by.
pub async fn stop(req: HttpRequest, state: State<AppState>) -> HttpResponse {
    let Some(principal) = state.resolve_principal(&req).await else {
        return error(401, "authentication required");
    };
    let Some(actor) = principal.impersonator else {
        return error(400, "you are not acting as anybody else");
    };
    // The account may have been deleted while it was lent out, in which case
    // there is nothing to go back to and the session simply ends.
    match user_exists(&state, actor).await {
        Ok(true) => {}
        Ok(false) => return error(401, "your own account no longer exists"),
        Err(resp) => return resp,
    }

    let session = Session::plain(actor);
    let Ok(token) = state.auth.issue_session(session) else {
        return error(500, "could not issue a token");
    };
    tracing::info!(actor = %actor, subject = %principal.user_id, "impersonation ended");
    let mut response = session_response(session);
    response["token"] = Value::String(token);
    HttpResponse::Ok().json(&response)
}

/// Which door this caller comes through, and therefore what session they get.
///
/// The deployment-wide grant is tried first because it is the wider one: a
/// global admin who happens also to administer the organisation they are
/// reaching into should get the unpinned session the back office promises, not
/// the narrow one they would have got as an ordinary admin.
async fn authorize(
    state: &AppState,
    req: &HttpRequest,
    principal: &Principal,
    target: Uuid,
) -> Result<Session, HttpResponse> {
    if state.is_global_admin(Some(principal)) {
        return Ok(Session {
            user_id: target,
            impersonator: Some(principal.user_id),
            org_lock: None,
        });
    }

    if !state.app.config.auth.allow_impersonation {
        return Err(error(403, "impersonation is switched off"));
    }
    let Some(org) = state.active_org(req, &Some(principal.clone())) else {
        return Err(error(
            403,
            "select an organisation with the X-Organization header",
        ));
    };
    if !principal.is_admin_of(org) {
        return Err(error(
            403,
            "only an admin of this organisation may act as one of its members",
        ));
    }
    // Membership is the boundary: an admin borrows an account *in their
    // organisation*, so somebody who is not in it is not theirs to borrow —
    // and the pin below is what stops the borrowed account from carrying them
    // anywhere else.
    if !state.organization_user_ids(org).await.contains(&target) {
        return Err(error(
            403,
            "that person is not a member of this organisation",
        ));
    }
    Ok(Session {
        user_id: target,
        impersonator: Some(principal.user_id),
        org_lock: Some(org),
    })
}

/// Whether a user row is still there.
async fn user_exists(state: &AppState, id: Uuid) -> Result<bool, HttpResponse> {
    let Some(user_tbl) = table(state, "user") else {
        return Err(error(500, "missing user resource"));
    };
    let sql = format!("SELECT id::text AS id FROM {user_tbl} WHERE id = $1::uuid LIMIT 1");
    match state
        .db
        .raw_json(&sql, &[Value::String(id.to_string())])
        .await
    {
        Ok(rows) => Ok(rows.as_array().is_some_and(|rows| !rows.is_empty())),
        Err(e) => Err(db_error(e)),
    }
}
