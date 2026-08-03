//! The HTTP providers: one request shape and one authentication scheme each.
//!
//! Every function here does the same three things — build a body, name the
//! endpoint and the credential, pull an id out of the reply — so the plumbing
//! (posting, timing out, turning a non-2xx into an [`EmailError::Provider`])
//! lives once in [`send`] and nowhere else.

use apiplant_core::EmailConfig;
use serde_json::{json, Value};

use crate::{Address, EmailError, Provider, Resolved};

/// One provider's HTTP request, before it is sent.
struct Request {
    url: String,
    builder: reqwest::RequestBuilder,
}

/// Post a resolved message to `provider` and return its message id.
pub(crate) async fn send(
    client: &reqwest::Client,
    provider: Provider,
    config: &EmailConfig,
    message: &Resolved,
) -> Result<String, EmailError> {
    let request = match provider {
        Provider::SendGrid => sendgrid(client, config, message),
        Provider::Brevo => brevo(client, config, message),
        Provider::Mailjet => mailjet(client, config, message),
        Provider::Mailgun => mailgun(client, config, message),
        Provider::Postmark => postmark(client, config, message),
        Provider::Resend => resend(client, config, message),
        // `Smtp` and `Ses` are handled by their own modules; `send` is only
        // reached for the plain-HTTP providers above.
        Provider::Smtp | Provider::Ses => {
            return Err(EmailError::Config(format!(
                "provider `{}` is not an HTTP provider",
                provider.as_str()
            )))
        }
    };

    let response = request.builder.send().await.map_err(|e| {
        EmailError::Transport(format!("{} ({}): {e}", provider.as_str(), request.url))
    })?;

    let status = response.status();
    // Some providers put the id in a header and return no body at all, so both
    // are read before the status is judged.
    let header_id = response
        .headers()
        .get("x-message-id")
        .and_then(|v| v.to_str().ok())
        .map(str::to_string);
    let body = response.text().await.unwrap_or_default();

    if !status.is_success() {
        return Err(EmailError::Provider {
            provider: provider.as_str().to_string(),
            status: status.as_u16(),
            body: truncate(&body),
        });
    }

    Ok(message_id(provider, &body)
        .or(header_id)
        .unwrap_or_default())
}

/// Dig the provider's identifier out of a successful reply. Each one names it
/// differently, and one (SendGrid) doesn't return one at all.
fn message_id(provider: Provider, body: &str) -> Option<String> {
    let value: Value = serde_json::from_str(body).ok()?;
    let id = match provider {
        Provider::Brevo => value.get("messageId").cloned(),
        Provider::Mailjet => value
            .get("Messages")?
            .get(0)?
            .get("To")?
            .get(0)?
            .get("MessageID")
            .cloned(),
        Provider::Mailgun => value.get("id").cloned(),
        Provider::Postmark => value.get("MessageID").cloned(),
        Provider::Resend => value.get("id").cloned(),
        _ => None,
    }?;
    match id {
        Value::String(s) => Some(s),
        other => Some(other.to_string()),
    }
}

/// A provider's error body can be a page of HTML; the log wants a line.
fn truncate(body: &str) -> String {
    const LIMIT: usize = 500;
    let body = body.trim();
    match body.char_indices().nth(LIMIT) {
        Some((end, _)) => format!("{}…", &body[..end]),
        None => body.to_string(),
    }
}

/// `[{ "email": …, "name": … }]`, the shape most of these APIs use.
fn objects(list: &[Address]) -> Vec<Value> {
    list.iter()
        .map(|a| {
            if a.name.is_empty() {
                json!({ "email": a.email })
            } else {
                json!({ "email": a.email, "name": a.name })
            }
        })
        .collect()
}

/// Mailjet capitalises everything.
fn mailjet_objects(list: &[Address]) -> Vec<Value> {
    list.iter()
        .map(|a| {
            if a.name.is_empty() {
                json!({ "Email": a.email })
            } else {
                json!({ "Email": a.email, "Name": a.name })
            }
        })
        .collect()
}

/// Add `key` to `object` only when `list` is non-empty — several of these APIs
/// reject an explicitly empty `cc`.
fn put_addresses(object: &mut serde_json::Map<String, Value>, key: &str, list: Vec<Value>) {
    if !list.is_empty() {
        object.insert(key.to_string(), Value::Array(list));
    }
}

fn sendgrid(client: &reqwest::Client, config: &EmailConfig, m: &Resolved) -> Request {
    let mut personalization = serde_json::Map::new();
    put_addresses(&mut personalization, "to", objects(&m.to));
    put_addresses(&mut personalization, "cc", objects(&m.cc));
    put_addresses(&mut personalization, "bcc", objects(&m.bcc));

    let mut content = Vec::new();
    if !m.text.is_empty() {
        content.push(json!({ "type": "text/plain", "value": m.text }));
    }
    if !m.html.is_empty() {
        content.push(json!({ "type": "text/html", "value": m.html }));
    }

    let mut body = json!({
        "personalizations": [Value::Object(personalization)],
        "from": objects(std::slice::from_ref(&m.from))[0],
        "subject": m.subject,
        "content": content,
    });
    if let Some(reply_to) = &m.reply_to {
        body["reply_to"] = objects(std::slice::from_ref(reply_to))[0].clone();
    }

    let url = "https://api.sendgrid.com/v3/mail/send".to_string();
    Request {
        builder: client.post(&url).bearer_auth(&config.api_key).json(&body),
        url,
    }
}

fn brevo(client: &reqwest::Client, config: &EmailConfig, m: &Resolved) -> Request {
    let mut body = serde_json::Map::new();
    body.insert(
        "sender".to_string(),
        objects(std::slice::from_ref(&m.from))[0].clone(),
    );
    put_addresses(&mut body, "to", objects(&m.to));
    put_addresses(&mut body, "cc", objects(&m.cc));
    put_addresses(&mut body, "bcc", objects(&m.bcc));
    body.insert("subject".to_string(), json!(m.subject));
    if !m.text.is_empty() {
        body.insert("textContent".to_string(), json!(m.text));
    }
    if !m.html.is_empty() {
        body.insert("htmlContent".to_string(), json!(m.html));
    }
    if let Some(reply_to) = &m.reply_to {
        body.insert(
            "replyTo".to_string(),
            objects(std::slice::from_ref(reply_to))[0].clone(),
        );
    }

    let url = "https://api.brevo.com/v3/smtp/email".to_string();
    Request {
        builder: client
            .post(&url)
            .header("api-key", &config.api_key)
            .json(&Value::Object(body)),
        url,
    }
}

fn mailjet(client: &reqwest::Client, config: &EmailConfig, m: &Resolved) -> Request {
    let mut entry = serde_json::Map::new();
    entry.insert(
        "From".to_string(),
        mailjet_objects(std::slice::from_ref(&m.from))[0].clone(),
    );
    put_addresses(&mut entry, "To", mailjet_objects(&m.to));
    put_addresses(&mut entry, "Cc", mailjet_objects(&m.cc));
    put_addresses(&mut entry, "Bcc", mailjet_objects(&m.bcc));
    entry.insert("Subject".to_string(), json!(m.subject));
    if !m.text.is_empty() {
        entry.insert("TextPart".to_string(), json!(m.text));
    }
    if !m.html.is_empty() {
        entry.insert("HTMLPart".to_string(), json!(m.html));
    }
    if let Some(reply_to) = &m.reply_to {
        entry.insert(
            "ReplyTo".to_string(),
            mailjet_objects(std::slice::from_ref(reply_to))[0].clone(),
        );
    }

    let url = "https://api.mailjet.com/v3.1/send".to_string();
    Request {
        builder: client
            .post(&url)
            // Mailjet's credential is a pair, sent as HTTP basic auth.
            .basic_auth(&config.api_key, Some(&config.api_secret))
            .json(&json!({ "Messages": [Value::Object(entry)] })),
        url,
    }
}

fn mailgun(client: &reqwest::Client, config: &EmailConfig, m: &Resolved) -> Request {
    // The only form-encoded provider here, and the only one that takes its
    // recipient lists as comma-separated strings.
    let joined = |list: &[Address]| {
        list.iter()
            .map(Address::to_header)
            .collect::<Vec<_>>()
            .join(",")
    };
    let mut form = vec![
        ("from".to_string(), m.from.to_header()),
        ("to".to_string(), joined(&m.to)),
        ("subject".to_string(), m.subject.clone()),
    ];
    if !m.cc.is_empty() {
        form.push(("cc".to_string(), joined(&m.cc)));
    }
    if !m.bcc.is_empty() {
        form.push(("bcc".to_string(), joined(&m.bcc)));
    }
    if !m.text.is_empty() {
        form.push(("text".to_string(), m.text.clone()));
    }
    if !m.html.is_empty() {
        form.push(("html".to_string(), m.html.clone()));
    }
    if let Some(reply_to) = &m.reply_to {
        form.push(("h:Reply-To".to_string(), reply_to.to_header()));
    }

    let url = format!("https://api.mailgun.net/v3/{}/messages", config.domain);
    Request {
        builder: client
            .post(&url)
            .basic_auth("api", Some(&config.api_key))
            .form(&form),
        url,
    }
}

fn postmark(client: &reqwest::Client, config: &EmailConfig, m: &Resolved) -> Request {
    let joined = |list: &[Address]| {
        list.iter()
            .map(Address::to_header)
            .collect::<Vec<_>>()
            .join(",")
    };
    let mut body = serde_json::Map::new();
    body.insert("From".to_string(), json!(m.from.to_header()));
    body.insert("To".to_string(), json!(joined(&m.to)));
    if !m.cc.is_empty() {
        body.insert("Cc".to_string(), json!(joined(&m.cc)));
    }
    if !m.bcc.is_empty() {
        body.insert("Bcc".to_string(), json!(joined(&m.bcc)));
    }
    body.insert("Subject".to_string(), json!(m.subject));
    if !m.text.is_empty() {
        body.insert("TextBody".to_string(), json!(m.text));
    }
    if !m.html.is_empty() {
        body.insert("HtmlBody".to_string(), json!(m.html));
    }
    if let Some(reply_to) = &m.reply_to {
        body.insert("ReplyTo".to_string(), json!(reply_to.to_header()));
    }

    let url = "https://api.postmarkapp.com/email".to_string();
    Request {
        builder: client
            .post(&url)
            .header("X-Postmark-Server-Token", &config.api_key)
            .header("Accept", "application/json")
            .json(&Value::Object(body)),
        url,
    }
}

fn resend(client: &reqwest::Client, config: &EmailConfig, m: &Resolved) -> Request {
    let headers =
        |list: &[Address]| -> Vec<Value> { list.iter().map(|a| json!(a.to_header())).collect() };
    let mut body = serde_json::Map::new();
    body.insert("from".to_string(), json!(m.from.to_header()));
    put_addresses(&mut body, "to", headers(&m.to));
    put_addresses(&mut body, "cc", headers(&m.cc));
    put_addresses(&mut body, "bcc", headers(&m.bcc));
    body.insert("subject".to_string(), json!(m.subject));
    if !m.text.is_empty() {
        body.insert("text".to_string(), json!(m.text));
    }
    if !m.html.is_empty() {
        body.insert("html".to_string(), json!(m.html));
    }
    if let Some(reply_to) = &m.reply_to {
        body.insert("reply_to".to_string(), json!(reply_to.to_header()));
    }

    let url = "https://api.resend.com/emails".to_string();
    Request {
        builder: client
            .post(&url)
            .bearer_auth(&config.api_key)
            .json(&Value::Object(body)),
        url,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config() -> EmailConfig {
        EmailConfig {
            provider: "sendgrid".into(),
            from: "no-reply@example.com".into(),
            api_key: "key".into(),
            api_secret: "secret".into(),
            domain: "mg.example.com".into(),
            ..EmailConfig::default()
        }
    }

    fn message() -> Resolved {
        Resolved {
            from: Address::named("no-reply@example.com", "Example"),
            reply_to: Some(Address::new("help@example.com")),
            to: vec![Address::named("ann@example.com", "Ann")],
            cc: vec![Address::new("bo@example.com")],
            bcc: Vec::new(),
            subject: "Hi".into(),
            text: "Hello".into(),
            html: "<p>Hello</p>".into(),
        }
    }

    /// Read back the body a builder produced, so the assertions below are about
    /// the bytes that would actually go over the wire.
    fn body_of(request: Request) -> (String, Value) {
        let built = request.builder.build().unwrap();
        let bytes = built.body().and_then(|b| b.as_bytes()).unwrap_or_default();
        let text = String::from_utf8_lossy(bytes).into_owned();
        let json = serde_json::from_str(&text).unwrap_or(Value::String(text));
        (request.url, json)
    }

    #[test]
    fn sendgrid_nests_recipients_in_a_personalization() {
        let client = reqwest::Client::new();
        let (url, body) = body_of(sendgrid(&client, &config(), &message()));

        assert_eq!(url, "https://api.sendgrid.com/v3/mail/send");
        assert_eq!(
            body["personalizations"][0]["to"][0]["email"],
            "ann@example.com"
        );
        assert_eq!(
            body["personalizations"][0]["cc"][0]["email"],
            "bo@example.com"
        );
        assert!(body["personalizations"][0].get("bcc").is_none());
        assert_eq!(body["from"]["email"], "no-reply@example.com");
        assert_eq!(body["content"][0]["type"], "text/plain");
        assert_eq!(body["content"][1]["type"], "text/html");
        assert_eq!(body["reply_to"]["email"], "help@example.com");
    }

    #[test]
    fn brevo_uses_sender_and_content_fields() {
        let client = reqwest::Client::new();
        let (url, body) = body_of(brevo(&client, &config(), &message()));

        assert_eq!(url, "https://api.brevo.com/v3/smtp/email");
        assert_eq!(body["sender"]["email"], "no-reply@example.com");
        assert_eq!(body["to"][0]["name"], "Ann");
        assert_eq!(body["textContent"], "Hello");
        assert_eq!(body["htmlContent"], "<p>Hello</p>");
        assert_eq!(body["replyTo"]["email"], "help@example.com");
    }

    #[test]
    fn mailjet_wraps_the_message_in_its_capitalised_envelope() {
        let client = reqwest::Client::new();
        let (url, body) = body_of(mailjet(&client, &config(), &message()));

        assert_eq!(url, "https://api.mailjet.com/v3.1/send");
        assert_eq!(body["Messages"][0]["From"]["Email"], "no-reply@example.com");
        assert_eq!(body["Messages"][0]["To"][0]["Email"], "ann@example.com");
        assert_eq!(body["Messages"][0]["TextPart"], "Hello");
        assert_eq!(body["Messages"][0]["HTMLPart"], "<p>Hello</p>");
    }

    #[test]
    fn mailgun_posts_a_form_to_the_configured_domain() {
        let client = reqwest::Client::new();
        let (url, body) = body_of(mailgun(&client, &config(), &message()));

        assert_eq!(url, "https://api.mailgun.net/v3/mg.example.com/messages");
        let form = body.as_str().unwrap();
        assert!(form.contains("to=Ann+%3Cann%40example.com%3E"), "{form}");
        assert!(form.contains("subject=Hi"), "{form}");
        assert!(form.contains("h%3AReply-To="), "{form}");
    }

    #[test]
    fn postmark_and_resend_take_flat_header_strings() {
        let client = reqwest::Client::new();

        let (url, body) = body_of(postmark(&client, &config(), &message()));
        assert_eq!(url, "https://api.postmarkapp.com/email");
        assert_eq!(body["From"], "Example <no-reply@example.com>");
        assert_eq!(body["To"], "Ann <ann@example.com>");
        assert_eq!(body["HtmlBody"], "<p>Hello</p>");

        let (url, body) = body_of(resend(&client, &config(), &message()));
        assert_eq!(url, "https://api.resend.com/emails");
        assert_eq!(body["to"][0], "Ann <ann@example.com>");
        assert_eq!(body["reply_to"], "help@example.com");
    }

    /// A body with no text part must not carry an empty one: some providers
    /// treat `""` as a body and send a blank email rather than an HTML one.
    #[test]
    fn empty_parts_are_omitted_rather_than_sent_blank() {
        let client = reqwest::Client::new();
        let mut html_only = message();
        html_only.text.clear();
        html_only.bcc.clear();
        html_only.cc.clear();

        let (_, body) = body_of(brevo(&client, &config(), &html_only));
        assert!(body.get("textContent").is_none());
        assert!(body.get("cc").is_none());

        let (_, body) = body_of(sendgrid(&client, &config(), &html_only));
        assert_eq!(body["content"].as_array().unwrap().len(), 1);
        assert_eq!(body["content"][0]["type"], "text/html");
    }

    #[test]
    fn message_ids_are_read_from_each_providers_reply() {
        assert_eq!(
            message_id(Provider::Brevo, r#"{"messageId":"<abc@brevo>"}"#),
            Some("<abc@brevo>".to_string())
        );
        assert_eq!(
            message_id(
                Provider::Mailjet,
                r#"{"Messages":[{"Status":"success","To":[{"MessageID":123}]}]}"#
            ),
            Some("123".to_string())
        );
        assert_eq!(
            message_id(Provider::Postmark, r#"{"MessageID":"pm-1"}"#),
            Some("pm-1".to_string())
        );
        assert_eq!(
            message_id(Provider::Resend, r#"{"id":"re_1"}"#),
            Some("re_1".to_string())
        );
        // SendGrid returns an empty body; the id comes from a header instead.
        assert_eq!(message_id(Provider::SendGrid, ""), None);
    }

    #[test]
    fn a_long_error_body_is_cut_down_to_a_log_line() {
        let long = "x".repeat(900);
        let short = truncate(&long);
        assert!(short.len() < 600, "{}", short.len());
        assert!(short.ends_with('…'));
        assert_eq!(truncate("  nope  "), "nope");
    }
}
