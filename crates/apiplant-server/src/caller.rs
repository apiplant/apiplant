//! Who is calling, for routes that are not apiplant's own.
//!
//! A program that embeds apiplant (see [`crate::Platform`]) mounts handlers of
//! its own beside the generated API, and those handlers need the same answers
//! apiplant's do: who is this, which organisation are they standing in, may
//! they do this. [`Caller`] is that answer as an ntex extractor, resolved the
//! one way every apiplant route resolves it: bearer token, API key, or the
//! session cookie, memberships read fresh, `X-Organization` validated against
//! them. [`Caller::require`] applies a permission in the grammar resources use
//! (`public`, `authenticated`, `member`, `role:<name>`, `private`, each with an
//! optional `@org_class=`), through the same check functions and the assistant
//! use.
//!
//! ```ignore
//! async fn list_files(caller: Caller, state: State<AppState>, req: HttpRequest) -> HttpResponse {
//!     if let Err(refusal) = caller.require(&state, &req, "member").await {
//!         return refusal;
//!     }
//!     let org = caller.active_org.unwrap(); // `member` guarantees one
//!     …
//! }
//! ```

use std::convert::Infallible;

use apiplant_auth::{OrgMembership, Principal};
use apiplant_core::Policy;
use ntex::http::Payload;
use ntex::web::{FromRequest, HttpRequest, HttpResponse};
use uuid::Uuid;

use crate::state::{AppState, Shared};

/// The resolved caller of one request.
#[derive(Debug, Clone, Default)]
pub struct Caller {
    /// Who is calling, with every organisation they belong to. `None` for an
    /// anonymous request.
    pub principal: Option<Principal>,
    /// The organisation the request acts in: `X-Organization`, accepted only
    /// if the caller belongs to it (or is a global admin), or the one an
    /// impersonated session is pinned to. Never guessed.
    pub active_org: Option<Uuid>,
    /// Whether the caller is one of the deployment's own administrators
    /// (`[organization] global_admin_role`).
    pub global_admin: bool,
}

impl Caller {
    /// Resolve the caller of `req`.
    pub async fn resolve(state: &AppState, req: &HttpRequest) -> Caller {
        let principal = state.resolve_principal(req).await;
        let active_org = state.active_org(req, &principal);
        let global_admin = state.is_global_admin(principal.as_ref());
        Caller {
            principal,
            active_org,
            global_admin,
        }
    }

    /// The signed-in user, if any.
    pub fn user_id(&self) -> Option<Uuid> {
        self.principal.as_ref().map(|p| p.user_id)
    }

    /// Whether anybody is signed in.
    pub fn is_authenticated(&self) -> bool {
        self.principal.is_some()
    }

    /// The caller's membership in the active organisation.
    pub fn membership(&self) -> Option<&OrgMembership> {
        let org = self.active_org?;
        self.principal.as_ref()?.membership(org)
    }

    /// Whether the caller holds `role` in the active organisation. An admin
    /// holds every role.
    pub fn has_role(&self, role: &str) -> bool {
        self.membership().is_some_and(|m| m.has_role(role))
    }

    /// Check a permission written in the resource grammar.
    ///
    /// `Err` is the response to send, with the status apiplant's own routes
    /// use: `401` when signing in would help, `403` when it would not, `404`
    /// for `private`. `owner` is treated as `private` here, because there is no
    /// row to own. Compare the owner column yourself when you need it.
    ///
    /// Resolves the caller again from `req`, so it is correct on its own; the
    /// extracted `Caller` is for reading who it was.
    pub async fn require(
        &self,
        state: &AppState,
        req: &HttpRequest,
        policy: &str,
    ) -> Result<(), HttpResponse> {
        crate::access::check_policy(state, req, &Policy::parse(policy), "not found")
            .await
            .map(|_| ())
    }
}

impl FromRequest<Shared> for Caller {
    type Error = Infallible;

    /// Never fails: an anonymous request is a `Caller` with no principal.
    async fn from_request(
        st: &Shared,
        req: &HttpRequest,
        _: &mut Payload,
    ) -> Result<Self, Self::Error> {
        Ok(Caller::resolve(&st.0, req).await)
    }
}
