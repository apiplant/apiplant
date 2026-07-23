//! Shared server state and caller-identity resolution.

use std::sync::Arc;

use apiplant_auth::{Authenticator, Principal};
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
    /// Pre-rendered OpenAPI document (JSON).
    pub openapi_json: Arc<String>,
    /// Pre-rendered Swagger UI page.
    pub docs_html: Arc<String>,
}

impl AppState {
    /// Resolve the caller from the `Authorization` header.
    ///
    /// * `Bearer <jwt>` — a session token.
    /// * `ApiKey <token>` — looked up (by SHA-256) in the `api_key` resource;
    ///   the request then acts as the key's owning user.
    ///
    /// Anonymous (no/invalid header) resolves to `None` rather than an error —
    /// public endpoints still work.
    ///
    /// Besides `Authorization: Bearer`/`ApiKey`, a dedicated `X-Api-Key` header
    /// is accepted so Swagger UI's `apiKey` security scheme works cleanly.
    pub async fn resolve_principal(&self, req: &HttpRequest) -> Option<Principal> {
        if let Some(key) = req.headers().get("x-api-key").and_then(|v| v.to_str().ok()) {
            if !key.is_empty() {
                return self.principal_from_api_key(key.trim()).await;
            }
        }

        let header = req.headers().get("authorization")?.to_str().ok()?;
        if let Some(token) = header.strip_prefix("Bearer ") {
            return self.auth.verify_token(token.trim()).ok();
        }
        if let Some(key) = header.strip_prefix("ApiKey ") {
            return self.principal_from_api_key(key.trim()).await;
        }
        None
    }

    async fn principal_from_api_key(&self, key: &str) -> Option<Principal> {
        let hash = Authenticator::hash_api_key(key);
        let api_key_tbl = self.table("api_key")?;
        let user_tbl = self.table("user")?;
        let role_tbl = self.table("role")?;

        let sql = format!(
            "SELECT u.id AS user_id, r.name AS role \
             FROM {api_key_tbl} k \
             JOIN {user_tbl} u ON u.id = k.owner_id \
             LEFT JOIN {role_tbl} r ON r.id = u.role_id \
             WHERE k.token_hash = $1 LIMIT 1"
        );
        let rows = self
            .db
            .raw_json(&sql, &[serde_json::Value::String(hash)])
            .await
            .ok()?;
        let row = rows.as_array()?.first()?;
        let user_id = Uuid::parse_str(row.get("user_id")?.as_str()?).ok()?;
        let role = row.get("role").and_then(|v| v.as_str()).map(str::to_owned);
        Some(Principal { user_id, role })
    }

    /// Quoted-safe physical table name for a resource by logical name.
    fn table(&self, resource: &str) -> Option<String> {
        self.app
            .resources
            .get(resource)
            .map(|r| format!("\"{}\"", r.table_name()))
    }
}
