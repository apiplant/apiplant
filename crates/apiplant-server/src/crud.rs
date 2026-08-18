//! Generic RESTful CRUD handlers, driven by resource schemas and multitenancy.
//!
//! Resources are **organisation-scoped by default**: every request must resolve
//! an active organisation, the caller must be a member, all queries are filtered
//! to that organisation, and `organization_id` is stamped on create. On top of
//! that isolation, each action's [`Access`] policy decides *who among the
//! members* may act (`member`, an org `role:`, or the row `owner`). A resource
//! marked `scope = "global"` opts out and is governed by permissions alone.
//!
//! Also supported on list/read: `?field=value` filtering, `?expand=` relation
//! inlining, and nested `GET /parent/{id}/child` collections.
//!
//! Every action additionally runs the resource's [lifecycle hooks](crate::hooks)
//! when it declares any: a `before_*` hook after authorization but before the
//! database call, an `after_*` hook on the way out.

use std::collections::HashMap;

use apiplant_auth::{Principal, ADMIN_ROLE};
use apiplant_core::schema::{Access, Effect, Policy, PolicySet, Rule};
use apiplant_core::{CrudAction, FieldType, HookEvent, Resource, ORG_CLASS_FIELD};
use apiplant_db::{value, Filter, Sort};
use ntex::web::types::{Json, Path, State};
use ntex::web::{HttpRequest, HttpResponse};
use serde_json::{json, Map, Value};
use uuid::Uuid;

use crate::hooks::{self, HookRequest};
use crate::response::{db_error, error, ok};
use crate::state::AppState;

const RESERVED: &[&str] = &[
    "limit",
    "offset",
    "expand",
    "via",
    "order",
    "search",
    "search_fields",
];

/// The caller plus their resolved active organisation, computed once per request.
struct Caller {
    principal: Option<Principal>,
    active_org: Option<Uuid>,
}

impl AppState {
    async fn caller(&self, req: &HttpRequest) -> Caller {
        let principal = self.resolve_principal(req).await;
        let active_org = self.active_org(req, &principal);
        Caller {
            principal,
            active_org,
        }
    }
}

impl Caller {
    /// The request-scoped context this handler's hooks will see.
    fn hook_request(&self, req: &HttpRequest, params: &HashMap<String, String>) -> HookRequest {
        HookRequest::new(req, params, self.principal.as_ref(), self.active_org)
    }
}

/// Column used for `owner` scoping: the resource's declared `owner_field` if it
/// exists as a column, otherwise the row's own `id` (self-ownership, e.g. users).
fn owner_column(r: &Resource) -> &str {
    if r.fields.contains_key(&r.meta.owner_field) {
        &r.meta.owner_field
    } else {
        "id"
    }
}

/// Drop the columns the server owns from a client-supplied body.
///
/// The tenant column and the owner column are *stamped*, never accepted: an
/// update that carried `organization_id` would move the row into another
/// organisation, because the `WHERE` clause checks where the row is now, not
/// where it is going. The password column goes the same way — it holds a hash,
/// and the only door that writes it is `POST <base>/auth/register`.
fn strip_server_owned(r: &Resource, data: &mut serde_json::Map<String, serde_json::Value>) {
    if r.is_org_scoped() {
        data.remove("organization_id");
    }
    let owner = owner_column(r);
    if owner != "id" {
        data.remove(owner);
    }
    // `user` always has an auth spec, declared or defaulted, exactly as the
    // auth routes resolve it.
    if r.meta.name == "user" || r.auth.is_some() {
        data.remove(&r.auth.clone().unwrap_or_default().password_field);
    }
}

/// Drop `organization.org_class` from a client body unless the caller is
/// somebody `[organization] org_class_editors` names.
///
/// The class is what a `@org_class=` permission is checked against, so an
/// organisation able to write its own class could grant itself whatever those
/// permissions guard. It is therefore server-owned like `organization_id`: not
/// refused with a `403` — which would make an ordinary rename of an
/// organisation fail because the client echoed a field back — but *stripped*,
/// exactly as the tenant column is.
///
/// The default setting is `private`, so an app that has said nothing has
/// classes only its operator can set.
fn strip_org_class(state: &AppState, r: &Resource, caller: &Caller, data: &mut Map<String, Value>) {
    if r.meta.name != "organization" || !data.contains_key(ORG_CLASS_FIELD) {
        return;
    }
    if !may_edit_org_class(&state.app.config.organization.org_class_policy(), caller) {
        data.remove(ORG_CLASS_FIELD);
    }
}

/// [`may_edit_org_class`] against the app's configured policy.
fn is_org_class_editor(state: &AppState, caller: &Caller) -> bool {
    may_edit_org_class(&state.app.config.organization.org_class_policy(), caller)
}

/// Stamp `[organization] default_org_class` on a new organisation that has no
/// class of its own.
///
/// Called from both doors an organisation comes through — `POST /organization`
/// and the personal one every account is given — so "every new organisation" in
/// the setting means every new organisation, not just the ones with a request
/// behind them.
///
/// Only fills a gap: a class editor who named one on create keeps it, and an
/// app that names no default leaves the column null, which no `@org_class=`
/// permission matches.
pub(crate) fn stamp_default_org_class(
    state: &AppState,
    r: &Resource,
    data: &mut Map<String, Value>,
) {
    if r.meta.name != "organization" || !r.fields.contains_key(ORG_CLASS_FIELD) {
        return;
    }
    let Some(default) = state.app.config.organization.default_class() else {
        return;
    };
    let named = data
        .get(ORG_CLASS_FIELD)
        .and_then(|value| value.as_str())
        .is_some_and(|class| !class.trim().is_empty());
    if !named {
        data.insert(
            ORG_CLASS_FIELD.to_string(),
            Value::String(default.to_string()),
        );
    }
}

/// Whether `caller` satisfies the `org_class_editors` policy.
fn may_edit_org_class(policy: &Policy, caller: &Caller) -> bool {
    match (&policy.level, caller.principal.as_ref()) {
        (Access::Private, _) | (_, None) => false,
        // Every level below is answered in the organisation the caller
        // *selected*, not the one being edited: this says who administers
        // classes across the deployment, which is what a staff organisation is.
        (level, Some(principal)) => {
            let membership = caller.active_org.and_then(|org| principal.membership(org));
            match membership {
                None => false,
                Some(m) if !policy.matches_org_class(m.org_class.as_deref()) => false,
                Some(m) => match level {
                    Access::Role(role) => m.has_role(role),
                    Access::Public | Access::Authenticated | Access::Member | Access::Owner => true,
                    Access::Private => false,
                },
            }
        }
    }
}

fn resource<'s>(state: &'s AppState, name: &str) -> Result<&'s Resource, HttpResponse> {
    state
        .app
        .resources
        .get(name)
        .ok_or_else(|| error(404, format!("unknown resource `{name}`")))
}

/// Authorize an action, returning the filters that must scope the query (org
/// isolation, ownership, org membership set) or an error response.
fn authorize(set: &PolicySet, caller: &Caller, r: &Resource) -> Result<Vec<Filter>, HttpResponse> {
    select(set, caller, r).map(|(_, filters)| filters)
}

/// Pick the clause of `set` that answers this caller, and scope the query the
/// way it asks.
///
/// An action's permission is a set of rules rather than one level, so the first
/// question is *which* rule the caller falls under. The order is the only one
/// that is safe:
///
/// 1. **`deny` first.** A caller matching any `deny` is refused even if they
///    also match an `allow`, so an exception carved out of a broad grant cannot
///    be won back by a second role they happen to hold.
/// 2. **`allow` before `own`.** Both are a yes, and `own` is the narrower of
///    the two, so someone matching both gets the wider answer — a parent who is
///    also a kid edits everything.
/// 3. **Otherwise no.** Nothing matched means nobody said yes, and the set
///    never has an implicit "everyone else".
///
/// Matching *is* the ordinary single-level check: a rule matches exactly when
/// its policy would have allowed the caller on its own. That is what keeps the
/// two grammars from drifting — `update = "role:parent"` and a set holding only
/// that one `allow` are the same code path.
fn select<'s>(
    set: &'s PolicySet,
    caller: &Caller,
    r: &Resource,
) -> Result<(&'s Rule, Vec<Filter>), HttpResponse> {
    // `private` is not a refusal but an absence, and it is the whole set when
    // it appears, so it answers before anything is matched.
    if set.is_private() {
        return Err(error(404, "not found"));
    }

    let matches = |policy: &Policy| authorize_policy(policy, caller, r);

    for rule in set.rules.iter().filter(|rule| rule.effect == Effect::Deny) {
        if denies(&rule.policy, caller, r) {
            return Err(forbidden(caller));
        }
    }

    // The refusal to report if nothing matches: the first positive clause's
    // own complaint, which names the role or class the caller is missing and is
    // more use than a bare "forbidden". Later clauses are tried regardless.
    let mut first_refusal = None;
    for effect in [Effect::Allow, Effect::Own] {
        for rule in set.rules.iter().filter(|rule| rule.effect == effect) {
            match matches(&rule.policy) {
                Ok(mut filters) => {
                    if effect == Effect::Own {
                        filters.extend(owner_narrowing(caller, r));
                    }
                    return Ok((rule, filters));
                }
                Err(refusal) => first_refusal.get_or_insert(refusal),
            };
        }
    }
    Err(first_refusal.unwrap_or_else(|| forbidden(caller)))
}

/// Whether a `deny` clause is about this caller.
///
/// Almost the ordinary check, with one deliberate exception: a `role:` here
/// matches only a role the caller **actually holds**, never the blanket one an
/// admin gets. `admin` satisfying every `role:` is what stops a new role from
/// locking an organisation's administrators out of their own data — read into a
/// denial it would do exactly that, and `deny = ["role:kid"]` would shut out the
/// very people who granted the role.
fn denies(policy: &Policy, caller: &Caller, r: &Resource) -> bool {
    if authorize_policy(policy, caller, r).is_err() {
        return false;
    }
    let Access::Role(role) = &policy.level else {
        return true;
    };
    caller
        .principal
        .as_ref()
        .zip(caller.active_org)
        .and_then(|(principal, org)| principal.membership(org))
        .is_some_and(|membership| membership.roles.iter().any(|held| held == role))
}

/// The refusal for a caller who matched nothing: `401` when credentials would
/// change the answer, `403` when they would not.
fn forbidden(caller: &Caller) -> HttpResponse {
    if caller.principal.is_none() {
        error(401, "authentication required")
    } else {
        error(403, "forbidden")
    }
}

/// The narrowing an `own` clause adds on top of whatever matched it.
///
/// This is exactly what the `owner` level does, including its exemption: an
/// admin of the organisation is not narrowed, because `owner` has never
/// narrowed them and an `own` clause is the same promise written per-role.
fn owner_narrowing(caller: &Caller, r: &Resource) -> Option<Filter> {
    let principal = caller.principal.as_ref()?;
    if r.is_org_scoped() {
        let membership = caller.active_org.and_then(|org| principal.membership(org))?;
        if membership.has_role(ADMIN_ROLE) {
            return None;
        }
    }
    Some(Filter::eq(owner_column(r).to_string(), principal.user_id))
}

/// Whether one clause admits this caller, and how it scopes the query.
fn authorize_policy(
    policy: &Policy,
    caller: &Caller,
    r: &Resource,
) -> Result<Vec<Filter>, HttpResponse> {
    if r.is_org_scoped() {
        authorize_org_scoped(policy, caller, r)
    } else {
        authorize_global(policy, caller, r)
    }
}

/// The refusal a class-qualified policy gives when the organisation in hand is
/// of the wrong class.
///
/// Deliberately names the class rather than saying "forbidden": the caller may
/// well hold the role, and the thing that is wrong is *which organisation they
/// selected*, which they can fix by selecting another.
fn wrong_class(class: &str) -> HttpResponse {
    error(403, format!("requires an organisation of class `{class}`"))
}

/// Org-scoped resources: membership in the active org is always required, the
/// query is always filtered to it, and the policy refines who may act.
fn authorize_org_scoped(
    policy: &Policy,
    caller: &Caller,
    r: &Resource,
) -> Result<Vec<Filter>, HttpResponse> {
    let access = &policy.level;
    if *access == Access::Private {
        return Err(error(404, "not found"));
    }
    let Some(principal) = caller.principal.as_ref() else {
        return Err(error(401, "authentication required"));
    };
    let Some(org) = caller.active_org else {
        return Err(error(
            403,
            "select an organisation with the X-Organization header",
        ));
    };
    let Some(membership) = principal.membership(org) else {
        return Err(error(403, "you are not a member of this organisation"));
    };
    // The class qualifier is checked before the level, and only ever narrows:
    // the active organisation has to be of the class the policy names.
    if let Some(class) = policy.org_class.as_deref() {
        if !membership.is_class(class) {
            return Err(wrong_class(class));
        }
    }

    let mut filters = vec![Filter::eq("organization_id", org)];
    match access {
        Access::Public | Access::Authenticated | Access::Member => {}
        Access::Owner => {
            if !membership.has_role(ADMIN_ROLE) {
                filters.push(Filter::eq(owner_column(r).to_string(), principal.user_id));
            }
        }
        Access::Role(role) => {
            // Any of the caller's roles will do, and an admin holds them all.
            if !membership.has_role(role) {
                return Err(error(
                    403,
                    format!("requires the `{role}` role in this organisation"),
                ));
            }
        }
        Access::Private => unreachable!("handled above"),
    }
    Ok(filters)
}

/// [`authorize`] plus the scoping that needs a database round-trip.
///
/// Only one policy does: `member` on the global `user` resource, which means
/// "shares an organisation with me" and so has to look the co-members up. Every
/// handler that turns a policy into a query goes through here.
async fn scope(
    state: &AppState,
    set: &PolicySet,
    caller: &Caller,
    r: &Resource,
    action: CrudAction,
) -> Result<Vec<Filter>, HttpResponse> {
    let (rule, mut filters) = select(set, caller, r)?;
    // The extras below are properties of the answer, not of how it was spelled,
    // so an `own` clause is read as `owner` however its own level reads.
    let access = match rule.effect {
        Effect::Own => &Access::Owner,
        _ => &rule.policy.level,
    };

    // Somebody who administers organisation *classes* administers them across
    // the deployment, so they have to be able to find an organisation they are
    // not in — otherwise the only organisations they could class are the ones
    // they had already joined, which is not what a back office is for.
    //
    // Reading only: their write is narrowed to the class column alone, in
    // [`update`], and every other action still goes by the ordinary policy.
    if r.meta.name == "organization"
        && matches!(action, CrudAction::List | CrudAction::Read)
        && is_org_class_editor(state, caller)
    {
        filters.retain(|filter| !matches!(filter, Filter::In { column, .. } if column == "id"));
    }
    if *access == Access::Owner && !r.is_org_scoped() {
        if let (Some(principal), Some(org)) = (caller.principal.as_ref(), caller.active_org) {
            if principal.is_admin_of(org) {
                // Widen the ownership narrowing to the whole organisation,
                // leaving any other narrowing the clause asked for in place.
                let owner = owner_column(r);
                let ids = state.organization_user_ids(org).await;
                filters.retain(|filter| !matches!(filter, Filter::Eq { column, .. } if column == owner));
                filters.push(Filter::in_uuids(owner, ids));
            }
        }
    }
    if *access == Access::Member && !r.is_org_scoped() && r.meta.name == "user" {
        // `authorize` let the caller through as authenticated; narrow the rows
        // to the people they actually share an organisation with.
        let principal = caller
            .principal
            .as_ref()
            .expect("member access requires a principal");
        let ids = state.co_member_user_ids(principal).await;
        filters.push(Filter::in_uuids("id", ids));
    }
    Ok(filters)
}

/// Global resources: no org isolation; the policy alone decides.
///
/// `member` is only meaningful on `organization` (scoped to your orgs) and on
/// `user` (scoped to your co-members, in [`scope`]). `role:` works everywhere,
/// but means different things: on `organization` it *filters* to the orgs you
/// hold the role in; on any other global resource there is nothing to filter,
/// so it gates on the role you hold in the organisation you have selected.
fn authorize_global(
    policy: &Policy,
    caller: &Caller,
    r: &Resource,
) -> Result<Vec<Filter>, HttpResponse> {
    let access = &policy.level;
    let class = policy.org_class.as_deref();
    let deny = || {
        if caller.principal.is_none() {
            error(401, "authentication required")
        } else {
            error(403, "forbidden")
        }
    };
    let is_org_resource = r.meta.name == "organization";

    // A class qualifier on a level that has no organisation in it — `public`,
    // `authenticated`, `owner` — still has to mean something, so it means what
    // it says: the caller must have selected an organisation of that class.
    // Otherwise `public@org_class=staff` would read as plain `public`, which is
    // the one direction a qualifier must never take.
    if let (Some(class), Access::Public | Access::Authenticated | Access::Owner) = (class, access) {
        let membership = caller
            .principal
            .as_ref()
            .and_then(|p| caller.active_org.and_then(|org| p.membership(org)));
        match membership {
            Some(m) if m.is_class(class) => {}
            Some(_) => return Err(wrong_class(class)),
            None => return Err(deny()),
        }
    }

    match access {
        Access::Public => Ok(Vec::new()),
        Access::Private => Err(error(404, "not found")),
        Access::Authenticated => match &caller.principal {
            Some(_) => Ok(Vec::new()),
            None => Err(deny()),
        },
        Access::Owner => match &caller.principal {
            Some(p) => Ok(vec![Filter::eq(owner_column(r).to_string(), p.user_id)]),
            None => Err(deny()),
        },
        Access::Member => match &caller.principal {
            // On `organization` the class is one more narrowing of the same
            // list: the orgs you belong to, of the class asked for.
            Some(p) if is_org_resource => {
                Ok(vec![Filter::in_uuids("id", p.org_ids_in_class(class))])
            }
            // Anywhere else there is nothing to filter, so a class qualifier
            // becomes a gate on the organisation the caller selected.
            Some(p) => match class {
                None => Ok(Vec::new()),
                Some(class) => match caller.active_org.and_then(|org| p.membership(org)) {
                    Some(m) if m.is_class(class) => Ok(Vec::new()),
                    Some(_) => Err(wrong_class(class)),
                    None => Err(error(
                        403,
                        "select an organisation with the X-Organization header",
                    )),
                },
            },
            None => Err(deny()),
        },
        // On `organization` itself, a role narrows the rows: the organisations
        // you hold it in. Anywhere else there is nothing to narrow — a global
        // table has no `organization_id` — so the role is a gate rather than a
        // filter: you hold it in the organisation you have selected, or you do
        // not get in.
        //
        // That has to mean *something* here, because the framework ships global
        // resources that depend on it: `billing_product` and `billing_price`
        // are `role:admin` to write, and the queue is `role:admin` to read.
        // Refusing outright would leave a catalogue nobody can edit and a
        // ledger nobody can look at.
        Access::Role(role) => match &caller.principal {
            Some(p) if is_org_resource => Ok(vec![Filter::in_uuids(
                "id",
                p.org_ids_with_role_in_class(role, class),
            )]),
            Some(p) => match caller.active_org.and_then(|org| p.membership(org)) {
                // The class is the organisation's, the role is the caller's in
                // it; both have to hold.
                Some(membership) if class.is_some_and(|class| !membership.is_class(class)) => {
                    Err(wrong_class(class.unwrap_or_default()))
                }
                // Any of the caller's roles will do, and an admin holds them
                // all — the same rule an org-scoped resource applies.
                Some(membership) if membership.has_role(role) => Ok(Vec::new()),
                Some(_) => Err(error(
                    403,
                    format!("requires the `{role}` role in this organisation"),
                )),
                None => Err(error(
                    403,
                    "select an organisation with the X-Organization header",
                )),
            },
            None => Err(deny()),
        },
    }
}

pub(crate) fn parse_query(qs: &str) -> HashMap<String, String> {
    serde_urlencoded::from_str(qs).unwrap_or_default()
}

fn expand_list(params: &HashMap<String, String>) -> Vec<String> {
    params
        .get("expand")
        .map(|s| {
            s.split(',')
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(String::from)
                .collect()
        })
        .unwrap_or_default()
}

fn field_filters(
    r: &Resource,
    params: &HashMap<String, String>,
) -> Result<Vec<Filter>, HttpResponse> {
    let mut filters = Vec::new();
    for (key, raw) in params {
        if RESERVED.contains(&key.as_str()) {
            continue;
        }
        // `?title~=depot` searches instead of matching: the one thing a search
        // box means and an equality filter cannot express. It is a separate
        // spelling rather than a change to `?title=`, because a filter that
        // silently matched substrings would be a trap for everything else.
        let (name, contains) = match key.strip_suffix('~') {
            Some(name) => (name, true),
            None => (key.as_str(), false),
        };
        let Some(field) = r.fields.get(name) else {
            continue;
        };
        // A hidden field is stripped from responses; filtering on one would
        // turn the list endpoint into an oracle for it (`?password_hash=…`).
        if field.hidden {
            continue;
        }
        if contains {
            // Substring matching on a number or a timestamp would be a search
            // of its text rendering, which is not a thing anyone means.
            if !matches!(field.ty, FieldType::String | FieldType::Text) {
                return Err(error(
                    400,
                    format!("`{name}` is not a text field, so `{name}~` cannot search it"),
                ));
            }
            filters.push(Filter::contains(name.to_string(), raw.clone()));
            continue;
        }
        match value::string_to_sql(field.ty, field.text_case(), raw) {
            Ok(v) => filters.push(Filter::Eq {
                column: key.clone(),
                value: v,
            }),
            Err(e) => return Err(error(400, format!("invalid filter `{key}`: {e}"))),
        }
    }
    Ok(filters)
}

/// Whether a column may be named in `?order=` — and, for `~`/`search`, looked
/// inside.
///
/// The two server-managed timestamps and `id` are not declared fields but are
/// real columns and the most useful sort keys there are, so they are allowed
/// explicitly. A hidden field is not: a response that refuses to show a value
/// should not answer questions about it by ordering rows.
fn sortable_column(r: &Resource, name: &str) -> bool {
    if name == "id" {
        return true;
    }
    if r.meta.timestamps && matches!(name, "created_at" | "updated_at") {
        return true;
    }
    r.fields.get(name).is_some_and(|field| !field.hidden)
}

/// `?order=name`, `?order=-created_at`, `?order=status,-created_at`.
///
/// A leading `-` (or a `:desc` suffix) reverses one key; keys apply in the
/// order given. An unknown or hidden column is a `400` rather than a silently
/// ignored parameter — a list that came back in the wrong order without saying
/// so is worse than one that refused.
fn sort_keys(r: &Resource, params: &HashMap<String, String>) -> Result<Vec<Sort>, HttpResponse> {
    let Some(raw) = params.get("order") else {
        return Ok(Vec::new());
    };
    let mut keys = Vec::new();
    for part in raw.split(',').map(str::trim).filter(|s| !s.is_empty()) {
        let (name, descending) = match part.strip_prefix('-') {
            Some(rest) => (rest.trim(), true),
            None => match part.split_once(':') {
                Some((name, dir)) => match dir.trim().to_ascii_lowercase().as_str() {
                    "asc" => (name.trim(), false),
                    "desc" => (name.trim(), true),
                    other => {
                        return Err(error(
                            400,
                            format!("`order` direction must be `asc` or `desc`, not `{other}`"),
                        ))
                    }
                },
                None => (part, false),
            },
        };
        if !sortable_column(r, name) {
            return Err(error(400, format!("cannot order by `{name}`")));
        }
        keys.push(Sort::new(name, descending));
    }
    Ok(keys)
}

/// `?search=<term>` — one term across several columns.
///
/// Which columns is the resource's decision (`[admin] search_fields`, falling
/// back to the single search field), so a search box needs to know nothing
/// about the resource. `?search_fields=a,b` narrows that to named columns for
/// callers that do: it is an API-only refinement, and every field it names is
/// checked the same way `?field~=` is.
fn search_filter(
    r: &Resource,
    params: &HashMap<String, String>,
) -> Result<Option<Filter>, HttpResponse> {
    let Some(term) = params
        .get("search")
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
    else {
        return Ok(None);
    };
    let columns = match params.get("search_fields") {
        Some(raw) => {
            let mut chosen = Vec::new();
            for name in raw.split(',').map(str::trim).filter(|s| !s.is_empty()) {
                let Some(field) = r.fields.get(name).filter(|field| !field.hidden) else {
                    return Err(error(400, format!("unknown search field `{name}`")));
                };
                if !matches!(field.ty, FieldType::String | FieldType::Text) {
                    return Err(error(
                        400,
                        format!("`{name}` is not a text field, so `search` cannot look in it"),
                    ));
                }
                chosen.push(name.to_string());
            }
            if chosen.is_empty() {
                return Err(error(400, "`search_fields` names no fields"));
            }
            chosen
        }
        None => {
            let configured = r.admin_search_fields();
            if configured.is_empty() {
                return Err(error(
                    400,
                    format!(
                        "`{}` has no searchable fields; name them in [admin] search_fields, or use `?<field>~=`",
                        r.meta.name
                    ),
                ));
            }
            configured
        }
    };
    Ok(Some(Filter::any_contains(columns, term.to_string())))
}

/// Inline the rows a `belongs_to` field points at, under the relation's name.
///
/// Expansion is a read of the *target* resource, so it goes through the target's
/// own `read` policy: a relation into something the caller may not read comes
/// back as `null` rather than as a row the direct endpoint would have refused.
async fn expand_relations(
    state: &AppState,
    r: &Resource,
    caller: &Caller,
    rows: &mut [serde_json::Value],
    relations: &[String],
) -> Result<(), HttpResponse> {
    for relation in relations {
        let Some(reference) = r.reference_by_relation(relation) else {
            return Err(error(
                400,
                format!("`{}` has no relation `{relation}` to expand", r.meta.name),
            ));
        };
        let Some(target) = state.app.resources.get(&reference.target) else {
            continue;
        };
        let scope = match scope(
            state,
            &target.permissions.read,
            caller,
            target,
            CrudAction::Read,
        )
        .await
        {
            Ok(filters) => filters,
            Err(_) => {
                for row in rows.iter_mut() {
                    if let Some(obj) = row.as_object_mut() {
                        obj.insert(relation.clone(), serde_json::Value::Null);
                    }
                }
                continue;
            }
        };

        let mut ids: Vec<Uuid> = Vec::new();
        for row in rows.iter() {
            if let Some(id) = row.get(&reference.field).and_then(|v| v.as_str()) {
                if let Ok(uuid) = Uuid::parse_str(id) {
                    if !ids.contains(&uuid) {
                        ids.push(uuid);
                    }
                }
            }
        }

        let fetched = state
            .db
            .fetch_by_ids(target, &ids, &scope)
            .await
            .map_err(db_error)?;
        let mut by_id: HashMap<String, serde_json::Value> = HashMap::new();
        if let Some(arr) = fetched.as_array() {
            for row in arr {
                if let Some(id) = row.get("id").and_then(|v| v.as_str()) {
                    by_id.insert(id.to_string(), row.clone());
                }
            }
        }

        for row in rows.iter_mut() {
            let embedded = row
                .get(&reference.field)
                .and_then(|v| v.as_str())
                .and_then(|id| by_id.get(id).cloned())
                .unwrap_or(serde_json::Value::Null);
            if let Some(obj) = row.as_object_mut() {
                obj.insert(relation.clone(), embedded);
            }
        }
    }
    Ok(())
}

pub async fn list(req: HttpRequest, state: State<AppState>, path: Path<String>) -> HttpResponse {
    let r = match resource(&state, &path) {
        Ok(r) => r,
        Err(resp) => return resp,
    };
    let params = parse_query(req.query_string());
    let caller = state.caller(&req).await;

    let mut filters = match scope(&state, &r.permissions.list, &caller, r, CrudAction::List).await {
        Ok(f) => f,
        Err(resp) => return resp,
    };
    match field_filters(r, &params) {
        Ok(mut f) => filters.append(&mut f),
        Err(resp) => return resp,
    }
    match search_filter(r, &params) {
        Ok(Some(f)) => filters.push(f),
        Ok(None) => {}
        Err(resp) => return resp,
    }
    let sort = match sort_keys(r, &params) {
        Ok(keys) => keys,
        Err(resp) => return resp,
    };

    let limit = params
        .get("limit")
        .and_then(|s| s.parse::<i64>().ok())
        .unwrap_or(50)
        .clamp(1, 500);
    let offset = params
        .get("offset")
        .and_then(|s| s.parse::<i64>().ok())
        .unwrap_or(0)
        .max(0);

    let hook_req = caller.hook_request(&req, &params);
    // A `before_list` that returns `{"data": …}` *is* the response: the query
    // below never runs. That is what lets a hook answer from a cache it knows
    // how to invalidate — see `docs/caching.md` and example 16.
    match hooks::run(&state, r, HookEvent::BeforeList, &hook_req, json!({})).await {
        Ok(Some(replacement)) => return ok(&replacement),
        Ok(None) => {}
        Err(resp) => return resp,
    }

    let result = match state.db.list(r, &filters, &sort, limit, offset).await {
        Ok(rows) => rows,
        Err(e) => return db_error(e),
    };

    let relations = expand_list(&params);
    let listed = if relations.is_empty() {
        result
    } else {
        let mut rows = result.as_array().cloned().unwrap_or_default();
        if let Err(resp) = expand_relations(&state, r, &caller, &mut rows, &relations).await {
            return resp;
        }
        serde_json::Value::Array(rows)
    };

    match hooks::run(&state, r, HookEvent::AfterList, &hook_req, listed.clone()).await {
        Ok(Some(replacement)) => ok(&replacement),
        Ok(None) => ok(&listed),
        Err(resp) => resp,
    }
}

pub async fn get(
    req: HttpRequest,
    state: State<AppState>,
    path: Path<(String, String)>,
) -> HttpResponse {
    let (name, id) = path.into_inner();
    let r = match resource(&state, &name) {
        Ok(r) => r,
        Err(resp) => return resp,
    };
    let id = match Uuid::parse_str(&id) {
        Ok(id) => id,
        Err(_) => return error(400, "invalid id"),
    };
    let params = parse_query(req.query_string());
    let caller = state.caller(&req).await;
    let filters = match scope(&state, &r.permissions.read, &caller, r, CrudAction::Read).await {
        Ok(f) => f,
        Err(resp) => return resp,
    };

    let hook_req = caller.hook_request(&req, &params).with_record(id);
    // As in `list`: a `before_read` that returns `{"data": …}` short-circuits
    // the fetch and that value is the response body. The permission check has
    // already run, so a hook answering from cache can't widen access — but the
    // row-level `filters` no longer apply, which is why a cached row must be
    // keyed by everything that scopes it.
    match hooks::run(&state, r, HookEvent::BeforeRead, &hook_req, json!({})).await {
        Ok(Some(replacement)) => return ok(&replacement),
        Ok(None) => {}
        Err(resp) => return resp,
    }

    let row = match state.db.get(r, id, &filters).await {
        Ok(Some(row)) => row,
        Ok(None) => return error(404, "not found"),
        Err(e) => return db_error(e),
    };

    let relations = expand_list(&params);
    let fetched = if relations.is_empty() {
        row
    } else {
        let mut rows = vec![row];
        if let Err(resp) = expand_relations(&state, r, &caller, &mut rows, &relations).await {
            return resp;
        }
        rows.into_iter().next().unwrap_or(serde_json::Value::Null)
    };

    match hooks::run(&state, r, HookEvent::AfterRead, &hook_req, fetched.clone()).await {
        Ok(Some(replacement)) => ok(&replacement),
        Ok(None) => ok(&fetched),
        Err(resp) => resp,
    }
}

/// `GET /parent/{id}/child` — the reverse (`has_many`) side of a relationship.
pub async fn nested_list(
    req: HttpRequest,
    state: State<AppState>,
    path: Path<(String, String, String)>,
) -> HttpResponse {
    let (parent_name, parent_id, child_name) = path.into_inner();
    let parent = match resource(&state, &parent_name) {
        Ok(r) => r,
        Err(resp) => return resp,
    };
    let child = match resource(&state, &child_name) {
        Ok(r) => r,
        Err(resp) => return resp,
    };
    let parent_id = match Uuid::parse_str(&parent_id) {
        Ok(id) => id,
        Err(_) => return error(400, "invalid id"),
    };

    let params = parse_query(req.query_string());
    let refs: Vec<_> = child
        .references()
        .into_iter()
        .filter(|rf| rf.target == parent.meta.name)
        .collect();
    let reference = match (refs.len(), params.get("via")) {
        (0, _) => {
            return error(
                400,
                format!("`{child_name}` has no relationship to `{parent_name}`"),
            )
        }
        (_, Some(via)) => match refs.into_iter().find(|rf| &rf.field == via) {
            Some(rf) => rf,
            None => {
                return error(
                    400,
                    format!("`{child_name}` has no reference field `{via}`"),
                )
            }
        },
        (1, None) => refs.into_iter().next().unwrap(),
        (_, None) => {
            return error(
                400,
                format!(
                    "`{child_name}` references `{parent_name}` more than once; add ?via=<field>"
                ),
            )
        }
    };

    let caller = state.caller(&req).await;
    let mut filters = match scope(
        &state,
        &child.permissions.list,
        &caller,
        child,
        CrudAction::List,
    )
    .await
    {
        Ok(f) => f,
        Err(resp) => return resp,
    };
    filters.push(Filter::eq(reference.field.clone(), parent_id));
    match field_filters(child, &params) {
        Ok(mut f) => filters.append(&mut f),
        Err(resp) => return resp,
    }
    match search_filter(child, &params) {
        Ok(Some(f)) => filters.push(f),
        Ok(None) => {}
        Err(resp) => return resp,
    }
    let sort = match sort_keys(child, &params) {
        Ok(keys) => keys,
        Err(resp) => return resp,
    };

    let limit = params
        .get("limit")
        .and_then(|s| s.parse::<i64>().ok())
        .unwrap_or(50)
        .clamp(1, 500);
    let offset = params
        .get("offset")
        .and_then(|s| s.parse::<i64>().ok())
        .unwrap_or(0)
        .max(0);

    // The rows returned are the child's, so the child's list hooks apply.
    let hook_req = caller.hook_request(&req, &params);
    match hooks::run(&state, child, HookEvent::BeforeList, &hook_req, json!({})).await {
        Ok(Some(replacement)) => return ok(&replacement),
        Ok(None) => {}
        Err(resp) => return resp,
    }

    let result = match state.db.list(child, &filters, &sort, limit, offset).await {
        Ok(rows) => rows,
        Err(e) => return db_error(e),
    };

    // `?expand=` means the same thing here as on the flat list, and against the
    // child's relations — the rows are the child's.
    let relations = expand_list(&params);
    let rows = if relations.is_empty() {
        result
    } else {
        let mut rows = result.as_array().cloned().unwrap_or_default();
        if let Err(resp) = expand_relations(&state, child, &caller, &mut rows, &relations).await {
            return resp;
        }
        serde_json::Value::Array(rows)
    };

    match hooks::run(&state, child, HookEvent::AfterList, &hook_req, rows.clone()).await {
        Ok(Some(replacement)) => ok(&replacement),
        Ok(None) => ok(&rows),
        Err(resp) => resp,
    }
}

pub async fn create(
    req: HttpRequest,
    state: State<AppState>,
    path: Path<String>,
    body: Json<serde_json::Map<String, serde_json::Value>>,
) -> HttpResponse {
    let r = match resource(&state, &path) {
        Ok(r) => r,
        Err(resp) => return resp,
    };
    let caller = state.caller(&req).await;
    if let Err(resp) = authorize(&r.permissions.create, &caller, r) {
        return resp;
    }
    // `user` ships with `create = "public"` so registration works; that door has
    // to close with registration itself, or `allow_registration = false` would
    // only move signup from `/auth/register` to `POST <base>/user`.
    if r.meta.name == "user"
        && caller.principal.is_none()
        && !state.app.config.auth.allow_registration
    {
        return error(403, "registration is disabled");
    }

    let params = parse_query(req.query_string());
    let hook_req = caller.hook_request(&req, &params);

    // The hook sees exactly what the client sent, and any body it returns is
    // stamped below — so a hook can never spoof the organisation or owner.
    let mut data = body.into_inner();
    match hooks::run(
        &state,
        r,
        HookEvent::BeforeCreate,
        &hook_req,
        serde_json::Value::Object(data.clone()),
    )
    .await
    {
        Ok(Some(replacement)) => {
            let hook = r.hook(HookEvent::BeforeCreate).unwrap_or_default();
            match hooks::replacement_object(replacement, hook) {
                Ok(map) => data = map,
                Err(resp) => return resp,
            }
        }
        Ok(None) => {}
        Err(resp) => return resp,
    }

    strip_server_owned(r, &mut data);
    strip_org_class(&state, r, &caller, &mut data);
    stamp_default_org_class(&state, r, &mut data);

    // Auto-stamp the organisation on org-scoped resources (never client-set).
    if r.is_org_scoped() {
        if let Some(org) = caller.active_org {
            data.insert(
                "organization_id".to_string(),
                serde_json::Value::String(org.to_string()),
            );
        }
    }
    // Auto-stamp the owner when the resource has a real owner column.
    let owner_col = owner_column(r);
    if owner_col != "id" && r.fields.contains_key(owner_col) {
        if let Some(p) = caller.principal.as_ref() {
            data.insert(
                owner_col.to_string(),
                serde_json::Value::String(p.user_id.to_string()),
            );
        }
    }

    // Granting a role somebody already holds is a no-op that looks like a
    // grant, and leaves a second copy to be revoked later.
    if r.meta.name == "membership_role" {
        if let Some(resp) = duplicate_role(&state, &data).await {
            return resp;
        }
    }

    let created = match state.db.create(r, &data).await {
        Ok(row) => row,
        Err(e) => return db_error(e),
    };

    // Bootstrap: whoever creates an organisation becomes its admin member, so
    // they can immediately manage it (org create is otherwise unmanageable).
    if r.meta.name == "organization" {
        if let Some(resp) = bootstrap_org_admin(&state, &caller, &created).await {
            return resp;
        }
    }

    // A user created through this door gets the same personal organisation a
    // registration would have given them — an account is an account.
    if r.meta.name == "user" {
        if let Some(id) = created
            .get("id")
            .and_then(|v| v.as_str())
            .and_then(|s| uuid::Uuid::parse_str(s).ok())
        {
            crate::auth_routes::create_personal_organization(&state, id, &created).await;
        }
    }

    let response = match hooks::run(
        &state,
        r,
        HookEvent::AfterCreate,
        &hook_req,
        created.clone(),
    )
    .await
    {
        Ok(Some(replacement)) => HttpResponse::Created().json(&replacement),
        Ok(None) => HttpResponse::Created().json(&created),
        // The hook rejected it, so there is nothing to announce.
        Err(resp) => return resp,
    };
    hooks::announce(&state, r, HookEvent::AfterCreate, &hook_req, &created).await;
    response
}

/// After an organisation is created, add the creator as an `admin` member.
async fn bootstrap_org_admin(
    state: &AppState,
    caller: &Caller,
    org: &serde_json::Value,
) -> Option<HttpResponse> {
    let (Some(principal), Some(membership_r)) = (
        caller.principal.as_ref(),
        state.app.resources.get("membership"),
    ) else {
        return None;
    };
    let Some(org_id) = org.get("id").and_then(|v| v.as_str()) else {
        return Some(error(500, "created organisation missing id"));
    };
    let mut m = serde_json::Map::new();
    m.insert(
        "user_id".into(),
        serde_json::Value::String(principal.user_id.to_string()),
    );
    m.insert(
        "organization_id".into(),
        serde_json::Value::String(org_id.to_string()),
    );
    m.insert("role".into(), serde_json::Value::String("admin".into()));
    match state.db.create(membership_r, &m).await {
        Ok(_) => None,
        Err(e) => {
            tracing::error!(error = %e, "failed to create bootstrap membership");
            Some(db_error(e))
        }
    }
}

/// The organisations a user belongs to, read straight from `membership`.
async fn organizations_of(state: &AppState, user_id: Uuid) -> Vec<Uuid> {
    let Some(memberships) = state.table("membership") else {
        return Vec::new();
    };
    let sql =
        format!("SELECT organization_id::text AS org FROM {memberships} WHERE user_id = $1::uuid");
    let rows = state
        .db
        .raw_json(&sql, &[serde_json::Value::String(user_id.to_string())])
        .await;
    let Ok(rows) = rows else {
        return Vec::new();
    };
    rows.as_array()
        .map(|rows| {
            rows.iter()
                .filter_map(|row| row.get("org").and_then(Value::as_str))
                .filter_map(|id| Uuid::parse_str(id).ok())
                .collect()
        })
        .unwrap_or_default()
}

/// Delete `org` if it has no members left. Best-effort: an organisation that
/// will not go — because the app hung something on it that refuses to cascade —
/// is not worth failing an account deletion that has already happened.
async fn discard_empty_organization(state: &AppState, org: Uuid) {
    let (Some(memberships), Some(organizations)) =
        (state.table("membership"), state.table("organization"))
    else {
        return;
    };
    let sql = format!(
        "DELETE FROM {organizations} WHERE id = $1::uuid \
         AND NOT EXISTS (SELECT 1 FROM {memberships} WHERE organization_id = $1::uuid)"
    );
    if let Err(e) = state
        .db
        .raw_json(&sql, &[serde_json::Value::String(org.to_string())])
        .await
    {
        tracing::warn!(error = %e, "could not discard an empty organization");
    }
}

// --- keeping an organisation administrable ----------------------------------

/// Whether a change to `resource`'s row `id` would take the caller's own
/// `admin` role away.
///
/// An admin may demote *another* admin — that is ordinary administration — but
/// not themselves. The rule reads like courtesy and is actually structural: an
/// organisation can only lose its last admin if that admin removes themselves,
/// so forbidding self-removal is what guarantees there is always somebody left
/// who can administer it.
///
/// `change` is the incoming body for an update, or `None` for a delete.
async fn removes_own_admin(
    state: &AppState,
    r: &Resource,
    id: Uuid,
    caller: &Caller,
    change: Option<&serde_json::Map<String, Value>>,
) -> bool {
    let Some(principal) = caller.principal.as_ref() else {
        return false;
    };
    let Some(org) = caller.active_org else {
        return false;
    };
    // Nothing to protect if they are not an admin here in the first place.
    if !principal.is_admin_of(org) {
        return false;
    }

    // A delete always removes whatever the row granted. An update only does so
    // if it actually moves the `role` off `admin`; leaving the field out means
    // leaving the role alone.
    let still_admin_after = match change {
        None => false,
        Some(data) => match data.get("role") {
            Some(Value::String(role)) => role == ADMIN_ROLE,
            // Not mentioned, or nulled: nulling a role removes it too.
            Some(Value::Null) => false,
            Some(_) => false,
            None => return false,
        },
    };
    if still_admin_after {
        return false;
    }

    let sql = match r.meta.name.as_str() {
        // Deleting or demoting your own membership takes every role with it.
        "membership" => match state.table("membership") {
            Some(table) => {
                format!("SELECT 1 AS hit FROM {table} WHERE id = $1::uuid AND user_id = $2::uuid")
            }
            None => return false,
        },
        // A `membership_role` row only matters when it is the admin grant.
        "membership_role" => match (state.table("membership_role"), state.table("membership")) {
            (Some(roles), Some(memberships)) => format!(
                "SELECT 1 AS hit FROM {roles} r JOIN {memberships} m ON m.id = r.membership_id \
                 WHERE r.id = $1::uuid AND m.user_id = $2::uuid AND r.role = 'admin'"
            ),
            _ => return false,
        },
        _ => return false,
    };

    let rows = state
        .db
        .raw_json(
            &sql,
            &[
                Value::String(id.to_string()),
                Value::String(principal.user_id.to_string()),
            ],
        )
        .await;
    matches!(rows, Ok(value) if value.as_array().is_some_and(|rows| !rows.is_empty()))
}

/// Refuse a second copy of a role somebody already holds.
///
/// Two grants of one role is not twice the permission, it is one role and a
/// trap: revoking the visible copy appears to do nothing. The primary `role`
/// column counts as a grant, so it cannot be shadowed either.
async fn duplicate_role(
    state: &AppState,
    data: &serde_json::Map<String, Value>,
) -> Option<HttpResponse> {
    let role = data.get("role").and_then(Value::as_str)?.trim();
    let membership_id = data.get("membership_id").and_then(Value::as_str)?;
    let (roles, memberships) = (state.table("membership_role")?, state.table("membership")?);

    let sql = format!(
        "SELECT 1 AS hit FROM {memberships} m \
         WHERE m.id = $1::uuid AND (m.role = $2 OR EXISTS ( \
             SELECT 1 FROM {roles} r WHERE r.membership_id = m.id AND r.role = $2))"
    );
    let rows = state
        .db
        .raw_json(
            &sql,
            &[
                Value::String(membership_id.to_string()),
                Value::String(role.to_string()),
            ],
        )
        .await
        .ok()?;
    rows.as_array()
        .is_some_and(|rows| !rows.is_empty())
        .then(|| error(409, format!("they already hold the `{role}` role here")))
}

pub async fn update(
    req: HttpRequest,
    state: State<AppState>,
    path: Path<(String, String)>,
    body: Json<serde_json::Map<String, serde_json::Value>>,
) -> HttpResponse {
    let (name, id) = path.into_inner();
    let r = match resource(&state, &name) {
        Ok(r) => r,
        Err(resp) => return resp,
    };
    let id = match Uuid::parse_str(&id) {
        Ok(id) => id,
        Err(_) => return error(400, "invalid id"),
    };
    let caller = state.caller(&req).await;
    let mut data = body.into_inner();
    let mut denied: Option<HttpResponse> = None;

    // A class editor may write `org_class` on **any** organisation, including
    // ones they are not in — that is the whole job — but only that column: an
    // organisation's own admins still own its name, its slug and its logo. So
    // a body that is nothing but the class is authorised by the setting, and
    // anything else goes by the resource's ordinary `update` policy.
    //
    // Judged on the body the client sent, so a `before_update` hook cannot
    // turn a permitted class change into a wider write; the final body is
    // checked again below, after the hook has had its say.
    let class_only = |data: &Map<String, Value>| {
        r.meta.name == "organization"
            && data.len() == 1
            && data.contains_key(ORG_CLASS_FIELD)
            && is_org_class_editor(&state, &caller)
    };
    let class_update = class_only(&data);
    let mut filters = match scope(
        &state,
        &r.permissions.update,
        &caller,
        r,
        CrudAction::Update,
    )
    .await
    {
        Ok(f) => f,
        // Refused by the policy, but the setting may still allow this one
        // column. Held rather than returned so the decision is made on the
        // body that will actually be written.
        Err(resp) if class_update => {
            denied = Some(resp);
            Vec::new()
        }
        Err(resp) => return resp,
    };

    let params = parse_query(req.query_string());
    let hook_req = caller.hook_request(&req, &params).with_record(id);

    if removes_own_admin(&state, r, id, &caller, Some(&data)).await {
        return error(
            403,
            "you cannot remove your own admin role — another admin can do it for you",
        );
    }
    match hooks::run(
        &state,
        r,
        HookEvent::BeforeUpdate,
        &hook_req,
        serde_json::Value::Object(data.clone()),
    )
    .await
    {
        Ok(Some(replacement)) => {
            let hook = r.hook(HookEvent::BeforeUpdate).unwrap_or_default();
            match hooks::replacement_object(replacement, hook) {
                Ok(map) => data = map,
                Err(resp) => return resp,
            }
        }
        Ok(None) => {}
        Err(resp) => return resp,
    }

    strip_server_owned(r, &mut data);
    strip_org_class(&state, r, &caller, &mut data);

    if class_only(&data) {
        // The class alone, so the setting authorises it wherever the
        // organisation is: drop the narrowing the ordinary policy applied.
        filters.clear();
        denied = None;
    }
    if let Some(resp) = denied {
        return resp;
    }

    let updated = match state.db.update(r, id, &data, &filters).await {
        Ok(Some(row)) => row,
        Ok(None) => return error(404, "not found"),
        Err(e) => return db_error(e),
    };

    let response = match hooks::run(
        &state,
        r,
        HookEvent::AfterUpdate,
        &hook_req,
        updated.clone(),
    )
    .await
    {
        Ok(Some(replacement)) => ok(&replacement),
        Ok(None) => ok(&updated),
        Err(resp) => return resp,
    };
    hooks::announce(&state, r, HookEvent::AfterUpdate, &hook_req, &updated).await;
    response
}

pub async fn delete(
    req: HttpRequest,
    state: State<AppState>,
    path: Path<(String, String)>,
) -> HttpResponse {
    let (name, id) = path.into_inner();
    let r = match resource(&state, &name) {
        Ok(r) => r,
        Err(resp) => return resp,
    };
    let id = match Uuid::parse_str(&id) {
        Ok(id) => id,
        Err(_) => return error(400, "invalid id"),
    };
    let caller = state.caller(&req).await;
    let filters = match scope(
        &state,
        &r.permissions.delete,
        &caller,
        r,
        CrudAction::Delete,
    )
    .await
    {
        Ok(f) => f,
        Err(resp) => return resp,
    };

    let params = parse_query(req.query_string());
    let hook_req = caller.hook_request(&req, &params).with_record(id);

    if removes_own_admin(&state, r, id, &caller, None).await {
        return error(
            403,
            "you cannot remove your own admin role — another admin can do it for you",
        );
    }

    // A delete hook needs the row it is about to lose, and so does a
    // `[publish] after_delete` — the row *is* the message, and after the
    // delete there is nowhere left to read it from. Fetched only when
    // something actually wants it, so an ordinary delete stays one statement.
    let needs_the_row = r.hook(HookEvent::BeforeDelete).is_some()
        || r.hook(HookEvent::AfterDelete).is_some()
        || r.publish.get(HookEvent::AfterDelete).is_some();
    let doomed = if needs_the_row {
        match state.db.get(r, id, &filters).await {
            Ok(Some(row)) => row,
            Ok(None) => return error(404, "not found"),
            Err(e) => return db_error(e),
        }
    } else {
        serde_json::Value::Null
    };
    let payload = || {
        if doomed.is_null() {
            json!({})
        } else {
            doomed.clone()
        }
    };

    if let Err(resp) = hooks::run(&state, r, HookEvent::BeforeDelete, &hook_req, payload()).await {
        return resp;
    }

    // Read before the row goes: the memberships cascade with it, so afterwards
    // there is nothing left to say which organisations it was in.
    let orphaned = if r.meta.name == "user" {
        organizations_of(&state, id).await
    } else {
        Vec::new()
    };

    match state.db.delete(r, id, &filters).await {
        Ok(true) => {}
        Ok(false) => return error(404, "not found"),
        Err(e) => return db_error(e),
    }

    // The personal organisation an account was given goes with it — but only
    // once nobody is left in it. An organisation somebody else still belongs
    // to is a workspace, whoever started it.
    for org in orphaned {
        discard_empty_organization(&state, org).await;
    }

    let response = match hooks::run(&state, r, HookEvent::AfterDelete, &hook_req, payload()).await {
        // A replacement turns the usual empty `204` into a `200` with a body.
        Ok(Some(replacement)) => ok(&replacement),
        Ok(None) => HttpResponse::NoContent().finish(),
        Err(resp) => return resp,
    };
    // The row is gone, so the message carries what it was — the last chance
    // anything downstream has to see it.
    hooks::announce(&state, r, HookEvent::AfterDelete, &hook_req, &payload()).await;
    response
}

#[cfg(test)]
mod tests {
    use super::*;
    use apiplant_auth::OrgMembership;

    fn parse_resource(src: &str) -> Resource {
        let resource: Resource = toml::from_str(src).unwrap();
        resource.validate().unwrap();
        resource
    }

    fn caller_with_org(access_org: Uuid, role: Option<&str>) -> Caller {
        Caller {
            principal: Some(Principal {
                user_id: Uuid::new_v4(),
                organizations: vec![OrgMembership::new(access_org, role.map(str::to_string), [])],
            }),
            active_org: Some(access_org),
        }
    }

    #[test]
    fn authorize_org_scoped_member_and_owner_filters() {
        let org = Uuid::new_v4();
        let resource = parse_resource(
            r#"
[resource]
name = "post"

[fields.owner_id]
type = "reference"
references = "user"
"#,
        );

        let caller = caller_with_org(org, Some("member"));
        let member = authorize_policy(&Access::Member.into(), &caller, &resource).unwrap();
        assert!(matches!(
            &member[0],
            Filter::Eq { column, .. } if column == "organization_id"
        ));

        let owner = authorize_policy(&Access::Owner.into(), &caller, &resource).unwrap();
        assert_eq!(owner.len(), 2);
        assert!(matches!(
            &owner[1],
            Filter::Eq { column, .. } if column == "owner_id"
        ));

        let admin = authorize_policy(
            &Access::Owner.into(),
            &caller_with_org(org, Some("admin")),
            &resource,
        )
        .unwrap();
        assert_eq!(admin.len(), 1);
        assert!(matches!(
            &admin[0],
            Filter::Eq { column, .. } if column == "organization_id"
        ));
    }

    /// A caller in one classed organisation, having selected it.
    fn caller_in_class(org: Uuid, role: Option<&str>, class: Option<&str>) -> Caller {
        Caller {
            principal: Some(Principal {
                user_id: Uuid::new_v4(),
                organizations: vec![OrgMembership::new(org, role.map(str::to_string), [])
                    .in_class(class.map(str::to_string))],
            }),
            active_org: Some(org),
        }
    }

    #[test]
    fn only_the_named_editors_may_write_an_organisations_class() {
        let staff = Uuid::new_v4();
        // The setting the feature exists for: the staff organisation edits
        // everyone's classes, including other organisations'.
        let policy = Policy::parse("member@org_class=staff");

        assert!(may_edit_org_class(
            &policy,
            &caller_in_class(staff, Some("member"), Some("staff"))
        ));
        // A member of any other organisation is not staff, whatever they are
        // there — an admin of their own org included.
        assert!(!may_edit_org_class(
            &policy,
            &caller_in_class(staff, Some("admin"), Some("school"))
        ));
        assert!(!may_edit_org_class(
            &policy,
            &caller_in_class(staff, Some("admin"), None)
        ));
        // Nobody selected an organisation, nobody signed in: no.
        assert!(!may_edit_org_class(
            &policy,
            &Caller {
                principal: None,
                active_org: None
            }
        ));
        // The default setting locks the column for everyone.
        assert!(!may_edit_org_class(
            &Policy::parse("private"),
            &caller_in_class(staff, Some("admin"), Some("staff"))
        ));
    }

    #[test]
    fn a_class_qualified_policy_narrows_an_org_scoped_resource() {
        let org = Uuid::new_v4();
        let resource = parse_resource(
            r#"
[resource]
name = "report"

[fields.title]
type = "string"
"#,
        );
        let policy = Policy::parse("role:admin@org_class=school");

        // The role holds and the class matches: ordinary org isolation.
        let allowed = authorize_policy(
            &policy,
            &caller_in_class(org, Some("admin"), Some("school")),
            &resource,
        )
        .unwrap();
        assert!(matches!(
            &allowed[0],
            Filter::Eq { column, .. } if column == "organization_id"
        ));

        // The same admin, in an organisation of another class — or of none —
        // is refused: the class can only ever narrow.
        for class in [Some("staff"), None] {
            let err = authorize_policy(
                &policy,
                &caller_in_class(org, Some("admin"), class),
                &resource,
            )
            .expect_err("wrong class must be refused");
            assert_eq!(err.status().as_u16(), 403);
        }

        // Right class, wrong role is still a refusal.
        assert!(authorize_policy(
            &policy,
            &caller_in_class(org, Some("member"), Some("school")),
            &resource
        )
        .is_err());

        // An unqualified policy is unaffected by any class.
        assert!(authorize_policy(
            &Access::Role("admin".into()).into(),
            &caller_in_class(org, Some("admin"), Some("staff")),
            &resource
        )
        .is_ok());
    }

    #[test]
    fn a_class_qualified_policy_filters_the_organization_resource() {
        let school = Uuid::new_v4();
        let staff = Uuid::new_v4();
        let caller = Caller {
            principal: Some(Principal {
                user_id: Uuid::new_v4(),
                organizations: vec![
                    OrgMembership::new(school, Some("admin".into()), [])
                        .in_class(Some("school".into())),
                    OrgMembership::new(staff, Some("admin".into()), [])
                        .in_class(Some("staff".into())),
                ],
            }),
            active_org: Some(staff),
        };
        let organization = parse_resource(apiplant_core::defaults::ORGANIZATION_TOML);

        // On `organization` the class is one more narrowing of the list, not a
        // gate: you see the schools you administer, and nothing else.
        let filters = authorize_policy(
            &Policy::parse("role:admin@org_class=school"),
            &caller,
            &organization,
        )
        .unwrap();
        assert!(matches!(
            &filters[0],
            Filter::In { column, values } if column == "id" && values.len() == 1
        ));

        let members = authorize_policy(
            &Policy::parse("member@org_class=staff"),
            &caller,
            &organization,
        )
        .unwrap();
        assert!(matches!(
            &members[0],
            Filter::In { column, values } if column == "id" && values.len() == 1
        ));
    }

    #[test]
    fn a_class_qualified_policy_gates_other_global_resources() {
        let org = Uuid::new_v4();
        let plan = parse_resource(
            r#"
[resource]
name = "plan"
scope = "global"

[fields.name]
type = "string"
"#,
        );

        // Nothing to filter on a global table, so the class is a gate on the
        // organisation the caller selected — including for levels that name no
        // organisation at all, which would otherwise ignore it.
        for policy in [
            "role:admin@org_class=staff",
            "authenticated@org_class=staff",
        ] {
            let policy = Policy::parse(policy);
            assert!(authorize_policy(
                &policy,
                &caller_in_class(org, Some("admin"), Some("staff")),
                &plan
            )
            .is_ok());
            let err = authorize_policy(
                &policy,
                &caller_in_class(org, Some("admin"), Some("school")),
                &plan,
            )
            .expect_err("wrong class must be refused");
            assert_eq!(err.status().as_u16(), 403);
        }

        // `public@org_class=…` is not public: it needs an organisation, so an
        // anonymous caller is asked to authenticate rather than let through.
        let anonymous = Caller {
            principal: None,
            active_org: None,
        };
        let err = authorize_policy(&Policy::parse("public@org_class=staff"), &anonymous, &plan)
            .expect_err("a class qualifier always needs an organisation");
        assert_eq!(err.status().as_u16(), 401);
    }

    #[test]
    fn authorize_global_resources_honours_special_organization_rules() {
        let org_a = Uuid::new_v4();
        let org_b = Uuid::new_v4();
        let caller = Caller {
            principal: Some(Principal {
                user_id: Uuid::new_v4(),
                organizations: vec![
                    OrgMembership::new(org_a, Some("admin".into()), []),
                    OrgMembership::new(org_b, Some("member".into()), []),
                ],
            }),
            active_org: None,
        };

        let organization = parse_resource(apiplant_core::defaults::ORGANIZATION_TOML);
        let plan = parse_resource(
            r#"
[resource]
name = "plan"
scope = "global"

[fields.name]
type = "string"
"#,
        );
        let org_filters = authorize_policy(&Access::Member.into(), &caller, &organization).unwrap();
        assert!(matches!(
            &org_filters[0],
            Filter::In { column, values } if column == "id" && values.len() == 2
        ));

        let admin_filters =
            authorize_policy(&Access::Role("admin".into()).into(), &caller, &organization).unwrap();
        assert!(matches!(
            &admin_filters[0],
            Filter::In { column, values } if column == "id" && values.len() == 1
        ));

        // A role on an ordinary global resource gates rather than filters, so
        // it needs an organisation to be checked against. Without one there is
        // no question to answer.
        let err = match authorize_policy(&Access::Role("admin".into()).into(), &caller, &plan) {
            Ok(_) => panic!("a role check needs an active organisation"),
            Err(err) => err,
        };
        assert_eq!(err.status().as_u16(), 403);

        // With one, the role decides — and it lets every row through, because a
        // global table has none to narrow to.
        let admin_here = Caller {
            principal: caller.principal.clone(),
            active_org: Some(org_a),
        };
        assert!(
            authorize_policy(&Access::Role("admin".into()).into(), &admin_here, &plan)
                .unwrap()
                .is_empty()
        );

        // Holding a different role in that organisation is not holding this one.
        let member_here = Caller {
            principal: caller.principal.clone(),
            active_org: Some(org_b),
        };
        let err = match authorize_policy(&Access::Role("admin".into()).into(), &member_here, &plan) {
            Ok(_) => panic!("a member is not an admin"),
            Err(err) => err,
        };
        assert_eq!(err.status().as_u16(), 403);
    }

    #[test]
    fn field_filters_ignore_unknown_keys_and_validate_types() {
        let resource = parse_resource(
            r#"
[resource]
name = "post"

[fields.published]
type = "boolean"

[fields.views]
type = "integer"
"#,
        );
        let params = parse_query("published=true&views=3&ignored=x");
        let filters = field_filters(&resource, &params).unwrap();
        assert_eq!(filters.len(), 2);

        let bad = parse_query("views=not-a-number");
        let err = match field_filters(&resource, &bad) {
            Ok(_) => panic!("bad filter should fail"),
            Err(err) => err,
        };
        assert_eq!(err.status().as_u16(), 400);
    }

    #[test]
    fn hidden_fields_are_not_filterable() {
        let resource = parse_resource(apiplant_core::defaults::USER_TOML);
        let filters =
            field_filters(&resource, &parse_query("password_hash=abc&email=a@b.c")).unwrap();
        assert_eq!(filters.len(), 1);
        assert!(matches!(&filters[0], Filter::Eq { column, .. } if column == "email"));
    }

    #[test]
    fn server_owned_columns_are_dropped_from_client_bodies() {
        let membership = parse_resource(apiplant_core::defaults::MEMBERSHIP_TOML);
        let mut body: serde_json::Map<String, serde_json::Value> = serde_json::from_str(
            r#"{"organization_id":"11111111-1111-1111-1111-111111111111","role":"admin"}"#,
        )
        .unwrap();
        strip_server_owned(&membership, &mut body);
        assert!(!body.contains_key("organization_id"));
        assert!(body.contains_key("role"));

        let user = parse_resource(apiplant_core::defaults::USER_TOML);
        let mut body: serde_json::Map<String, serde_json::Value> =
            serde_json::from_str(r#"{"password_hash":"$argon2id$forged","email":"a@b.c"}"#)
                .unwrap();
        strip_server_owned(&user, &mut body);
        assert!(!body.contains_key("password_hash"));
        assert!(body.contains_key("email"));

        let api_key = parse_resource(apiplant_core::defaults::API_KEY_TOML);
        let mut body: serde_json::Map<String, serde_json::Value> = serde_json::from_str(
            r#"{"owner_id":"11111111-1111-1111-1111-111111111111","name":"ci"}"#,
        )
        .unwrap();
        strip_server_owned(&api_key, &mut body);
        assert!(!body.contains_key("owner_id"));
        assert!(body.contains_key("name"));
    }

    #[test]
    fn sort_keys_accept_both_spellings_and_refuse_unknown_columns() {
        let resource = parse_resource(
            r#"
[resource]
name = "post"

[fields.title]
type = "string"

[fields.secret]
type = "string"
hidden = true
"#,
        );

        assert!(sort_keys(&resource, &parse_query("")).unwrap().is_empty());
        assert_eq!(
            sort_keys(&resource, &parse_query("order=title")).unwrap(),
            vec![Sort::new("title", false)]
        );
        assert_eq!(
            sort_keys(&resource, &parse_query("order=-title")).unwrap(),
            vec![Sort::new("title", true)]
        );
        assert_eq!(
            sort_keys(&resource, &parse_query("order=title:desc")).unwrap(),
            vec![Sort::new("title", true)]
        );
        // Several keys, applied left to right, including the implicit columns.
        assert_eq!(
            sort_keys(&resource, &parse_query("order=title,-created_at")).unwrap(),
            vec![Sort::new("title", false), Sort::new("created_at", true)]
        );

        for bad in ["order=nope", "order=secret", "order=title:sideways"] {
            let err = sort_keys(&resource, &parse_query(bad)).unwrap_err();
            assert_eq!(err.status().as_u16(), 400, "{bad} should be refused");
        }
    }

    #[test]
    fn search_covers_configured_fields_and_may_be_narrowed() {
        let resource = parse_resource(
            r#"
[resource]
name = "post"

[admin]
search_fields = ["title", "body"]

[fields.title]
type = "string"

[fields.body]
type = "text"

[fields.views]
type = "integer"

[fields.secret]
type = "string"
hidden = true
"#,
        );

        assert!(search_filter(&resource, &parse_query(""))
            .unwrap()
            .is_none());
        // Whitespace is not a search term.
        assert!(search_filter(&resource, &parse_query("search=%20"))
            .unwrap()
            .is_none());

        let searched = |qs: &str| match search_filter(&resource, &parse_query(qs)) {
            Ok(Some(Filter::AnyContains { columns, value })) => (columns, value),
            other => panic!("expected a search filter, got {}", other.is_ok()),
        };

        assert_eq!(
            searched("search=depot"),
            (
                vec!["title".to_string(), "body".to_string()],
                "depot".to_string()
            )
        );
        assert_eq!(
            searched("search=depot&search_fields=body"),
            (vec!["body".to_string()], "depot".to_string())
        );

        // A hidden column, a non-text column and an empty list are all refused
        // rather than quietly widened back to the configured set.
        for bad in [
            "search=x&search_fields=secret",
            "search=x&search_fields=views",
            "search=x&search_fields=nope",
            "search=x&search_fields=,",
        ] {
            let err = search_filter(&resource, &parse_query(bad)).unwrap_err();
            assert_eq!(err.status().as_u16(), 400, "{bad} should be refused");
        }
    }

    #[test]
    fn expand_list_parses_csv_relations() {
        let params = parse_query("expand=post, owner ,,author");
        assert_eq!(expand_list(&params), vec!["post", "owner", "author"]);
    }
}
