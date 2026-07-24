//! The loaded application: an app directory turned into config + resources.

use crate::config::Config;
use crate::schema::{Field, FieldType, OnDelete, Resource};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// Everything the server needs, assembled from an app directory.
#[derive(Debug, Clone)]
pub struct App {
    /// Root of the app directory.
    pub root: PathBuf,
    pub config: Config,
    /// All resources, keyed by name. Built-ins are included (and overridable).
    pub resources: BTreeMap<String, Resource>,
    /// Present when an `https/` directory with cert + key was found.
    pub tls: Option<TlsPaths>,
    /// Directory scanned for compiled function libraries.
    pub functions_dir: PathBuf,
}

/// Resolved TLS material.
#[derive(Debug, Clone)]
pub struct TlsPaths {
    pub cert: PathBuf,
    pub key: PathBuf,
}

impl App {
    /// Load an app directory. Missing pieces fall back to safe defaults, so the
    /// smallest valid app is an empty directory.
    pub fn load(root: impl AsRef<Path>) -> crate::Result<App> {
        let root = root.as_ref().to_path_buf();
        let config = Config::load(&root)?;

        let mut resources = BTreeMap::new();

        // 1. Seed with the built-ins.
        for (name, src) in crate::defaults::builtins() {
            resources.insert(name.to_string(), crate::defaults::parse_builtin(src));
        }

        // 2. Load user-defined models, overriding built-ins by name.
        let models_dir = root.join("models");
        if models_dir.is_dir() {
            for entry in std::fs::read_dir(&models_dir).map_err(|e| crate::Error::Io {
                path: models_dir.clone(),
                source: e,
            })? {
                let entry = entry.map_err(|e| crate::Error::Io {
                    path: models_dir.clone(),
                    source: e,
                })?;
                let path = entry.path();
                if path.extension().and_then(|e| e.to_str()) != Some("toml") {
                    continue;
                }
                let resource = Resource::load(&path)?;
                tracing::info!(resource = %resource.meta.name, "loaded model");
                resources.insert(resource.meta.name.clone(), resource);
            }
        }

        // 3. Make multitenancy automatic: every org-scoped resource carries an
        //    `organization_id` foreign key. Inject it where the author didn't
        //    declare one, so the column, its FK, and org filtering all just work.
        for resource in resources.values_mut() {
            if resource.is_org_scoped() && !resource.fields.contains_key("organization_id") {
                resource.fields.insert(
                    "organization_id".to_string(),
                    Field {
                        ty: FieldType::Reference,
                        references: Some("organization".to_string()),
                        required: true,
                        unique: false,
                        hidden: false,
                        default: None,
                        max_length: None,
                        on_delete: Some(OnDelete::Cascade),
                    },
                );
            }
        }

        // 4. TLS is inferred from the presence of an `https/` directory.
        let tls = Self::detect_tls(&root);
        if tls.is_some() {
            tracing::info!("https/ directory found — serving over TLS");
        }

        Ok(App {
            functions_dir: root.join("functions"),
            root,
            config,
            resources,
            tls,
        })
    }

    /// Look for a cert + key under `https/`, tolerating common filenames.
    fn detect_tls(root: &Path) -> Option<TlsPaths> {
        let dir = root.join("https");
        if !dir.is_dir() {
            return None;
        }
        let cert = ["cert.pem", "fullchain.pem", "certificate.pem", "server.crt"]
            .iter()
            .map(|f| dir.join(f))
            .find(|p| p.exists())?;
        let key = ["key.pem", "privkey.pem", "server.key", "private.pem"]
            .iter()
            .map(|f| dir.join(f))
            .find(|p| p.exists())?;
        Some(TlsPaths { cert, key })
    }

    /// Resource names in dependency order (referenced resources first), so a
    /// migrator can create tables without violating foreign keys.
    pub fn resources_in_dependency_order(&self) -> Vec<&Resource> {
        let mut ordered: Vec<&Resource> = Vec::new();
        let mut placed: std::collections::HashSet<&str> = std::collections::HashSet::new();

        // Simple repeated passes; resource graphs are tiny.
        let mut remaining: Vec<&Resource> = self.resources.values().collect();
        while !remaining.is_empty() {
            let mut progressed = false;
            remaining.retain(|r| {
                let deps_ready = r.fields.values().all(|f| match &f.references {
                    Some(target) => placed.contains(target.as_str()) || target == &r.meta.name,
                    None => true,
                });
                if deps_ready {
                    ordered.push(r);
                    placed.insert(r.meta.name.as_str());
                    progressed = true;
                    false
                } else {
                    true
                }
            });
            if !progressed {
                // Cyclic or dangling reference — emit the rest as-is rather than loop forever.
                ordered.extend(remaining.drain(..));
            }
        }
        ordered
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_app_dir(label: &str) -> PathBuf {
        let mut dir = std::env::temp_dir();
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        dir.push(format!("apiplant-app-{label}-{}-{stamp}", std::process::id()));
        fs::create_dir_all(dir.join("models")).unwrap();
        dir
    }

    #[test]
    fn empty_app_loads_builtins_and_no_tls() {
        let dir = temp_app_dir("empty");
        let app = App::load(&dir).unwrap();

        assert!(app.resources.contains_key("organization"));
        assert!(app.resources.contains_key("membership"));
        assert!(app.resources.contains_key("user"));
        assert!(app.resources.contains_key("api_key"));
        assert!(app.resources.contains_key("oauth_connection"));
        assert!(app.tls.is_none());
        assert_eq!(app.functions_dir, dir.join("functions"));

        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn org_scoped_resources_get_organization_id_injected() {
        let dir = temp_app_dir("org-scope");
        fs::write(
            dir.join("models/post.toml"),
            r#"
[resource]
name = "post"

[fields.title]
type = "string"
required = true
"#,
        )
        .unwrap();
        fs::write(
            dir.join("models/plan.toml"),
            r#"
[resource]
name = "plan"
scope = "global"

[fields.name]
type = "string"
"#,
        )
        .unwrap();

        let app = App::load(&dir).unwrap();
        let post = app.resources.get("post").unwrap();
        let plan = app.resources.get("plan").unwrap();

        let org = post.fields.get("organization_id").unwrap();
        assert_eq!(org.ty, FieldType::Reference);
        assert_eq!(org.references.as_deref(), Some("organization"));
        assert!(org.required);
        assert_eq!(org.on_delete, Some(OnDelete::Cascade));
        assert!(!plan.fields.contains_key("organization_id"));

        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn same_named_model_replaces_builtin_resource() {
        let dir = temp_app_dir("override-user");
        fs::write(
            dir.join("models/users.toml"),
            r#"
[resource]
name = "user"
scope = "global"

[auth]
identity_field = "username"
password_field = "password_hash"

[fields.username]
type = "string"
required = true

[fields.password_hash]
type = "string"
hidden = true
"#,
        )
        .unwrap();

        let app = App::load(&dir).unwrap();
        let user = app.resources.get("user").unwrap();

        assert!(user.fields.contains_key("username"));
        assert!(!user.fields.contains_key("email"));
        assert_eq!(
            user.auth.as_ref().unwrap().identity_field,
            "username"
        );

        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn tls_detection_accepts_common_cert_and_key_names() {
        let dir = temp_app_dir("tls");
        fs::create_dir_all(dir.join("https")).unwrap();
        fs::write(dir.join("https/fullchain.pem"), "cert").unwrap();
        fs::write(dir.join("https/privkey.pem"), "key").unwrap();

        let app = App::load(&dir).unwrap();
        let tls = app.tls.unwrap();
        assert_eq!(tls.cert, dir.join("https/fullchain.pem"));
        assert_eq!(tls.key, dir.join("https/privkey.pem"));

        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn dependency_order_places_parents_before_children() {
        let dir = temp_app_dir("deps");
        fs::write(
            dir.join("models/post.toml"),
            r#"
[resource]
name = "post"

[fields.owner_id]
type = "reference"
references = "user"

[fields.title]
type = "string"
"#,
        )
        .unwrap();
        fs::write(
            dir.join("models/comment.toml"),
            r#"
[resource]
name = "comment"

[fields.post_id]
type = "reference"
references = "post"

[fields.owner_id]
type = "reference"
references = "user"

[fields.body]
type = "text"
"#,
        )
        .unwrap();

        let app = App::load(&dir).unwrap();
        let order: Vec<_> = app
            .resources_in_dependency_order()
            .into_iter()
            .map(|r| r.meta.name.as_str())
            .collect();

        let user_idx = order.iter().position(|name| *name == "user").unwrap();
        let post_idx = order.iter().position(|name| *name == "post").unwrap();
        let comment_idx = order.iter().position(|name| *name == "comment").unwrap();

        assert!(user_idx < post_idx);
        assert!(post_idx < comment_idx);

        fs::remove_dir_all(dir).unwrap();
    }
}
