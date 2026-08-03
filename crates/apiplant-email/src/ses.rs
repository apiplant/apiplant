//! Amazon SES v2, over its HTTPS API.
//!
//! SES is the one provider here that can't be authenticated with a header
//! containing a key: every request is signed with [AWS Signature Version 4], a
//! keyed hash over the canonical form of the request. The whole of that scheme
//! is [`sign`] below — about forty lines, which is why this crate signs
//! requests itself rather than depending on the AWS SDK to do it.
//!
//! (SES also speaks SMTP, and `provider = "smtp"` with SES's SMTP credentials
//! is a perfectly good alternative. This path exists because the API needs no
//! separate SMTP credential and no port 587 egress.)
//!
//! [AWS Signature Version 4]: https://docs.aws.amazon.com/IAM/latest/UserGuide/reference_sigv4-signing.html

use apiplant_core::EmailConfig;
use chrono::Utc;
use hmac::{Hmac, Mac};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use crate::{EmailError, Resolved};

type HmacSha256 = Hmac<Sha256>;

const SERVICE: &str = "ses";
const PATH: &str = "/v2/email/outbound-emails";

/// Send one message through the SES v2 API.
pub(crate) async fn send(
    client: &reqwest::Client,
    config: &EmailConfig,
    message: &Resolved,
) -> Result<String, EmailError> {
    let host = format!("email.{}.amazonaws.com", config.region);
    let url = format!("https://{host}{PATH}");
    let body = serde_json::to_string(&body(message)).unwrap_or_default();

    let now = Utc::now();
    let amz_date = now.format("%Y%m%dT%H%M%SZ").to_string();
    let date_stamp = now.format("%Y%m%d").to_string();
    let authorization = sign(
        &config.api_key,
        &config.api_secret,
        &config.region,
        &host,
        &amz_date,
        &date_stamp,
        &body,
    );

    let response = client
        .post(&url)
        .header("host", &host)
        .header("content-type", "application/json")
        .header("x-amz-date", &amz_date)
        .header("authorization", authorization)
        .body(body)
        .send()
        .await
        .map_err(|e| EmailError::Transport(format!("ses ({url}): {e}")))?;

    let status = response.status();
    let text = response.text().await.unwrap_or_default();
    if !status.is_success() {
        return Err(EmailError::Provider {
            provider: "ses".to_string(),
            status: status.as_u16(),
            body: text.trim().chars().take(500).collect(),
        });
    }

    Ok(serde_json::from_str::<Value>(&text)
        .ok()
        .and_then(|v| {
            v.get("MessageId")
                .and_then(Value::as_str)
                .map(str::to_string)
        })
        .unwrap_or_default())
}

/// The `SendEmail` request body: SES nests the parts more deeply than the other
/// providers, and takes the sender as a formatted header string.
fn body(m: &Resolved) -> Value {
    let addresses = |list: &[crate::Address]| -> Vec<String> {
        list.iter().map(crate::Address::to_header).collect()
    };

    let mut content = serde_json::Map::new();
    if !m.text.is_empty() {
        content.insert("Text".to_string(), json!({ "Data": m.text }));
    }
    if !m.html.is_empty() {
        content.insert("Html".to_string(), json!({ "Data": m.html }));
    }

    let mut destination = serde_json::Map::new();
    for (key, list) in [
        ("ToAddresses", &m.to),
        ("CcAddresses", &m.cc),
        ("BccAddresses", &m.bcc),
    ] {
        if !list.is_empty() {
            destination.insert(key.to_string(), json!(addresses(list)));
        }
    }

    let mut body = json!({
        "FromEmailAddress": m.from.to_header(),
        "Destination": Value::Object(destination),
        "Content": {
            "Simple": {
                "Subject": { "Data": m.subject },
                "Body": Value::Object(content),
            }
        }
    });
    if let Some(reply_to) = &m.reply_to {
        body["ReplyToAddresses"] = json!([reply_to.to_header()]);
    }
    body
}

/// Build the `Authorization` header for a signed SES request.
///
/// Four steps, in AWS's order: hash the canonical request, wrap that in the
/// "string to sign", derive a signing key that is scoped to the date, region
/// and service, then sign. Scoping the key is the point of the derivation
/// chain — a leaked signing key is useless tomorrow, in another region, or for
/// another service.
#[allow(clippy::too_many_arguments)]
fn sign(
    access_key: &str,
    secret_key: &str,
    region: &str,
    host: &str,
    amz_date: &str,
    date_stamp: &str,
    body: &str,
) -> String {
    // The headers must be signed in the order they appear here, lowercase, and
    // the same set must be named in `SignedHeaders`.
    let signed_headers = "content-type;host;x-amz-date";
    let canonical_headers =
        format!("content-type:application/json\nhost:{host}\nx-amz-date:{amz_date}\n");
    let canonical_request = format!(
        "POST\n{PATH}\n\n{canonical_headers}\n{signed_headers}\n{}",
        hex_sha256(body.as_bytes())
    );

    let scope = format!("{date_stamp}/{region}/{SERVICE}/aws4_request");
    let string_to_sign = format!(
        "AWS4-HMAC-SHA256\n{amz_date}\n{scope}\n{}",
        hex_sha256(canonical_request.as_bytes())
    );

    let signing_key = signing_key(secret_key, date_stamp, region, SERVICE);
    let signature = hex::encode(hmac(&signing_key, string_to_sign.as_bytes()));

    format!(
        "AWS4-HMAC-SHA256 Credential={access_key}/{scope}, \
         SignedHeaders={signed_headers}, Signature={signature}"
    )
}

/// The date-, region- and service-scoped key a signature is computed with.
fn signing_key(secret_key: &str, date_stamp: &str, region: &str, service: &str) -> Vec<u8> {
    let date_key = hmac(
        format!("AWS4{secret_key}").as_bytes(),
        date_stamp.as_bytes(),
    );
    let region_key = hmac(&date_key, region.as_bytes());
    let service_key = hmac(&region_key, service.as_bytes());
    hmac(&service_key, b"aws4_request")
}

fn hex_sha256(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

fn hmac(key: &[u8], data: &[u8]) -> Vec<u8> {
    // `new_from_slice` only fails for key lengths HMAC-SHA256 doesn't accept,
    // and it accepts every length.
    let mut mac = HmacSha256::new_from_slice(key).expect("HMAC accepts keys of any length");
    mac.update(data);
    mac.finalize().into_bytes().to_vec()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Address;

    fn message() -> Resolved {
        Resolved {
            from: Address::named("no-reply@example.com", "Example"),
            reply_to: Some(Address::new("help@example.com")),
            to: vec![Address::new("ann@example.com")],
            cc: Vec::new(),
            bcc: vec![Address::new("audit@example.com")],
            subject: "Hi".into(),
            text: "Hello".into(),
            html: String::new(),
        }
    }

    #[test]
    fn the_body_uses_ses_v2_shapes() {
        let body = body(&message());

        assert_eq!(body["FromEmailAddress"], "Example <no-reply@example.com>");
        assert_eq!(body["Destination"]["ToAddresses"][0], "ann@example.com");
        assert_eq!(body["Destination"]["BccAddresses"][0], "audit@example.com");
        // An empty list is omitted, not sent as `[]`.
        assert!(body["Destination"].get("CcAddresses").is_none());
        assert_eq!(body["Content"]["Simple"]["Subject"]["Data"], "Hi");
        assert_eq!(body["Content"]["Simple"]["Body"]["Text"]["Data"], "Hello");
        assert!(body["Content"]["Simple"]["Body"].get("Html").is_none());
        assert_eq!(body["ReplyToAddresses"][0], "help@example.com");
    }

    /// The derivation chain, checked against the worked example in AWS's own
    /// signing documentation. Everything else about SigV4 is arrangement; this
    /// is the part that is either bit-for-bit right or silently rejected.
    #[test]
    fn the_signing_key_matches_aws_published_test_vector() {
        let key = signing_key(
            "wJalrXUtnFEMI/K7MDENG+bPxRfiCYEXAMPLEKEY",
            "20150830",
            "us-east-1",
            "iam",
        );
        assert_eq!(
            hex::encode(key),
            "c4afb1cc5771d871763a393e44b703571b55cc28424d1a5e86da6ed3c154a4b9"
        );
    }

    #[test]
    fn the_authorization_header_names_the_credential_scope_and_signed_headers() {
        let header = sign(
            "AKIDEXAMPLE",
            "wJalrXUtnFEMI/K7MDENG+bPxRfiCYEXAMPLEKEY",
            "eu-west-1",
            "email.eu-west-1.amazonaws.com",
            "20240115T120000Z",
            "20240115",
            r#"{"FromEmailAddress":"a@example.com"}"#,
        );

        assert!(header.starts_with(
            "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20240115/eu-west-1/ses/aws4_request"
        ));
        assert!(header.contains("SignedHeaders=content-type;host;x-amz-date"));
        // A signature is 64 hex characters, and the secret must not be in it.
        let signature = header.rsplit("Signature=").next().unwrap();
        assert_eq!(signature.len(), 64);
        assert!(signature.chars().all(|c| c.is_ascii_hexdigit()));
    }

    /// The derivation chain must actually be scoped: same request, different
    /// day or region, different signature.
    #[test]
    fn signatures_are_scoped_to_the_date_region_and_body() {
        let base = |region: &str, date: &str, body: &str| {
            sign(
                "AKID",
                "secret",
                region,
                "host",
                "20240115T120000Z",
                date,
                body,
            )
        };
        let one = base("eu-west-1", "20240115", "{}");
        assert_ne!(one, base("us-east-1", "20240115", "{}"));
        assert_ne!(one, base("eu-west-1", "20240116", "{}"));
        assert_ne!(one, base("eu-west-1", "20240115", r#"{"a":1}"#));
    }
}
