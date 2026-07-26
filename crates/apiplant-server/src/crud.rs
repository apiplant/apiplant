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
use apiplant_core::schema::Access;
use apiplant_core::{HookEvent, Resource};
use apiplant_db::{value, Filter};
use ntex::web::types::{Json, Path, State};
use ntex::web::{HttpRequest, HttpResponse};
use serde_json::{json, Value};
use uuid::Uuid;

use crate::hooks::{self, HookRequest};
use crate::response::{db_error, error, ok};
use crate::state::AppState;

const RESERVED: &[&str] = &["limit", "offset", "expand", "via"];

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

fn resource<'s>(state: &'s AppState, name: &str) -> Result<&'s Resource, HttpResponse> {
    state
        .app
        .resources
        .get(name)
        .ok_or_else(|| error(404, format!("unknown resource `{name}`")))
}

/// Authorize an action, returning the filters that must scope the query (org
/// isolation, ownership, org membership set) or an error response.
fn authorize(access: &Access, caller: &Caller, r: &Resource) -> Result<Vec<Filter>, HttpResponse> {
    if r.is_org_scoped() {
        authorize_org_scoped(access, caller, r)
    } else {
        authorize_global(access, caller, r)
    }
}

/// Org-scoped resources: membership in the active org is always required, the
/// query is always filtered to it, and the policy refines who may act.
fn authorize_org_scoped(
    access: &Access,
    caller: &Caller,
    r: &Resource,
) -> Result<Vec<Filter>, HttpResponse> {
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

    let mut filters = vec![Filter::eq("organization_id", org)];
    match access {
        Access::Public | Access::Authenticated | Access::Member => {}
        Access::Owner => filters.push(Filter::eq(owner_column(r).to_string(), principal.user_id)),
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
    access: &Access,
    caller: &Caller,
    r: &Resource,
) -> Result<Vec<Filter>, HttpResponse> {
    let mut filters = authorize(access, caller, r)?;
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

/// Global resources: no org isolation; the policy alone decides. `member`/`role`
/// are only meaningful on `organization` (scoped to your orgs) and on `user`
/// (scoped to your co-members, in [`scope`]).
fn authorize_global(
    access: &Access,
    caller: &Caller,
    r: &Resource,
) -> Result<Vec<Filter>, HttpResponse> {
    let deny = || {
        if caller.principal.is_none() {
            error(401, "authentication required")
        } else {
            error(403, "forbidden")
        }
    };
    let is_org_resource = r.meta.name == "organization";
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
            Some(p) if is_org_resource => Ok(vec![Filter::in_uuids("id", p.org_ids())]),
            Some(_) => Ok(Vec::new()),
            None => Err(deny()),
        },
        Access::Role(role) => match &caller.principal {
            Some(p) if is_org_resource => {
                Ok(vec![Filter::in_uuids("id", p.org_ids_with_role(role))])
            }
            Some(_) => Err(error(
                403,
                "role permissions apply to organisation-scoped resources",
            )),
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
        let Some(field) = r.fields.get(key) else {
            continue;
        };
        // A hidden field is stripped from responses; filtering on one would
        // turn the list endpoint into an oracle for it (`?password_hash=…`).
        if field.hidden {
            continue;
        }
        match value::string_to_sql(field.ty, raw) {
            Ok(v) => filters.push(Filter::Eq {
                column: key.clone(),
                value: v,
            }),
            Err(e) => return Err(error(400, format!("invalid filter `{key}`: {e}"))),
        }
    }
    Ok(filters)
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
        let scope = match scope(state, &target.permissions.read, caller, target).await {
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

    let mut filters = match scope(&state, &r.permissions.list, &caller, r).await {
        Ok(f) => f,
        Err(resp) => return resp,
    };
    match field_filters(r, &params) {
        Ok(mut f) => filters.append(&mut f),
        Err(resp) => return resp,
    }

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
    if let Err(resp) = hooks::run(&state, r, HookEvent::BeforeList, &hook_req, json!({})).await {
        return resp;
    }

    let result = match state.db.list(r, &filters, limit, offset).await {
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
    let filters = match scope(&state, &r.permissions.read, &caller, r).await {
        Ok(f) => f,
        Err(resp) => return resp,
    };

    let hook_req = caller.hook_request(&req, &params).with_record(id);
    if let Err(resp) = hooks::run(&state, r, HookEvent::BeforeRead, &hook_req, json!({})).await {
        return resp;
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
    let mut filters = match scope(&state, &child.permissions.list, &caller, child).await {
        Ok(f) => f,
        Err(resp) => return resp,
    };
    filters.push(Filter::eq(reference.field.clone(), parent_id));

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
    if let Err(resp) = hooks::run(&state, child, HookEvent::BeforeList, &hook_req, json!({})).await
    {
        return resp;
    }

    let rows = match state.db.list(child, &filters, limit, offset).await {
        Ok(rows) => rows,
        Err(e) => return db_error(e),
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

    match hooks::run(
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
        Err(resp) => resp,
    }
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
    let filters = match scope(&state, &r.permissions.update, &caller, r).await {
        Ok(f) => f,
        Err(resp) => return resp,
    };

    let params = parse_query(req.query_string());
    let hook_req = caller.hook_request(&req, &params).with_record(id);

    let mut data = body.into_inner();
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

    let updated = match state.db.update(r, id, &data, &filters).await {
        Ok(Some(row)) => row,
        Ok(None) => return error(404, "not found"),
        Err(e) => return db_error(e),
    };

    match hooks::run(
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
        Err(resp) => resp,
    }
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
    let filters = match scope(&state, &r.permissions.delete, &caller, r).await {
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

    // A delete hook needs the row it is about to lose, so fetch it first — but
    // only when one is actually declared.
    let has_delete_hook =
        r.hook(HookEvent::BeforeDelete).is_some() || r.hook(HookEvent::AfterDelete).is_some();
    let doomed = if has_delete_hook {
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

    match state.db.delete(r, id, &filters).await {
        Ok(true) => {}
        Ok(false) => return error(404, "not found"),
        Err(e) => return db_error(e),
    }

    match hooks::run(&state, r, HookEvent::AfterDelete, &hook_req, payload()).await {
        // A replacement turns the usual empty `204` into a `200` with a body.
        Ok(Some(replacement)) => ok(&replacement),
        Ok(None) => HttpResponse::NoContent().finish(),
        Err(resp) => resp,
    }
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
        let member = authorize(&Access::Member, &caller, &resource).unwrap();
        assert!(matches!(
            &member[0],
            Filter::Eq { column, .. } if column == "organization_id"
        ));

        let owner = authorize(&Access::Owner, &caller, &resource).unwrap();
        assert_eq!(owner.len(), 2);
        assert!(matches!(
            &owner[1],
            Filter::Eq { column, .. } if column == "owner_id"
        ));
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

        let org_filters = authorize(&Access::Member, &caller, &organization).unwrap();
        assert!(matches!(
            &org_filters[0],
            Filter::In { column, values } if column == "id" && values.len() == 2
        ));

        let admin_filters =
            authorize(&Access::Role("admin".into()), &caller, &organization).unwrap();
        assert!(matches!(
            &admin_filters[0],
            Filter::In { column, values } if column == "id" && values.len() == 1
        ));

        let err = match authorize(&Access::Role("admin".into()), &caller, &plan) {
            Ok(_) => panic!("role-gated global resource should be rejected"),
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
    fn expand_list_parses_csv_relations() {
        let params = parse_query("expand=post, owner ,,author");
        assert_eq!(expand_list(&params), vec!["post", "owner", "author"]);
    }
}
