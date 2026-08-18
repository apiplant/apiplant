//! Shared server state, caller-identity resolution, and active-organisation
//! resolution.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use apiplant_ai::Ai;
use apiplant_auth::{Authenticator, OrgMembership, Principal, Session as AuthSession};
use apiplant_cache::Cache;
use apiplant_core::{Access, App, Policy, ORG_CLASS_FIELD};
use apiplant_db::Db;
use apiplant_email::Mailer;
use apiplant_oauth::Providers;
use apiplant_payments::Payments;
use apiplant_queue::Queue;
use apiplant_storage::Storage;
use ntex::web::HttpRequest;
use uuid::Uuid;

use crate::functions::FunctionRegistry;

/// Immutable state shared across every worker and request.
#[derive(Clone)]
pub struct AppState {
    pub app: Arc<App>,
    pub db: Db,
    pub auth: Authenticator,
    pub functions: Arc<FunctionRegistry>,
    /// The app's email provider, when `[email]` names one. Functions reach it
    /// through `send_email`; nothing else in the server sends mail.
    pub mailer: Option<Mailer>,
    /// The app's own wording for the messages the framework sends, compiled at
    /// boot from its `emails/` directory. Empty in the app that has none, which
    /// is the ordinary case — the built-in messages are used then.
    pub email_templates: Arc<crate::email_templates::EmailTemplates>,
    /// The app's Redis cache, when `[cache]` names one. Functions reach it
    /// through `cache`; no framework path caches through it.
    pub cache: Option<Cache>,
    /// The app's payment provider, when `[payments]` names one. Behind the
    /// `/billing` endpoints, the `billing_*` hooks and a function's
    /// `payments` call; nothing else in the server takes money.
    pub payments: Option<Payments>,
    /// The app's AI assistant, when `[ai]` names a provider. Behind the
    /// `/ai/chat` endpoint and a function's `chat` call; nothing else in the
    /// server talks to a model.
    pub ai: Option<Ai>,
    /// The providers `[oauth]` switched on, when it named any. Behind the
    /// `<base>/auth/oauth` endpoints and nothing else — no other path in the
    /// server talks to an identity provider.
    pub oauth: Option<Arc<Providers>>,
    /// Per-agent AI assistants whose config overrides the app-wide `[ai]`.
    pub agent_ais: Arc<HashMap<String, Ai>>,
    /// The app's file store, when `[storage]` names a backend. Behind the
    /// `<base>/uploads` endpoint and the `file` field type, and nothing else —
    /// no other path in the server writes to disk or to a bucket.
    pub storage: Option<Storage>,
    /// The app's message queue. Not optional and never absent: `publish` needs
    /// no configuration, because `queue_message` is a built-in resource every
    /// app has. What `[queues]` configures is the *subscriber* half.
    ///
    /// Behind a function's `publish`, the `<base>/queues/{topic}` endpoint and
    /// a resource's `[publish]` declarations.
    pub queue: Queue,
    /// How often one client may call each endpoint, resolved on boot from
    /// `[rate_limit]` and every override beside it. Enforced by
    /// [`crate::rate_limit`], which wraps the whole API scope.
    pub rate_limit: Arc<crate::rate_limit::RateLimitPolicy>,
    /// What is measured about a request and where it is sent, resolved on boot
    /// from `[observability]`. Enforced by [`crate::telemetry`], which wraps
    /// the API scope outside the rate limiter so a throttled request is still
    /// a request that shows up in the numbers.
    pub telemetry: Arc<crate::telemetry::TelemetryPolicy>,
    /// Everything served alongside the API: the dashboard and the public site.
    pub statics: Arc<Statics>,
    /// The admin manifest for this app, built on boot.
    pub admin_manifest: Arc<String>,
    /// Pre-rendered OpenAPI document (JSON).
    pub openapi_json: Arc<String>,
    /// Pre-rendered Swagger UI page.
    pub docs_html: Arc<String>,
}

/// What the server serves besides the API: the admin dashboard, the app's
/// `public/` directory, and the page for requests that match nothing.
///
/// Resolved once, on boot, so every worker registers the same routes — and so
/// the route table is decided in one place rather than at request time.
#[derive(Debug, Default, Clone)]
pub struct Statics {
    /// Path the dashboard is mounted at, or `None` when it's switched off.
    pub admin_path: Option<String>,
    /// Static site root (`public/`) when the app has one.
    pub public_dir: Option<PathBuf>,
    /// Route patterns for the files in it, one entry per URL they answer on.
    pub public_routes: Vec<String>,
    /// Page to answer unmatched requests with.
    pub not_found_page: Option<PathBuf>,
    /// URL prefix stored files answer on (`/files`), when the app stores any.
    pub storage_base: Option<String>,
}

impl Statics {
    /// Work out what a loaded app serves statically.
    pub fn resolve(app: &App) -> Statics {
        let admin_path = app
            .config
            .admin
            .enabled
            .then(|| app.config.admin.path.clone());
        let public_dir = app.root.join(&app.config.public.dir);
        let public_dir = (app.config.public.enabled && public_dir.is_dir()).then_some(public_dir);

        let mut public_routes = Vec::new();
        let mut not_found_page = None;
        if let Some(root) = &public_dir {
            let mut files = Vec::new();
            crate::walk_public(root, "", &mut files);
            files.sort();
            public_routes = files.iter().flat_map(|f| crate::public_routes(f)).collect();

            // A 404 page is opt-out, not opt-in: `404.html` is what people
            // already call the file, so finding one is enough to use it.
            let candidate = app.config.public.not_found.as_deref().unwrap_or("404.html");
            let page = root.join(candidate);
            if page.is_file() {
                not_found_page = Some(page);
            } else if app.config.public.not_found.is_some() {
                tracing::warn!(
                    path = %page.display(),
                    "public.not_found points at a file that doesn't exist"
                );
            }
        }

        Statics {
            storage_base: app
                .config
                .storage
                .is_active()
                .then(|| app.config.storage.normalized_public_base()),
            admin_path,
            public_dir,
            public_routes,
            not_found_page,
        }
    }
}

impl AppState {
    /// Resolve the caller (identity + organisation memberships) from the request.
    ///
    /// Identity comes from `Authorization: Bearer <jwt>`, `Authorization: ApiKey
    /// <key>`, or `X-Api-Key: <key>`. Memberships (and the caller's role in each
    /// organisation) are loaded fresh from the database so changes take effect
    /// immediately. Anonymous callers resolve to `None`.
    pub async fn resolve_principal(&self, req: &HttpRequest) -> Option<Principal> {
        let session = self.resolve_session(req).await?;
        let mut organizations = self.load_memberships(session.user_id).await;
        // A pinned session sees one organisation and no other. Done by
        // *dropping* the rest rather than by consulting the lock at each use:
        // every `role:` check, every tenant filter and every hook context is
        // built from this list, so a membership that is not in it cannot be
        // reached by something that forgot to ask about the lock.
        if let Some(locked) = session.org_lock {
            organizations.retain(|membership| membership.org_id == locked);
        }
        Some(Principal {
            user_id: session.user_id,
            organizations,
            impersonator: session.impersonator,
            org_lock: session.org_lock,
        })
    }

    /// The session behind a request: who it acts as, and whether somebody else
    /// is acting as them.
    ///
    /// An API key is always a plain session — a key belongs to an account, and
    /// no door issues an impersonating one.
    pub(crate) async fn resolve_session(&self, req: &HttpRequest) -> Option<AuthSession> {
        if let Some(key) = req.headers().get("x-api-key").and_then(|v| v.to_str().ok()) {
            if !key.is_empty() {
                return self
                    .user_id_from_api_key(key.trim())
                    .await
                    .map(AuthSession::plain);
            }
        }
        let header = req.headers().get("authorization")?.to_str().ok()?;
        if let Some(token) = header.strip_prefix("Bearer ") {
            return self.auth.verify_session(token.trim()).ok();
        }
        if let Some(key) = header.strip_prefix("ApiKey ") {
            return self
                .user_id_from_api_key(key.trim())
                .await
                .map(AuthSession::plain);
        }
        None
    }

    async fn user_id_from_api_key(&self, key: &str) -> Option<Uuid> {
        let hash = Authenticator::hash_api_key(key);
        let api_key_tbl = self.table("api_key")?;
        let sql = format!(
            "SELECT owner_id::text AS uid FROM {api_key_tbl} WHERE token_hash = $1 LIMIT 1"
        );
        let rows = self
            .db
            .raw_json(&sql, &[serde_json::Value::String(hash)])
            .await
            .ok()?;
        let row = rows.as_array()?.first()?;
        Uuid::parse_str(row.get("uid")?.as_str()?).ok()
    }

    /// Load the caller's organisation memberships, with every role they hold
    /// in each.
    ///
    /// Roles come from two places — the membership's own primary `role` column
    /// and its `membership_role` rows — and both are read here, once per
    /// request, so a role granted or revoked takes effect on the next call
    /// rather than whenever a token happens to expire.
    async fn load_memberships(&self, user_id: Uuid) -> Vec<OrgMembership> {
        let Some(membership_tbl) = self.table("membership") else {
            return Vec::new();
        };

        // The organisation's class rides along on every membership: a
        // `@org_class=` permission is answered from the principal, so it must
        // not cost a query of its own. An app that replaced the built-in
        // `organization` model without the column simply has no classes.
        let class_select = self.org_class_join();

        // An app is free to drop the built-in `membership_role` resource, in
        // which case the primary role is all there is.
        let sql = match self.table("membership_role") {
            Some(role_tbl) => format!(
                "SELECT m.organization_id::text AS org, m.role AS role, r.role AS extra{select} \
                 FROM {membership_tbl} m \
                 LEFT JOIN {role_tbl} r ON r.membership_id = m.id{join} \
                 WHERE m.user_id = $1::uuid",
                select = class_select.0,
                join = class_select.1,
            ),
            None => format!(
                "SELECT m.organization_id::text AS org, m.role AS role, NULL AS extra{select} \
                 FROM {membership_tbl} m{join} WHERE m.user_id = $1::uuid",
                select = class_select.0,
                join = class_select.1,
            ),
        };

        let rows = match self
            .db
            .raw_json(&sql, &[serde_json::Value::String(user_id.to_string())])
            .await
        {
            Ok(v) => v,
            Err(_) => return Vec::new(),
        };

        // The join returns one row per role, so the organisations have to be
        // folded back together — in first-seen order, so the result does not
        // reshuffle between requests.
        let mut order: Vec<Uuid> = Vec::new();
        let mut primary: HashMap<Uuid, Option<String>> = HashMap::new();
        let mut extras: HashMap<Uuid, Vec<String>> = HashMap::new();
        let mut classes: HashMap<Uuid, Option<String>> = HashMap::new();

        for row in rows.as_array().map(Vec::as_slice).unwrap_or_default() {
            let Some(org) = row
                .get("org")
                .and_then(|v| v.as_str())
                .and_then(|s| Uuid::parse_str(s).ok())
            else {
                continue;
            };
            // The primary role repeats on every joined row; the first one is
            // the one, and it also fixes the organisation's place in the order.
            if let std::collections::hash_map::Entry::Vacant(slot) = primary.entry(org) {
                order.push(org);
                slot.insert(
                    row.get("role")
                        .and_then(|v| v.as_str())
                        .map(str::to_owned)
                        .filter(|role| !role.is_empty()),
                );
            }
            classes.entry(org).or_insert_with(|| {
                row.get("org_class")
                    .and_then(|v| v.as_str())
                    .map(str::to_owned)
                    .filter(|class| !class.is_empty())
            });
            if let Some(extra) = row.get("extra").and_then(|v| v.as_str()) {
                if !extra.is_empty() {
                    extras.entry(org).or_default().push(extra.to_string());
                }
            }
        }

        order
            .into_iter()
            .map(|org| {
                OrgMembership::new(
                    org,
                    primary.get(&org).cloned().flatten(),
                    extras.remove(&org).unwrap_or_default(),
                )
                .in_class(classes.remove(&org).flatten())
            })
            .collect()
    }

    /// Every user who shares at least one organisation with `principal`
    /// (including the caller themselves).
    ///
    /// This is what `member` means on the global `user` resource: colleagues are
    /// visible to each other, strangers are not. Resolved per request from the
    /// membership table, like the memberships themselves, so a user removed from
    /// an organisation stops being visible immediately.
    pub async fn co_member_user_ids(&self, principal: &Principal) -> Vec<Uuid> {
        let orgs = principal.org_ids();
        let mut ids = vec![principal.user_id];
        if orgs.is_empty() {
            return ids;
        }
        let Some(membership_tbl) = self.table("membership") else {
            return ids;
        };
        let placeholders = (1..=orgs.len())
            .map(|i| format!("${i}::uuid"))
            .collect::<Vec<_>>()
            .join(", ");
        let sql = format!(
            "SELECT DISTINCT user_id::text AS uid FROM {membership_tbl} \
             WHERE organization_id IN ({placeholders})"
        );
        let params: Vec<serde_json::Value> = orgs
            .iter()
            .map(|id| serde_json::Value::String(id.to_string()))
            .collect();
        let rows = match self.db.raw_json(&sql, &params).await {
            Ok(v) => v,
            Err(_) => return ids,
        };
        if let Some(arr) = rows.as_array() {
            for row in arr {
                if let Some(id) = row
                    .get("uid")
                    .and_then(|v| v.as_str())
                    .and_then(|s| Uuid::parse_str(s).ok())
                {
                    if !ids.contains(&id) {
                        ids.push(id);
                    }
                }
            }
        }
        ids
    }

    /// Every user who belongs to a specific organisation.
    pub async fn organization_user_ids(&self, org: Uuid) -> Vec<Uuid> {
        let Some(membership_tbl) = self.table("membership") else {
            return Vec::new();
        };
        let sql = format!(
            "SELECT DISTINCT user_id::text AS uid FROM {membership_tbl} \
             WHERE organization_id = $1::uuid"
        );
        let rows = match self
            .db
            .raw_json(&sql, &[serde_json::Value::String(org.to_string())])
            .await
        {
            Ok(v) => v,
            Err(_) => return Vec::new(),
        };
        let mut ids = Vec::new();
        if let Some(arr) = rows.as_array() {
            for row in arr {
                if let Some(id) = row
                    .get("uid")
                    .and_then(|v| v.as_str())
                    .and_then(|s| Uuid::parse_str(s).ok())
                {
                    if !ids.contains(&id) {
                        ids.push(id);
                    }
                }
            }
        }
        ids
    }

    /// Resolve the caller's active organisation for this request: the
    /// `X-Organization` header, if it names an org the caller belongs to.
    ///
    /// There is no fallback. Every account has at least its personal
    /// organisation and may have any number beside it, so "the one you are in"
    /// is never a safe guess — a request that does not say which organisation
    /// it means is answered as one that has none.
    pub fn active_org(&self, req: &HttpRequest, principal: &Option<Principal>) -> Option<Uuid> {
        resolve_active_org(req, principal, self.is_global_admin(principal.as_ref()))
    }

    /// Whether this caller is one of the deployment's own administrators —
    /// whoever `[organization] global_admin_role` names.
    ///
    /// Asked of the caller's memberships as a whole rather than of the
    /// organisation they have selected, and that is the difference between
    /// this and every other check in the system. An ordinary permission asks
    /// "what may you do *here*", so it is answered where you stand; this one
    /// asks "who are you", and someone whose standing evaporated the moment
    /// they looked at another tenant's organisation would be no use as a back
    /// office.
    ///
    /// `private` — the default — names nobody, so an app that has not asked
    /// for a back office does not have one.
    ///
    /// It is also the wider of the two doors into
    /// [impersonation](crate::impersonation): support access that stopped
    /// applying the moment they looked at the tenant they were supporting
    /// would be no access at all, which is exactly how this reads a caller.
    pub fn is_global_admin(&self, principal: Option<&Principal>) -> bool {
        let policy = self.app.config.organization.global_admin_policy();
        principal.is_some_and(|principal| is_global_admin(&policy, principal))
    }

    // --- what this deployment can do with a mailbox -----------------------
    //
    // Three features exist only if the app can send mail, and the `[auth]`
    // flags that govern them each default to following `[email]`. Asking the
    // question through these four methods — rather than reading the flags —
    // keeps "is it configured" and "is it switched on" in one place, so no
    // caller can accidentally offer a door that cannot open.

    /// Whether this app can send email at all.
    pub fn email_enabled(&self) -> bool {
        self.mailer.is_some()
    }

    /// Whether a new account must confirm its address before it can sign in.
    pub fn requires_email_verification(&self) -> bool {
        self.app
            .config
            .auth
            .requires_email_verification(self.email_enabled())
    }

    /// Whether an admin may invite somebody who has no account yet.
    pub fn invitations_enabled(&self) -> bool {
        self.app
            .config
            .auth
            .invitations_enabled(self.email_enabled())
    }

    /// Whether this app can take money at all.
    ///
    /// The `/billing` routes are mounted on this, the `billing_*` resources
    /// exist on it, and the dashboard hides its billing screens without it —
    /// so no interface offers a checkout that would land on a 404.
    pub fn payments_enabled(&self) -> bool {
        self.payments.is_some()
    }

    /// Whether anybody can sign in with a third-party account here.
    ///
    /// The `<base>/auth/oauth` routes are mounted on this and the `oauth_state`
    /// table exists on it, so an app that configured no provider has neither
    /// the endpoints nor the table — and the admin manifest says so, which is
    /// what stops a dashboard offering a button that would land on a 404.
    pub fn oauth_enabled(&self) -> bool {
        self.oauth.is_some()
    }

    /// Whether this app has an assistant at all.
    ///
    /// The `/ai` routes are mounted on this and a function's `chat` call fails
    /// without it, so no interface offers a chat box that would land on a 404.
    pub fn ai_enabled(&self) -> bool {
        self.ai.is_some()
    }

    /// Whether a forgotten password can be reset from a link.
    pub fn password_reset_enabled(&self) -> bool {
        self.app
            .config
            .auth
            .password_reset_enabled(self.email_enabled())
    }

    /// Quoted-safe physical table name for a resource by logical name.
    /// The `SELECT` fragment and `JOIN` that bring an organisation's class into
    /// a membership query — both empty when the app's `organization` resource has
    /// no `org_class` column, so a replaced built-in still loads memberships.
    fn org_class_join(&self) -> (String, String) {
        let has_class = self
            .app
            .resources
            .get("organization")
            .is_some_and(|r| r.fields.contains_key(ORG_CLASS_FIELD));
        match (has_class, self.table("organization")) {
            (true, Some(org_tbl)) => (
                format!(", o.{ORG_CLASS_FIELD} AS org_class"),
                format!(" LEFT JOIN {org_tbl} o ON o.id = m.organization_id"),
            ),
            _ => (String::new(), String::new()),
        }
    }

    pub(crate) fn table(&self, resource: &str) -> Option<String> {
        self.app
            .resources
            .get(resource)
            .map(|r| format!("\"{}\"", r.table_name()))
    }
}

/// Whether `principal` satisfies a `global_admin_role` policy, without a whole
/// [`AppState`] to ask it of.
///
/// Answered across **every** organisation the caller belongs to, not the one
/// they selected, which is what makes it a statement about who they are rather
/// than about where they stand — see [`AppState::is_global_admin`].
pub(crate) fn is_global_admin(policy: &Policy, principal: &Principal) -> bool {
    if policy.level == Access::Private {
        return false;
    }
    // An impersonated session is never a back office, whoever is behind it: the
    // token acts as the borrowed account, and that account's own standing is
    // the whole of what it may do. Otherwise a global admin could impersonate
    // anybody and keep their own powers while wearing somebody else's name,
    // which is the one shape of this no audit trail can untangle.
    if principal.is_impersonating() {
        return false;
    }
    principal.organizations.iter().any(|membership| {
        policy.matches_org_class(membership.org_class.as_deref())
            && match &policy.level {
                Access::Role(role) => membership.has_role(role),
                Access::Public | Access::Authenticated | Access::Member | Access::Owner => true,
                Access::Private => false,
            }
    })
}

fn resolve_active_org(
    req: &HttpRequest,
    principal: &Option<Principal>,
    global_admin: bool,
) -> Option<Uuid> {
    let principal = principal.as_ref()?;
    // A token pinned to one organisation is answered by the pin, not by the
    // header: an organisation admin borrowing a member's account may not steer
    // it anywhere but their own organisation, and refusing the header rather
    // than ignoring it would only tell them to stop sending it.
    if let Some(locked) = principal.org_lock {
        return Some(locked);
    }
    if let Some(raw) = req
        .headers()
        .get("x-organization")
        .and_then(|v| v.to_str().ok())
    {
        let org = Uuid::parse_str(raw.trim()).ok()?;
        // A deployment administrator may stand in any organisation, including
        // ones they have never joined — that is what administering the
        // deployment means, and what makes "look at this tenant's data" a
        // thing they can do at all.
        return (global_admin || principal.is_member(org)).then_some(org);
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use ntex::web::test;

    fn principal(orgs: &[(Uuid, Option<&str>)]) -> Principal {
        Principal::new(
            Uuid::new_v4(),
            orgs.iter()
                .map(|(org_id, role)| OrgMembership::new(*org_id, role.map(str::to_string), []))
                .collect(),
        )
    }

    /// A caller who belongs to one classed organisation.
    fn member_of_class(role: Option<&str>, class: Option<&str>) -> Principal {
        Principal::new(
            Uuid::new_v4(),
            vec![
                OrgMembership::new(Uuid::new_v4(), role.map(str::to_string), [])
                    .in_class(class.map(str::to_string)),
            ],
        )
    }

    #[test]
    fn only_the_named_role_administers_the_deployment() {
        // The setting the feature exists for: admins of a staff organisation
        // run the back office.
        let policy = Policy::parse("role:admin@org_class=staff");

        assert!(is_global_admin(
            &policy,
            &member_of_class(Some("admin"), Some("staff"))
        ));
        // Staff, but not an admin of it.
        assert!(!is_global_admin(
            &policy,
            &member_of_class(Some("support"), Some("staff"))
        ));
        // An admin of any other organisation is not staff — nor is an admin of
        // an unclassed one.
        assert!(!is_global_admin(
            &policy,
            &member_of_class(Some("admin"), Some("school"))
        ));
        assert!(!is_global_admin(
            &policy,
            &member_of_class(Some("admin"), None)
        ));
        // The default setting names nobody.
        assert!(!is_global_admin(
            &Policy::parse("private"),
            &member_of_class(Some("admin"), Some("staff"))
        ));
    }

    #[test]
    fn one_qualifying_membership_is_enough_wherever_the_caller_is_standing() {
        let policy = Policy::parse("role:admin@org_class=staff");
        let customer = Uuid::new_v4();
        let mut principal = member_of_class(Some("admin"), Some("staff"));
        principal.organizations.push(
            OrgMembership::new(customer, Some("member".into()), [])
                .in_class(Some("customer".into())),
        );

        // Standing in the customer organisation does not stop them being the
        // deployment's administrator: the question is who they are.
        assert!(is_global_admin(&policy, &principal));

        // …but a session borrowed from somebody else is not, whoever holds it.
        principal.impersonator = Some(Uuid::new_v4());
        assert!(!is_global_admin(&policy, &principal));
    }

    #[test]
    fn a_global_admin_may_stand_in_an_organisation_they_do_not_belong_to() {
        let elsewhere = Uuid::new_v4();
        let caller = Some(member_of_class(Some("admin"), Some("staff")));
        let req = test::TestRequest::default()
            .header("x-organization", elsewhere.to_string())
            .to_http_request();

        assert_eq!(resolve_active_org(&req, &caller, true), Some(elsewhere));
        assert_eq!(resolve_active_org(&req, &caller, false), None);
    }

    #[test]
    fn a_pinned_session_ignores_the_header() {
        let locked = Uuid::new_v4();
        let mut borrowed = member_of_class(Some("member"), None);
        borrowed.impersonator = Some(Uuid::new_v4());
        borrowed.org_lock = Some(locked);
        let caller = Some(borrowed);
        let req = test::TestRequest::default()
            .header("x-organization", Uuid::new_v4().to_string())
            .to_http_request();

        assert_eq!(resolve_active_org(&req, &caller, false), Some(locked));
    }

    #[test]
    fn active_org_prefers_valid_header() {
        let wanted = Uuid::new_v4();
        let req = test::TestRequest::default()
            .header("x-organization", wanted.to_string())
            .to_http_request();
        let caller = Some(principal(&[
            (wanted, Some("admin")),
            (Uuid::new_v4(), Some("member")),
        ]));

        assert_eq!(resolve_active_org(&req, &caller, false), Some(wanted));
    }

    #[test]
    fn an_organization_is_never_guessed_at() {
        let only = Uuid::new_v4();
        let req = test::TestRequest::default().to_http_request();

        // Even a caller with exactly one organisation has to name it: the next
        // one they create would otherwise silently change what their existing
        // requests mean.
        let one_org = Some(principal(&[(only, Some("member"))]));
        assert_eq!(resolve_active_org(&req, &one_org, false), None);
        assert_eq!(resolve_active_org(&req, &None, false), None);
    }

    #[test]
    fn a_header_naming_an_organization_you_are_not_in_is_refused() {
        let req = test::TestRequest::default()
            .header("x-organization", Uuid::new_v4().to_string())
            .to_http_request();
        let caller = Some(principal(&[(Uuid::new_v4(), Some("admin"))]));
        assert_eq!(resolve_active_org(&req, &caller, false), None);
    }
}
