//! The optional subsystems, under one name whether or not they are compiled in.
//!
//! `payments`, `ai`, `cache`, `email`, `oauth`, `storage` and `queues` are
//! Cargo features (`telemetry` has its own stand-in module, `telemetry_off`). With one on, its name here
//! is the real type from its crate. With it off, the name is a stand-in that
//! **cannot be constructed**: an uninhabited enum whose constructor fails for
//! an app that configured the subsystem and returns `None` for one that
//! didn't. Every value of it is therefore an `Option` that is always `None`,
//! so the code that holds one (the state, the function bridge, the queue
//! subscriber) compiles unchanged, and the code that would *use* one never
//! gets a value to use.
//!
//! The parts that cannot be written that way (routes, request types, the
//! function host's `chat` call) are behind `#[cfg(feature = "…")]` at the
//! site, and those are the only `cfg`s the features need.
//!
//! An app that configures a subsystem this build lacks is refused at boot by
//! [`check`], which names the feature to turn on. Serving with a `[payments]`
//! section silently ignored would look like a working shop until somebody
//! tried to pay.

use apiplant_core::App;

/// Why a subsystem's constructor refused: the app asked for something this
/// build was compiled without.
#[derive(Debug, thiserror::Error)]
#[error(
    "[{section}] is configured, but this build of apiplant-server has no `{feature}` feature \
     — enable it, or remove the section from main.toml"
)]
pub struct NotCompiledIn {
    pub section: &'static str,
    pub feature: &'static str,
}

impl NotCompiledIn {
    #[allow(dead_code)] // unused when every feature is on
    const fn new(section: &'static str, feature: &'static str) -> Self {
        NotCompiledIn { section, feature }
    }
}

/// Refuse an app that configures a subsystem this build was compiled without.
///
/// Called once at boot, before anything is connected, so the error is the
/// first thing the operator sees rather than a 404 from a missing route.
pub fn check(app: &App) -> Result<(), NotCompiledIn> {
    let _ = app;
    #[cfg(not(feature = "payments"))]
    if app.config.payments.enabled() {
        return Err(NotCompiledIn::new("payments", "payments"));
    }
    #[cfg(not(feature = "ai"))]
    if app.config.ai.enabled() || app.agents.values().any(|agent| agent.ai.is_some()) {
        return Err(NotCompiledIn::new("ai", "ai"));
    }
    #[cfg(not(feature = "cache"))]
    if app.config.cache.is_active() {
        return Err(NotCompiledIn::new("cache", "cache"));
    }
    #[cfg(not(feature = "telemetry"))]
    if app.config.observability.enabled {
        return Err(NotCompiledIn::new("observability", "telemetry"));
    }
    #[cfg(not(feature = "email"))]
    if app.config.email.enabled() {
        return Err(NotCompiledIn::new("email", "email"));
    }
    #[cfg(not(feature = "oauth"))]
    if app.config.oauth.enabled() {
        return Err(NotCompiledIn::new("oauth", "oauth"));
    }
    // Storage is on by default (`backend = "local"`), so a build without it
    // reads that default as "none". What it refuses is a bucket somebody
    // configured on purpose.
    #[cfg(not(feature = "storage"))]
    if app.config.storage.backend.trim().eq_ignore_ascii_case("s3") {
        return Err(NotCompiledIn::new("storage", "storage"));
    }
    #[cfg(not(feature = "queues"))]
    if app.config.queues.is_active()
        || app
            .resources
            .values()
            .any(|resource| resource.publish.iter().next().is_some())
    {
        return Err(NotCompiledIn::new("queues", "queues"));
    }
    #[cfg(not(feature = "admin"))]
    if app.config.admin.enabled {
        tracing::warn!(
            "[admin] is enabled, but this build has no `admin` feature — the dashboard is not served"
        );
    }
    Ok(())
}

/// The features this build was compiled with, for the admin manifest and the
/// startup log.
pub fn compiled_features() -> Vec<&'static str> {
    let mut features = Vec::new();
    if cfg!(feature = "payments") {
        features.push("payments");
    }
    if cfg!(feature = "ai") {
        features.push("ai");
    }
    if cfg!(feature = "cache") {
        features.push("cache");
    }
    if cfg!(feature = "telemetry") {
        features.push("telemetry");
    }
    if cfg!(feature = "typescript") {
        features.push("typescript");
    }
    for (on, name) in [
        (cfg!(feature = "email"), "email"),
        (cfg!(feature = "oauth"), "oauth"),
        (cfg!(feature = "storage"), "storage"),
        (cfg!(feature = "queues"), "queues"),
        (cfg!(feature = "admin"), "admin"),
    ] {
        if on {
            features.push(name);
        }
    }
    features
}

#[cfg(feature = "payments")]
pub use apiplant_payments::Payments;
#[cfg(not(feature = "payments"))]
pub use disabled::Payments;

#[cfg(feature = "ai")]
pub use apiplant_ai::Ai;
#[cfg(not(feature = "ai"))]
pub use disabled::Ai;

#[cfg(feature = "cache")]
pub use apiplant_cache::Cache;
#[cfg(not(feature = "cache"))]
pub use disabled::Cache;

#[cfg(feature = "email")]
pub use apiplant_email::Mailer;
#[cfg(not(feature = "email"))]
pub use disabled::Mailer;

#[cfg(feature = "email")]
pub use crate::email_templates::EmailTemplates;
#[cfg(not(feature = "email"))]
pub use disabled::EmailTemplates;

#[cfg(feature = "oauth")]
pub use apiplant_oauth::Providers;
#[cfg(not(feature = "oauth"))]
pub use disabled::Providers;

#[cfg(feature = "storage")]
pub use apiplant_storage::Storage;
#[cfg(not(feature = "storage"))]
pub use disabled::Storage;

#[cfg(feature = "queues")]
pub use apiplant_queue::Queue;
#[cfg(not(feature = "queues"))]
pub use disabled::Queue;

/// The app's email templates, compiled from its `emails/` directory and
/// branded with its name and logo. A build without `email` has none.
pub fn email_templates(app: &App) -> anyhow::Result<std::sync::Arc<EmailTemplates>> {
    #[cfg(feature = "email")]
    {
        let brand = crate::emails::Links::from_app(app);
        Ok(std::sync::Arc::new(
            EmailTemplates::load(&app.root)?.with_brand(&brand.app_name, brand.logo_url.as_deref()),
        ))
    }
    #[cfg(not(feature = "email"))]
    {
        let _ = app;
        Ok(std::sync::Arc::new(EmailTemplates))
    }
}

/// The stand-ins. Each is an empty enum: there is no value of it, so every
/// method body is `match *self {}` and none of them can run.
#[allow(dead_code)] // which of these is used depends on the features
mod disabled {
    use super::NotCompiledIn;
    use apiplant_core::config::{
        AiConfig, CacheConfig, EmailConfig, OAuthConfig, PaymentsConfig, StorageConfig,
    };
    use apiplant_core::App;
    use serde_json::Value;

    #[derive(Clone, Debug)]
    pub enum Payments {}

    impl Payments {
        pub fn from_config(
            config: &PaymentsConfig,
            _landing: &str,
        ) -> Result<Option<Payments>, NotCompiledIn> {
            match config.enabled() {
                true => Err(NotCompiledIn::new("payments", "payments")),
                false => Ok(None),
            }
        }

        pub async fn execute(&self, _request: &str) -> Result<Value, NotCompiledIn> {
            match *self {}
        }
    }

    #[derive(Clone, Debug)]
    pub enum Ai {}

    impl Ai {
        pub fn from_config(config: &AiConfig) -> Result<Option<Ai>, NotCompiledIn> {
            match config.enabled() {
                true => Err(NotCompiledIn::new("ai", "ai")),
                false => Ok(None),
            }
        }
    }

    #[derive(Clone, Debug)]
    pub enum Cache {}

    impl Cache {
        pub async fn connect(config: &CacheConfig) -> Result<Option<Cache>, NotCompiledIn> {
            match config.is_active() {
                true => Err(NotCompiledIn::new("cache", "cache")),
                false => Ok(None),
            }
        }

        pub async fn execute(&self, _request: &str) -> Result<Value, NotCompiledIn> {
            match *self {}
        }
    }

    #[derive(Clone, Debug)]
    pub enum Mailer {}

    impl Mailer {
        pub fn from_config(config: &EmailConfig) -> Result<Option<Mailer>, NotCompiledIn> {
            match config.enabled() {
                true => Err(NotCompiledIn::new("email", "email")),
                false => Ok(None),
            }
        }
    }

    /// No templates: there is nothing to send them with.
    #[derive(Debug, Default)]
    pub struct EmailTemplates;

    impl EmailTemplates {
        pub fn names(&self) -> Vec<String> {
            Vec::new()
        }
    }

    #[derive(Debug)]
    pub enum Providers {}

    impl Providers {
        pub fn from_config(
            config: &OAuthConfig,
            _callback_base: &str,
        ) -> Result<Option<Providers>, NotCompiledIn> {
            match config.enabled() {
                true => Err(NotCompiledIn::new("oauth", "oauth")),
                false => Ok(None),
            }
        }
    }

    #[derive(Clone, Debug)]
    pub enum Storage {}

    impl Storage {
        /// Always `None`: `backend = "local"` is the default, and a build
        /// without storage reads it as `none` (`check` refuses `s3`).
        pub fn connect(
            _config: &StorageConfig,
            _app_root: &std::path::Path,
        ) -> Result<Option<Storage>, NotCompiledIn> {
            Ok(None)
        }

        pub fn max_bytes(&self) -> u64 {
            match *self {}
        }
    }

    /// Not optional in the state, so not uninhabited: a queue that refuses
    /// every message, which `check` makes sure nothing asks it for.
    #[derive(Clone, Debug)]
    pub struct Queue;

    impl Queue {
        pub fn new(_db: &apiplant_db::Db, _app: &App) -> Queue {
            Queue
        }

        pub async fn publish(
            &self,
            _topic: &str,
            _message: &Value,
            _published_by: &str,
        ) -> Result<Value, NotCompiledIn> {
            Err(NotCompiledIn::new("queues", "queues"))
        }

        pub async fn execute(
            &self,
            _request: &str,
            _published_by: &str,
        ) -> Result<Value, NotCompiledIn> {
            Err(NotCompiledIn::new("queues", "queues"))
        }
    }
}
