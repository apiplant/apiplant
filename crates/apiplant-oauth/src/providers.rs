//! The providers apiplant ships, and the two places they differ.
//!
//! Every OAuth 2.0 provider agrees on the shape of the dance — redirect with a
//! `client_id`, come back with a `code`, swap the code for a token — and none of
//! them agree on anything else. This module is where that disagreement is
//! written down once, so the flow itself never asks who it is talking to:
//!
//! * [`Builtin`] holds the endpoints and the flags — does it want PKCE, does it
//!   insist on HTTP Basic, will it ever give an email address;
//! * [`Style`] says how to read the profile that comes back.
//!
//! An app can name a provider that is not here by supplying the three URLs in
//! its `[oauth.<name>]` block; almost everything issuing tokens today speaks
//! OpenID Connect, which is [`Style::Oidc`] and needs no code at all.

use serde_json::Value;

/// How a provider's userinfo response is read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Style {
    /// OpenID Connect standard claims: `sub`, `email`, `email_verified`,
    /// `name`, `picture`. Google, LinkedIn, and most of the rest of the world.
    Oidc,
    /// GitHub's own shape, which predates OIDC: a numeric `id`, `login`, and an
    /// address that usually has to be fetched separately.
    GitHub,
    /// X's: everything wrapped in `data`, and no email at all.
    X,
}

impl Style {
    pub fn parse(value: &str) -> Option<Style> {
        match value.trim().to_lowercase().as_str() {
            "" | "oidc" | "openid" | "openid_connect" => Some(Style::Oidc),
            "github" => Some(Style::GitHub),
            "x" | "twitter" => Some(Style::X),
            _ => None,
        }
    }
}

/// Everything about a provider that is the same for every deployment.
///
/// Credentials are not here — those come from `[oauth.<name>]`. Endpoints are,
/// because an app that must paste four URLs into its config to sign in with
/// Google is an app where four URLs can be pasted in wrong.
#[derive(Debug, Clone)]
pub struct Builtin {
    pub key: &'static str,
    /// What the sign-in button says.
    pub label: &'static str,
    pub authorize_url: &'static str,
    pub token_url: &'static str,
    pub userinfo_url: &'static str,
    /// The least this app can ask for and still learn who signed in. Every
    /// extra scope is another line on a consent screen, and another reason for
    /// somebody to press Cancel.
    pub scopes: &'static str,
    pub style: Style,
    /// Whether the provider supports (or, for X, requires) PKCE.
    ///
    /// A confidential server client redeeming a code with a secret is already
    /// protected against an intercepted code, so PKCE here is defence in depth
    /// — which is why the two providers that do not offer it are not a problem.
    pub pkce: bool,
    /// Whether the client credentials go in the `Authorization: Basic` header
    /// of the token request rather than its form body.
    ///
    /// RFC 6749 says a server *must* support Basic and *may* support the body.
    /// In practice everyone accepts the body except X — which is why this is a
    /// flag and not a comment saying "X is special".
    pub basic_auth: bool,
    /// Whether this provider will ever hand over an email address.
    ///
    /// False for X: an app needs elevated access to request one, so an X
    /// sign-in produces an account with no address. That is a fact about X
    /// rather than an error, and the flow plans for it instead of failing.
    pub provides_email: bool,
    /// Where the app is registered, quoted in the error a missing credential
    /// produces — the fastest way to fix that error is a link to the page.
    pub console: &'static str,
}

pub const BUILTIN: &[Builtin] = &[
    Builtin {
        key: "github",
        label: "GitHub",
        authorize_url: "https://github.com/login/oauth/authorize",
        token_url: "https://github.com/login/oauth/access_token",
        userinfo_url: "https://api.github.com/user",
        // `user:email` is what makes the *primary, verified* address reachable.
        // Without it `GET /user` returns whatever the person made public, which
        // is frequently nothing.
        scopes: "read:user user:email",
        style: Style::GitHub,
        pkce: false,
        basic_auth: false,
        provides_email: true,
        console: "https://github.com/settings/developers",
    },
    Builtin {
        key: "google",
        label: "Google",
        authorize_url: "https://accounts.google.com/o/oauth2/v2/auth",
        token_url: "https://oauth2.googleapis.com/token",
        userinfo_url: "https://openidconnect.googleapis.com/v1/userinfo",
        scopes: "openid email profile",
        style: Style::Oidc,
        pkce: true,
        basic_auth: false,
        provides_email: true,
        console: "https://console.cloud.google.com/apis/credentials",
    },
    Builtin {
        key: "linkedin",
        label: "LinkedIn",
        authorize_url: "https://www.linkedin.com/oauth/v2/authorization",
        token_url: "https://www.linkedin.com/oauth/v2/accessToken",
        // LinkedIn's OpenID Connect userinfo. The old `/v2/me` +
        // `/v2/emailAddress` pair and its `r_liteprofile` scope are gone: an app
        // created today gets "Sign In with LinkedIn using OpenID Connect", and
        // the product has to be requested before these scopes are available.
        userinfo_url: "https://api.linkedin.com/v2/userinfo",
        scopes: "openid profile email",
        style: Style::Oidc,
        pkce: false,
        basic_auth: false,
        provides_email: true,
        console: "https://www.linkedin.com/developers/apps",
    },
    Builtin {
        key: "x",
        label: "X",
        authorize_url: "https://x.com/i/oauth2/authorize",
        token_url: "https://api.x.com/2/oauth2/token",
        userinfo_url: "https://api.x.com/2/users/me?user.fields=profile_image_url,username",
        // `users.read` needs `tweet.read` beside it — X refuses the pair split
        // up. No `offline.access`: that asks for a refresh token, and an app
        // that only signs people in has nothing to refresh.
        scopes: "users.read tweet.read",
        style: Style::X,
        // Required, not optional: X rejects an authorize request with no
        // `code_challenge`, confidential clients included.
        pkce: true,
        // X is the reason `basic_auth` exists. A confidential client that sends
        // `client_secret` in the token request body gets a 401 and no
        // explanation worth reading.
        basic_auth: true,
        provides_email: false,
        console: "https://developer.x.com/en/portal/dashboard",
    },
];

pub fn builtin(key: &str) -> Option<&'static Builtin> {
    BUILTIN.iter().find(|p| p.key == key)
}

/// The names apiplant knows, for an error message worth reading.
pub fn known() -> String {
    BUILTIN.iter().map(|p| p.key).collect::<Vec<_>>().join(", ")
}

/// Who signed in, in the same five fields whichever provider answered.
#[derive(Debug, Clone, Default)]
pub struct Profile {
    /// The provider's immutable id for this person. Never a username: GitHub
    /// and X both let people change theirs, and both let a freed name be taken
    /// by somebody else — an account keyed on one would hand the new owner the
    /// old owner's account.
    pub id: String,
    pub email: Option<String>,
    /// Whether the *provider* says it verified that address.
    ///
    /// The whole account-matching decision hangs on this, so it is never
    /// assumed: a provider that does not say gets `false`, and `false` means an
    /// address good enough to display and not good enough to match an existing
    /// account on.
    pub email_verified: bool,
    pub display_name: Option<String>,
    pub avatar_url: Option<String>,
}

/// Normalise a userinfo response.
///
/// `emails` is GitHub's second call — `None` for everyone else.
pub fn profile(
    style: Style,
    label: &str,
    body: &Value,
    emails: Option<&Value>,
) -> Result<Profile, String> {
    let profile = match style {
        Style::GitHub => {
            let id = match &body["id"] {
                Value::Number(n) => n.to_string(),
                Value::String(s) => s.clone(),
                _ => return Err(format!("{label} returned a profile with no id")),
            };
            // `email` on the profile is the *public* one, and is usually null.
            // The real answer is in `/user/emails`: the address marked primary
            // and verified, which is the one GitHub itself would write to.
            let (email, verified) = match emails.and_then(Value::as_array) {
                Some(list) => {
                    // Verified beats primary, because "verified" is what the
                    // matching rule needs and "primary" is only what GitHub
                    // shows first. An unverified address is still taken when it
                    // is all there is — it is worth displaying — and reported as
                    // unverified, which is what keeps it from matching anything.
                    let chosen = list
                        .iter()
                        .find(|e| e["primary"] == true && e["verified"] == true)
                        .or_else(|| list.iter().find(|e| e["verified"] == true))
                        .or_else(|| list.iter().find(|e| e["primary"] == true))
                        .or_else(|| list.first());
                    (
                        chosen.and_then(|e| e["email"].as_str()).map(str::to_string),
                        chosen.is_some_and(|e| e["verified"] == true),
                    )
                }
                // Only reachable when `user:email` was refused or the second
                // call failed.
                None => (text(body, "email"), false),
            };
            Profile {
                id,
                email,
                email_verified: verified,
                display_name: text(body, "name").or_else(|| text(body, "login")),
                avatar_url: text(body, "avatar_url"),
            }
        }
        Style::Oidc => Profile {
            id: body["sub"]
                .as_str()
                .ok_or_else(|| format!("{label} returned a profile with no `sub`"))?
                .to_string(),
            email: text(body, "email"),
            // Google sends a real boolean; LinkedIn has been known to send the
            // string "true". Both are accepted, anything else is `false`.
            email_verified: match &body["email_verified"] {
                Value::Bool(b) => *b,
                Value::String(s) => s == "true",
                _ => false,
            },
            display_name: text(body, "name")
                .or_else(|| text(body, "given_name"))
                .or_else(|| text(body, "preferred_username")),
            avatar_url: text(body, "picture"),
        },
        Style::X => {
            let data = &body["data"];
            Profile {
                id: data["id"]
                    .as_str()
                    .ok_or_else(|| format!("{label} returned a profile with no id"))?
                    .to_string(),
                email: None,
                email_verified: false,
                display_name: text(data, "name").or_else(|| text(data, "username")),
                avatar_url: text(data, "profile_image_url"),
            }
        }
    };

    if profile.id.trim().is_empty() {
        return Err(format!("{label} returned an empty account id"));
    }
    Ok(profile)
}

/// A non-empty string field, or `None`. A missing key, a JSON `null` and `""`
/// all mean the same thing here — the provider didn't say — and collapsing them
/// keeps that out of every caller.
fn text(value: &Value, key: &str) -> Option<String> {
    value[key]
        .as_str()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn github_prefers_the_verified_address_over_the_primary_one() {
        let body = json!({ "id": 42, "login": "octo", "name": "Octo Cat" });
        let emails = json!([
            { "email": "public@example.com", "primary": true, "verified": false },
            { "email": "real@example.com", "primary": false, "verified": true },
        ]);
        let p = profile(Style::GitHub, "GitHub", &body, Some(&emails)).unwrap();
        assert_eq!(p.email.as_deref(), Some("real@example.com"));
        assert!(p.email_verified);
        assert_eq!(p.id, "42");
    }

    /// An address nobody verified is still worth showing, and must never be
    /// reported as verified — that flag is what the matching rule trusts.
    #[test]
    fn an_unverified_github_address_is_kept_but_flagged() {
        let body = json!({ "id": 42, "login": "octo" });
        let emails = json!([{ "email": "maybe@example.com", "primary": true, "verified": false }]);
        let p = profile(Style::GitHub, "GitHub", &body, Some(&emails)).unwrap();
        assert_eq!(p.email.as_deref(), Some("maybe@example.com"));
        assert!(!p.email_verified);
    }

    #[test]
    fn oidc_reads_the_standard_claims_and_linkedins_stringly_boolean() {
        let body = json!({
            "sub": "abc", "email": "a@b.com", "email_verified": "true",
            "name": "Ann", "picture": "https://example.com/a.png"
        });
        let p = profile(Style::Oidc, "LinkedIn", &body, None).unwrap();
        assert_eq!(p.id, "abc");
        assert!(p.email_verified);
        assert_eq!(p.display_name.as_deref(), Some("Ann"));
    }

    #[test]
    fn x_has_no_email_and_wraps_everything_in_data() {
        let body = json!({ "data": { "id": "9", "name": "Ex", "username": "ex" } });
        let p = profile(Style::X, "X", &body, None).unwrap();
        assert_eq!(p.id, "9");
        assert!(p.email.is_none());
        assert!(!p.email_verified);
    }

    #[test]
    fn a_profile_without_an_id_is_an_error_not_an_empty_account() {
        assert!(profile(Style::Oidc, "Google", &json!({ "email": "a@b.com" }), None).is_err());
        assert!(profile(Style::GitHub, "GitHub", &json!({ "login": "octo" }), None).is_err());
    }
}
