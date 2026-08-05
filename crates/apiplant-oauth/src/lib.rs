//! # apiplant-oauth
//!
//! Signing in with somebody else's account, as two lines of configuration
//! rather than a handshake to implement.
//!
//! ```toml
//! [oauth.github]
//! client_id     = "${GITHUB_CLIENT_ID}"
//! client_secret = "${GITHUB_CLIENT_SECRET}"
//! ```
//!
//! That is the whole integration. apiplant knows GitHub's authorize URL, its
//! token URL, where its profile lives, which scopes reach a verified address,
//! that it has no use for PKCE and that it will take the client secret in a
//! form body — and the same for Google, LinkedIn and X, which disagree with
//! GitHub and with each other on every one of those points.
//!
//! ## What this crate is, and is not
//!
//! It is the part of a sign-in that talks to a provider: build an authorize
//! URL, redeem a code, read a profile, hand back the same five fields whoever
//! answered. It holds no database handle and issues no session — deciding
//! *whose account this is* is the app's business, and lives in
//! `apiplant-server`'s oauth routes where the user table and the hooks are.
//!
//! The split is deliberate. Everything here is testable against a fixture and
//! has no opinion about accounts; everything there is about accounts and has no
//! opinion about GitHub.
//!
//! ## The security-relevant parts
//!
//! * **`state`** is minted by the caller and stored hashed; this crate only
//!   puts it in the URL. What makes it worth anything is that the callback
//!   refuses a `state` the server never issued — the defence against login
//!   CSRF, where an attacker completes *their* provider flow in your browser so
//!   that your next save lands in their account.
//! * **PKCE** ([`Flow::verifier`]) is generated here and never leaves the
//!   server: only its SHA-256 travels with the redirect.
//! * **`redirect_uri`** is repeated identically in the token request, because
//!   the provider compares the two and refuses on any difference. It comes from
//!   the stored flow, never from the request that completes it.
//! * **The client secret** appears in exactly one request: the token exchange,
//!   server to provider. It is never in a URL, a redirect or a page.

mod providers;

use std::collections::BTreeMap;
use std::time::Duration;

use apiplant_core::config::{OAuthConfig, OAuthProviderConfig};
use base64::Engine as _;
use sha2::{Digest, Sha256};

pub use providers::{Builtin, Profile, Style, BUILTIN};

/// Anything that can go wrong between this server and a provider.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// The app named a provider it has no credentials for, or none at all.
    #[error("{0}")]
    NotConfigured(String),
    /// A provider apiplant does not ship, configured without the URLs it would
    /// need. Caught at boot, so it is a startup failure and not a 500 in front
    /// of somebody trying to sign in.
    #[error("{0}")]
    Misconfigured(String),
    /// The provider could not be reached at all.
    #[error("{provider} could not be reached ({stage}): {source}")]
    Unreachable {
        provider: String,
        stage: &'static str,
        #[source]
        source: reqwest::Error,
    },
    /// The provider answered, and said no. The message is the provider's own —
    /// it is written for whoever misconfigured something, and names no secret
    /// of ours.
    #[error("{provider} refused the {stage}: {detail}")]
    Refused {
        provider: String,
        stage: &'static str,
        detail: String,
    },
    /// The provider answered with something this crate could not read.
    #[error("{0}")]
    Unreadable(String),
}

/// One provider, with its built-in defaults and this app's config merged — what
/// a sign-in actually uses.
#[derive(Debug, Clone)]
pub struct Provider {
    pub key: String,
    pub label: String,
    pub client_id: String,
    client_secret: String,
    pub authorize_url: String,
    pub token_url: String,
    pub userinfo_url: String,
    pub scopes: String,
    /// Where the provider sends the browser back. Registered in the provider's
    /// dashboard, and compared byte for byte — including a trailing slash.
    pub redirect_uri: String,
    pub style: Style,
    pub pkce: bool,
    pub basic_auth: bool,
    /// Whether this provider will ever give an email address. An app that
    /// requires a confirmed address cannot use one that says no.
    pub provides_email: bool,
    /// A logo for the button, when the app supplied one. Empty for the four
    /// apiplant draws itself, and for an unknown provider whose config named
    /// no image — a client falls back to whatever it shows for those.
    pub icon: String,
}

/// Every provider this app has switched on.
///
/// Built once at boot: a provider missing its URLs fails the boot rather than
/// the first person to click its button.
#[derive(Debug, Clone, Default)]
pub struct Providers {
    providers: BTreeMap<String, Provider>,
    client: Option<reqwest::Client>,
}

impl Providers {
    /// Resolve `[oauth]` into usable providers, or `None` when the app has
    /// configured none.
    ///
    /// `callback_base` is where the provider is told to send the browser, minus
    /// the provider's name — `https://example.com/api/auth/oauth`. Each
    /// provider's redirect URI is that plus `/<provider>/callback`, unless the
    /// block names one outright.
    pub fn from_config(
        config: &OAuthConfig,
        callback_base: &str,
    ) -> Result<Option<Providers>, Error> {
        let mut providers = BTreeMap::new();
        for (name, settings) in &config.providers {
            if !settings.is_active() {
                // Named but switched off, or named with no credentials — which
                // is how one committed config serves a deployment that has
                // GitHub keys and one that has all four.
                continue;
            }
            let provider = resolve(name, settings, callback_base)?;
            providers.insert(provider.key.clone(), provider);
        }
        if providers.is_empty() {
            return Ok(None);
        }

        let client = reqwest::Client::builder()
            // A provider that hangs must not hold a request open until somebody
            // notices. Both halves of a handshake are a single round trip to a
            // large company's edge; ten seconds is generous.
            .timeout(Duration::from_secs(10))
            .connect_timeout(Duration::from_secs(5))
            // GitHub rejects a request without one, and every provider's
            // support team appreciates knowing who is calling.
            .user_agent(concat!("apiplant/", env!("CARGO_PKG_VERSION")))
            .build()
            .map_err(|e| Error::Misconfigured(format!("could not build an HTTP client: {e}")))?;

        Ok(Some(Providers {
            providers,
            client: Some(client),
        }))
    }

    pub fn get(&self, key: &str) -> Option<&Provider> {
        self.providers.get(key.trim().to_lowercase().as_str())
    }

    pub fn iter(&self) -> impl Iterator<Item = &Provider> {
        self.providers.values()
    }

    pub fn is_empty(&self) -> bool {
        self.providers.is_empty()
    }

    /// The reply `GET <base>/auth/oauth` gives: enough for a sign-in page to
    /// draw its buttons and know where each one goes.
    pub fn describe(&self, base: &str) -> serde_json::Value {
        serde_json::Value::Array(
            self.iter()
                .map(|p| {
                    serde_json::json!({
                        "provider": p.key,
                        "label": p.label,
                        "provides_email": p.provides_email,
                        "icon": p.icon,
                        "start_url": format!("{base}/{}/start", p.key),
                    })
                })
                .collect(),
        )
    }

    fn client(&self) -> Result<&reqwest::Client, Error> {
        self.client
            .as_ref()
            .ok_or_else(|| Error::NotConfigured("no OAuth providers are configured".into()))
    }

    /// Swap an authorization code for an access token.
    ///
    /// The one request that carries the client secret, and therefore the one
    /// that can never happen in a browser.
    pub async fn exchange_code(
        &self,
        provider: &Provider,
        code: &str,
        verifier: Option<&str>,
        redirect_uri: &str,
    ) -> Result<String, Error> {
        let mut form: Vec<(&str, &str)> = vec![
            ("grant_type", "authorization_code"),
            ("code", code),
            // From the stored flow, not from the request that got here: a code
            // redeemed against a `redirect_uri` of the caller's choosing is a
            // code redeemed by somebody else's app.
            ("redirect_uri", redirect_uri),
            ("client_id", &provider.client_id),
        ];
        if let Some(verifier) = verifier {
            form.push(("code_verifier", verifier));
        }
        if !provider.basic_auth {
            form.push(("client_secret", &provider.client_secret));
        }

        let mut request = self
            .client()?
            .post(&provider.token_url)
            // GitHub answers `access_token=…&scope=…` — form encoding, still,
            // in 2026 — unless asked for JSON. Everyone else ignores this.
            .header("accept", "application/json")
            .form(&form);
        if provider.basic_auth {
            request = request.basic_auth(&provider.client_id, Some(&provider.client_secret));
        }

        let body = send(request, &provider.label, "token exchange").await?;

        // A provider can answer 200 with an error object in it: the status code
        // describes the HTTP call, not the grant.
        if let Some(error) = body["error"].as_str() {
            let detail = body["error_description"].as_str().unwrap_or(error);
            return Err(Error::Refused {
                provider: provider.label.clone(),
                stage: "code",
                detail: detail.to_string(),
            });
        }
        body["access_token"]
            .as_str()
            .filter(|token| !token.is_empty())
            .map(str::to_string)
            .ok_or_else(|| {
                Error::Unreadable(format!("{} returned no access token", provider.label))
            })
    }

    /// Read the profile that token grants access to.
    pub async fn fetch_profile(
        &self,
        provider: &Provider,
        access_token: &str,
    ) -> Result<Profile, Error> {
        let body = send(
            self.client()?
                .get(&provider.userinfo_url)
                .bearer_auth(access_token)
                .header("accept", "application/json"),
            &provider.label,
            "profile",
        )
        .await?;

        let emails = match provider.style {
            // `<userinfo>/emails`, derived rather than hardcoded, so a GitHub
            // Enterprise host configured through `userinfo_url` is asked about
            // its own users instead of github.com's.
            //
            // A refused `user:email` scope makes this a 403, and that costs the
            // address rather than the sign-in: the call above already said who
            // this is.
            Style::GitHub => send(
                self.client()?
                    .get(format!("{}/emails", provider.userinfo_url))
                    .bearer_auth(access_token)
                    .header("accept", "application/json"),
                &provider.label,
                "emails",
            )
            .await
            .map_err(|error| {
                tracing::debug!(provider = %provider.key, %error, "could not read the email list");
                error
            })
            .ok(),
            _ => None,
        };

        providers::profile(provider.style, &provider.label, &body, emails.as_ref())
            .map_err(Error::Unreadable)
    }
}

impl Provider {
    /// Start a sign-in: everything the browser needs, and everything the
    /// callback will need kept back.
    ///
    /// The caller stores [`Flow::state_hash`] and [`Flow::verifier`], and sends
    /// the browser to [`Flow::authorize_url`]. `state` itself is returned for a
    /// caller that wants to show it; it is not required to keep it.
    pub fn start(&self) -> Result<Flow, Error> {
        // 32 bytes of CSPRNG. A `state` anybody can predict is the same as not
        // having one.
        let state = secret(32);
        let verifier = self.pkce.then(|| secret(64));

        let mut url = url::Url::parse(&self.authorize_url).map_err(|e| {
            Error::Misconfigured(format!("{} has an unusable authorize URL: {e}", self.label))
        })?;
        {
            // `query_pairs_mut`, not `format!`: a scope list contains spaces, a
            // redirect URI contains `:` and `/`, and a state is base64url —
            // all of which need encoding and none of which are worth getting
            // wrong by hand.
            let mut query = url.query_pairs_mut();
            query
                .append_pair("response_type", "code")
                .append_pair("client_id", &self.client_id)
                .append_pair("redirect_uri", &self.redirect_uri)
                .append_pair("scope", &self.scopes)
                .append_pair("state", &state);
            if let Some(verifier) = &verifier {
                query
                    .append_pair("code_challenge", &code_challenge(verifier))
                    .append_pair("code_challenge_method", "S256");
            }
            if self.key == "google" {
                // Without `access_type=online` Google issues a refresh token
                // this app has said it does not want. `select_account` is what
                // makes "sign in as somebody else" possible on a shared
                // machine, which is otherwise not possible at all.
                query
                    .append_pair("access_type", "online")
                    .append_pair("prompt", "select_account");
            }
        }

        Ok(Flow {
            state_hash: sha256_hex(&state),
            state,
            verifier,
            authorize_url: url.to_string(),
            redirect_uri: self.redirect_uri.clone(),
        })
    }
}

/// A sign-in that has been started and not yet finished.
#[derive(Debug, Clone)]
pub struct Flow {
    /// The `state` parameter, already in `authorize_url`.
    pub state: String,
    /// What to store: the value itself is in a URL, in browser history and in
    /// the provider's logs, so only its hash belongs in a table.
    pub state_hash: String,
    /// The PKCE verifier to store, when this provider uses PKCE.
    pub verifier: Option<String>,
    /// Where to send the browser.
    pub authorize_url: String,
    /// What the provider was told, to be repeated at the token exchange.
    pub redirect_uri: String,
}

/// Merge a provider's built-in defaults with an app's `[oauth.<name>]` block.
fn resolve(
    name: &str,
    settings: &OAuthProviderConfig,
    callback_base: &str,
) -> Result<Provider, Error> {
    let key = name.trim().to_lowercase();
    let builtin = providers::builtin(&key);

    let pick = |configured: &str, default: &str| match configured.trim() {
        "" => default.to_string(),
        value => value.to_string(),
    };
    let required = |configured: &str, field: &str| -> Result<String, Error> {
        match configured.trim() {
            "" => Err(Error::Misconfigured(format!(
                "[oauth.{key}] needs `{field}`: apiplant ships {known}, and anything \
                 else has to say where its endpoints are",
                known = providers::known()
            ))),
            value => Ok(value.to_string()),
        }
    };

    let style = match builtin {
        // An app may still override the style of a provider apiplant knows —
        // pointing `github` at a GitHub Enterprise host keeps GitHub's shape,
        // but pointing it at something else may not.
        Some(b) if settings.style.trim().is_empty() => b.style,
        _ => Style::parse(&settings.style).ok_or_else(|| {
            Error::Misconfigured(format!(
                "[oauth.{key}] has an unknown `style` — use `oidc` or `github`"
            ))
        })?,
    };

    if settings.client_secret.trim().is_empty() {
        // Every provider apiplant ships is a confidential client, and a
        // deployment that got this far has a client id — so an empty secret is
        // a half-finished setup, and saying so at boot beats a 401 from the
        // provider at the worst moment.
        return Err(Error::Misconfigured(format!(
            "[oauth.{key}] has a `client_id` but no `client_secret`{}",
            match builtin {
                Some(b) => format!(" — both are on {}", b.console),
                None => String::new(),
            }
        )));
    }

    Ok(Provider {
        label: pick(
            &settings.label,
            builtin.map(|b| b.label).unwrap_or(&capitalise(&key)),
        ),
        client_id: settings.client_id.trim().to_string(),
        client_secret: settings.client_secret.trim().to_string(),
        authorize_url: match builtin {
            Some(b) => pick(&settings.authorize_url, b.authorize_url),
            None => required(&settings.authorize_url, "authorize_url")?,
        },
        token_url: match builtin {
            Some(b) => pick(&settings.token_url, b.token_url),
            None => required(&settings.token_url, "token_url")?,
        },
        userinfo_url: match builtin {
            Some(b) => pick(&settings.userinfo_url, b.userinfo_url),
            None => required(&settings.userinfo_url, "userinfo_url")?,
        },
        scopes: match builtin {
            Some(b) => pick(&settings.scopes, b.scopes),
            // No default worth guessing: an unknown provider's scopes are its
            // own vocabulary, and asking for the wrong ones fails at the
            // consent screen where it is hardest to debug.
            None => required(&settings.scopes, "scopes")?,
        },
        redirect_uri: pick(
            &settings.redirect_uri,
            &format!("{}/{key}/callback", callback_base.trim_end_matches('/')),
        ),
        icon: settings.icon.trim().to_string(),
        pkce: settings
            .pkce
            .unwrap_or_else(|| builtin.is_some_and(|b| b.pkce)),
        basic_auth: builtin.is_some_and(|b| b.basic_auth),
        provides_email: builtin.map(|b| b.provides_email).unwrap_or(true),
        style,
        key,
    })
}

fn capitalise(key: &str) -> String {
    let mut chars = key.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
        None => String::new(),
    }
}

/// `n` bytes from the OS CSPRNG, base64url-encoded.
fn secret(bytes: usize) -> String {
    use rand::RngCore;
    let mut buffer = vec![0u8; bytes];
    rand::thread_rng().fill_bytes(&mut buffer);
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(buffer)
}

/// SHA-256, hex — how a `state` is stored and looked up.
pub fn sha256_hex(input: &str) -> String {
    let digest = Sha256::digest(input.as_bytes());
    digest.iter().map(|b| format!("{b:02x}")).collect()
}

/// SHA-256, base64url, unpadded — PKCE's `S256` challenge (RFC 7636).
fn code_challenge(verifier: &str) -> String {
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()))
}

/// Send a request and read JSON, or an error worth reading.
async fn send(
    request: reqwest::RequestBuilder,
    label: &str,
    stage: &'static str,
) -> Result<serde_json::Value, Error> {
    let response = request.send().await.map_err(|source| Error::Unreachable {
        provider: label.to_string(),
        stage,
        source,
    })?;
    let status = response.status();
    let body = response.text().await.map_err(|source| Error::Unreachable {
        provider: label.to_string(),
        stage,
        source,
    })?;

    if !status.is_success() {
        // The provider's own words, truncated: they name what is wrong with the
        // app's registration, which is exactly what whoever is reading the log
        // needs, and nothing this server holds.
        return Err(Error::Refused {
            provider: label.to_string(),
            stage,
            detail: format!("HTTP {} — {}", status.as_u16(), truncate(&body, 500)),
        });
    }
    serde_json::from_str(&body)
        .map_err(|e| Error::Unreadable(format!("{label} {stage}: reply was not JSON ({e})")))
}

fn truncate(text: &str, limit: usize) -> String {
    match text.char_indices().nth(limit) {
        Some((index, _)) => format!("{}…", &text[..index]),
        None => text.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use apiplant_core::config::OAuthConfig;

    fn config(toml_src: &str) -> OAuthConfig {
        toml::from_str(toml_src).expect("test config parses")
    }

    #[test]
    fn a_provider_needs_only_two_credentials() {
        let providers = Providers::from_config(
            &config(
                r#"
                [github]
                client_id = "abc"
                client_secret = "shh"
                "#,
            ),
            "https://example.com/api/auth/oauth",
        )
        .unwrap()
        .expect("one provider");

        let github = providers.get("github").expect("github");
        assert_eq!(
            github.authorize_url,
            "https://github.com/login/oauth/authorize"
        );
        assert_eq!(github.scopes, "read:user user:email");
        assert_eq!(
            github.redirect_uri,
            "https://example.com/api/auth/oauth/github/callback"
        );
        assert!(!github.pkce, "GitHub does not offer PKCE");
        assert!(github.provides_email);
    }

    #[test]
    fn no_configured_provider_means_no_oauth_at_all() {
        assert!(Providers::from_config(&config(""), "https://example.com")
            .unwrap()
            .is_none());
        // Named, credentialed, and switched off.
        let off = config(
            r#"
            [google]
            client_id = "abc"
            client_secret = "shh"
            enabled = false
            "#,
        );
        assert!(Providers::from_config(&off, "https://example.com")
            .unwrap()
            .is_none());
    }

    /// Boot rather than the first person to press the button.
    #[test]
    fn an_unknown_provider_must_bring_its_own_endpoints() {
        let bare = config(
            r#"
            [gitlab]
            client_id = "abc"
            client_secret = "shh"
            "#,
        );
        let error = Providers::from_config(&bare, "https://example.com").unwrap_err();
        assert!(error.to_string().contains("authorize_url"), "{error}");

        let complete = config(
            r#"
            [gitlab]
            client_id = "abc"
            client_secret = "shh"
            authorize_url = "https://gitlab.com/oauth/authorize"
            token_url = "https://gitlab.com/oauth/token"
            userinfo_url = "https://gitlab.com/oauth/userinfo"
            scopes = "openid email"
            "#,
        );
        let providers = Providers::from_config(&complete, "https://example.com")
            .unwrap()
            .expect("gitlab");
        let gitlab = providers.get("gitlab").unwrap();
        assert_eq!(gitlab.style, Style::Oidc, "OIDC is the sane default");
        assert_eq!(gitlab.label, "Gitlab");
    }

    #[test]
    fn a_client_id_without_a_secret_is_a_boot_failure() {
        let half = config(
            r#"
            [github]
            client_id = "abc"
            "#,
        );
        let error = Providers::from_config(&half, "https://example.com").unwrap_err();
        assert!(error.to_string().contains("client_secret"), "{error}");
    }

    #[test]
    fn the_authorize_url_carries_state_and_pkce_where_the_provider_wants_it() {
        let providers = Providers::from_config(
            &config(
                r#"
                [google]
                client_id = "abc"
                client_secret = "shh"
                "#,
            ),
            "https://example.com/api/auth/oauth",
        )
        .unwrap()
        .unwrap();

        let flow = providers.get("google").unwrap().start().unwrap();
        assert!(flow.authorize_url.contains("code_challenge_method=S256"));
        assert!(flow
            .authorize_url
            .contains(&format!("state={}", urlencode(&flow.state))));
        assert!(flow.verifier.is_some());
        // The stored value is the hash, never the state itself.
        assert_eq!(flow.state_hash, sha256_hex(&flow.state));
        assert_ne!(flow.state_hash, flow.state);
        // Two flows never share a state.
        let second = providers.get("google").unwrap().start().unwrap();
        assert_ne!(flow.state, second.state);
    }

    #[test]
    fn a_provider_without_pkce_sends_no_challenge() {
        let providers = Providers::from_config(
            &config(
                r#"
                [github]
                client_id = "abc"
                client_secret = "shh"
                "#,
            ),
            "https://example.com/api/auth/oauth",
        )
        .unwrap()
        .unwrap();
        let flow = providers.get("github").unwrap().start().unwrap();
        assert!(!flow.authorize_url.contains("code_challenge"));
        assert!(flow.verifier.is_none());
    }

    /// RFC 7636's own worked example, so the encoding is right rather than
    /// merely self-consistent.
    #[test]
    fn the_pkce_challenge_matches_the_rfc() {
        let verifier = "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk";
        assert_eq!(
            code_challenge(verifier),
            "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"
        );
    }

    fn urlencode(value: &str) -> String {
        value
            .replace('+', "%2B")
            .replace('/', "%2F")
            .replace('=', "%3D")
    }
}
