//! Functions the framework ships with.
//!
//! A built-in is an ordinary Rust `fn` registered in the [function
//! registry](crate::functions::FunctionRegistry) under a manifest of its own. It
//! sees the same [`HostBridge`] a dynamically-loaded function does — database,
//! config, caller, hook context — so anything a built-in does, an app could have
//! written itself as a `functions/` library, and an app that ships a function
//! with the same name replaces it.
//!
//! They exist for the logic that has to live *behind* the API rather than in
//! front of it. [`organization_join`] is the example: turning an email address
//! into a member of an organisation needs a user lookup that the person doing
//! the adding is deliberately not allowed to perform themselves.
//!
//! Built-ins are `private`: they have no HTTP endpoint and are reached only as a
//! resource's lifecycle hook.

use apiplant_abi::{FunctionManifest, HostApi, HttpMethod, Visibility};
use apiplant_core::App;
use serde_json::{json, Map, Value};

use crate::functions::{FunctionRegistry, HostBridge};

/// Register every built-in into a fresh registry. Called by
/// [`FunctionRegistry::load`].
pub fn register_all(registry: &mut FunctionRegistry, app: &App) {
    registry.register_builtin(
        manifest(
            ORGANIZATION_JOIN,
            "Resolve the user being added to an organisation, by id or identity.",
        ),
        organization_join,
        organization_join_config(app),
    );
}

/// Reserved name prefix. Every built-in wears it so that an app naming a
/// function of its own can never collide with one by accident — and so that a
/// hook pointing at `apiplant_…` is visibly the framework's, not the app's.
pub const PREFIX: &str = "apiplant_";

/// Name of the membership `before_create` built-in, as
/// [`MEMBERSHIP_TOML`](apiplant_core::defaults::MEMBERSHIP_TOML) declares it.
pub const ORGANIZATION_JOIN: &str = "apiplant_organization_join";

/// A built-in's manifest: private, POST, version-locked to the framework.
fn manifest(name: &str, description: &str) -> FunctionManifest {
    FunctionManifest {
        name: name.into(),
        version: env!("CARGO_PKG_VERSION").into(),
        description: description.into(),
        visibility: Visibility::Private,
        role: "".into(),
        method: HttpMethod::Post,
        permission: "private".into(),
        admin: "".into(),
        config_schema: "".into(),
        input_schema: "".into(),
        output_schema: "".into(),
    }
}

/// What [`organization_join`] needs to know about *this* app: the physical
/// tables to query and which user column is the identity people type.
///
/// Passed as the function's config because that is how a function receives
/// deployment facts — a built-in gets it from the loaded schema instead of from
/// a `functions/<name>.toml`.
fn organization_join_config(app: &App) -> String {
    let table = |name: &str| {
        app.resources
            .get(name)
            .map(|r| format!("\"{}\"", r.table_name()))
    };
    let identity_field = app
        .resources
        .get("user")
        .and_then(|r| r.auth.as_ref())
        .map(|auth| auth.identity_field.clone())
        .unwrap_or_else(|| "email".to_string());
    json!({
        "user_table": table("user"),
        "membership_table": table("membership"),
        "identity_field": identity_field,
    })
    .to_string()
}

/// `before_create` on `membership`: work out *who* is being added.
///
/// The submitted body may name the person either way:
///
/// * `user_id` — used as given,
/// * `email` (whatever the app's identity field is) — looked up here.
///
/// The lookup belongs on this side of the API. A member listing users only sees
/// the people they already share an organisation with (see the `user` model's
/// `read = "member"`), so the person doing the adding cannot resolve an outsider's
/// address to an id — which is exactly who they are trying to add. Doing it in a
/// hook keeps that asymmetry: the address is resolved for the one purpose it was
/// given for, and nothing about the account comes back.
///
/// Rejects, rather than letting the insert fail later:
///
/// | Situation | Status |
/// |-----------|--------|
/// | neither `user_id` nor an identity | `422` |
/// | no account with that identity | `404` |
/// | already a member of this organisation | `409` |
pub fn organization_join(bridge: &HostBridge, input: &str) -> Result<String, String> {
    let mut data: Map<String, Value> = match serde_json::from_str(input) {
        Ok(Value::Object(map)) => map,
        _ => return Ok(reject(400, "expected a JSON object")),
    };
    let config: Value = serde_json::from_str(&bridge.config()).unwrap_or(Value::Null);
    let identity_field = config["identity_field"].as_str().unwrap_or("email");

    // The identity is an instruction to this hook, not a column on `membership`.
    let identity = data
        .remove(identity_field)
        .and_then(|v| v.as_str().map(str::to_string))
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());

    let user_id = match nonempty(data.get("user_id")) {
        Some(id) => id,
        None => {
            let Some(identity) = identity else {
                return Ok(reject(
                    422,
                    &format!("provide the member's `user_id` or their {identity_field}"),
                ));
            };
            let Some(user_table) = config["user_table"].as_str() else {
                return Err("the `user` resource is missing".to_string());
            };
            let sql = format!(
                "SELECT id::text AS id FROM {user_table} WHERE lower({identity_field}) = lower($1) LIMIT 1"
            );
            match first_column(bridge, &sql, vec![Value::String(identity.clone())], "id")? {
                Some(id) => id,
                // Deliberately the same shape of answer as a wrong address on a
                // login form: it says nothing about who else has an account.
                None => {
                    return Ok(reject(
                        404,
                        &format!("nobody is registered with that {identity_field}"),
                    ))
                }
            }
        }
    };

    // A second membership row in the same organisation is never what the caller
    // meant, and it would double the person in every listing.
    if let Some(membership_table) = config["membership_table"].as_str() {
        let hook: Value = serde_json::from_str(&bridge.hook()).unwrap_or(Value::Null);
        if let Some(org) = hook["organization_id"].as_str() {
            let sql = format!(
                "SELECT id::text AS id FROM {membership_table} \
                 WHERE organization_id = $1::uuid AND user_id = $2::uuid LIMIT 1"
            );
            let params = vec![
                Value::String(org.to_string()),
                Value::String(user_id.clone()),
            ];
            if first_column(bridge, &sql, params, "id")?.is_some() {
                return Ok(reject(
                    409,
                    "they are already a member of this organization",
                ));
            }
        }
    }

    data.insert("user_id".to_string(), Value::String(user_id));
    Ok(json!({ "data": data }).to_string())
}

/// A hook rejection in the [protocol](crate::hooks) the host understands.
fn reject(status: u16, message: &str) -> String {
    json!({ "error": { "status": status, "message": message } }).to_string()
}

fn nonempty(value: Option<&Value>) -> Option<String> {
    value
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

/// Run a query and read one column out of its first row, if there is one.
fn first_column(
    bridge: &HostBridge,
    sql: &str,
    params: Vec<Value>,
    column: &str,
) -> Result<Option<String>, String> {
    let request = json!({ "sql": sql, "params": params }).to_string();
    let raw = match bridge.query(request.as_str().into()) {
        abi_stable::std_types::RResult::ROk(v) => v.into_string(),
        abi_stable::std_types::RResult::RErr(e) => return Err(e.into_string()),
    };
    let rows: Value = serde_json::from_str(&raw).map_err(|e| e.to_string())?;
    Ok(rows
        .get(0)
        .and_then(|row| row.get(column))
        .and_then(Value::as_str)
        .map(str::to_string))
}

#[cfg(test)]
mod tests {
    use super::*;
    use apiplant_core::defaults;

    /// The smallest possible app: built-in resources, nothing else.
    fn empty_app() -> App {
        let dir = std::env::temp_dir().join(format!(
            "apiplant-builtins-{}-{:?}",
            std::process::id(),
            std::time::SystemTime::now()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let app = App::load(&dir).unwrap();
        std::fs::remove_dir_all(&dir).ok();
        app
    }

    /// The namespace is the whole point of the prefix: check it holds for every
    /// built-in, not just the one that exists today.
    #[test]
    fn every_builtin_lives_in_the_reserved_namespace() {
        let app = empty_app();
        let mut registry = FunctionRegistry::default();
        register_all(&mut registry, &app);

        let names: Vec<String> = registry
            .iter()
            .map(|f| f.manifest.name.to_string())
            .collect();
        assert!(!names.is_empty());
        for name in &names {
            assert!(
                name.starts_with(PREFIX),
                "`{name}` is missing the `{PREFIX}` prefix"
            );
        }
    }

    /// A built-in referenced by a built-in resource must actually be registered,
    /// or every write to that resource fails closed with a 500.
    #[test]
    fn the_membership_hook_resolves_to_a_registered_builtin() {
        let membership = defaults::parse_builtin(defaults::MEMBERSHIP_TOML);
        let hook = membership
            .hook(apiplant_core::HookEvent::BeforeCreate)
            .expect("membership declares a before_create hook");
        assert_eq!(hook, ORGANIZATION_JOIN);

        let mut registry = FunctionRegistry::default();
        register_all(&mut registry, &empty_app());
        assert!(registry.get(hook).is_some());
    }

    #[test]
    fn builtins_are_not_exposed_over_http() {
        let mut registry = FunctionRegistry::default();
        register_all(&mut registry, &empty_app());
        for f in registry.iter() {
            assert_eq!(f.manifest.visibility, Visibility::Private);
        }
    }
}
