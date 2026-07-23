//! Generic RESTful CRUD handlers, driven entirely by resource schemas.
//!
//! Every resource is served by the same handlers; the resource name is a path
//! segment resolved against [`AppState`] at request time. Each handler evaluates
//! the resource's per-action [`Access`] policy for the caller and, when the
//! policy is `owner`, transparently scopes the query to owned rows.
//!
//! On top of plain CRUD, list/read support:
//! * **filtering** — any `?field=value` whose key is a column adds an equality
//!   predicate,
//! * **relation expansion** — `?expand=owner,role` inlines referenced records,
//! * **nested collections** — `GET /parent/{id}/child` lists the children that
//!   reference the parent (the reverse, `has_many`, side of a relationship).

use std::collections::HashMap;

use apiplant_auth::{evaluate, Decision, Principal};
use apiplant_core::schema::Access;
use apiplant_core::Resource;
use apiplant_db::{value, Filter};
use ntex::web::types::{Json, Path, State};
use ntex::web::{HttpRequest, HttpResponse};
use uuid::Uuid;

use crate::response::{db_error, error, ok};
use crate::state::AppState;

/// Reserved query keys that are not field filters.
const RESERVED: &[&str] = &["limit", "offset", "expand"];

/// Column used for `owner` scoping: the resource's declared `owner_field` if it
/// exists as a column, otherwise the row's own `id` (self-ownership, e.g. users).
fn owner_column(r: &Resource) -> &str {
    if r.fields.contains_key(&r.meta.owner_field) {
        &r.meta.owner_field
    } else {
        "id"
    }
}

/// Resolve the resource or return a 404 response.
fn resource<'s>(state: &'s AppState, name: &str) -> Result<&'s Resource, HttpResponse> {
    state
        .app
        .resources
        .get(name)
        .ok_or_else(|| error(404, format!("unknown resource `{name}`")))
}

/// Authorize a read/write action, returning owner-scoping filters (empty when
/// unrestricted) or an error response.
fn authorize(
    access: &Access,
    principal: &Option<Principal>,
    r: &Resource,
) -> Result<Vec<Filter>, HttpResponse> {
    match evaluate(access, principal.as_ref()) {
        Decision::Allow => Ok(Vec::new()),
        Decision::AllowOwned => {
            let p = principal.as_ref().expect("AllowOwned implies a principal");
            Ok(vec![Filter::new(owner_column(r).to_string(), p.user_id)])
        }
        Decision::Deny => Err(if principal.is_none() {
            error(401, "authentication required")
        } else {
            error(403, "forbidden")
        }),
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

/// Turn `?field=value` params (excluding reserved + unknown keys) into typed
/// equality filters. Unknown keys are ignored; a value that can't be parsed for
/// its column type is an error.
fn field_filters(r: &Resource, params: &HashMap<String, String>) -> Result<Vec<Filter>, HttpResponse> {
    let mut filters = Vec::new();
    for (key, raw) in params {
        if RESERVED.contains(&key.as_str()) {
            continue;
        }
        let Some(field) = r.fields.get(key) else {
            continue;
        };
        match value::string_to_sql(field.ty, raw) {
            Ok(v) => filters.push(Filter { column: key.clone(), value: v }),
            Err(e) => return Err(error(400, format!("invalid filter `{key}`: {e}"))),
        }
    }
    Ok(filters)
}

/// Expand `belongs_to` relations into an array of result rows, in place.
/// Batches one query per relation (`WHERE id IN (...)`), so no N+1.
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

        // Collect the distinct referenced ids present in this page.
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
            .fetch_by_ids(target, &ids)
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
    let principal = state.resolve_principal(&req).await;

    let mut filters = match authorize(&r.permissions.list, &principal, r) {
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
    let principal = state.resolve_principal(&req).await;
    let filters = match authorize(&r.permissions.read, &principal, r) {
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

/// `GET /parent/{id}/child` — the reverse (`has_many`) side of a relationship:
/// lists `child` rows whose reference field points at the parent id.
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

    // Find the child field that references the parent. `?via=<field>` picks one
    // when several fields reference the same parent.
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

    let principal = state.resolve_principal(&req).await;
    let mut filters = match authorize(&child.permissions.list, &principal, child) {
        Ok(f) => f,
        Err(resp) => return resp,
    };
    filters.push(Filter::new(reference.field.clone(), parent_id));

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
    let principal = state.resolve_principal(&req).await;
    if let Err(resp) = authorize(&r.permissions.create, &principal, r) {
        return resp;
    }

    let mut data = body.into_inner();
    // Stamp the owner column from the authenticated caller whenever the resource
    // has a real owner field. The creator owns the row — this is what makes an
    // `update = "owner"` (or `delete = "owner"`) policy enforceable later. A
    // client can't spoof it: we always overwrite with the caller's own id.
    let owner_col = owner_column(r);
    if owner_col != "id" && r.fields.contains_key(owner_col) {
        if let Some(p) = principal.as_ref() {
            data.insert(
                owner_col.to_string(),
                serde_json::Value::String(p.user_id.to_string()),
            );
        }
    }

    match state.db.create(r, &data).await {
        Ok(row) => HttpResponse::Created().json(&row),
        Err(e) => db_error(e),
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
    let principal = state.resolve_principal(&req).await;
    let filters = match authorize(&r.permissions.update, &principal, r) {
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
    let principal = state.resolve_principal(&req).await;
    let filters = match authorize(&r.permissions.delete, &principal, r) {
        Ok(f) => f,
        Err(resp) => return resp,
    };
    match state.db.delete(r, id, &filters).await {
        Ok(true) => HttpResponse::NoContent().finish(),
        Ok(false) => error(404, "not found"),
        Err(e) => db_error(e),
    }
}
