//! Answering "may this caller do this", in the one grammar the whole framework
//! uses.
//!
//! A resource's `[permissions]`, a function's manifest and `[ai] access` all
//! spell access the same way — `public`, `authenticated`, `member`,
//! `role:<name>`, `private` — so the check that enforces it lives here rather
//! than once per endpoint. Two callers today ([functions](crate::function_routes)
//! and [the assistant](crate::ai_routes)); the value is that neither can drift
//! from the other.

use apiplant_abi::FunctionAccess;
use apiplant_auth::Principal;
use ntex::web::types::State;
use ntex::web::{HttpRequest, HttpResponse};

use crate::response::error;
use crate::state::AppState;

/// Resolve the caller and check them against `access`.
///
/// `Ok` carries the principal — `None` for an anonymous caller of a `public`
/// endpoint. `Err` is the response to send, already the right status: `401`
/// when credentials would help, `403` when they wouldn't, and `404` for
/// `private`, which is not merely forbidden but not there.
pub async fn check(
    state: &State<AppState>,
    req: &HttpRequest,
    access: &FunctionAccess,
    missing: &str,
) -> Result<Option<Principal>, HttpResponse> {
    let principal = state.resolve_principal(req).await;

    match access {
        FunctionAccess::Public => {}
        // Not "you may not", but "there is nothing here" — so probing cannot
        // enumerate what exists.
        FunctionAccess::Private => return Err(error(404, missing.to_string())),
        FunctionAccess::Authenticated => {
            if principal.is_none() {
                return Err(error(401, "authentication required"));
            }
        }
        // `member` and `role:` are both organisation-scoped: they need an
        // active organisation the caller actually belongs to.
        FunctionAccess::Member | FunctionAccess::Role(_) => {
            if principal.is_none() {
                return Err(error(401, "authentication required"));
            }
            // `active_org` already refuses an organisation the caller does not
            // belong to, so membership is settled by having one at all — a
            // member with no role is still a member.
            let org = state.active_org(req, &principal);
            let ok = match (access, org, principal.as_ref()) {
                (FunctionAccess::Member, Some(org), Some(caller)) => caller.is_member(org),
                (FunctionAccess::Role(required), Some(org), Some(caller)) => {
                    // Any of the caller's roles will do, and an admin holds all.
                    caller.has_role_in(org, required)
                }
                _ => false,
            };
            if !ok {
                return Err(error(403, "forbidden"));
            }
        }
    }
    Ok(principal)
}
