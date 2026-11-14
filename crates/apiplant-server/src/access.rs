//! Answering "may this caller do this", in the one grammar the whole framework
//! uses.
//!
//! A resource's `[permissions]`, a function's manifest and `[ai] access` all
//! spell access the same way — `public`, `authenticated`, `member`,
//! `role:<name>`, `private` — so the check that enforces it lives here rather
//! than once per endpoint. Two callers today ([functions](crate::function_routes)
//! and [the assistant](crate::ai_routes)); the value is that neither can drift
//! from the other.

use apiplant_abi::{FunctionAccess, FunctionPolicy};
use apiplant_auth::Principal;
use apiplant_core::{Access, Policy};
use ntex::web::{HttpRequest, HttpResponse};

use crate::response::error;
use crate::state::AppState;

/// [`check`], for a permission in the resource grammar.
///
/// The two grammars differ only by `owner`, which needs a row to compare
/// against and so is `private` here. This is the door code outside the crate
/// comes through, via [`crate::Caller::require`].
pub async fn check_policy(
    state: &AppState,
    req: &HttpRequest,
    policy: &Policy,
    missing: &str,
) -> Result<Option<Principal>, HttpResponse> {
    check(state, req, &to_function_policy(policy), missing).await
}

/// Resolve the caller and check them against `access`.
///
/// `Ok` carries the principal — `None` for an anonymous caller of a `public`
/// endpoint. `Err` is the response to send, already the right status: `401`
/// when credentials would help, `403` when they wouldn't, and `404` for
/// `private`, which is not merely forbidden but not there.
pub async fn check(
    state: &AppState,
    req: &HttpRequest,
    policy: &FunctionPolicy,
    missing: &str,
) -> Result<Option<Principal>, HttpResponse> {
    let principal = state.resolve_principal(req).await;
    let access = &policy.access;

    // The deployment's own administrators, exactly as in [`crate::crud`]: the
    // role and the organisation stop being questions, and `private` does not.
    // A function nobody may call is not a function they may call either.
    if !matches!(access, FunctionAccess::Private) && state.is_global_admin(principal.as_ref()) {
        return Ok(principal);
    }

    // A `@org_class=` qualifier narrows every level, including the ones that
    // name no organisation: it says the caller must be acting inside an
    // organisation of that class, so `public@org_class=staff` is a members-only
    // endpoint, not a public one.
    if let Some(class) = policy.org_class.as_deref() {
        if !matches!(access, FunctionAccess::Private) {
            let membership = principal.as_ref().and_then(|p| {
                state
                    .active_org(req, &principal)
                    .and_then(|org| p.membership(org))
            });
            match membership {
                Some(m) if m.is_class(class) => {}
                Some(_) => {
                    return Err(error(
                        403,
                        format!("requires an organisation of class `{class}`"),
                    ))
                }
                None if principal.is_some() => {
                    return Err(error(
                        403,
                        "select an organisation with the X-Organization header",
                    ))
                }
                None => return Err(error(401, "authentication required")),
            }
        }
    }

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

/// The resource grammar (`[queues] publish`, [`crate::Caller::require`]) in
/// the function one, which is what [`check`] reads. They differ only by `owner`, which
/// [`QueuesConfig::publish_access`](apiplant_core::QueuesConfig::publish_access)
/// has already ruled out because a topic has no owner column to compare against.
pub(crate) fn to_function_policy(policy: &Policy) -> FunctionPolicy {
    FunctionPolicy {
        access: to_function_access(&policy.level),
        org_class: policy.org_class.clone(),
    }
}

fn to_function_access(access: &Access) -> FunctionAccess {
    match access {
        Access::Public => FunctionAccess::Public,
        Access::Authenticated => FunctionAccess::Authenticated,
        Access::Member => FunctionAccess::Member,
        Access::Role(role) => FunctionAccess::Role(role.clone()),
        Access::Owner | Access::Private => FunctionAccess::Private,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The two grammars have to agree, or an app's `publish = "role:admin"`
    /// would silently enforce something else.
    #[test]
    fn the_two_access_grammars_line_up() {
        assert_eq!(to_function_access(&Access::Public), FunctionAccess::Public);
        assert_eq!(
            to_function_access(&Access::Authenticated),
            FunctionAccess::Authenticated
        );
        assert_eq!(to_function_access(&Access::Member), FunctionAccess::Member);
        assert_eq!(
            to_function_access(&Access::Role("admin".into())),
            FunctionAccess::Role("admin".into())
        );
        assert_eq!(
            to_function_access(&Access::Private),
            FunctionAccess::Private
        );
        // `owner` names a column on a row; a topic hasn't got one, so it closes
        // the door rather than meaning something arbitrary.
        assert_eq!(to_function_access(&Access::Owner), FunctionAccess::Private);
    }
}
