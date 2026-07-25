//! Shared server state, caller-identity resolution, and active-organisation
//! resolution.

use std::path::PathBuf;
use std::sync::Arc;

use apiplant_auth::{Authenticator, OrgMembership, Principal};
use apiplant_core::App;
use apiplant_db::Db;
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
    /// Static admin bundle root (`admin/`) when the app ships one.
    pub admin_dir: Option<PathBuf>,
    /// Pre-rendered OpenAPI document (JSON).
    pub openapi_json: Arc<String>,
    /// Pre-rendered Swagger UI page.
    pub docs_html: Arc<String>,
}

impl AppState {
    /// Resolve the caller (identity + organisation memberships) from the request.
    ///
    /// Identity comes from `Authorization: Bearer <jwt>`, `Authorization: ApiKey
    /// <key>`, or `X-Api-Key: <key>`. Memberships (and the caller's role in each
    /// organisation) are loaded fresh from the database so changes take effect
    /// immediately. Anonymous callers resolve to `None`.
    pub async fn resolve_principal(&self, req: &HttpRequest) -> Option<Principal> {
        let user_id = self.resolve_user_id(req).await?;
        let organizations = self.load_memberships(user_id).await;
        Some(Principal {
            user_id,
            organizations,
        })
    }

    async fn resolve_user_id(&self, req: &HttpRequest) -> Option<Uuid> {
        if let Some(key) = req.headers().get("x-api-key").and_then(|v| v.to_str().ok()) {
            if !key.is_empty() {
                return self.user_id_from_api_key(key.trim()).await;
            }
        }
        let header = req.headers().get("authorization")?.to_str().ok()?;
        if let Some(token) = header.strip_prefix("Bearer ") {
            return self.auth.verify_token(token.trim()).ok();
        }
        if let Some(key) = header.strip_prefix("ApiKey ") {
            return self.user_id_from_api_key(key.trim()).await;
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

    /// Load the caller's organisation memberships (with their per-org role).
    async fn load_memberships(&self, user_id: Uuid) -> Vec<OrgMembership> {
        let Some(membership_tbl) = self.table("membership") else {
            return Vec::new();
        };
        let sql = format!(
            "SELECT organization_id::text AS org, role FROM {membership_tbl} \
             WHERE user_id = $1::uuid"
        );
        let rows = match self
            .db
            .raw_json(&sql, &[serde_json::Value::String(user_id.to_string())])
            .await
        {
            Ok(v) => v,
            Err(_) => return Vec::new(),
        };
        rows.as_array()
            .map(|arr| {
                arr.iter()
                    .filter_map(|row| {
                        let org_id = Uuid::parse_str(row.get("org")?.as_str()?).ok()?;
                        let role = row.get("role").and_then(|v| v.as_str()).map(str::to_owned);
                        Some(OrgMembership { org_id, role })
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Resolve the caller's active organisation for this request:
    ///
    /// 1. the `X-Organization` header if it names an org the caller belongs to,
    /// 2. otherwise the caller's only organisation (if they have exactly one),
    /// 3. otherwise `None` (a multi-org caller must pick one).
    pub fn active_org(&self, req: &HttpRequest, principal: &Option<Principal>) -> Option<Uuid> {
        resolve_active_org(req, principal)
    }

    /// Quoted-safe physical table name for a resource by logical name.
    fn table(&self, resource: &str) -> Option<String> {
        self.app
            .resources
            .get(resource)
            .map(|r| format!("\"{}\"", r.table_name()))
    }
}

fn resolve_active_org(req: &HttpRequest, principal: &Option<Principal>) -> Option<Uuid> {
    let principal = principal.as_ref()?;
    if let Some(raw) = req
        .headers()
        .get("x-organization")
        .and_then(|v| v.to_str().ok())
    {
        let org = Uuid::parse_str(raw.trim()).ok()?;
        return principal.is_member(org).then_some(org);
    }
    if principal.organizations.len() == 1 {
        return Some(principal.organizations[0].org_id);
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use ntex::web::test;

    fn principal(orgs: &[(Uuid, Option<&str>)]) -> Principal {
        Principal {
            user_id: Uuid::new_v4(),
            organizations: orgs
                .iter()
                .map(|(org_id, role)| OrgMembership {
                    org_id: *org_id,
                    role: role.map(str::to_string),
                })
                .collect(),
        }
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

        assert_eq!(resolve_active_org(&req, &caller), Some(wanted));
    }

    #[test]
    fn active_org_falls_back_to_only_membership_and_requires_selection_otherwise() {
        let only = Uuid::new_v4();
        let req = test::TestRequest::default().to_http_request();

        let one_org = Some(principal(&[(only, Some("member"))]));
        let multi_org = Some(principal(&[
            (only, Some("member")),
            (Uuid::new_v4(), Some("admin")),
        ]));

        assert_eq!(resolve_active_org(&req, &one_org), Some(only));
        assert_eq!(resolve_active_org(&req, &multi_org), None);
        assert_eq!(resolve_active_org(&req, &None), None);
    }
}
