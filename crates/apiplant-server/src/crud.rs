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

use std::collections::HashMap;

use apiplant_auth::Principal;
use apiplant_core::schema::Access;
use apiplant_core::Resource;
use apiplant_db::{value, Filter};
use ntex::web::types::{Json, Path, State};
use ntex::web::{HttpRequest, HttpResponse};
use uuid::Uuid;

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

/// Column used for `owner` scoping: the resource's declared `owner_field` if it
/// exists as a column, otherwise the row's own `id` (self-ownership, e.g. users).
fn owner_column(r: &Resource) -> &str {
    if r.fields.contains_key(&r.meta.owner_field) {
        &r.meta.owner_field
    } else {
        "id"
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
fn authorize(
    access: &Access,
    caller: &Caller,
    r: &Resource,
) -> Result<Vec<Filter>, HttpResponse> {
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
            if membership.role.as_deref() != Some(role.as_str()) {
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

/// Global resources: no org isolation; the policy alone decides. `member`/`role`
/// are only meaningful on the `organization` resource (scoped to your orgs).
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

fn parse_query(qs: &str) -> HashMap<String, String> {
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

async fn expand_relations(
    state: &AppState,
    r: &Resource,
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

        let fetched = state.db.fetch_by_ids(target, &ids).await.map_err(db_error)?;
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

    let mut filters = match authorize(&r.permissions.list, &caller, r) {
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

    let result = match state.db.list(r, &filters, limit, offset).await {
        Ok(rows) => rows,
        Err(e) => return db_error(e),
    };

    let relations = expand_list(&params);
    if relations.is_empty() {
        return ok(&result);
    }
    let mut rows = result.as_array().cloned().unwrap_or_default();
    if let Err(resp) = expand_relations(&state, r, &mut rows, &relations).await {
        return resp;
    }
    ok(&serde_json::Value::Array(rows))
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
    let filters = match authorize(&r.permissions.read, &caller, r) {
        Ok(f) => f,
        Err(resp) => return resp,
    };
    match state.db.get(r, id, &filters).await {
        Ok(Some(row)) => {
            let relations = expand_list(&params);
            if relations.is_empty() {
                return ok(&row);
            }
            let mut rows = vec![row];
            if let Err(resp) = expand_relations(&state, r, &mut rows, &relations).await {
                return resp;
            }
            ok(&rows.into_iter().next().unwrap_or(serde_json::Value::Null))
        }
        Ok(None) => error(404, "not found"),
        Err(e) => db_error(e),
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
            None => return error(400, format!("`{child_name}` has no reference field `{via}`")),
        },
        (1, None) => refs.into_iter().next().unwrap(),
        (_, None) => {
            return error(
                400,
                format!("`{child_name}` references `{parent_name}` more than once; add ?via=<field>"),
            )
        }
    };

    let caller = state.caller(&req).await;
    let mut filters = match authorize(&child.permissions.list, &caller, child) {
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

    match state.db.list(child, &filters, limit, offset).await {
        Ok(rows) => ok(&rows),
        Err(e) => db_error(e),
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

    let mut data = body.into_inner();

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

    HttpResponse::Created().json(&created)
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
    let filters = match authorize(&r.permissions.update, &caller, r) {
        Ok(f) => f,
        Err(resp) => return resp,
    };
    match state.db.update(r, id, &body.into_inner(), &filters).await {
        Ok(Some(row)) => ok(&row),
        Ok(None) => error(404, "not found"),
        Err(e) => db_error(e),
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
    let filters = match authorize(&r.permissions.delete, &caller, r) {
        Ok(f) => f,
        Err(resp) => return resp,
    };
    match state.db.delete(r, id, &filters).await {
        Ok(true) => HttpResponse::NoContent().finish(),
        Ok(false) => error(404, "not found"),
        Err(e) => db_error(e),
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
                organizations: vec![OrgMembership {
                    org_id: access_org,
                    role: role.map(str::to_string),
                }],
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
                    OrgMembership {
                        org_id: org_a,
                        role: Some("admin".into()),
                    },
                    OrgMembership {
                        org_id: org_b,
                        role: Some("member".into()),
                    },
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

        let admin_filters = authorize(&Access::Role("admin".into()), &caller, &organization).unwrap();
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
    fn expand_list_parses_csv_relations() {
        let params = parse_query("expand=post, owner ,,author");
        assert_eq!(expand_list(&params), vec!["post", "owner", "author"]);
    }
}
