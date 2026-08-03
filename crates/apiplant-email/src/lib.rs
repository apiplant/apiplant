//! # apiplant-email
//!
//! One way to send a message, whichever service actually sends it.
//!
//! An app names a provider in `main.toml`:
//!
//! ```toml
//! [email]
//! provider = "sendgrid"
//! from     = "no-reply@example.com"
//! api_key  = "${SENDGRID_API_KEY}"
//! ```
//!
//! …and a function calls `ctx.send_email(...)`. Everything between those two
//! points is this crate: it builds the provider's own request shape, signs or
//! authenticates it, and normalises the reply to a [`Sent`] receipt. Changing
//! `provider` to `ses` changes the wire format, the authentication scheme and
//! the endpoint — and changes nothing a function can see.
//!
//! ## Supported providers
//!
//! | `provider` | Transport | Credentials |
//! |------------|-----------|-------------|
//! | `smtp` | SMTP (STARTTLS/implicit TLS) | `[email.smtp]` host/username/password |
//! | `ses` | Amazon SES v2 HTTPS API, SigV4 | `api_key` = access key id, `api_secret` = secret, `region` |
//! | `sendgrid` | `api.sendgrid.com/v3/mail/send` | `api_key` |
//! | `brevo` (`sendinblue`) | `api.brevo.com/v3/smtp/email` | `api_key` |
//! | `mailjet` | `api.mailjet.com/v3.1/send` | `api_key` + `api_secret` |
//! | `mailgun` | `api.mailgun.net/v3/<domain>/messages` | `api_key` + `domain` |
//! | `postmark` | `api.postmarkapp.com/email` | `api_key` (server token) |
//! | `resend` | `api.resend.com/emails` | `api_key` |
//!
//! Anything not on that list still works over `smtp`, which every one of them
//! also speaks.

mod providers;
mod ses;
mod smtp;

use std::time::Duration;

use apiplant_core::EmailConfig;
use serde::{Deserialize, Deserializer, Serialize};

/// What went wrong while sending.
#[derive(Debug, thiserror::Error)]
pub enum EmailError {
    /// The app's `[email]` section can't produce a working client — an unknown
    /// provider, a missing key, no `from` address. Raised at startup where
    /// possible, so a deployment fails to boot rather than failing at the first
    /// password reset.
    #[error("email configuration: {0}")]
    Config(String),

    /// The message itself is unusable: no recipient, no body, no sender.
    #[error("invalid message: {0}")]
    Message(String),

    /// The provider could not be reached, or timed out.
    #[error("email transport: {0}")]
    Transport(String),

    /// The provider answered, and said no.
    #[error("{provider} rejected the message ({status}): {body}")]
    Provider {
        provider: String,
        status: u16,
        body: String,
    },
}

/// One mailbox: an address, optionally with a display name.
///
/// Accepts every spelling a caller might reasonably reach for —
/// `"ann@example.com"`, `"Ann <ann@example.com>"` or
/// `{ "email": "ann@example.com", "name": "Ann" }` — because the alternative is
/// a function author discovering the one true form from a 400.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Address {
    pub email: String,
    pub name: String,
}

impl Address {
    pub fn new(email: impl Into<String>) -> Self {
        Address {
            email: email.into(),
            name: String::new(),
        }
    }

    pub fn named(email: impl Into<String>, name: impl Into<String>) -> Self {
        Address {
            email: email.into(),
            name: name.into(),
        }
    }

    /// Parse `Ann <ann@example.com>` or a bare address.
    pub fn parse(value: &str) -> Address {
        let value = value.trim();
        if let (Some(open), Some(close)) = (value.rfind('<'), value.rfind('>')) {
            if open < close {
                let email = value[open + 1..close].trim().to_string();
                let name = value[..open].trim().trim_matches('"').trim().to_string();
                return Address { email, name };
            }
        }
        Address::new(value)
    }

    /// The RFC 5322 form: `Ann <ann@example.com>`, or just the address when
    /// there is no name.
    pub fn to_header(&self) -> String {
        if self.name.is_empty() {
            self.email.clone()
        } else {
            format!("{} <{}>", self.name, self.email)
        }
    }
}

impl<'de> Deserialize<'de> for Address {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Address, D::Error> {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Raw {
            Text(String),
            Object {
                #[serde(alias = "address")]
                email: String,
                #[serde(default)]
                name: String,
            },
        }
        Ok(match Raw::deserialize(deserializer)? {
            Raw::Text(text) => Address::parse(&text),
            Raw::Object { email, name } => Address { email, name },
        })
    }
}

/// A message to send.
///
/// Deserialised straight from the JSON a function hands the host, so the field
/// names here are the ones a function author writes.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(default)]
pub struct Message {
    /// Recipients. A bare string is accepted as a list of one.
    #[serde(deserialize_with = "one_or_many")]
    pub to: Vec<Address>,
    #[serde(deserialize_with = "one_or_many")]
    pub cc: Vec<Address>,
    #[serde(deserialize_with = "one_or_many")]
    pub bcc: Vec<Address>,
    pub subject: String,
    /// Plain-text body. Send at least one of `text` and `html`.
    pub text: String,
    /// HTML body.
    pub html: String,
    /// Overrides `[email] from` for this message.
    pub from: Option<Address>,
    /// Overrides `[email] reply_to` for this message.
    pub reply_to: Option<Address>,
}

impl Message {
    /// A message to one recipient. Chain [`subject`](Self::subject) and
    /// [`text`](Self::text) / [`html`](Self::html) onto it.
    pub fn to(recipient: impl Into<String>) -> Message {
        Message {
            to: vec![Address::parse(&recipient.into())],
            ..Message::default()
        }
    }

    pub fn subject(mut self, subject: impl Into<String>) -> Message {
        self.subject = subject.into();
        self
    }

    pub fn text(mut self, body: impl Into<String>) -> Message {
        self.text = body.into();
        self
    }

    pub fn html(mut self, body: impl Into<String>) -> Message {
        self.html = body.into();
        self
    }

    /// Fill in what the message didn't say from the app's configuration, then
    /// check that what's left can actually be sent.
    fn resolve(&self, config: &EmailConfig) -> Result<Resolved, EmailError> {
        let from = match &self.from {
            Some(from) if !from.email.is_empty() => from.clone(),
            _ => Address::named(config.from.clone(), config.from_name.clone()),
        };
        if from.email.is_empty() {
            return Err(EmailError::Config(
                "no sender: set `from` in [email], or per message".to_string(),
            ));
        }
        if self.to.iter().all(|a| a.email.is_empty()) {
            return Err(EmailError::Message("no recipient".to_string()));
        }
        if self.subject.is_empty() && self.text.is_empty() && self.html.is_empty() {
            return Err(EmailError::Message(
                "nothing to send: give a subject, text or html".to_string(),
            ));
        }

        let reply_to = match &self.reply_to {
            Some(reply_to) if !reply_to.email.is_empty() => Some(reply_to.clone()),
            _ if !config.reply_to.is_empty() => Some(Address::parse(&config.reply_to)),
            _ => None,
        };

        let strip = |list: &[Address]| -> Vec<Address> {
            list.iter()
                .filter(|a| !a.email.is_empty())
                .cloned()
                .collect()
        };

        Ok(Resolved {
            from,
            reply_to,
            to: strip(&self.to),
            cc: strip(&self.cc),
            bcc: strip(&self.bcc),
            subject: self.subject.clone(),
            text: self.text.clone(),
            html: self.html.clone(),
        })
    }
}

/// A [`Message`] with the app's defaults filled in and its invariants checked.
/// Providers only ever see one of these, so none of them has to re-derive the
/// sender or re-check for a missing recipient.
#[derive(Debug, Clone)]
pub(crate) struct Resolved {
    pub from: Address,
    pub reply_to: Option<Address>,
    pub to: Vec<Address>,
    pub cc: Vec<Address>,
    pub bcc: Vec<Address>,
    pub subject: String,
    pub text: String,
    pub html: String,
}

/// Proof that a provider accepted a message.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Sent {
    /// The provider that took it.
    pub provider: String,
    /// The provider's own identifier for the message, when it returns one —
    /// what you quote at their support desk. Empty when it returns nothing.
    pub id: String,
    /// How many recipients it went to (`to` + `cc` + `bcc`).
    pub recipients: usize,
}

/// Which service sends the mail.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Provider {
    Smtp,
    Ses,
    SendGrid,
    Brevo,
    Mailjet,
    Mailgun,
    Postmark,
    Resend,
}

impl Provider {
    /// Parse the `[email] provider` string. `sendinblue` and `mailinblue` are
    /// accepted for Brevo, which is what it used to be called and what plenty
    /// of existing configuration still says.
    pub fn parse(value: &str) -> Option<Provider> {
        match value.trim().to_ascii_lowercase().as_str() {
            "smtp" => Some(Provider::Smtp),
            "ses" | "aws" | "aws-ses" | "amazon-ses" => Some(Provider::Ses),
            "sendgrid" => Some(Provider::SendGrid),
            "brevo" | "sendinblue" | "mailinblue" => Some(Provider::Brevo),
            "mailjet" => Some(Provider::Mailjet),
            "mailgun" => Some(Provider::Mailgun),
            "postmark" => Some(Provider::Postmark),
            "resend" => Some(Provider::Resend),
            _ => None,
        }
    }

    /// The canonical name, used in logs and in a [`Sent`] receipt.
    pub fn as_str(&self) -> &'static str {
        match self {
            Provider::Smtp => "smtp",
            Provider::Ses => "ses",
            Provider::SendGrid => "sendgrid",
            Provider::Brevo => "brevo",
            Provider::Mailjet => "mailjet",
            Provider::Mailgun => "mailgun",
            Provider::Postmark => "postmark",
            Provider::Resend => "resend",
        }
    }

    /// Every accepted spelling, for error messages.
    pub fn names() -> &'static str {
        "none, smtp, ses, sendgrid, brevo, mailjet, mailgun, postmark, resend"
    }
}

/// A configured, ready-to-use sender.
///
/// Built once at boot and shared by every worker: the HTTP client pools its
/// connections and the SMTP transport pools its sessions, so a per-request
/// `Mailer` would be strictly worse and is not offered.
#[derive(Clone)]
pub struct Mailer {
    provider: Provider,
    config: EmailConfig,
    transport: Transport,
}

#[derive(Clone)]
enum Transport {
    Http(reqwest::Client),
    Smtp(smtp::SmtpTransport),
}

impl std::fmt::Debug for Mailer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Deliberately does not print `config`: it holds the API key.
        f.debug_struct("Mailer")
            .field("provider", &self.provider.as_str())
            .field("from", &self.config.from)
            .finish()
    }
}

impl Mailer {
    /// Build the sender an app's `[email]` section describes.
    ///
    /// `Ok(None)` means the app doesn't send mail (`provider = "none"`, the
    /// default) — not an error, just nothing to build. `Err` means it *asked*
    /// for a provider and the request can't be honoured, which is worth failing
    /// the boot over: the alternative is discovering it at the first send.
    pub fn from_config(config: &EmailConfig) -> Result<Option<Mailer>, EmailError> {
        if !config.enabled() {
            return Ok(None);
        }
        let provider = Provider::parse(&config.provider).ok_or_else(|| {
            EmailError::Config(format!(
                "unknown provider `{}`; expected one of: {}",
                config.provider,
                Provider::names()
            ))
        })?;

        if config.from.is_empty() {
            return Err(EmailError::Config(
                "set `from` in [email] — a provider needs a sender address".to_string(),
            ));
        }

        let timeout = Duration::from_secs(config.timeout_secs.max(1));
        let transport = match provider {
            Provider::Smtp => Transport::Smtp(smtp::build(&config.smtp, timeout)?),
            _ => {
                Self::check_credentials(provider, config)?;
                let client = reqwest::Client::builder()
                    .timeout(timeout)
                    .user_agent(concat!("apiplant/", env!("CARGO_PKG_VERSION")))
                    .build()
                    .map_err(|e| EmailError::Config(e.to_string()))?;
                Transport::Http(client)
            }
        };

        Ok(Some(Mailer {
            provider,
            config: config.clone(),
            transport,
        }))
    }

    /// The credentials each HTTP provider cannot work without. Checked up
    /// front so a missing key is a boot error naming the key, rather than a
    /// `401` from a third party at 3am.
    fn check_credentials(provider: Provider, config: &EmailConfig) -> Result<(), EmailError> {
        let missing = |field: &str| {
            Err(EmailError::Config(format!(
                "[email] {field} is required for provider `{}`",
                provider.as_str()
            )))
        };
        if config.api_key.is_empty() {
            return missing("api_key");
        }
        match provider {
            Provider::Ses => {
                if config.api_secret.is_empty() {
                    return missing("api_secret");
                }
                if config.region.is_empty() {
                    return missing("region");
                }
            }
            Provider::Mailjet if config.api_secret.is_empty() => return missing("api_secret"),
            Provider::Mailgun if config.domain.is_empty() => return missing("domain"),
            _ => {}
        }
        Ok(())
    }

    /// Which provider this mailer sends through.
    pub fn provider(&self) -> Provider {
        self.provider
    }

    /// Send one message.
    pub async fn send(&self, message: &Message) -> Result<Sent, EmailError> {
        let resolved = message.resolve(&self.config)?;
        let recipients = resolved.to.len() + resolved.cc.len() + resolved.bcc.len();

        let id = match (&self.transport, self.provider) {
            (Transport::Smtp(transport), _) => smtp::send(transport, &resolved).await?,
            (Transport::Http(client), Provider::Ses) => {
                ses::send(client, &self.config, &resolved).await?
            }
            (Transport::Http(client), provider) => {
                providers::send(client, provider, &self.config, &resolved).await?
            }
        };

        tracing::info!(
            provider = self.provider.as_str(),
            recipients,
            id = %id,
            "sent email"
        );
        Ok(Sent {
            provider: self.provider.as_str().to_string(),
            id,
            recipients,
        })
    }
}

/// Accept `"a@b"`, `["a@b", …]` or `null` wherever a list of addresses is
/// expected. Sending to one person is the common case and shouldn't need
/// brackets.
fn one_or_many<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Vec<Address>, D::Error> {
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum OneOrMany {
        Many(Vec<Address>),
        One(Address),
        None,
    }
    Ok(match OneOrMany::deserialize(deserializer)? {
        OneOrMany::Many(list) => list,
        OneOrMany::One(one) => vec![one],
        OneOrMany::None => Vec::new(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config(provider: &str) -> EmailConfig {
        EmailConfig {
            provider: provider.to_string(),
            from: "no-reply@example.com".to_string(),
            from_name: "Example".to_string(),
            api_key: "key".to_string(),
            api_secret: "secret".to_string(),
            region: "eu-west-1".to_string(),
            domain: "mg.example.com".to_string(),
            ..EmailConfig::default()
        }
    }

    #[test]
    fn provider_names_include_the_ones_people_actually_type() {
        assert_eq!(Provider::parse("SendGrid"), Some(Provider::SendGrid));
        assert_eq!(Provider::parse(" aws "), Some(Provider::Ses));
        // Brevo was Sendinblue; configuration written then still has to load.
        assert_eq!(Provider::parse("sendinblue"), Some(Provider::Brevo));
        assert_eq!(Provider::parse("mailinblue"), Some(Provider::Brevo));
        assert_eq!(Provider::parse("postal"), None);
    }

    #[test]
    fn addresses_parse_from_every_spelling() {
        assert_eq!(
            Address::parse("ann@example.com"),
            Address::new("ann@example.com")
        );
        assert_eq!(
            Address::parse("Ann Lee <ann@example.com>"),
            Address::named("ann@example.com", "Ann Lee")
        );
        assert_eq!(
            Address::parse("\"Lee, Ann\" <ann@example.com>"),
            Address::named("ann@example.com", "Lee, Ann")
        );
        assert_eq!(
            Address::named("ann@example.com", "Ann").to_header(),
            "Ann <ann@example.com>"
        );
        assert_eq!(
            Address::new("ann@example.com").to_header(),
            "ann@example.com"
        );
    }

    #[test]
    fn a_message_deserialises_from_the_json_a_function_writes() {
        let message: Message = serde_json::from_str(
            r#"{
                "to": "ann@example.com",
                "cc": [{"email": "bo@example.com", "name": "Bo"}],
                "subject": "Hi",
                "text": "Hello"
            }"#,
        )
        .unwrap();

        assert_eq!(message.to, vec![Address::new("ann@example.com")]);
        assert_eq!(message.cc, vec![Address::named("bo@example.com", "Bo")]);
        assert!(message.bcc.is_empty());
        assert_eq!(message.subject, "Hi");
        assert!(message.html.is_empty());
    }

    #[test]
    fn resolve_fills_the_sender_in_from_config_and_a_message_may_override_it() {
        let config = config("sendgrid");

        let inherited = Message::to("ann@example.com")
            .subject("Hi")
            .text("Hello")
            .resolve(&config)
            .unwrap();
        assert_eq!(
            inherited.from,
            Address::named("no-reply@example.com", "Example")
        );

        let mut overridden = Message::to("ann@example.com").subject("Hi");
        overridden.from = Some(Address::new("sales@example.com"));
        assert_eq!(
            overridden.resolve(&config).unwrap().from.email,
            "sales@example.com"
        );
    }

    #[test]
    fn resolve_rejects_messages_that_cannot_be_sent() {
        let config = config("sendgrid");

        let no_recipient = Message::default().subject("Hi").resolve(&config);
        assert!(matches!(no_recipient, Err(EmailError::Message(_))));

        let empty = Message::to("ann@example.com").resolve(&config);
        assert!(matches!(empty, Err(EmailError::Message(_))));

        let no_sender = Message::to("ann@example.com")
            .subject("Hi")
            .resolve(&EmailConfig::default());
        assert!(matches!(no_sender, Err(EmailError::Config(_))));
    }

    #[test]
    fn a_disabled_email_section_builds_no_mailer() {
        assert!(Mailer::from_config(&EmailConfig::default())
            .unwrap()
            .is_none());
    }

    /// A misconfigured provider must fail at boot: at send time it is somebody
    /// else's password reset that disappears.
    #[test]
    fn missing_credentials_are_a_configuration_error() {
        let unknown = Mailer::from_config(&config("mailchimp"));
        assert!(matches!(unknown, Err(EmailError::Config(_))));

        let mut no_key = config("sendgrid");
        no_key.api_key.clear();
        let err = Mailer::from_config(&no_key).unwrap_err().to_string();
        assert!(err.contains("api_key"), "{err}");

        let mut no_secret = config("mailjet");
        no_secret.api_secret.clear();
        assert!(Mailer::from_config(&no_secret)
            .unwrap_err()
            .to_string()
            .contains("api_secret"));

        let mut no_domain = config("mailgun");
        no_domain.domain.clear();
        assert!(Mailer::from_config(&no_domain)
            .unwrap_err()
            .to_string()
            .contains("domain"));

        let mut no_from = config("resend");
        no_from.from.clear();
        assert!(Mailer::from_config(&no_from)
            .unwrap_err()
            .to_string()
            .contains("from"));
    }

    /// The API key must not reach a log line by way of a debug print.
    #[test]
    fn debug_does_not_leak_credentials() {
        let mailer = Mailer::from_config(&config("sendgrid")).unwrap().unwrap();
        let printed = format!("{mailer:?}");
        assert!(!printed.contains("key"), "{printed}");
        assert!(printed.contains("sendgrid"), "{printed}");
    }
}
