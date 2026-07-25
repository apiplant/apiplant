use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{anyhow, bail, Context, Result};
use apiplant_abi::{HttpMethod, Visibility};
use apiplant_core::schema::{relation_name, Access, Field, FieldType, OnDelete, Resource};
use apiplant_core::App;
use apiplant_server::functions::FunctionRegistry;
use serde::Serialize;
use serde_json::Value;

const INDEX_HTML: &str = include_str!(env!("APIPLANT_ADMIN_INDEX_PATH"));
const APP_JS: &str = include_str!(env!("APIPLANT_ADMIN_JS_PATH"));
const APP_CSS: &str = include_str!(env!("APIPLANT_ADMIN_CSS_PATH"));
const HEAD_PNG: &[u8] = include_bytes!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../studio/public/head.png"
));
const HEAD_INVERTED_PNG: &[u8] = include_bytes!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../studio/public/head-inverted.png"
));

#[derive(Debug, Clone)]
pub struct Options {
    pub api: String,
    pub out: Option<PathBuf>,
}

#[derive(Debug, Serialize)]
struct AdminManifest {
    title: String,
    app_name: String,
    api_base_url: String,
    docs_url: Option<String>,
    auth: AuthManifest,
    resources: Vec<ResourceManifest>,
    functions: Vec<FunctionManifest>,
}

#[derive(Debug, Serialize)]
struct AuthManifest {
    identity_field: String,
    allow_registration: bool,
}

#[derive(Debug, Serialize)]
struct ResourceManifest {
    name: String,
    builtin: bool,
    scope: &'static str,
    owner_field: String,
    fields: Vec<FieldManifest>,
    relations: Vec<RelationManifest>,
    permissions: ActionPermissionsManifest,
    permission_summary: String,
    endpoint_summary: String,
}

#[derive(Debug, Serialize)]
struct ActionPermissionsManifest {
    list: ActionPermissionManifest,
    read: ActionPermissionManifest,
    create: ActionPermissionManifest,
    update: ActionPermissionManifest,
    delete: ActionPermissionManifest,
}

#[derive(Debug, Serialize)]
struct ActionPermissionManifest {
    value: String,
    note: String,
    requires_org: bool,
}

#[derive(Debug, Serialize)]
struct FieldManifest {
    name: String,
    #[serde(rename = "type")]
    ty: &'static str,
    required: bool,
    unique: bool,
    hidden: bool,
    references: Option<String>,
    relation: Option<String>,
    on_delete: Option<&'static str>,
    default_value: Option<Value>,
    writable: bool,
}

#[derive(Debug, Serialize)]
struct RelationManifest {
    field: String,
    relation: String,
    target: String,
}

#[derive(Debug, Serialize)]
struct FunctionManifest {
    name: String,
    description: String,
    method: &'static str,
    visibility: &'static str,
    visibility_label: String,
    role: Option<String>,
    note: String,
}

pub fn build(app_dir: &Path, options: Options) -> Result<PathBuf> {
    let app = App::load(app_dir)?;
    let api_base_url = normalize_api_base(
        &options.api,
        &app.config.server.base_path,
        app.tls.is_some(),
    )?;
    let output_dir = options.out.unwrap_or_else(|| app_dir.join("admin"));
    let registry = FunctionRegistry::load_dir(&app.functions_dir);
    let manifest = build_manifest(&app, &registry, api_base_url.clone())?;

    fs::create_dir_all(&output_dir)
        .with_context(|| format!("failed to create {}", output_dir.display()))?;

    write_text(output_dir.join("index.html"), INDEX_HTML)?;
    write_text(output_dir.join("app.js"), APP_JS)?;
    write_text(output_dir.join("app.css"), &admin_css())?;
    write_json(output_dir.join("apiplant-admin.json"), &manifest)?;
    write_bytes(output_dir.join("head.png"), HEAD_PNG)?;
    write_bytes(output_dir.join("head-inverted.png"), HEAD_INVERTED_PNG)?;

    Ok(output_dir)
}

fn build_manifest(
    app: &App,
    functions: &FunctionRegistry,
    api_base_url: String,
) -> Result<AdminManifest> {
    let app_name = app
        .root
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("apiplant app")
        .to_string();
    let identity_field = app
        .resources
        .get("user")
        .and_then(|resource| resource.auth.as_ref())
        .map(|auth| auth.identity_field.clone())
        .unwrap_or_else(|| "email".to_string());
    let docs_url = if app.config.docs.enabled {
        Some(format!("{}{}", api_base_url, app.config.docs.path))
    } else {
        None
    };
    let hook_functions = app
        .resources
        .values()
        .flat_map(|resource| {
            resource
                .hooks
                .iter()
                .map(|(_, function)| function.to_string())
        })
        .collect::<BTreeSet<_>>();

    let resources = app
        .resources
        .values()
        .map(|resource| resource_manifest(resource, &app.config.server.base_path))
        .collect::<Vec<_>>();

    let mut loaded_functions = functions
        .iter()
        .filter(|entry| entry.manifest.visibility != Visibility::Private)
        .filter(|entry| !hook_functions.contains(entry.manifest.name.as_str()))
        .map(|entry| function_manifest(entry.manifest.name.as_str(), &entry.manifest))
        .collect::<Vec<_>>();
    loaded_functions.sort_by(|left, right| left.name.cmp(&right.name));

    Ok(AdminManifest {
        title: format!("{app_name} admin"),
        app_name,
        api_base_url,
        docs_url,
        auth: AuthManifest {
            identity_field,
            allow_registration: app.config.auth.allow_registration,
        },
        resources,
        functions: loaded_functions,
    })
}

fn resource_manifest(resource: &Resource, base_path: &str) -> ResourceManifest {
    let fields = resource
        .fields
        .iter()
        .map(|(name, field)| field_manifest(name, field, &resource.meta.owner_field))
        .collect::<Vec<_>>();
    let relations = resource
        .references()
        .into_iter()
        .map(|reference| RelationManifest {
            field: reference.field,
            relation: reference.relation,
            target: reference.target,
        })
        .collect::<Vec<_>>();
    let scope = if resource.is_org_scoped() {
        "organization"
    } else {
        "global"
    };
    let permissions = ActionPermissionsManifest {
        list: permission_manifest(&resource.permissions.list, resource.is_org_scoped()),
        read: permission_manifest(&resource.permissions.read, resource.is_org_scoped()),
        create: permission_manifest(&resource.permissions.create, resource.is_org_scoped()),
        update: permission_manifest(&resource.permissions.update, resource.is_org_scoped()),
        delete: permission_manifest(&resource.permissions.delete, resource.is_org_scoped()),
    };
    let resource_path = if base_path.is_empty() {
        format!("/{}", resource.meta.name)
    } else {
        format!("{}/{}", base_path, resource.meta.name)
    };

    ResourceManifest {
        name: resource.meta.name.clone(),
        builtin: is_builtin_resource(&resource.meta.name),
        scope,
        owner_field: resource.meta.owner_field.clone(),
        fields,
        relations,
        permissions,
        permission_summary: if resource.is_org_scoped() {
            "Org-scoped: every request runs inside the active organization, and the API still enforces owner and role checks per user.".to_string()
        } else {
            "Global: no tenant filter is injected, so only the declared action permissions gate access.".to_string()
        },
        endpoint_summary: format!(
            "Collection at {resource_path}; individual records at {resource_path}/{{id}}; nested lists remain available on parents that reference this resource."
        ),
    }
}

fn is_builtin_resource(name: &str) -> bool {
    matches!(
        name,
        "organization" | "membership" | "user" | "api_key" | "oauth_connection"
    )
}

fn field_manifest(name: &str, field: &Field, owner_field: &str) -> FieldManifest {
    let references = field.references.clone();
    let relation = references.as_ref().map(|_| relation_name(name).to_string());

    FieldManifest {
        name: name.to_string(),
        ty: field_type_name(field.ty),
        required: field.required,
        unique: field.unique,
        hidden: field.hidden,
        references,
        relation,
        on_delete: field.on_delete.map(on_delete_name),
        default_value: field.default.clone(),
        writable: !field.hidden && name != owner_field && name != "organization_id",
    }
}

fn permission_manifest(access: &Access, org_scoped: bool) -> ActionPermissionManifest {
    ActionPermissionManifest {
        value: access_value(access),
        note: access_note(access, org_scoped),
        requires_org: org_scoped || matches!(access, Access::Role(_)),
    }
}

fn function_manifest(name: &str, manifest: &apiplant_abi::FunctionManifest) -> FunctionManifest {
    let visibility = visibility_value(manifest.visibility);
    let role = (!manifest.role.is_empty()).then(|| manifest.role.to_string());

    FunctionManifest {
        name: name.to_string(),
        description: manifest.description.to_string(),
        method: method_name(manifest.method),
        visibility,
        visibility_label: match &role {
            Some(role) => format!("role:{role}"),
            None => visibility.to_string(),
        },
        role,
        note: visibility_note(manifest.visibility, manifest.role.as_str()),
    }
}

fn access_value(access: &Access) -> String {
    match access {
        Access::Public => "public".to_string(),
        Access::Authenticated => "authenticated".to_string(),
        Access::Member => "member".to_string(),
        Access::Owner => "owner".to_string(),
        Access::Role(role) => format!("role:{role}"),
        Access::Private => "private".to_string(),
    }
}

fn access_note(access: &Access, org_scoped: bool) -> String {
    if org_scoped {
        return match access {
            Access::Private => "Not exposed.".to_string(),
            Access::Owner => "Requires membership of the active organization; the API narrows the operation to rows owned by the caller.".to_string(),
            Access::Role(role) => {
                format!("Requires the `{role}` role in the active organization.")
            }
            _ => "Requires membership of the active organization.".to_string(),
        };
    }

    match access {
        Access::Public => "Public — no authentication required.".to_string(),
        Access::Authenticated => "Requires authentication.".to_string(),
        Access::Member => {
            "Requires authentication; `member` on a global resource behaves like `authenticated`."
                .to_string()
        }
        Access::Owner => {
            "Requires authentication; the API narrows the operation to rows owned by the caller."
                .to_string()
        }
        Access::Role(role) => format!("Requires the `{role}` role in the active organization."),
        Access::Private => "Not exposed.".to_string(),
    }
}

fn visibility_value(visibility: Visibility) -> &'static str {
    match visibility {
        Visibility::Public => "public",
        Visibility::Authenticated => "authenticated",
        Visibility::RoleGated => "role",
        Visibility::Private => "private",
    }
}

fn visibility_note(visibility: Visibility, role: &str) -> String {
    match visibility {
        Visibility::Public => "Public — callable without authentication.".to_string(),
        Visibility::Authenticated => "Requires an authenticated user or API key.".to_string(),
        Visibility::RoleGated => {
            let role = if role.is_empty() { "required" } else { role };
            format!("Requires the `{role}` role in the active organization.")
        }
        Visibility::Private => {
            "Private hooks are not emitted into the static admin panel.".to_string()
        }
    }
}

fn field_type_name(ty: FieldType) -> &'static str {
    match ty {
        FieldType::String => "string",
        FieldType::Text => "text",
        FieldType::Integer => "integer",
        FieldType::BigInt => "big_int",
        FieldType::Float => "float",
        FieldType::Boolean => "boolean",
        FieldType::Uuid => "uuid",
        FieldType::Timestamp => "timestamp",
        FieldType::Json => "json",
        FieldType::Reference => "reference",
    }
}

fn on_delete_name(on_delete: OnDelete) -> &'static str {
    match on_delete {
        OnDelete::Restrict => "restrict",
        OnDelete::SetNull => "set_null",
        OnDelete::Cascade => "cascade",
        OnDelete::NoAction => "no_action",
    }
}

fn method_name(method: HttpMethod) -> &'static str {
    match method {
        HttpMethod::Get => "GET",
        HttpMethod::Post => "POST",
        HttpMethod::Put => "PUT",
        HttpMethod::Delete => "DELETE",
    }
}

fn normalize_api_base(raw: &str, base_path: &str, prefer_https: bool) -> Result<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        bail!("--api requires a domain or full API URL");
    }

    let mut url = if trimmed.contains("://") {
        trimmed.to_string()
    } else {
        format!(
            "{}://{}",
            if prefer_https { "https" } else { "http" },
            trimmed
        )
    };

    if !url.starts_with("http://") && !url.starts_with("https://") {
        bail!("--api must resolve to an http:// or https:// URL");
    }

    let scheme_end = url
        .find("://")
        .map(|index| index + 3)
        .ok_or_else(|| anyhow!("invalid API URL"))?;

    match url[scheme_end..].find('/') {
        None => {
            if !base_path.is_empty() {
                url.push_str(base_path);
            }
        }
        Some(relative_start) => {
            let path_start = scheme_end + relative_start;
            let path = &url[path_start..];
            if path == "/" {
                url.truncate(path_start);
                if !base_path.is_empty() {
                    url.push_str(base_path);
                }
            } else {
                while url.ends_with('/') {
                    url.pop();
                }
            }
        }
    }

    while url.ends_with('/') {
        url.pop();
    }

    Ok(url)
}

fn admin_css() -> String {
    APP_CSS
        .replace("url(/head.png)", "url(./head.png)")
        .replace("url(/head-inverted.png)", "url(./head-inverted.png)")
}

fn write_text(path: PathBuf, text: &str) -> Result<()> {
    fs::write(&path, text).with_context(|| format!("failed to write {}", path.display()))
}

fn write_bytes(path: PathBuf, bytes: &[u8]) -> Result<()> {
    fs::write(&path, bytes).with_context(|| format!("failed to write {}", path.display()))
}

fn write_json(path: PathBuf, manifest: &AdminManifest) -> Result<()> {
    let bytes = serde_json::to_vec_pretty(manifest)?;
    fs::write(&path, bytes).with_context(|| format!("failed to write {}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_dir(label: &str) -> PathBuf {
        let mut dir = std::env::temp_dir();
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        dir.push(format!(
            "apiplant-admin-{label}-{}-{stamp}",
            std::process::id()
        ));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn api_base_uses_app_base_path_when_only_a_domain_is_given() {
        assert_eq!(
            normalize_api_base("admin.example.com", "/api", true).unwrap(),
            "https://admin.example.com/api"
        );
        assert_eq!(
            normalize_api_base("127.0.0.1:8099", "", false).unwrap(),
            "http://127.0.0.1:8099"
        );
    }

    #[test]
    fn explicit_api_paths_are_preserved() {
        assert_eq!(
            normalize_api_base("https://example.com/custom/", "/api", true).unwrap(),
            "https://example.com/custom"
        );
        assert_eq!(
            normalize_api_base("https://example.com/", "/api", true).unwrap(),
            "https://example.com/api"
        );
    }

    #[test]
    fn build_writes_static_admin_files_and_manifest() {
        let app_dir = temp_dir("app");
        let out_dir = temp_dir("out");

        fs::create_dir_all(app_dir.join("models")).unwrap();
        fs::write(
            app_dir.join("main.toml"),
            r#"
[server]
base_path = "/api"

[auth]
allow_registration = true
"#,
        )
        .unwrap();
        fs::write(
            app_dir.join("models/post.toml"),
            r#"
[resource]
name = "post"

[permissions]
list = "public"
read = "public"
create = "authenticated"
update = "owner"
delete = "role:admin"

[fields.title]
type = "string"
required = true

[fields.body]
type = "text"
"#,
        )
        .unwrap();

        let written = build(
            &app_dir,
            Options {
                api: "https://example.com".to_string(),
                out: Some(out_dir.clone()),
            },
        )
        .unwrap();

        assert_eq!(written, out_dir);
        assert!(out_dir.join("index.html").exists());
        assert!(out_dir.join("app.js").exists());
        assert!(out_dir.join("app.css").exists());
        assert!(out_dir.join("head.png").exists());
        assert!(out_dir.join("head-inverted.png").exists());

        let manifest: Value =
            serde_json::from_slice(&fs::read(out_dir.join("apiplant-admin.json")).unwrap())
                .unwrap();
        assert_eq!(manifest["api_base_url"], "https://example.com/api");
        assert_eq!(manifest["auth"]["identity_field"], "email");
        assert!(manifest["resources"]
            .as_array()
            .unwrap()
            .iter()
            .any(|resource| resource["name"] == "post"));
        assert!(manifest["functions"].as_array().unwrap().is_empty());

        fs::remove_dir_all(app_dir).unwrap();
        fs::remove_dir_all(out_dir).unwrap();
    }
}
