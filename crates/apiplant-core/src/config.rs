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
    pub payments: PaymentsConfig,
    pub ai: AiConfig,
    pub oauth: OAuthConfig,
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
            ai_assistance: AdminAiAssistanceConfig::default(),
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
}
