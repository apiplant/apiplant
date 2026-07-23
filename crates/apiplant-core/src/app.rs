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
