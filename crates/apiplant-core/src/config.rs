//! `main.toml` — the top-level server configuration.
//!
//! Every field is optional: a missing file, or a file missing any given key,
//! falls back to a safe default. The only piece of configuration that is
//! *inferred* rather than declared is TLS: if the app directory contains an
//! `https/` folder with a cert + key, the server serves HTTPS.
//!
//! ## Environment variables
//!
//! Any string value here — like any string in any of the app's TOML files — may
//! reference the environment: `url = "$DATABASE_URL"`,
//! `port = "${PORT:-8080}"`. See [`crate::env`] for the syntax.

use serde::Deserialize;
use std::path::Path;

/// Fully-resolved server configuration.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct Config {
    pub app: AppConfig,
    pub server: ServerConfig,
    pub database: DatabaseConfig,
    pub auth: AuthConfig,
    pub docs: DocsConfig,
    pub admin: AdminConfig,
    pub public: PublicConfig,
    pub email: EmailConfig,
    pub cache: CacheConfig,
}

/// What the app calls itself.
///
/// The directory an app lives in is a developer's filing decision —
/// `07-functions`, `api-v2`, `backend` — and the dashboard header is read by
/// people who never see it. This is where an app says the name they should
/// read instead.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct AppConfig {
    /// Display name, used wherever the app is named to a person — the admin
    /// dashboard's header and title. Unset falls back to the directory name.
    pub name: Option<String>,
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
/// Every app gets one, and there is only one: the interface is embedded in
/// `apiplant` and its manifest is derived from the app on boot. Turn it off for
/// a deployment that shouldn't expose an operator console at all — an app that
/// wants its own can serve one from `public/` like any other page.
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct AdminConfig {
    /// Serve the admin dashboard (default true).
    pub enabled: bool,
    /// Path the dashboard is served at, outside `base_path`.
    pub path: String,
    /// Image shown in place of the apiplant mark, as a URL the browser can
    /// fetch — usually a file in `public/`. Unset keeps the apiplant mark.
    pub logo: Option<String>,
}

impl Default for AdminConfig {
    fn default() -> Self {
        AdminConfig {
            enabled: true,
            path: "/admin".to_string(),
            logo: None,
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

/// Outbound email: which provider sends it, and the credentials to do so.
///
/// Off by default (`provider = "none"`): an app that never sends mail carries
/// no configuration and no client. Turning it on is one line plus a key, and
/// every provider is reached through the same [`send_email`] call from a
/// function — swapping SendGrid for SES is a config change, not a code change.
///
/// [`send_email`]: https://docs.rs/apiplant-function
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct EmailConfig {
    /// `none` (default), `smtp`, `ses`, `sendgrid`, `brevo` (aka `sendinblue`),
    /// `mailjet`, `mailgun`, `postmark` or `resend`.
    pub provider: String,
    /// Envelope sender, e.g. `no-reply@example.com`. Required once enabled; a
    /// message may override it per-send.
    pub from: String,
    /// Display name shown beside `from`.
    pub from_name: String,
    /// Default `Reply-To`. Empty = none.
    pub reply_to: String,
    /// The provider's API key. For `ses` this is the AWS access key id; for
    /// `mailjet` the public key; for `smtp` it is unused (see [`SmtpConfig`]).
    pub api_key: String,
    /// The second half of a two-part credential: the AWS secret access key for
    /// `ses`, the private key for `mailjet`. Unused elsewhere.
    pub api_secret: String,
    /// AWS region for `ses`, e.g. `eu-west-1`.
    pub region: String,
    /// Sending domain for `mailgun`, e.g. `mg.example.com`.
    pub domain: String,
    /// How long one send may take before it is abandoned.
    pub timeout_secs: u64,
    /// Connection details for `provider = "smtp"`.
    pub smtp: SmtpConfig,
}

impl Default for EmailConfig {
    fn default() -> Self {
        EmailConfig {
            provider: "none".to_string(),
            from: String::new(),
            from_name: String::new(),
            reply_to: String::new(),
            api_key: String::new(),
            api_secret: String::new(),
            region: String::new(),
            domain: String::new(),
            timeout_secs: 15,
            smtp: SmtpConfig::default(),
        }
    }
}

impl EmailConfig {
    /// Whether a provider is configured at all. `none` and the empty string
    /// both mean "this app doesn't send mail".
    pub fn enabled(&self) -> bool {
        !matches!(
            self.provider.trim().to_ascii_lowercase().as_str(),
            "" | "none"
        )
    }
}

/// SMTP transport settings, used only when `provider = "smtp"`.
///
/// Every provider here also speaks SMTP, so this is the escape hatch for one
/// that has no first-class entry above — or for a company relay that has no API
/// at all.
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct SmtpConfig {
    pub host: String,
    /// `0` (the default) picks the port that matches `encryption`: 465 for
    /// `tls`, 587 for `starttls`, 25 for `none`.
    pub port: u16,
    pub username: String,
    pub password: String,
    /// `starttls` (default), `tls` (implicit TLS, usually port 465) or `none`.
    pub encryption: String,
}

impl Default for SmtpConfig {
    fn default() -> Self {
        SmtpConfig {
            host: String::new(),
            port: 0,
            username: String::new(),
            password: String::new(),
            encryption: "starttls".to_string(),
        }
    }
}

/// An optional Redis cache.
///
/// Nothing in the framework caches through it: resources, permissions and the
/// admin manifest all behave exactly the same whether it is configured or not.
/// It exists so a *function* has somewhere to put a rate-limit counter, a
/// memoised third-party response or a short-lived token — see the `cache_*`
/// helpers on a function's `Context`.
///
/// Off unless `url` is set, so an app that doesn't want one pays nothing.
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct CacheConfig {
    /// Turn the configured cache off without deleting its settings.
    pub enabled: bool,
    /// Connection URL, e.g. `redis://127.0.0.1:6379` or `rediss://…/0`. Empty
    /// (the default) means no cache.
    pub url: String,
    /// Prepended to every key a function uses, so several apps can share one
    /// Redis without colliding.
    pub prefix: String,
    /// Expiry applied to a `set` that doesn't ask for one. `0` = keys persist.
    pub default_ttl_secs: u64,
    /// How long one cache operation may take before it is abandoned.
    pub timeout_secs: u64,
}

impl Default for CacheConfig {
    fn default() -> Self {
        CacheConfig {
            enabled: true,
            url: String::new(),
            prefix: String::new(),
            default_ttl_secs: 0,
            timeout_secs: 5,
        }
    }
}

impl CacheConfig {
    /// Whether a cache should be connected: switched on *and* pointed at a
    /// server.
    pub fn is_active(&self) -> bool {
        self.enabled && !self.url.trim().is_empty()
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
            // `$VAR` in any string value is read from the environment here,
            // which is what keeps credentials out of a committed main.toml.
            crate::env::parse_toml::<Config>(&text, "main.toml")
                .map_err(|e| crate::Error::Toml { path, source: e })?
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
        // Email and cache are opt-in: an app that says nothing gets neither.
        assert!(!config.email.enabled());
        assert!(!config.cache.is_active());

        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn email_and_cache_load_from_their_sections() {
        let dir = temp_dir("email-cache");
        fs::write(
            dir.join("main.toml"),
            r#"
[email]
provider = "sendgrid"
from = "no-reply@example.com"
from_name = "Example"
api_key = "SG.literal"

[cache]
url = "redis://127.0.0.1:6379"
prefix = "example:"
default_ttl_secs = 300
"#,
        )
        .unwrap();

        let config = Config::load(&dir).unwrap();

        assert!(config.email.enabled());
        assert_eq!(config.email.provider, "sendgrid");
        assert_eq!(config.email.from, "no-reply@example.com");
        assert_eq!(config.email.api_key, "SG.literal");
        // Untouched defaults still apply inside a section that was given.
        assert_eq!(config.email.timeout_secs, 15);
        assert_eq!(config.email.smtp.encryption, "starttls");

        assert!(config.cache.is_active());
        assert_eq!(config.cache.prefix, "example:");
        assert_eq!(config.cache.default_ttl_secs, 300);

        fs::remove_dir_all(dir).unwrap();
    }

    /// `enabled = false` has to beat a perfectly good URL, or switching the
    /// cache off would mean deleting the settings needed to switch it back on.
    #[test]
    fn a_disabled_cache_stays_off_even_with_a_url() {
        let config = CacheConfig {
            enabled: false,
            url: "redis://127.0.0.1:6379".into(),
            ..CacheConfig::default()
        };
        assert!(!config.is_active());
    }

    /// `Config::load` reads its file through the same expansion every other
    /// app-directory TOML gets — including a URL assembled from several
    /// variables, which is the case a whole-value substitution can't do.
    #[test]
    fn load_expands_environment_references_anywhere_in_the_file() {
        std::env::set_var("APIPLANT_TEST_JWT", "from-env-jwt");
        std::env::set_var("APIPLANT_TEST_MAIL", "from-env-key");
        std::env::set_var("APIPLANT_TEST_DB_USER", "alice");
        std::env::set_var("APIPLANT_TEST_DB_PASS", "s3cret");
        let dir = temp_dir("env");
        fs::write(
            dir.join("main.toml"),
            r#"
[server]
domain = "${APIPLANT_TEST_DOMAIN:-api.example.com}"

[database]
url = "postgres://$APIPLANT_TEST_DB_USER:$APIPLANT_TEST_DB_PASS@db:5432/app"

[auth]
jwt_secret = "$APIPLANT_TEST_JWT"

[email]
provider = "brevo"
api_key = "${APIPLANT_TEST_MAIL}"
from = "no-reply@example.com"
"#,
        )
        .unwrap();

        let config = Config::load(&dir).unwrap();
        assert_eq!(
            config.database.resolved_url(),
            "postgres://alice:s3cret@db:5432/app"
        );
        assert_eq!(config.auth.jwt_secret, "from-env-jwt");
        assert_eq!(config.email.api_key, "from-env-key");
        // An unset variable falls back to the default written beside it.
        assert_eq!(config.server.domain.as_deref(), Some("api.example.com"));

        for name in [
            "APIPLANT_TEST_JWT",
            "APIPLANT_TEST_MAIL",
            "APIPLANT_TEST_DB_USER",
            "APIPLANT_TEST_DB_PASS",
        ] {
            std::env::remove_var(name);
        }
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
