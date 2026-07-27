//! Reading a [`FunctionManifest`] out of JSON.
//!
//! Two loaders need this and neither is Rust-shaped: a C library returns its
//! manifest as a JSON string from `apiplant_manifest`, and a TypeScript module
//! exports one as `export const manifest`. Both describe the same thing in the
//! same words, so they read it with the same code — which is also what keeps the
//! defaults (notably: *absent means private*) from drifting apart.

use serde_json::Value;

use crate::{FunctionAccess, FunctionManifest, HttpMethod, Visibility};
use abi_stable::std_types::RString;

/// Build a [`FunctionManifest`] from one entry of a manifest array.
///
/// `name` is the only required field. Everything else has a default, and the
/// default for access is the closed end: a function that says nothing about who
/// may call it is not reachable.
pub fn manifest_from_json(entry: &Value) -> Result<FunctionManifest, String> {
    let name = entry
        .get("name")
        .and_then(Value::as_str)
        .filter(|n| !n.is_empty())
        .ok_or_else(|| "a manifest entry has no `name`".to_string())?;

    let string = |key: &str| -> RString {
        entry
            .get(key)
            .and_then(Value::as_str)
            .unwrap_or_default()
            .into()
    };

    // Schemas may be written inline as objects or handed over pre-serialised.
    let schema = |key: &str| -> RString {
        match entry.get(key) {
            None | Some(Value::Null) => RString::new(),
            Some(Value::String(s)) => s.as_str().into(),
            Some(other) => other.to_string().into(),
        }
    };

    // `permission` is the current key and `visibility` the original one; they
    // share a grammar, so either may carry the policy and `permission` wins.
    let access_str = entry
        .get("permission")
        .or_else(|| entry.get("visibility"))
        .and_then(Value::as_str)
        // Default to the closed end: an unreadable or absent policy should hide
        // an endpoint, never publish one.
        .unwrap_or("private");
    let access = FunctionAccess::parse(access_str).ok_or_else(|| {
        format!(
            "function `{name}` has unknown permission `{access_str}`; expected \
             \"public\", \"authenticated\", \"member\", \"role:<name>\" or \"private\""
        )
    })?;
    // `visibility` is kept in step so anything still reading it — generated
    // docs, older tooling — sees the nearest equivalent.
    let (visibility, role) = match &access {
        FunctionAccess::Public => (Visibility::Public, String::new()),
        FunctionAccess::Authenticated | FunctionAccess::Member => {
            (Visibility::Authenticated, String::new())
        }
        FunctionAccess::Role(role) => (Visibility::RoleGated, role.clone()),
        FunctionAccess::Private => (Visibility::Private, String::new()),
    };

    let method_str = entry
        .get("method")
        .and_then(Value::as_str)
        .unwrap_or("POST");
    let method = match method_str.to_ascii_uppercase().as_str() {
        "GET" => HttpMethod::Get,
        "POST" => HttpMethod::Post,
        "PUT" => HttpMethod::Put,
        "DELETE" => HttpMethod::Delete,
        other => {
            return Err(format!(
                "function `{name}` has unsupported method `{other}`; \
                 expected GET, POST, PUT or DELETE"
            ))
        }
    };

    Ok(FunctionManifest {
        name: name.into(),
        version: match entry.get("version").and_then(Value::as_str) {
            Some(v) => v.into(),
            None => "0.0.0".into(),
        },
        description: string("description"),
        visibility,
        role: role.into(),
        method,
        permission: access.as_string().into(),
        admin: schema("admin"),
        config_schema: schema("config_schema"),
        input_schema: schema("input_schema"),
        output_schema: schema("output_schema"),
    })
}
