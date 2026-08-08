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

use crate::schema::{Access, Policy};
use serde::Deserialize;
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

/// Fully-resolved server configuration.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct Config {
    pub app: AppConfig,
    pub server: ServerConfig,
    pub database: DatabaseConfig,
    pub auth: AuthConfig,
    pub rate_limit: RateLimitConfig,
    pub docs: DocsConfig,
    pub admin: AdminConfig,
    pub public: PublicConfig,
    pub email: EmailConfig,
    pub cache: CacheConfig,
    pub storage: StorageConfig,
    pub queues: QueuesConfig,
    pub payments: PaymentsConfig,
    pub ai: AiConfig,
    pub oauth: OAuthConfig,
    pub observability: ObservabilityConfig,
    pub organization: OrganizationConfig,
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

/// Accepts either a bare string or a list of them, so a config that names one
/// domain doesn't have to be written as a one-element list.
fn one_or_many<'de, D: serde::Deserializer<'de>>(de: D) -> Result<Vec<String>, D::Error> {
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum OneOrMany {
        One(String),
        Many(Vec<String>),
    }
    Ok(match OneOrMany::deserialize(de)? {
        OneOrMany::One(s) => vec![s],
        OneOrMany::Many(v) => v,
    })
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct ServerConfig {
    /// Interface to bind, e.g. `0.0.0.0`. Empty or `*` means every interface
    /// and is normalised to `0.0.0.0` on load.
    pub host: String,
    /// TCP port.
    pub port: u16,
    /// Only answer requests whose `Host:` header is one of these. Written as a
    /// single string (`domain = "api.example.com"`) or a list
    /// (`domain = ["api.example.com", "www.example.com"]`). Unset — or the
    /// catch-all spellings `""`, `*` and `_` (nginx's `server_name _`) —
    /// answers any host, and all of them normalise to an empty list on load.
    #[serde(deserialize_with = "one_or_many")]
    pub domain: Vec<String>,
    /// Sub-path the API is mounted under, e.g. `/api`. Always starts with `/`
    /// and never ends with one (normalised on load).
    pub base_path: String,
    /// Number of worker threads. `None` = one per CPU.
    pub workers: Option<usize>,
    /// The origin this server is reached at from outside — `https://api.example.com`.
    ///
    /// Only links that leave the process need it: an invitation email has to
    /// name a URL, and a request's own `Host:` header is the wrong source for
    /// one (a message is composed once and read anywhere, possibly behind a
    /// proxy that rewrote it). Unset falls back to the first configured
    /// `domain`, then to `http://<host>:<port>`, which is right for local
    /// development and wrong in front of a load balancer — so set it there.
    pub public_url: String,
}

impl Default for ServerConfig {
    fn default() -> Self {
        ServerConfig {
            host: "0.0.0.0".to_string(),
            port: 8080,
            domain: Vec::new(),
            base_path: "/".to_string(),
            workers: None,
            public_url: String::new(),
        }
    }
}

impl ServerConfig {
    /// The origin to put in a link that will be read outside this process.
    ///
    /// Prefers what the app declared, then the first domain it answers for,
    /// and only then the socket it happens to be bound to.
    pub fn public_origin(&self) -> String {
        if !self.public_url.is_empty() {
            return self.public_url.trim_end_matches('/').to_string();
        }
        if let Some(domain) = self.domain.first() {
            // A bare domain is a hostname, not a URL; assume the scheme every
            // deployment that has a domain name is using.
            return if domain.contains("://") {
                domain.trim_end_matches('/').to_string()
            } else {
                format!("https://{domain}")
            };
        }
        let host = match self.host.as_str() {
            "0.0.0.0" | "" | "*" | "::" => "localhost",
            host => host,
        };
        format!("http://{host}:{}", self.port)
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

    // --- the three features that need a mailbox to reach ------------------
    //
    // Each is `Option<bool>` rather than `bool` because their honest default is
    // not a constant: it is "yes, if this app can send email". Leaving one unset
    // means it follows `[email]`, so configuring a provider turns all three on
    // and configuring none leaves them off — and neither one asks the developer
    // to keep two sections in step. Setting one explicitly always wins, which is
    // how an app that sends mail can still refuse, say, open registration.
    /// Require a new account to confirm its address before it can sign in.
    /// Unset follows `[email]`.
    pub require_email_verification: Option<bool>,
    /// Offer `POST /auth/invitations`, so an admin can add someone who has no
    /// account yet. Unset follows `[email]`.
    pub allow_invitations: Option<bool>,
    /// Offer `POST /auth/password/forgot` and `/auth/password/reset`. Unset
    /// follows `[email]`.
    pub allow_password_reset: Option<bool>,

    /// How long an organisation invitation stays valid (default 7 days).
    pub invite_ttl_secs: u64,
    /// How long an address-confirmation link stays valid (default 24 hours).
    pub verification_ttl_secs: u64,
    /// How long a password-reset link stays valid (default 1 hour). Short on
    /// purpose: it is a live credential sitting in a mailbox.
    pub password_reset_ttl_secs: u64,
}

impl Default for AuthConfig {
    fn default() -> Self {
        AuthConfig {
            jwt_secret: String::new(),
            session_ttl_secs: 60 * 60 * 24 * 7,
            allow_registration: true,
            require_email_verification: None,
            allow_invitations: None,
            allow_password_reset: None,
            invite_ttl_secs: 60 * 60 * 24 * 7,
            verification_ttl_secs: 60 * 60 * 24,
            password_reset_ttl_secs: 60 * 60,
        }
    }
}

impl AuthConfig {
    /// Whether new accounts must confirm their address, given whether the app
    /// can send mail at all. An unset flag follows the mailer: asking for a
    /// confirmation nobody can deliver would lock every new account out.
    pub fn requires_email_verification(&self, email_enabled: bool) -> bool {
        self.require_email_verification.unwrap_or(email_enabled) && email_enabled
    }

    /// Whether invitations are offered. See
    /// [`requires_email_verification`](Self::requires_email_verification) for
    /// why an explicit `true` still needs a mailer.
    pub fn invitations_enabled(&self, email_enabled: bool) -> bool {
        self.allow_invitations.unwrap_or(email_enabled) && email_enabled
    }

    /// Whether password reset is offered.
    pub fn password_reset_enabled(&self, email_enabled: bool) -> bool {
        self.allow_password_reset.unwrap_or(email_enabled) && email_enabled
    }
}

/// How many requests one client may make, before the API starts answering
/// `429 Too Many Requests`.
///
/// Off until asked for: `default = "off"` means an app that says nothing here
/// is limited nowhere, and an upgrade cannot start refusing traffic that used
/// to be served. Naming a rate switches it on for every endpoint at once:
///
/// ```toml
/// [rate_limit]
/// default = "100/1m"
/// ```
///
/// A resource narrows or lifts that per action in its own `[rate_limit]`
/// section, and a function does the same with a `rate_limit` key in its
/// `functions/<name>.toml` — see [`crate::RateLimits`].
///
/// ## Who "one client" is
///
/// The peer socket address, which a caller cannot forge. Behind a reverse
/// proxy that is the *proxy's* address for every request — one bucket for
/// everybody, throttling all callers together — so a deployment behind one has
/// to set `trust_proxy_headers = true` and make sure the proxy *overwrites*
/// `X-Forwarded-For` rather than appending to it. Trusting that header with
/// nothing in front of the server hands every caller their own rate limit for
/// the price of a header line, which is the same as having none.
#[derive(Debug, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct RateLimitConfig {
    /// Turn every limit off — the app's, the resources' and the functions' —
    /// without deleting what they say. The switch to flip while an incident is
    /// being diagnosed.
    pub enabled: bool,
    /// The rule every endpoint gets unless something narrower says otherwise.
    /// `"off"` (the default) limits nothing.
    pub default: crate::schema::RateLimitRule,
    /// Read the client address from `X-Forwarded-For` / `X-Real-IP` when
    /// present, instead of the peer socket. Only true behind a proxy you
    /// control; see the section note above.
    pub trust_proxy_headers: bool,
    /// How often the tracked clients are swept for buckets nobody has used.
    pub cleanup_interval_secs: u64,
    /// How long a client's bucket is kept after their last request. Bounds
    /// what a flood of one-request-each addresses can cost in memory.
    pub stale_after_secs: u64,
}

impl Default for RateLimitConfig {
    fn default() -> Self {
        RateLimitConfig {
            enabled: true,
            default: crate::schema::RateLimitRule::Off,
            trust_proxy_headers: false,
            cleanup_interval_secs: 60,
            stale_after_secs: 600,
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
    /// Title shown in the UI and the spec's `info.title`. Unset falls back to
    /// the app's name — see [`App::docs_title`](crate::App::docs_title) — so an
    /// app that renames itself renames its docs too.
    pub title: Option<String>,
}

impl Default for DocsConfig {
    fn default() -> Self {
        DocsConfig {
            enabled: true,
            path: "/docs".to_string(),
            title: None,
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
    /// Fall back to [Gravatar] for an account with no `avatar_url` of its own,
    /// using the hash of its email address (default false).
    ///
    /// Off by default because it is a request to a third party for every face
    /// the dashboard draws, and the hash of an address is enough to confirm a
    /// guess at it. A deployment that would rather not tell gravatar.com who
    /// its users are should leave this alone; the dashboard falls back to
    /// initials, which need nobody's help.
    ///
    /// Turning this on is not enough on its own: the dashboard hashes the
    /// address with WebCrypto, which browsers only expose in a secure context.
    /// Reached over plain http at anything other than `localhost` or
    /// `127.0.0.1` — `http://0.0.0.0:8099/admin/`, a LAN address — there is no
    /// `crypto.subtle` and every face falls back to initials. What the server
    /// binds to does not matter; the address in the URL bar does.
    ///
    /// [Gravatar]: https://gravatar.com
    pub gravatar: bool,
    /// Optional AI help for writing text in the admin dashboard.
    pub ai_assistance: AdminAiAssistanceConfig,
}

/// Extra browser-side prompting that fills text fields through the app's
/// configured AI provider.
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct AdminAiAssistanceConfig {
    /// Show the "fill with AI" control in the dashboard.
    pub enabled: bool,
    /// Optional system prompt sent only by the dashboard's own field helper.
    pub system: String,
    /// Placeholder shown in the helper's prompt box.
    pub prompt_placeholder: String,
}

impl Default for AdminAiAssistanceConfig {
    fn default() -> Self {
        AdminAiAssistanceConfig {
            enabled: false,
            system: String::new(),
            prompt_placeholder: "Describe what you want AI to write for this field.".to_string(),
        }
    }
}

impl Default for AdminConfig {
    fn default() -> Self {
        AdminConfig {
            enabled: true,
            path: "/admin".to_string(),
            logo: None,
            gravatar: false,
            ai_assistance: AdminAiAssistanceConfig::default(),
        }
    }
}

/// The `[organization]` section: deployment-wide rules about the tenant itself.
///
/// Only one today, and it exists because `organization.org_class` is not an
/// ordinary column. A class decides what a `@org_class=` permission lets people
/// do, so an organisation that could rename its own class could grant itself
/// access — which is why the column is server-owned, and why saying who may
/// write it is a deployment decision rather than a row-level one.
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct OrganizationConfig {
    /// Who may set or change an organisation's `org_class`, in the same
    /// grammar as `[permissions]` — typically a class of its own, e.g.
    /// `"member@org_class=staff"`.
    ///
    /// Defaults to `"private"`: with no setting, no request can write the
    /// column at all and classes are fixed by the operator (seed data, or SQL).
    pub org_class_editors: String,

    /// The class stamped on an organisation created with none — every
    /// organisation the API makes, personal ones included.
    ///
    /// Empty (the default) leaves new organisations unclassed, which no
    /// `@org_class=` permission matches. Set it where a deployment's ordinary
    /// tenant is *some* kind — `"customer"` — so the permissions written for
    /// that class apply from the moment an organisation exists, rather than
    /// after somebody remembers to class it.
    ///
    /// A class editor who names a class on create is not overridden: this
    /// fills the column in, it does not own it.
    pub default_org_class: String,
}

impl Default for OrganizationConfig {
    fn default() -> Self {
        OrganizationConfig {
            org_class_editors: "private".to_string(),
            default_org_class: String::new(),
        }
    }
}

impl OrganizationConfig {
    /// The parsed policy for writing `org_class`. An unparseable setting is
    /// `private`, like every other access string in the system.
    pub fn org_class_policy(&self) -> Policy {
        Policy::parse(&self.org_class_editors)
    }

    /// The class new organisations start with, if the app names one.
    pub fn default_class(&self) -> Option<&str> {
        let value = self.default_org_class.trim();
        (!value.is_empty()).then_some(value)
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
    /// The mark shown in the banner of the messages the framework sends, as a
    /// path inside [`PublicConfig::dir`] — `logo.png` or `/img/logo.svg`, both
    /// of which mean the same file. It is turned into an absolute URL against
    /// `[server] public_url`, because a mail client fetches it from the
    /// internet rather than from a page. An empty string, or a path with no
    /// file behind it, leaves the banner showing the app's name alone.
    pub logo: String,
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
            logo: "logo.png".to_string(),
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

/// Where uploaded files go.
///
/// A `file` field holds a *relative* URL — `/files/2026/…/logo.png` — never a
/// bucket address, and the server answers that URL from whichever backend is
/// configured. That is the whole point of the indirection: an app that starts
/// on a mounted volume and later moves to S3 changes four lines of TOML and
/// nothing else. No row is rewritten, because no row ever named the backend.
///
/// ```toml
/// # A directory — a Docker volume, in practice.
/// [storage]
/// backend = "local"
/// dir     = "storage"
///
/// # Or block storage. `r2` is `s3` with an endpoint, and so is MinIO.
/// [storage]
/// backend           = "s3"
/// bucket            = "app-uploads"
/// region            = "auto"
/// endpoint          = "https://${R2_ACCOUNT}.r2.cloudflarestorage.com"
/// access_key_id     = "${R2_ACCESS_KEY_ID}"
/// secret_access_key = "${R2_SECRET_ACCESS_KEY}"
/// ```
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct StorageConfig {
    /// `local` (the default), `s3`, or `none` to refuse uploads outright.
    pub backend: String,
    /// `local`: the directory uploads are written to, relative to the app root
    /// unless absolute. In a container this is what you mount a volume at.
    pub dir: String,
    /// URL prefix the stored links carry and the server answers on. Always
    /// starts with `/` and never ends with one (normalised on load).
    pub public_base: String,
    /// Largest upload accepted, in megabytes.
    pub max_size_mb: u64,
    /// Content types an upload may declare, as exact types (`image/png`) or
    /// wildcards (`image/*`). Empty (the default) accepts anything — an
    /// authenticated caller is already trusted to write a row.
    pub allowed_types: Vec<String>,
    /// `s3`: the bucket. Required when `backend = "s3"`.
    pub bucket: String,
    /// `s3`: the region. R2 and most S3-compatibles want `auto`.
    pub region: String,
    /// `s3`: the API origin. Empty uses AWS's own
    /// (`https://<bucket>.s3.<region>.amazonaws.com`); set it for R2, MinIO,
    /// Backblaze or any other S3-compatible service.
    pub endpoint: String,
    /// `s3`: credentials.
    pub access_key_id: String,
    pub secret_access_key: String,
    /// `s3`: address objects as `<endpoint>/<bucket>/<key>` rather than putting
    /// the bucket in the hostname. Required by MinIO and by R2 (the default
    /// when an `endpoint` is set).
    pub path_style: Option<bool>,
    /// Key prefix inside the bucket or directory, so several apps can share one.
    pub prefix: String,
    /// Serve files from somewhere else entirely — a CDN, or a public bucket —
    /// by storing absolute URLs under this origin instead of relative ones.
    ///
    /// Empty (the default) keeps links relative and proxies reads through the
    /// server, which is what makes a private bucket work. Setting it is a
    /// deliberate trade: faster, but the objects must be publicly readable and
    /// the links stop being portable.
    pub base_url: String,
}

impl Default for StorageConfig {
    fn default() -> Self {
        StorageConfig {
            backend: "local".to_string(),
            dir: "storage".to_string(),
            public_base: "/files".to_string(),
            max_size_mb: 10,
            allowed_types: Vec::new(),
            bucket: String::new(),
            region: "auto".to_string(),
            endpoint: String::new(),
            access_key_id: String::new(),
            secret_access_key: String::new(),
            path_style: None,
            prefix: String::new(),
            base_url: String::new(),
        }
    }
}

impl StorageConfig {
    /// Whether uploads are accepted at all.
    pub fn is_active(&self) -> bool {
        !matches!(self.backend.trim().to_lowercase().as_str(), "none" | "")
    }

    /// `/files` — leading slash, no trailing slash, whatever was written.
    pub fn normalized_public_base(&self) -> String {
        let trimmed = self.public_base.trim().trim_matches('/');
        match trimmed.is_empty() {
            true => "/files".to_string(),
            false => format!("/{trimmed}"),
        }
    }

    /// Whether objects are addressed as `<endpoint>/<bucket>/<key>`. Explicit
    /// when written down; otherwise path-style exactly when a custom endpoint
    /// is set, since that is what every S3-compatible service but AWS wants.
    pub fn uses_path_style(&self) -> bool {
        self.path_style.unwrap_or(!self.endpoint.trim().is_empty())
    }

    pub fn max_size_bytes(&self) -> u64 {
        self.max_size_mb.saturating_mul(1024 * 1024)
    }
}

/// Background work: a message published now, handled by a function shortly
/// after, outside the request that caused it.
///
/// The transport is Postgres and nothing else — no broker to run, no second
/// thing that can be down. A `publish` writes a row to `queue_message` and
/// fires a `NOTIFY`; a subscriber wakes on that notification and claims the row
/// with `FOR UPDATE SKIP LOCKED`. The two halves matter for different reasons:
/// the *row* is what makes the message survive a restart and lets a failure be
/// retried, and the *notification* is what makes it happen in milliseconds
/// rather than on the next poll.
///
/// Because a message is a row, the guarantee is **at-least-once**: a handler
/// that succeeds but crashes before its row is marked done runs again. Write
/// handlers that can be run twice — the same reason `billing_event` exists.
///
/// ```toml
/// [queues]
/// # Which function handles which topic. One name, or several.
/// [queues.subscribe]
/// "user.signed_up" = "send_welcome"
/// "order.paid"     = ["fulfil_order", "notify_ops"]
/// ```
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct QueuesConfig {
    /// Turn message handling off without deleting the subscriptions. Publishing
    /// still records rows, so nothing is lost while it is off — it is a pause,
    /// not a drain.
    pub enabled: bool,
    /// Prepended to the Postgres `NOTIFY` channel this app wakes on, so two
    /// apps sharing one database don't wake each other for nothing.
    pub prefix: String,
    /// Topic → the function(s) that handle it. Written as one name or a list:
    ///
    /// ```toml
    /// [queues.subscribe]
    /// "user.signed_up" = "send_welcome"
    /// "order.paid"     = ["fulfil_order", "notify_ops"]
    /// ```
    ///
    /// Each subscriber gets its **own** row and its own retries, so a failing
    /// `notify_ops` never re-runs `fulfil_order`.
    #[serde(deserialize_with = "topic_subscriptions")]
    pub subscribe: BTreeMap<String, Vec<String>>,
    /// How often to sweep for work regardless of notifications. The `NOTIFY` is
    /// what makes delivery immediate; this is the safety net that picks up a
    /// message published while this process was starting, a retry whose backoff
    /// has expired, and anything a dropped connection lost the wakeup for.
    pub poll_secs: u64,
    /// Most messages claimed in one go. Larger batches trade latency on the
    /// last message for fewer round trips.
    pub batch: u32,
    /// How many times a message is tried before it is left `failed` for a person
    /// to look at. `1` means no retries at all.
    pub max_attempts: u32,
    /// Base of the retry backoff, in seconds: attempt *n* waits
    /// `retry_backoff_secs * 2^(n-1)`, so the default retries after 10s, 20s,
    /// 40s, 80s and then gives up.
    pub retry_backoff_secs: u64,
    /// How long a claimed message may be worked on before another subscriber
    /// is allowed to take it.
    ///
    /// This is what makes a killed process — an OOM, a rolling deploy, a lost
    /// node — recoverable rather than a message stuck forever in `running`.
    /// Set it comfortably above the slowest handler: expiring the lease early
    /// is what turns at-least-once into "twice, concurrently".
    pub lease_secs: u64,
    /// Delete handled messages after this many hours, on the same sweep. `0`
    /// keeps them forever, which is a reasonable choice for a low-volume app
    /// that wants the ledger.
    pub retain_hours: u64,
    /// Who may publish over HTTP at `POST <base>/queues/{topic}`, in the same
    /// grammar a resource's `[permissions]` uses.
    ///
    /// `private` — the default — means there is no such endpoint at all. A
    /// topic is an internal name that triggers real work, so it is not
    /// something to expose without deciding to.
    pub publish: String,
}

impl Default for QueuesConfig {
    fn default() -> Self {
        QueuesConfig {
            enabled: true,
            prefix: "apiplant".to_string(),
            subscribe: BTreeMap::new(),
            poll_secs: 30,
            batch: 10,
            max_attempts: 5,
            retry_backoff_secs: 10,
            lease_secs: 300,
            retain_hours: 24,
            publish: "private".to_string(),
        }
    }
}

/// Accepts `"topic" = "fn"` and `"topic" = ["fn", "fn"]` in the same table, for
/// the same reason [`one_or_many`] exists: the common case is one subscriber,
/// and it shouldn't have to be written as a one-element list.
fn topic_subscriptions<'de, D: serde::Deserializer<'de>>(
    de: D,
) -> Result<BTreeMap<String, Vec<String>>, D::Error> {
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum OneOrMany {
        One(String),
        Many(Vec<String>),
    }
    let raw = BTreeMap::<String, OneOrMany>::deserialize(de)?;
    Ok(raw
        .into_iter()
        .map(|(topic, subscribers)| {
            let subscribers = match subscribers {
                OneOrMany::One(name) => vec![name],
                OneOrMany::Many(names) => names,
            };
            (
                topic.trim().to_string(),
                subscribers
                    .into_iter()
                    .map(|name| name.trim().to_string())
                    .filter(|name| !name.is_empty())
                    .collect(),
            )
        })
        .filter(|(topic, subscribers): &(String, Vec<String>)| {
            !topic.is_empty() && !subscribers.is_empty()
        })
        .collect())
}

impl QueuesConfig {
    /// The `NOTIFY` channel this app's publishers and subscribers meet on.
    ///
    /// One channel for the whole app rather than one per topic: the payload
    /// carries the topic, a listener has a single subscription to re-establish
    /// after a reconnect, and adding a topic needs no new `LISTEN`. It also
    /// sidesteps Postgres's 63-byte limit on a channel name, which an app's own
    /// topic names would otherwise have to live inside.
    pub fn channel(&self) -> String {
        let prefix = self.prefix.trim().trim_matches('_');
        match prefix.is_empty() {
            true => "apiplant_queue".to_string(),
            false => format!("{prefix}_queue"),
        }
    }

    /// Whether `topic` is a name this app will carry.
    ///
    /// Deliberately narrow — letters, digits, and `. _ - :` — because a topic
    /// is an identifier that ends up in config keys, log lines and dashboard
    /// filters, and a topic with a space or a quote in it reads as a mistake
    /// everywhere it appears. Checked when publishing rather than trusted, since
    /// a topic can arrive from a function's runtime string.
    pub fn valid_topic(topic: &str) -> bool {
        let topic = topic.trim();
        !topic.is_empty()
            && topic.len() <= 200
            && topic
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-' | ':'))
    }

    /// The functions subscribed to a topic, in the order they were declared.
    pub fn subscribers(&self, topic: &str) -> &[String] {
        self.subscribe
            .get(topic.trim())
            .map(Vec::as_slice)
            .unwrap_or(&[])
    }

    /// Every function name any topic points at, deduplicated. Used at boot to
    /// report a subscription whose function isn't loaded.
    pub fn subscribed_functions(&self) -> BTreeSet<&str> {
        self.subscribe
            .values()
            .flatten()
            .map(String::as_str)
            .collect()
    }

    /// Whether a subscriber loop should run: switched on *and* something to
    /// listen for. Publishing does not depend on this — a message published
    /// with no subscriber is still recorded, which is what makes "why didn't my
    /// handler run?" answerable.
    pub fn is_active(&self) -> bool {
        self.enabled && !self.subscribe.is_empty()
    }

    /// The resolved policy for the HTTP publish endpoint. An unparseable
    /// `publish` closes the door, matching how every other access string here
    /// treats a typo — and so does `owner`, which names a column on a row and
    /// means nothing for a topic.
    pub fn publish_access(&self) -> Policy {
        let policy = Policy::parse(&self.publish);
        match policy.level {
            Access::Owner => Access::Private.into(),
            _ => policy,
        }
    }

    /// Seconds to wait before retrying a message that has failed `attempts`
    /// times, doubling each time and capped at an hour so a poisoned message
    /// doesn't schedule itself past the retention sweep.
    pub fn retry_delay_secs(&self, attempts: u32) -> u64 {
        let doubling = 1u64
            .checked_shl(attempts.saturating_sub(1))
            .unwrap_or(u64::MAX);
        self.retry_backoff_secs
            .saturating_mul(doubling)
            .min(60 * 60)
    }
}

/// Signing in with somebody else's account.
///
/// Each `[oauth.<provider>]` block turns one provider on, and a block needs
/// only the two credentials that provider issued:
///
/// ```toml
/// [oauth.github]
/// client_id     = "${GITHUB_CLIENT_ID}"
/// client_secret = "${GITHUB_CLIENT_SECRET}"
///
/// [oauth.google]
/// client_id     = "${GOOGLE_CLIENT_ID}"
/// client_secret = "${GOOGLE_CLIENT_SECRET}"
/// ```
///
/// Everything else — the authorize URL, the token URL, where the profile is
/// read from, which scopes ask for an email, whether the provider wants PKCE,
/// whether it insists on the client secret as HTTP Basic — apiplant knows for
/// `github`, `google`, `linkedin` and `x`. A provider it does not know is
/// configured in full (see [`OAuthProviderConfig::style`]), which is how a
/// fifth one is added without waiting for a release.
///
/// Turning any of this on mounts `<base>/auth/oauth/…` and adds the
/// `oauth_state` [resource](crate::defaults). With no block at all, none of it
/// exists.
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct OAuthConfig {
    /// Whether a **verified** address from a provider may sign somebody in to
    /// an existing account carrying the same address (default true).
    ///
    /// This is the convenience that makes "I registered with a password, then
    /// came back through Google" work, and it is safe only because the address
    /// must be one the provider says it verified. An unverified address is
    /// never matched, whatever this is set to — that is not a policy, it is the
    /// difference between signing in and taking over. Set it false and a
    /// matching address is refused with an answer that says how to connect the
    /// two deliberately — sign in the way you already can, then link the
    /// provider from an authenticated session. Inconvenient, and never wrong.
    ///
    /// The same refusal is what an *unverified* matching address always gets,
    /// whatever this is set to.
    pub link_by_verified_email: bool,
    /// How long a started sign-in stays completable, in seconds (default 600).
    /// Long enough to read a consent screen, short enough that an abandoned
    /// flow is not a lasting hole. Clamped to 60–3600.
    pub state_ttl_secs: u64,
    /// Where the browser lands after a successful sign-in through the
    /// *redirecting* endpoint, as a path on this site (default `/`).
    ///
    /// A caller can override it per flow with `?return_to=/somewhere`, which is
    /// accepted only as a path — never a full URL — because a redirect target
    /// somebody else chooses is how a sign-in page becomes a phishing hop.
    pub success_redirect: String,
    /// Where a *failed* sign-in lands, as a path. Empty (the default) answers
    /// with a plain JSON error instead, which is what you want while setting
    /// providers up and not what you want in front of users.
    pub failure_redirect: String,
    /// How the session token reaches the browser on the redirecting endpoint:
    ///
    /// | Value | Effect |
    /// |---|---|
    /// | `fragment` (default) | `…/#token=…` — a fragment is never sent to a server, so it stays out of proxy logs and `Referer` headers |
    /// | `query` | `…?token=…` — easier to read from a server-rendered page, and it *is* in those logs |
    /// | `json` | no redirect at all: the callback answers `{ "token": …, "user": … }`, which is what a single-page app posting the code itself wants |
    pub token_delivery: String,
    /// The `user` column a provider's name is written to on sign-in, or empty
    /// to write none. `display_name` is in the built-in model; an app that
    /// calls it something else names it here, and one that would rather keep
    /// its own copy of a name sets this to `""`.
    pub name_field: String,
    /// The `user` column a provider's picture is written to, or empty for none.
    /// Same bargain as `name_field`.
    ///
    /// Both are written on *every* sign-in, not only the first: people change
    /// their name and their picture, and a copy that is only ever right on the
    /// day the account was created is worse than no copy.
    pub avatar_field: String,
    /// The providers, keyed by name. Written as `[oauth.github]` rather than
    /// `[oauth.providers.github]` — the flattening is what buys that, and the
    /// cost is that a mistyped setting above becomes a provider nobody asked
    /// for, which is refused at boot rather than ignored.
    #[serde(flatten)]
    pub providers: std::collections::BTreeMap<String, OAuthProviderConfig>,
}

impl Default for OAuthConfig {
    fn default() -> Self {
        OAuthConfig {
            link_by_verified_email: true,
            state_ttl_secs: 600,
            success_redirect: "/".to_string(),
            failure_redirect: String::new(),
            token_delivery: "fragment".to_string(),
            name_field: "display_name".to_string(),
            avatar_field: "avatar_url".to_string(),
            providers: std::collections::BTreeMap::new(),
        }
    }
}

impl OAuthConfig {
    /// Whether any provider is usable — which is what mounts the routes.
    pub fn enabled(&self) -> bool {
        self.providers.values().any(OAuthProviderConfig::is_active)
    }

    /// The names of the providers that are on, in a stable order.
    pub fn active_providers(&self) -> Vec<&str> {
        self.providers
            .iter()
            .filter(|(_, p)| p.is_active())
            .map(|(name, _)| name.as_str())
            .collect()
    }

    /// `state_ttl_secs`, clamped to something a sign-in can actually happen in.
    pub fn state_ttl(&self) -> u64 {
        self.state_ttl_secs.clamp(60, 3600)
    }
}

/// One provider's credentials, and the overrides an unknown provider needs.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct OAuthProviderConfig {
    /// The client id the provider issued. An empty one leaves the provider off,
    /// which is what lets a committed config name every provider and a
    /// deployment supply only the credentials it has.
    pub client_id: String,
    /// The client secret. Required for every provider apiplant ships, all of
    /// which are confidential clients.
    pub client_secret: String,
    /// Space-separated scopes, overriding the built-in default. The defaults
    /// ask for the least that identifies somebody; widen this only for scopes
    /// the app will actually use, since every one is another line on a consent
    /// screen and another reason to press Cancel.
    pub scopes: String,
    /// Where the browser is sent to consent. Required for an unknown provider.
    pub authorize_url: String,
    /// Where the code is redeemed. Required for an unknown provider.
    pub token_url: String,
    /// Where the profile is read. Required for an unknown provider.
    pub userinfo_url: String,
    /// How to read that profile, for a provider apiplant does not ship:
    /// `oidc` (default — standard `sub`/`email`/`email_verified`/`name`/
    /// `picture` claims, which is what almost everything speaks today) or
    /// `github` (GitHub's older shape).
    pub style: String,
    /// What the sign-in button should say. Defaults to the built-in label, or
    /// to the provider's own name capitalised.
    pub label: String,
    /// The redirect URI registered with the provider. Empty (the default)
    /// derives it — `<public_url><base_path>/auth/oauth/<provider>/callback` —
    /// which is right unless something in front of this server rewrites paths.
    pub redirect_uri: String,
    /// Whether PKCE is used. Unset follows what the provider supports; X
    /// *requires* it, GitHub does not offer it.
    pub pkce: Option<bool>,
    /// Set false to keep a fully credentialed provider switched off — the way
    /// to take a sign-in button away for a while without deleting the secrets.
    pub enabled: Option<bool>,
    /// A logo for the sign-in button, as a URL a browser can fetch — usually a
    /// file in [`public/`](PublicConfig), such as `/oauth/gitlab.svg`.
    ///
    /// apiplant draws GitHub, Google, LinkedIn and X itself, so this is for the
    /// providers it does not ship: without it their button gets the provider's
    /// initial on a plain tile, which works and looks like what it is.
    ///
    /// <https://github.com/edent/SuperTinyIcons> is a good place to get one —
    /// several hundred brand marks, each a few hundred bytes of hand-drawn SVG,
    /// MIT licensed. They are what apiplant's own four are drawn from. Save the
    /// file into `public/` and point this at it.
    pub icon: String,
}

impl OAuthProviderConfig {
    /// Whether this block is complete enough to sign anybody in.
    pub fn is_active(&self) -> bool {
        self.enabled.unwrap_or(true) && !self.client_id.trim().is_empty()
    }
}

/// Payments: who takes the money, and how the checkout is set up.
///
/// Off by default (`provider = "none"`). Turning it on does three things an
/// app would otherwise build by hand: it connects a Stripe client, it adds the
/// `billing_*` [resources](crate::defaults) — catalogue, customers,
/// subscriptions, payments — so billing state is queryable through the same
/// permissions and roles as everything else, and it mounts the `/billing`
/// endpoints that start a checkout and receive Stripe's webhooks.
///
/// Nothing here is a price. Prices live in `billing_price` rows, because a
/// price is data an operator changes on a Tuesday, not configuration that
/// wants a deployment.
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct PaymentsConfig {
    /// `none` (default) or `stripe`.
    pub provider: String,
    /// Stripe secret key (`sk_live_…` / `sk_test_…`). Required once enabled.
    pub secret_key: String,
    /// Stripe publishable key (`pk_live_…`). Not a secret: it is handed to the
    /// browser by `GET <base>/billing/config`, which is how a front end
    /// mounts Stripe's own elements without hardcoding a key per environment.
    pub publishable_key: String,
    /// Signing secret for the webhook endpoint (`whsec_…`).
    ///
    /// Without it `POST <base>/billing/webhook` refuses every delivery — an
    /// unverified webhook is an unauthenticated request that edits
    /// subscriptions, and accepting one because it is inconvenient not to is
    /// how somebody else grants themselves a plan.
    pub webhook_secret: String,
    /// ISO 4217 currency for prices that don't name one, e.g. `eur`.
    pub currency: String,
    /// Let Stripe Tax work out and apply the right tax for the customer's
    /// location (default true).
    ///
    /// On means the amounts here are what you charge *before* tax and Stripe
    /// adds what the buyer owes. It needs an origin address and active
    /// registrations in the Stripe dashboard; with none, Stripe adds nothing
    /// and the charge is the price.
    pub automatic_tax: bool,
    /// Ask the buyer for a VAT/GST number at checkout (default true when
    /// `automatic_tax` is on — a business buyer's number is what makes the
    /// reverse charge apply).
    pub tax_id_collection: Option<bool>,
    /// Collect a full billing address at checkout rather than only what the
    /// card requires. `auto` (default) or `required`; automatic tax needs an
    /// address, so `auto` still collects enough to place the customer.
    pub billing_address: String,
    /// Two-letter ISO country codes a physical product may be shipped to.
    ///
    /// Only consulted for a product marked `shippable`; a digital one never
    /// asks for a shipping address whatever is listed here. Empty means the
    /// app sells nothing it has to post, and a checkout for a shippable
    /// product is refused rather than quietly taking money for something with
    /// nowhere to send it.
    pub shipping_countries: Vec<String>,
    /// Stripe [tax code] for a product that isn't shipped, when the row does
    /// not name one. The default is "general — electronically supplied
    /// services", which is what most software actually is.
    ///
    /// [tax code]: https://stripe.com/docs/tax/tax-categories
    pub digital_tax_code: String,
    /// Stripe tax code for a shippable product that doesn't name one. The
    /// default is "general — tangible goods".
    pub physical_tax_code: String,
    /// Where Stripe returns the buyer after a completed checkout. Empty falls
    /// back to the dashboard's billing screen — see
    /// [`ServerConfig::public_origin`].
    pub success_url: String,
    /// Where Stripe returns a buyer who backed out. Empty falls back to the
    /// dashboard's billing screen.
    pub cancel_url: String,
    /// Where the Stripe customer portal returns to. Empty falls back to the
    /// dashboard's billing screen.
    pub portal_return_url: String,
    /// How long one Stripe API call may take before it is abandoned.
    pub timeout_secs: u64,
}

impl Default for PaymentsConfig {
    fn default() -> Self {
        PaymentsConfig {
            provider: "none".to_string(),
            secret_key: String::new(),
            publishable_key: String::new(),
            webhook_secret: String::new(),
            currency: "usd".to_string(),
            automatic_tax: true,
            tax_id_collection: None,
            billing_address: "auto".to_string(),
            shipping_countries: Vec::new(),
            digital_tax_code: "txcd_10000000".to_string(),
            physical_tax_code: "txcd_99999999".to_string(),
            success_url: String::new(),
            cancel_url: String::new(),
            portal_return_url: String::new(),
            timeout_secs: 20,
        }
    }
}

impl PaymentsConfig {
    /// Whether a provider is configured at all. `none` and the empty string
    /// both mean "this app doesn't take money".
    pub fn enabled(&self) -> bool {
        !matches!(
            self.provider.trim().to_ascii_lowercase().as_str(),
            "" | "none"
        )
    }

    /// The currency to use for an amount that didn't name one, lowercased the
    /// way Stripe wants it.
    pub fn default_currency(&self) -> String {
        let currency = self.currency.trim().to_ascii_lowercase();
        if currency.is_empty() {
            "usd".to_string()
        } else {
            currency
        }
    }

    /// Whether checkout asks for a tax number. Unset follows `automatic_tax`:
    /// collecting a VAT number is only useful to somebody computing tax with
    /// it, and asking for one you ignore is a field that does nothing.
    pub fn collects_tax_ids(&self) -> bool {
        self.tax_id_collection.unwrap_or(self.automatic_tax)
    }

    /// The countries a physical order may be shipped to, upper-cased the way
    /// Stripe wants them, with blanks and duplicates dropped.
    pub fn shipping_destinations(&self) -> Vec<String> {
        let mut seen = Vec::new();
        for country in &self.shipping_countries {
            let code = country.trim().to_ascii_uppercase();
            if !code.is_empty() && !seen.contains(&code) {
                seen.push(code);
            }
        }
        seen
    }

    /// Whether this app posts anything anywhere. False means every product is
    /// digital, and no checkout will ever ask for a shipping address.
    pub fn ships(&self) -> bool {
        !self.shipping_destinations().is_empty()
    }

    /// The tax code for a product that named none — which one depends on
    /// whether it is posted, because that is the distinction the rate actually
    /// turns on.
    pub fn default_tax_code(&self, shippable: bool) -> String {
        let configured = match shippable {
            true => self.physical_tax_code.trim(),
            false => self.digital_tax_code.trim(),
        };
        configured.to_string()
    }

    /// Whether the webhook endpoint can verify a delivery. Payments still work
    /// without it — the checkout completes and Stripe has the money — but
    /// nothing of ours would ever hear about it.
    pub fn webhooks_enabled(&self) -> bool {
        self.enabled() && !self.webhook_secret.trim().is_empty()
    }
}

/// An AI chat assistant: which service answers, and what to say to it.
///
/// Off by default (`provider = "none"`). Turning it on connects one client and
/// mounts `<base>/ai/chat`, which takes a list of messages and streams the
/// reply back token by token — and gives every function a `chat` call over the
/// same provider.
///
/// The three providers differ only in wire format. `custom` is the one that
/// matters most in practice: anything speaking the OpenAI chat-completions
/// shape — llama.cpp, vLLM, Ollama, LM Studio, a gateway of your own — is
/// reached by pointing [`endpoint`](Self::endpoint) at it, with no key at all
/// if it wants none.
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct AiConfig {
    /// `none` (default), `openai`, `anthropic` or `custom`.
    pub provider: String,
    /// Where to send the request.
    ///
    /// Empty uses the provider's own API (`https://api.openai.com`,
    /// `https://api.anthropic.com`) and is required for `custom`. A bare origin
    /// or a base path (`http://localhost:8080`, `.../v1`) gets the provider's
    /// standard path appended; a URL that already names the full path
    /// (`…/v1/chat/completions`, `…/v1/messages`) is used exactly as written,
    /// for a gateway that mounts it somewhere of its own.
    pub endpoint: String,
    /// Model to ask for when a request doesn't name one, e.g. `gpt-4o-mini`.
    /// Some local servers serve a single model and ignore this.
    pub model: String,
    /// The provider's API key. **Optional**: a local model behind
    /// `provider = "custom"` usually wants no credential, and sending an empty
    /// one is different from sending none — so an empty key means the request
    /// carries no authorization header at all.
    pub api_key: String,
    /// Prepended to every conversation as the system prompt, unless the request
    /// carries its own. Empty = none.
    pub system: String,
    /// Cap on the tokens generated per reply. Anthropic requires one, so this
    /// is sent to every provider rather than being special-cased.
    pub max_tokens: u32,
    /// Sampling temperature sent when a request doesn't name one. Negative
    /// (the default) sends nothing and lets the provider choose.
    pub temperature: f32,
    /// Whether provider reasoning should be surfaced to callers when the
    /// provider emits it. This is a *display* decision and says nothing about
    /// whether the model thinks — see `thinking` for that.
    pub reasoning: bool,
    /// Whether to ask the provider to think, using its own switch for it.
    ///
    /// `None` (the default) sends nothing and leaves the model on whatever its
    /// template does. `Some(false)` turns thinking off, `Some(true)` turns it
    /// on. Worth setting: thinking is billed against `max_tokens` like any
    /// other output, so a thinking model on a small budget can spend the whole
    /// thing reasoning and answer with nothing at all.
    ///
    /// How it is sent depends on the provider: Anthropic has a `thinking`
    /// parameter, and OpenAI-compatible local servers (llama.cpp, vLLM, SGLang,
    /// Ollama) take `chat_template_kwargs.enable_thinking`, which is what the
    /// Qwen-family templates read. OpenAI's own reasoning models expose only
    /// `reasoning_effort` and cannot be switched off, so this is not sent to
    /// them.
    pub thinking: Option<bool>,
    /// Who may call `<base>/ai/chat`, in the grammar a resource's
    /// `[permissions]` uses: `public`, `authenticated` (the default), `member`,
    /// `role:<name>`.
    ///
    /// Defaulting to `authenticated` is deliberate. The endpoint spends money
    /// (or a GPU) on behalf of whoever calls it, and a public one is an open
    /// proxy to your provider account — which is a decision an app should have
    /// to write down.
    pub access: String,
    /// How long one completion may take before it is abandoned. Generous by
    /// default: a long answer from a local model is slow, not broken.
    pub timeout_secs: u64,
}

impl Default for AiConfig {
    fn default() -> Self {
        AiConfig {
            provider: "none".to_string(),
            endpoint: String::new(),
            model: String::new(),
            api_key: String::new(),
            system: String::new(),
            max_tokens: 2048,
            temperature: -1.0,
            reasoning: false,
            thinking: None,
            access: "authenticated".to_string(),
            timeout_secs: 300,
        }
    }
}

impl AiConfig {
    /// Whether a provider is configured at all. `none` and the empty string
    /// both mean "this app has no assistant".
    pub fn enabled(&self) -> bool {
        !matches!(
            self.provider.trim().to_ascii_lowercase().as_str(),
            "" | "none"
        )
    }

    /// The sampling temperature to send, or `None` to let the provider decide.
    pub fn default_temperature(&self) -> Option<f32> {
        (self.temperature >= 0.0).then_some(self.temperature)
    }
}

/// Logs, traces and metrics — what the server says about itself, and where it
/// says it.
///
/// Everything here is off-by-default except the logs, which every process has
/// always written to the terminal. Turning `enabled` on does not by itself send
/// anything anywhere: it arms the section, and an `[observability.otlp]`
/// `endpoint` is what makes traces and metrics leave the process. Without one
/// the spans are still built and still carried through the logs — so a
/// deployment gets request ids and structured errors for free, and an OTLP
/// collector only when it has somewhere to put the data.
///
/// ## Why OTLP and nothing else
///
/// OTLP is the wire format every backend now speaks — Jaeger, Tempo, Honeycomb,
/// Datadog, New Relic, the OpenTelemetry Collector — so one exporter reaches
/// all of them, and a deployment that wants something exotic points this at a
/// Collector and translates there rather than here. The transport is HTTP
/// (`:4318`), not gRPC: it goes through the `reqwest` client this binary
/// already links, where gRPC would compile a second RPC stack for the same
/// bytes.
///
/// ## Environment
///
/// The standard `OTEL_*` variables are read when the corresponding key is
/// unset — `OTEL_EXPORTER_OTLP_ENDPOINT`, `OTEL_EXPORTER_OTLP_HEADERS`,
/// `OTEL_SERVICE_NAME`, `OTEL_TRACES_SAMPLER_ARG` — because that is how a
/// sidecar-injected collector configures the pods around it, and an app should
/// not have to be rebuilt to be scraped.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ObservabilityConfig {
    /// Arm the section. Off means: log to the terminal as always, build no
    /// spans, export nothing.
    pub enabled: bool,
    /// What this service calls itself in a trace. Unset falls back to
    /// `OTEL_SERVICE_NAME`, then to the app's name, then to `apiplant`.
    pub service_name: Option<String>,
    /// The build being traced. Unset falls back to the `apiplant` version,
    /// which is right until an app starts shipping a version of its own.
    pub service_version: Option<String>,
    /// `production`, `staging`, … Exported as `deployment.environment.name`,
    /// which is the attribute every backend groups by first.
    pub environment: Option<String>,
    /// Extra resource attributes attached to every span and metric —
    /// `region`, `tenant`, `k8s.pod.name`. Values may reference the
    /// environment like any other string here.
    pub resource_attributes: BTreeMap<String, String>,
    pub logs: LogsConfig,
    pub traces: TracesConfig,
    pub metrics: MetricsConfig,
    pub otlp: OtlpConfig,
}

/// How the process writes to its own stdout.
///
/// This one applies whether or not `enabled` is set: a server writes logs
/// before it has been told to be observable.
#[derive(Debug, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct LogsConfig {
    /// `pretty` (the default) for a terminal, `json` for anything that will
    /// parse the line — a log shipper, `kubectl logs | jq`, CloudWatch.
    pub format: LogFormat,
    /// The `RUST_LOG` filter to use when the environment does not set one.
    /// `RUST_LOG` always wins, because it is what someone reaches for while
    /// debugging a running container.
    pub level: String,
    /// Include the current span's fields — request id, method, route — on
    /// every line written inside it. This is what makes a JSON log searchable
    /// by request without a trace backend.
    pub span_fields: bool,
}

impl Default for LogsConfig {
    fn default() -> Self {
        LogsConfig {
            format: LogFormat::Pretty,
            // ntex logs a line per worker at INFO, which drowns out the
            // startup output on machines with many cores.
            level: "info,apiplant=debug,ntex_server=warn".to_string(),
            span_fields: true,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LogFormat {
    #[default]
    Pretty,
    /// One line per event, fields inline — the terminal format without the
    /// indentation, for a log file a person still reads.
    Compact,
    /// One JSON object per line.
    Json,
}

/// Distributed traces: one span per request, children for the work inside it.
#[derive(Debug, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct TracesConfig {
    /// Build spans at all. On (with `[observability] enabled`) even when no
    /// exporter is configured, because the request id and the error fields a
    /// span carries are worth having in the logs alone.
    pub enabled: bool,
    /// Fraction of *root* requests recorded, `0.0`–`1.0`. This is *head*
    /// sampling — the decision is made before the request runs, so it cannot
    /// prefer the ones that will fail. Keeping every failure and a fraction of
    /// the rest is tail sampling, which is a Collector processor's job, not
    /// this server's: sample everything here and decide there.
    ///
    /// A sampled trace is sampled whole — a child never disagrees with its parent — and an
    /// incoming `traceparent` decides for its own trace, so a request arriving
    /// from an already-sampled caller is kept regardless of this number.
    pub sample_ratio: f64,
    /// Return the trace id to the caller as `X-Trace-Id`. What turns "it was
    /// slow at 14:02" from a support ticket into a lookup.
    pub response_header: bool,
    /// Request headers to copy onto the span. Never include anything that
    /// carries a credential — `authorization` and `cookie` are refused even if
    /// they are listed here.
    pub capture_headers: Vec<String>,
    /// Paths that are never traced, matched as prefixes after `base_path`.
    /// Health checks and asset requests are noise that costs money per span.
    pub exclude_paths: Vec<String>,
}

impl Default for TracesConfig {
    fn default() -> Self {
        TracesConfig {
            enabled: true,
            sample_ratio: 1.0,
            response_header: true,
            capture_headers: Vec::new(),
            exclude_paths: vec!["/_health".to_string()],
        }
    }
}

/// Metrics: the four numbers you page on, on the OpenTelemetry HTTP semantic
/// conventions so a stock dashboard reads them without being taught the app.
#[derive(Debug, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct MetricsConfig {
    /// Record and export metrics. Needs an OTLP endpoint to go anywhere.
    pub enabled: bool,
    /// How often the accumulated measurements are pushed to the collector.
    pub interval_secs: u64,
}

impl Default for MetricsConfig {
    fn default() -> Self {
        MetricsConfig {
            enabled: true,
            interval_secs: 60,
        }
    }
}

/// Where the data goes.
#[derive(Debug, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct OtlpConfig {
    /// Base URL of an OTLP/HTTP receiver — `http://localhost:4318`, or a
    /// vendor's ingest URL. The signal paths (`/v1/traces`, `/v1/metrics`) are
    /// appended. Unset falls back to `OTEL_EXPORTER_OTLP_ENDPOINT`; unset in
    /// both places exports nothing.
    pub endpoint: Option<String>,
    /// `http/protobuf` (the default, and what every collector accepts) or
    /// `http/json` for a receiver that only speaks JSON.
    pub protocol: OtlpProtocol,
    /// Sent with every export request — this is where a vendor's API key goes.
    /// Use `$VAR` rather than writing the key into a committed file.
    pub headers: BTreeMap<String, String>,
    /// How long one export may take before it is abandoned. The exporter drops
    /// the batch rather than blocking the process behind a collector that has
    /// stopped answering.
    pub timeout_secs: u64,
}

impl Default for OtlpConfig {
    fn default() -> Self {
        OtlpConfig {
            endpoint: None,
            protocol: OtlpProtocol::HttpProtobuf,
            headers: BTreeMap::new(),
            timeout_secs: 10,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
pub enum OtlpProtocol {
    #[default]
    #[serde(rename = "http/protobuf")]
    HttpProtobuf,
    #[serde(rename = "http/json")]
    HttpJson,
}

impl ObservabilityConfig {
    /// The endpoint to export to, config first and then the environment.
    ///
    /// `None` means nothing is exported — which is a supported way to run:
    /// spans still carry the logs, they simply stay in the process.
    pub fn endpoint(&self) -> Option<String> {
        self.otlp
            .endpoint
            .clone()
            .or_else(|| std::env::var("OTEL_EXPORTER_OTLP_ENDPOINT").ok())
            .map(|e| e.trim().trim_end_matches('/').to_string())
            .filter(|e| !e.is_empty())
    }

    /// The name this service reports itself under.
    pub fn service_name(&self, app_name: &str) -> String {
        self.service_name
            .clone()
            .or_else(|| std::env::var("OTEL_SERVICE_NAME").ok())
            .map(|n| n.trim().to_string())
            .filter(|n| !n.is_empty())
            .unwrap_or_else(|| app_name.to_string())
    }

    /// Every header sent with an export, the config's merged over anything
    /// `OTEL_EXPORTER_OTLP_HEADERS` supplied — the file is the more specific
    /// statement, so it wins a collision.
    pub fn export_headers(&self) -> BTreeMap<String, String> {
        let mut headers = BTreeMap::new();
        if let Ok(from_env) = std::env::var("OTEL_EXPORTER_OTLP_HEADERS") {
            // `key=value,key=value`, as the OTel specification defines it.
            for pair in from_env.split(',') {
                if let Some((key, value)) = pair.split_once('=') {
                    headers.insert(key.trim().to_string(), value.trim().to_string());
                }
            }
        }
        headers.extend(self.otlp.headers.clone());
        headers
    }

    /// Whether anything at all is being collected.
    pub fn is_active(&self) -> bool {
        self.enabled && (self.traces.enabled || self.metrics.enabled)
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
        // "bind everywhere" has three spellings people arrive with: leaving it
        // out, the wildcard, and the address itself. They all mean 0.0.0.0.
        let host = self.server.host.trim();
        if host.is_empty() || host == "*" {
            self.server.host = "0.0.0.0".to_string();
        } else {
            self.server.host = host.to_string();
        }

        // Same idea for the vhost filter: an empty or wildcard `domain` is a
        // request for no filter at all, not a filter for the empty host. `_` is
        // there because nginx spells its catch-all `server_name _`. A wildcard
        // anywhere in the list wins — it already answers every host, so the
        // named entries beside it can't narrow anything.
        let domains = std::mem::take(&mut self.server.domain);
        let mut wildcard = false;
        for d in domains {
            match d.trim() {
                "" | "*" | "_" | "0.0.0.0" => wildcard = true,
                d => self.server.domain.push(d.to_string()),
            }
        }
        if wildcard {
            self.server.domain.clear();
        }

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

        // A zero sweep interval is a timer that fires forever; a zero staleness
        // discards every bucket the moment it is written, which is the same as
        // having no rate limit at all. Neither is what `0` was meant to say.
        let defaults = RateLimitConfig::default();
        if self.rate_limit.cleanup_interval_secs == 0 {
            self.rate_limit.cleanup_interval_secs = defaults.cleanup_interval_secs;
        }
        if self.rate_limit.stale_after_secs == 0 {
            self.rate_limit.stale_after_secs = defaults.stale_after_secs;
        }

        // A ratio outside 0..1 is a typo for one of the ends — "10" for ten
        // percent is the common one — and silently sampling nothing is the
        // worst way to find out.
        self.observability.traces.sample_ratio =
            self.observability.traces.sample_ratio.clamp(0.0, 1.0);
        if self.observability.metrics.interval_secs == 0 {
            self.observability.metrics.interval_secs = MetricsConfig::default().interval_secs;
        }
        if self.observability.otlp.timeout_secs == 0 {
            self.observability.otlp.timeout_secs = OtlpConfig::default().timeout_secs;
        }
        // Matched as prefixes against a path that always starts with `/`.
        for path in &mut self.observability.traces.exclude_paths {
            if !path.starts_with('/') {
                *path = format!("/{path}");
            }
        }
        // Header names are compared lowercase, because HTTP/2 sends them that
        // way and a config written in `Title-Case` should still match.
        for header in &mut self.observability.traces.capture_headers {
            *header = header.trim().to_ascii_lowercase();
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
        assert!(!config.admin.ai_assistance.enabled);
        assert_eq!(
            config.admin.ai_assistance.prompt_placeholder,
            "Describe what you want AI to write for this field."
        );
        assert!(config.public.enabled);
        assert_eq!(config.public.dir, "public");
        assert_eq!(config.public.not_found, None);
        // Email, cache and payments are opt-in: an app that says nothing gets
        // none of them.
        assert!(!config.email.enabled());
        assert!(!config.cache.is_active());
        assert!(!config.payments.enabled());

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

    #[test]
    fn payments_load_from_their_section() {
        let dir = temp_dir("payments");
        fs::write(
            dir.join("main.toml"),
            r#"
[payments]
provider = "stripe"
secret_key = "sk_test_literal"
webhook_secret = "whsec_literal"
currency = "EUR"
"#,
        )
        .unwrap();

        let config = Config::load(&dir).unwrap();

        assert!(config.payments.enabled());
        assert!(config.payments.webhooks_enabled());
        // Stripe wants a lowercase currency, and nobody writes one.
        assert_eq!(config.payments.default_currency(), "eur");
        // Untouched defaults still apply inside a section that was given.
        assert!(config.payments.automatic_tax);
        assert_eq!(config.payments.timeout_secs, 20);

        fs::remove_dir_all(dir).unwrap();
    }

    /// A configured provider with no signing secret still takes money — the
    /// checkout is Stripe's page — but nothing of ours would hear that it
    /// worked, so the two questions are answered separately.
    #[test]
    fn webhooks_need_their_own_secret() {
        let payments = PaymentsConfig {
            provider: "stripe".into(),
            secret_key: "sk_test".into(),
            ..PaymentsConfig::default()
        };
        assert!(payments.enabled());
        assert!(!payments.webhooks_enabled());
    }

    /// Asking for a VAT number is only useful to somebody computing tax with
    /// it, so the default follows automatic tax — and an app can still say
    /// otherwise in either direction.
    #[test]
    fn tax_id_collection_follows_automatic_tax_unless_told_otherwise() {
        let with_tax = PaymentsConfig::default();
        assert!(with_tax.automatic_tax && with_tax.collects_tax_ids());

        let no_tax = PaymentsConfig {
            automatic_tax: false,
            ..PaymentsConfig::default()
        };
        assert!(!no_tax.collects_tax_ids());

        let explicit = PaymentsConfig {
            automatic_tax: false,
            tax_id_collection: Some(true),
            ..PaymentsConfig::default()
        };
        assert!(explicit.collects_tax_ids());
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
        assert_eq!(config.server.domain, ["api.example.com"]);

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
    fn load_treats_wildcard_host_and_domain_as_everything() {
        for (host, domain) in [
            ("", "\"\""),
            ("*", "\"*\""),
            (" 0.0.0.0 ", "\"_\""),
            ("*", "[]"),
            // A wildcard beside named hosts still means "answer any host".
            ("*", "[\"api.example.com\", \"*\"]"),
        ] {
            let dir = temp_dir("wildcards");
            fs::write(
                dir.join("main.toml"),
                format!("[server]\nhost = \"{host}\"\ndomain = {domain}\n"),
            )
            .unwrap();

            let config = Config::load(&dir).unwrap();

            assert_eq!(config.server.host, "0.0.0.0", "host {host:?}");
            assert!(config.server.domain.is_empty(), "domain {domain}");
            fs::remove_dir_all(&dir).unwrap();
        }
    }

    /// `domain` takes a list as readily as a single string, and each entry is
    /// trimmed the same way.
    #[test]
    fn load_accepts_a_list_of_domains() {
        let dir = temp_dir("domains");
        fs::write(
            dir.join("main.toml"),
            "[server]\ndomain = [\"api.example.com\", \" www.example.com \"]\n",
        )
        .unwrap();

        let config = Config::load(&dir).unwrap();

        assert_eq!(config.server.domain, ["api.example.com", "www.example.com"]);
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

[admin.ai_assistance]
enabled = true
system = "Return only the field content."
prompt_placeholder = "Tell AI what to draft"

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
        assert!(config.admin.ai_assistance.enabled);
        assert_eq!(
            config.admin.ai_assistance.system,
            "Return only the field content."
        );
        assert_eq!(
            config.admin.ai_assistance.prompt_placeholder,
            "Tell AI what to draft"
        );
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

    #[test]
    fn observability_is_off_until_it_is_asked_for() {
        let config = Config::default();
        assert!(!config.observability.enabled);
        assert!(!config.observability.is_active());
        // Off, but the logs still have a format and a level — a process writes
        // to its terminal before anyone configures monitoring.
        assert_eq!(config.observability.logs.format, LogFormat::Pretty);
        assert!(config.observability.logs.level.contains("info"));
    }

    #[test]
    fn an_observability_section_is_read_whole() {
        let dir = temp_dir("observability");
        fs::write(
            dir.join("main.toml"),
            r#"
[observability]
enabled = true
service_name = "checkout"
environment = "production"
resource_attributes = { region = "eu-west-1" }

[observability.logs]
format = "json"

[observability.traces]
sample_ratio = 0.25
capture_headers = ["X-Request-Id"]
exclude_paths = ["_health", "/metrics"]

[observability.otlp]
endpoint = "http://collector:4318/"
protocol = "http/json"
headers = { authorization = "Bearer t" }
"#,
        )
        .unwrap();
        let config = Config::load(&dir).unwrap();
        let observability = &config.observability;

        assert!(observability.is_active());
        assert_eq!(observability.logs.format, LogFormat::Json);
        assert_eq!(observability.otlp.protocol, OtlpProtocol::HttpJson);
        // The trailing slash goes, because the signal path is appended to this.
        assert_eq!(
            observability.endpoint().as_deref(),
            Some("http://collector:4318")
        );
        assert_eq!(observability.service_name("fallback"), "checkout");
        assert_eq!(
            observability.export_headers().get("authorization").unwrap(),
            "Bearer t"
        );
        // Both spellings of an excluded path end up matchable against a
        // request path, and a header is lowercased to match what HTTP/2 sends.
        assert_eq!(observability.traces.exclude_paths, ["/_health", "/metrics"]);
        assert_eq!(observability.traces.capture_headers, ["x-request-id"]);
        assert_eq!(observability.traces.sample_ratio, 0.25);
    }

    #[test]
    fn a_sample_ratio_outside_the_range_is_a_typo_for_one_of_the_ends() {
        let dir = temp_dir("sampling");
        fs::write(
            dir.join("main.toml"),
            "[observability.traces]\nsample_ratio = 10.0\n",
        )
        .unwrap();
        // "10" meant "ten percent"; sampling nothing would be the worst
        // possible reading of it, so it clamps to "everything" instead.
        assert_eq!(
            Config::load(&dir)
                .unwrap()
                .observability
                .traces
                .sample_ratio,
            1.0
        );
    }

    #[test]
    fn the_app_name_is_the_service_name_when_nothing_else_says_otherwise() {
        let observability = ObservabilityConfig::default();
        assert!(
            observability.endpoint().is_none()
                || std::env::var("OTEL_EXPORTER_OTLP_ENDPOINT").is_ok()
        );
        assert_eq!(observability.service_name("my-app"), "my-app");
    }
}
