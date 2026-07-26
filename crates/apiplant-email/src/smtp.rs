//! Plain SMTP, via [`lettre`].
//!
//! The universal fallback: every provider in this crate also accepts SMTP, and
//! so does the relay in the corner of the office that accepts nothing else. It
//! is also the only transport here that builds a MIME message rather than
//! handing parts to somebody else's API — `text` + `html` become a
//! `multipart/alternative`, which is what a mail client expects when both are
//! present.

use std::time::Duration;

use apiplant_core::SmtpConfig;
use lettre::message::{header::ContentType, Mailbox, MultiPart, SinglePart};
use lettre::transport::smtp::authentication::Credentials;
use lettre::{AsyncSmtpTransport, AsyncTransport, Tokio1Executor};

use crate::{Address, EmailError, Resolved};

pub(crate) type SmtpTransport = AsyncSmtpTransport<Tokio1Executor>;

/// Build the transport an app's `[email.smtp]` section describes.
///
/// Connections are pooled inside the transport, so this is called once at boot
/// and the result shared; a fresh TLS handshake per email would dominate the
/// cost of sending one.
pub(crate) fn build(config: &SmtpConfig, timeout: Duration) -> Result<SmtpTransport, EmailError> {
    if config.host.is_empty() {
        return Err(EmailError::Config(
            "set [email.smtp] host for provider `smtp`".to_string(),
        ));
    }

    let encryption = config.encryption.trim().to_ascii_lowercase();
    let mut builder = match encryption.as_str() {
        // Implicit TLS: the connection is encrypted before the SMTP greeting.
        "tls" | "ssl" | "implicit" | "wrapper" => SmtpTransport::relay(&config.host)
            .map_err(|e| EmailError::Config(format!("smtp tls: {e}")))?,
        // Opportunistic upgrade after `EHLO`, and the usual choice on 587.
        "starttls" | "" => SmtpTransport::starttls_relay(&config.host)
            .map_err(|e| EmailError::Config(format!("smtp starttls: {e}")))?,
        // Cleartext. Reasonable for a relay on localhost, and nowhere else —
        // hence the warning rather than a silent downgrade.
        "none" | "plain" | "insecure" => {
            tracing::warn!(
                host = %config.host,
                "[email.smtp] encryption = \"none\": credentials and messages are sent in cleartext"
            );
            SmtpTransport::builder_dangerous(&config.host)
        }
        other => {
            return Err(EmailError::Config(format!(
                "unknown [email.smtp] encryption `{other}`; expected starttls, tls or none"
            )))
        }
    };

    builder = builder.timeout(Some(timeout));
    if config.port != 0 {
        builder = builder.port(config.port);
    }
    // A relay that authenticates by IP has no username; sending empty
    // credentials to one is an error, so absence has to stay absence.
    if !config.username.is_empty() {
        builder = builder.credentials(Credentials::new(
            config.username.clone(),
            config.password.clone(),
        ));
    }

    Ok(builder.build())
}

/// Send one message, returning the `Message-ID` the server assigned.
pub(crate) async fn send(
    transport: &SmtpTransport,
    message: &Resolved,
) -> Result<String, EmailError> {
    let mailbox = |address: &Address| -> Result<Mailbox, EmailError> {
        let text = address.to_header();
        text.parse::<Mailbox>()
            .map_err(|e| EmailError::Message(format!("bad address `{text}`: {e}")))
    };

    let mut builder = lettre::Message::builder()
        .from(mailbox(&message.from)?)
        .subject(message.subject.clone());
    for to in &message.to {
        builder = builder.to(mailbox(to)?);
    }
    for cc in &message.cc {
        builder = builder.cc(mailbox(cc)?);
    }
    for bcc in &message.bcc {
        builder = builder.bcc(mailbox(bcc)?);
    }
    if let Some(reply_to) = &message.reply_to {
        builder = builder.reply_to(mailbox(reply_to)?);
    }

    let body = match (message.text.is_empty(), message.html.is_empty()) {
        // Both parts: `multipart/alternative`, plain text first, so a client
        // that can't render HTML picks the fallback rather than the markup.
        (false, false) => builder.multipart(MultiPart::alternative_plain_html(
            message.text.clone(),
            message.html.clone(),
        )),
        (true, false) => builder.singlepart(
            SinglePart::builder()
                .header(ContentType::TEXT_HTML)
                .body(message.html.clone()),
        ),
        // Text-only, and the empty-body case — which `Message::resolve` only
        // allows through when there is at least a subject.
        _ => builder.singlepart(
            SinglePart::builder()
                .header(ContentType::TEXT_PLAIN)
                .body(message.text.clone()),
        ),
    }
    .map_err(|e| EmailError::Message(e.to_string()))?;

    let response = transport
        .send(body)
        .await
        .map_err(|e| EmailError::Transport(format!("smtp: {e}")))?;

    // The server's reply to `DATA` usually carries a queue id; it's the closest
    // thing SMTP has to the message id an API returns.
    let id = response
        .message()
        .next()
        .unwrap_or_default()
        .trim()
        .to_string();
    Ok(id)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config(encryption: &str) -> SmtpConfig {
        SmtpConfig {
            host: "smtp.example.com".into(),
            port: 0,
            username: "user".into(),
            password: "pass".into(),
            encryption: encryption.into(),
        }
    }

    #[test]
    fn each_encryption_mode_builds_a_transport() {
        let timeout = Duration::from_secs(5);
        for mode in ["starttls", "tls", "none", ""] {
            assert!(
                build(&config(mode), timeout).is_ok(),
                "encryption = {mode:?} should build"
            );
        }
    }

    #[test]
    fn a_missing_host_or_an_unknown_mode_is_a_configuration_error() {
        let timeout = Duration::from_secs(5);

        let mut no_host = config("starttls");
        no_host.host.clear();
        assert!(build(&no_host, timeout)
            .unwrap_err()
            .to_string()
            .contains("host"));

        let err = build(&config("quantum"), timeout).unwrap_err().to_string();
        assert!(err.contains("unknown"), "{err}");
    }
}
