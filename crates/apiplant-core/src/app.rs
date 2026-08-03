//! The loaded application: an app directory turned into config + resources.

use crate::agent::Agent;
use crate::config::Config;
use crate::schema::{Field, FieldAdmin, FieldType, OnDelete, Resource};
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
    /// All configured agents, keyed by name.
    pub agents: BTreeMap<String, Agent>,
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
        let mut agents = BTreeMap::new();

        // 1. Seed with the built-ins.
        for (name, src) in crate::defaults::builtins() {
            resources.insert(name.to_string(), crate::defaults::parse_builtin(src));
        }

        // 1b. The billing resources, only for an app that takes money. They
        //     are seeded here rather than in `builtins()` so that an app with
        //     no `[payments]` provider carries neither the tables nor the
        //     endpoints — see `defaults::billing_builtins`.
        if config.payments.enabled() {
            for (name, src) in crate::defaults::billing_builtins() {
                resources.insert(name.to_string(), crate::defaults::parse_builtin(src));
            }
        }

        // 1c. Configured agents, loaded from `agents/`, each optionally adding
        //     generated history resources.
        let agents_dir = root.join("agents");
        if agents_dir.is_dir() {
            for entry in std::fs::read_dir(&agents_dir).map_err(|e| crate::Error::Io {
                path: agents_dir.clone(),
                source: e,
            })? {
                let entry = entry.map_err(|e| crate::Error::Io {
                    path: agents_dir.clone(),
                    source: e,
                })?;
                let path = entry.path();
                if path.extension().and_then(|e| e.to_str()) != Some("toml") {
                    continue;
                }
                let agent = Agent::load(&path)?;
                for (name, resource) in agent.storage_resources()? {
                    resources.insert(name, resource);
                }
                tracing::info!(agent = %agent.meta.name, "loaded agent");
                agents.insert(agent.meta.name.clone(), agent);
            }
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
                        // Injected and stamped by the framework — an operator
                        // should never see, let alone type, a tenant id.
                        admin: FieldAdmin {
                            visible: false,
                            ..FieldAdmin::default()
                        },
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
            agents,
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

    /// What to call this app wherever it is named to a person — the admin
    /// dashboard's header, the API docs, the CLI.
    ///
    /// `[app] name` when the app gives itself one; the directory it lives in
    /// otherwise, which is a filing decision (`07-functions`, `backend`) rather
    /// than a name anybody should have to read. A blank name is not a name: it
    /// would render as a heading with nothing in it, so it falls back too.
    pub fn display_name(&self) -> String {
        self.config
            .app
            .name
            .as_deref()
            .map(str::trim)
            .filter(|name| !name.is_empty())
            .map(str::to_string)
            .unwrap_or_else(|| {
                self.root
                    .file_name()
                    .and_then(|name| name.to_str())
                    .unwrap_or("apiplant app")
                    .to_string()
            })
    }

    /// Title for the API docs: `[docs] title` when set, the app's name
    /// otherwise — so an app that names itself once is named everywhere.
    pub fn docs_title(&self) -> String {
        self.config
            .docs
            .title
            .as_deref()
            .map(str::trim)
            .filter(|title| !title.is_empty())
            .map(str::to_string)
            .unwrap_or_else(|| self.display_name())
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
                ordered.append(&mut remaining);
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
        dir.push(format!(
            "apiplant-app-{label}-{}-{stamp}",
            std::process::id()
        ));
        fs::create_dir_all(dir.join("models")).unwrap();
        dir
    }

    /// One name, given once: the dashboard header and the API docs both read
    /// it, and an app that never names itself is called after its directory.
    #[test]
    fn display_name_and_docs_title_share_one_source() {
        let dir = temp_app_dir("naming");
        let directory = dir.file_name().unwrap().to_str().unwrap().to_string();

        let unnamed = App::load(&dir).unwrap();
        assert_eq!(unnamed.display_name(), directory);
        assert_eq!(unnamed.docs_title(), directory);

        fs::write(dir.join("main.toml"), "[app]\nname = \"Acme Logistics\"\n").unwrap();
        let named = App::load(&dir).unwrap();
        assert_eq!(named.display_name(), "Acme Logistics");
        assert_eq!(named.docs_title(), "Acme Logistics");

        // A blank name is not a name — it would render as an empty heading.
        fs::write(dir.join("main.toml"), "[app]\nname = \"   \"\n").unwrap();
        assert_eq!(App::load(&dir).unwrap().display_name(), directory);

        // `[docs] title` still wins for the docs alone, for an app whose API is
        // published under a different name than the app answers to.
        fs::write(
            dir.join("main.toml"),
            "[app]\nname = \"Acme Logistics\"\n\n[docs]\ntitle = \"Acme Freight API\"\n",
        )
        .unwrap();
        let split = App::load(&dir).unwrap();
        assert_eq!(split.display_name(), "Acme Logistics");
        assert_eq!(split.docs_title(), "Acme Freight API");

        fs::remove_dir_all(&dir).unwrap();
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
        assert!(app.agents.is_empty());
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
        assert_eq!(user.auth.as_ref().unwrap().identity_field, "username");

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

    #[test]
    fn agents_are_loaded_and_seed_history_resources() {
        let dir = temp_app_dir("agent");
        fs::create_dir_all(dir.join("agents")).unwrap();
        fs::write(
            dir.join("agents/coach.toml"),
            r#"
[agent]
name = "coach"
system = "Be helpful."
storage.enabled = true

[permissions]
chat = "authenticated"
history = "owner"
"#,
        )
        .unwrap();

        let app = App::load(&dir).unwrap();
        assert!(app.agents.contains_key("coach"));
        assert!(app.resources.contains_key("ai_coach_thread"));
        assert!(app.resources.contains_key("ai_coach_message"));

        fs::remove_dir_all(dir).unwrap();
    }
}
