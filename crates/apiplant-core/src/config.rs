//! `main.toml` — the top-level server configuration.
//!
//! Every field is optional: a missing file, or a file missing any given key,
//! falls back to a safe default. The only piece of configuration that is
//! *inferred* rather than declared is TLS: if the app directory contains an
//! `https/` folder with a cert + key, the server serves HTTPS.

use serde::Deserialize;
use std::path::Path;

/// Fully-resolved server configuration.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct Config {
    pub server: ServerConfig,
    pub database: DatabaseConfig,
    pub auth: AuthConfig,
    pub docs: DocsConfig,
    pub admin: AdminConfig,
    pub public: PublicConfig,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct ServerConfig {
    /// Interface to bind, e.g. `0.0.0.0`.
    pub host: String,
    /// TCP port.
    pub port: u16,
    /// Only answer requests for this `Host:` header. `None` = answer any host.
    pub domain: Option<String>,
    /// Sub-path the API is mounted under, e.g. `/api`. Always starts with `/`
    /// and never ends with one (normalised on load).
    pub base_path: String,
    /// Number of worker threads. `None` = one per CPU.
    pub workers: Option<usize>,
}

impl Default for ServerConfig {
    fn default() -> Self {
        ServerConfig {
            host: "0.0.0.0".to_string(),
            port: 8080,
            domain: None,
            base_path: "/".to_string(),
            workers: None,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct DatabaseConfig {
    /// Full connection URL. When empty it is assembled from the parts below.
    pub url: String,
    pub host: String,
    pub port: u16,
    pub name: String,
    pub user: String,
    pub password: String,
    /// Max pool connections.
    pub max_connections: u32,
    /// Run pending migrations on boot.
    pub auto_migrate: bool,
}

impl Default for DatabaseConfig {
    fn default() -> Self {
        DatabaseConfig {
            url: String::new(),
            host: "localhost".to_string(),
            port: 5432,
            name: "apiplant".to_string(),
            user: "postgres".to_string(),
            password: "postgres".to_string(),
            max_connections: 16,
            auto_migrate: true,
        }
    }
}

impl DatabaseConfig {
    /// The connection URL, assembled from parts when `url` was left empty.
    pub fn resolved_url(&self) -> String {
        if !self.url.is_empty() {
            return self.url.clone();
        }
        format!(
            "postgres://{}:{}@{}:{}/{}",
            self.user, self.password, self.host, self.port, self.name
        )
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct AuthConfig {
    /// Secret used to sign session JWTs. Auto-generated (and warned about) when
    /// left empty — set it in production so tokens survive restarts.
    pub jwt_secret: String,
    /// Session token lifetime in seconds.
    pub session_ttl_secs: u64,
    /// Allow self-service signup on `POST /auth/register`.
    pub allow_registration: bool,
}

impl Default for AuthConfig {
    fn default() -> Self {
        AuthConfig {
            jwt_secret: String::new(),
            session_ttl_secs: 60 * 60 * 24 * 7,
            allow_registration: true,
        }
    }
}

/// Interactive API documentation (OpenAPI spec + Swagger UI).
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct DocsConfig {
    /// Serve the OpenAPI spec and Swagger UI (default true).
    pub enabled: bool,
    /// Path (under `base_path`) the Swagger UI is served at.
    pub path: String,
    /// Title shown in the UI and the spec's `info.title`.
    pub title: String,
}

impl Default for DocsConfig {
    fn default() -> Self {
        DocsConfig {
            enabled: true,
            path: "/docs".to_string(),
            title: "apiplant API".to_string(),
        }
    }
}

/// The built-in admin dashboard, served from the binary itself.
///
/// Every app gets one without generating anything: the interface is embedded in
/// `apiplant` and its manifest is derived from the app on boot. Turn it off for
/// a deployment that shouldn't expose an operator console at all.
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct AdminConfig {
    /// Serve the admin dashboard (default true).
    pub enabled: bool,
    /// Path the dashboard is served at, outside `base_path`.
    pub path: String,
}

impl Default for AdminConfig {
    fn default() -> Self {
        AdminConfig {
            enabled: true,
            path: "/admin".to_string(),
        }
    }
}

/// Static files served from the app's `public/` directory.
///
/// When the directory exists its contents are served at the site root, so
/// `public/index.html` answers `/` and `public/style.css` answers `/style.css`.
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct PublicConfig {
    /// Serve `dir` at the root when it exists (default true).
    pub enabled: bool,
    /// Directory (relative to the app root) holding the static site.
    pub dir: String,
    /// Page returned for requests that match nothing, relative to `dir`.
    /// Defaults to `404.html` when that file exists.
    pub not_found: Option<String>,
}

impl Default for PublicConfig {
    fn default() -> Self {
        PublicConfig {
            enabled: true,
            dir: "public".to_string(),
            not_found: None,
        }
    }
}

impl Config {
    /// Load `main.toml` from an app directory, applying defaults for anything
    /// absent. A missing file is not an error.
    pub fn load(app_dir: &Path) -> crate::Result<Self> {
        let path = app_dir.join("main.toml");
        let mut config = if path.exists() {
            let text = std::fs::read_to_string(&path).map_err(|e| crate::Error::Io {
                path: path.clone(),
                source: e,
            })?;
            toml::from_str::<Config>(&text).map_err(|e| crate::Error::Toml { path, source: e })?
        } else {
            tracing::info!("no main.toml found, using defaults");
            Config::default()
        };
        config.normalise();
        Ok(config)
    }

    fn normalise(&mut self) {
        let bp = self.server.base_path.trim_end_matches('/');
        self.server.base_path = if bp.is_empty() {
            String::new()
        } else if bp.starts_with('/') {
            bp.to_string()
        } else {
            format!("/{bp}")
        };

        if !self.docs.path.starts_with('/') {
            self.docs.path = format!("/{}", self.docs.path);
        }

        let admin = self.admin.path.trim_matches('/');
        self.admin.path = if admin.is_empty() {
            AdminConfig::default().path
        } else {
            format!("/{admin}")
        };
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_dir(label: &str) -> std::path::PathBuf {
        let mut dir = std::env::temp_dir();
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        dir.push(format!(
            "apiplant-config-{label}-{}-{stamp}",
            std::process::id()
        ));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn missing_main_toml_uses_defaults() {
        let dir = temp_dir("defaults");
        let config = Config::load(&dir).unwrap();

        assert_eq!(config.server.host, "0.0.0.0");
        assert_eq!(config.server.port, 8080);
        assert_eq!(config.server.base_path, "");
        assert_eq!(
            config.database.resolved_url(),
            "postgres://postgres:postgres@localhost:5432/apiplant"
        );
        assert!(config.auth.allow_registration);
        assert!(config.docs.enabled);
        assert_eq!(config.docs.path, "/docs");
        // The dashboard and the public site are on by default; an app opts out.
        assert!(config.admin.enabled);
        assert_eq!(config.admin.path, "/admin");
        assert!(config.public.enabled);
        assert_eq!(config.public.dir, "public");
        assert_eq!(config.public.not_found, None);

        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn load_normalises_paths_and_prefers_explicit_database_url() {
        let dir = temp_dir("normalise");
        fs::write(
            dir.join("main.toml"),
            r#"
[server]
base_path = "api/"
workers = 8

[database]
url = "postgres://db.example/custom"
host = "ignored"
port = 9999
name = "ignored"
user = "ignored"
password = "ignored"

[docs]
path = "swagger"

[admin]
path = "console/"

[public]
dir = "site"
not_found = "oops.html"
"#,
        )
        .unwrap();

        let config = Config::load(&dir).unwrap();

        assert_eq!(config.server.base_path, "/api");
        assert_eq!(config.server.workers, Some(8));
        assert_eq!(config.docs.path, "/swagger");
        assert_eq!(config.admin.path, "/console");
        assert_eq!(config.public.dir, "site");
        assert_eq!(config.public.not_found.as_deref(), Some("oops.html"));
        assert_eq!(
            config.database.resolved_url(),
            "postgres://db.example/custom"
        );

        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn resolved_url_is_assembled_from_parts_when_url_is_empty() {
        let config = DatabaseConfig {
            url: String::new(),
            host: "db".into(),
            port: 5433,
            name: "plants".into(),
            user: "alice".into(),
            password: "secret".into(),
            max_connections: 16,
            auto_migrate: true,
        };

        assert_eq!(
            config.resolved_url(),
            "postgres://alice:secret@db:5433/plants"
        );
    }
}
