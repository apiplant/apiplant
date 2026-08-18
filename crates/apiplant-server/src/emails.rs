//! The messages the framework itself sends, and the links inside them.
//!
//! Three flows reach a person through their mailbox — [an invitation to an
//! organisation](invitation), [confirming an address](verification), [resetting
//! a password](password_reset) — and all three are the same shape: a sentence
//! saying what is being asked, a URL carrying a single-use token, and a note of
//! when it stops working.
//!
//! They are deliberately plain. An app that wants its own wording and its own
//! letterhead should send its own message from a hook — `after_register` and
//! `before_api_key` already exist for exactly that, and a function has the
//! whole `send_email` API. What lives here is the version that has to work in
//! an app which has configured nothing but a provider and a `from` address, so
//! it is a paragraph of text and a link, in both plain text and the least
//! surprising HTML that renders in a dark mailbox as well as a light one.
//!
//! ## Where the links point
//!
//! At the **admin dashboard**, not at the API: the URL in the message is opened
//! by a person in a browser, and the endpoint that spends the token is a
//! `POST` that wants a password typed into a form first. The dashboard's
//! hash-routed screens (`#/accept-invite`, `#/verify-email`, `#/reset-password`)
//! are that form. An app that serves its own front end sets
//! [`links_base`](Links::from_app) through `[server] public_url` and can point
//! its own page at the same three endpoints.

use std::sync::Arc;

use apiplant_core::App;

use crate::email_templates::EmailTemplates;

/// Where the links in an outgoing message point.
///
/// Resolved once per message from the app's configuration rather than from the
/// request that triggered it: a `Host:` header describes the hop that arrived,
/// and the message is read somewhere else entirely, possibly days later.
#[derive(Debug, Clone)]
pub struct Links {
    /// Origin plus dashboard path, e.g. `https://example.com/admin`.
    base: String,
    /// What the app calls itself, for the subject line and the banner.
    pub app_name: String,
    /// Absolute URL of the mark in the banner, when there is a file behind
    /// `[email] logo`. `None` leaves the banner showing the name alone.
    pub logo_url: Option<String>,
    /// The app's own versions of these messages, when it wrote any. Carried
    /// here rather than passed alongside because every composer already takes
    /// a `Links`, and the two are decided together: an override still gets the
    /// same URL and the same mark as the message it replaces.
    templates: Option<Arc<EmailTemplates>>,
}

impl Links {
    pub fn from_app(app: &App) -> Links {
        let origin = app.config.server.public_origin();
        let logo_url = logo_url(app, &origin);
        // The dashboard is where the forms live. With it switched off there is
        // nowhere of ours to send anyone, so the link falls back to the origin
        // and the app is expected to serve its own page there.
        let base = match app.config.admin.enabled {
            true => format!("{origin}{}", app.config.admin.path.trim_end_matches('/')),
            false => origin,
        };
        Links {
            base,
            app_name: app.display_name(),
            logo_url,
            templates: None,
        }
    }

    /// Point these links at an app's own templates.
    pub fn with_templates(mut self, templates: Arc<EmailTemplates>) -> Links {
        self.templates = Some(templates);
        self
    }

    /// A dashboard link for `screen`, carrying `token`.
    fn to(&self, screen: &str, token: &str) -> String {
        format!("{}/#/{screen}?token={}", self.base, urlencode(token))
    }

    /// The variables every template gets, whichever message it is.
    ///
    /// Facts, not prose: the app's name, its mark, the URL, and how long that
    /// URL lasts. An override writes its own sentences — handing it the
    /// framework's would only invite a template that half-uses them.
    fn vars(&self, url: &str, expires_in: &str) -> liquid::Object {
        liquid::object!({
            "app_name": self.app_name.clone(),
            "logo_url": self.logo_url.clone().unwrap_or_default(),
            "url": url,
            "expires_in": expires_in,
        })
    }

    /// The app's version of `name`, when it wrote one.
    ///
    /// A template that fails to *render* — a filter applied to the wrong sort
    /// of value — falls back to the built-in message rather than sending
    /// nothing: the flows here are how somebody recovers an account, and a
    /// plain message beats no message. Parse errors cannot reach this point;
    /// they stop the app at boot.
    fn rendered(&self, name: &str, vars: &liquid::Object, subject: &str) -> Option<Composed> {
        let templates = self.templates.as_ref()?;
        if !templates.has(name) {
            return None;
        }
        match templates.render(name, vars, subject) {
            Ok(rendered) => Some(Composed {
                subject: rendered.subject,
                text: rendered.text,
                html: rendered.html,
            }),
            Err(error) => {
                tracing::error!(%error, template = name, "falling back to the built-in message");
                None
            }
        }
    }
}

/// The absolute URL of `[email] logo`, or `None` when there is nothing to show.
///
/// The setting is a path *inside the public directory*, so `logo.png` and
/// `/logo.png` name the same file and both become `<origin>/logo.png`. A mail
/// client fetches the image from the internet, months after the message was
/// composed, so a relative path would be meaningless and a missing file would
/// be a broken image in somebody's inbox — hence the check on disk, and the
/// silence when it fails. It runs once per message, alongside a network send.
fn logo_url(app: &App, origin: &str) -> Option<String> {
    let path = app.config.email.logo.trim().trim_start_matches('/');
    if path.is_empty() || !app.config.public.enabled {
        return None;
    }
    // A path that climbs out of the public directory is a mistake, not a
    // feature: the file wouldn't be served, so the URL couldn't work.
    if std::path::Path::new(path)
        .components()
        .any(|part| !matches!(part, std::path::Component::Normal(_)))
    {
        return None;
    }
    let file = app.root.join(&app.config.public.dir).join(path);
    file.is_file()
        .then(|| format!("{origin}/{}", path.replace(' ', "%20")))
}

/// Percent-encode a token for a query string.
///
/// Our own tokens are `prefix_<hex>` and need no encoding at all; this exists
/// so that a link is still correct if the token format ever grows a character
/// that does.
fn urlencode(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char)
            }
            other => out.push_str(&format!("%{other:02X}")),
        }
    }
    out
}

/// A composed message, ready to hand to the mailer.
pub struct Composed {
    pub subject: String,
    pub text: String,
    pub html: String,
}

impl Composed {
    /// Turn this into an [`apiplant_email::Message`] addressed to `recipient`.
    pub fn to(self, recipient: &str) -> apiplant_email::Message {
        apiplant_email::Message::to(recipient)
            .subject(self.subject)
            .text(self.text)
            .html(self.html)
    }
}

/// "You have been invited to <organisation>".
///
/// Names the person who sent it when we know who that is: an unexpected
/// invitation from a colleague is a different message from an unexpected
/// invitation from nobody, and the second one is what a phishing attempt looks
/// like.
pub fn invitation(
    links: &Links,
    organization: &str,
    inviter: Option<&str>,
    token: &str,
    expires_in: &str,
) -> Composed {
    let url = links.to("accept-invite", token);
    let who = match inviter {
        Some(name) if !name.is_empty() => format!("{name} has invited you"),
        _ => "You have been invited".to_string(),
    };
    let lead = format!("{who} to join {organization} on {}.", links.app_name);
    let note = format!(
        "Opening the link lets you choose a password and join. \
         It stops working in {expires_in}."
    );
    let subject = format!("You're invited to join {organization}");
    // The two facts only this message has, on top of the common ones.
    let mut vars = links.vars(&url, expires_in);
    vars.insert("organization".into(), liquid::model::Value::scalar(organization.to_string()));
    vars.insert(
        "inviter".into(),
        liquid::model::Value::scalar(inviter.unwrap_or_default().to_string()),
    );
    if let Some(composed) = links.rendered("invitation", &vars, &subject) {
        return composed;
    }
    Composed {
        subject,
        text: plain(&lead, "Accept the invitation:", &url, &note),
        html: html(links, &lead, "Accept the invitation", &url, &note),
    }
}

/// "Confirm your email address" — sent on registration when
/// `[auth] require_email_verification` is on.
pub fn verification(links: &Links, token: &str, expires_in: &str) -> Composed {
    let url = links.to("verify-email", token);
    let lead = format!(
        "Confirm this address to finish setting up your {} account.",
        links.app_name
    );
    let note = format!("The link stops working in {expires_in}.");
    let subject = format!("Confirm your email for {}", links.app_name);
    if let Some(composed) = links.rendered("verification", &links.vars(&url, expires_in), &subject) {
        return composed;
    }
    Composed {
        subject,
        text: plain(&lead, "Confirm your address:", &url, &note),
        html: html(links, &lead, "Confirm my address", &url, &note),
    }
}

/// "Reset your password".
///
/// Says plainly that an unrequested one can be ignored, because it can: the
/// existing password keeps working until this link is actually used.
pub fn password_reset(links: &Links, token: &str, expires_in: &str) -> Composed {
    let url = links.to("reset-password", token);
    let lead = format!(
        "Somebody asked to reset the password for this {} account.",
        links.app_name
    );
    let note = format!(
        "The link stops working in {expires_in}. If this wasn't you, ignore this \
         message — your password has not changed."
    );
    let subject = format!("Reset your {} password", links.app_name);
    if let Some(composed) = links.rendered("password_reset", &links.vars(&url, expires_in), &subject)
    {
        return composed;
    }
    Composed {
        subject,
        text: plain(&lead, "Choose a new password:", &url, &note),
        html: html(links, &lead, "Choose a new password", &url, &note),
    }
}

/// The plain-text half. The URL sits on its own line so that every mail client
/// linkifies it and every human can copy it.
fn plain(lead: &str, call: &str, url: &str, note: &str) -> String {
    format!("{lead}\n\n{call}\n{url}\n\n{note}\n")
}

/// The HTML half: a dark banner carrying the app's mark and name, then one
/// column of text, a button, and the URL again underneath.
///
/// Everything here is a table with inline styles, because a mail client is not
/// a browser: Outlook lays out with tables, Gmail strips `<style>` blocks and
/// most clients ignore anything that isn't on the element itself. The single
/// media query is the one exception, and it only narrows the padding on a phone
/// — a client that drops it still gets a readable message.
///
/// The URL is repeated as text under the button. A button is a link somebody's
/// client may decide not to render, and a link nobody can read is a support
/// ticket.
fn html(links: &Links, lead: &str, call: &str, url: &str, note: &str) -> String {
    let lead = escape(lead);
    let call = escape(call);
    let note = escape(note);
    let href = escape(url);
    let name = escape(&links.app_name);
    let year_free_footer =
        format!("You are receiving this because somebody used this address at {name}.");
    let footer = escape(&year_free_footer);

    // The mark, when there is one. `max-height` keeps a large file in its
    // place; the `alt` text is what a client that blocks images shows instead,
    // and it is the app's name because that is what the image says.
    let mark = match &links.logo_url {
        Some(src) => format!(
            r#"<img src="{}" alt="{name}" width="36" height="36" style="display:block;border:0;outline:none;text-decoration:none;height:36px;width:auto;max-height:36px;">"#,
            escape(src)
        ),
        None => String::new(),
    };
    let banner_cells = match links.logo_url {
        Some(_) => format!(
            r#"<td style="padding:0 12px 0 0;vertical-align:middle;">{mark}</td>
<td style="vertical-align:middle;font-size:18px;line-height:1.3;font-weight:700;color:#ffffff;letter-spacing:-0.2px;">{name}</td>"#
        ),
        None => format!(
            r#"<td style="vertical-align:middle;font-size:18px;line-height:1.3;font-weight:700;color:#ffffff;letter-spacing:-0.2px;">{name}</td>"#
        ),
    };

    format!(
        r#"<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width,initial-scale=1">
<meta name="color-scheme" content="light dark">
<meta name="supported-color-schemes" content="light dark">
<title>{name}</title>
<style>
@media only screen and (max-width:600px) {{
  .ap-pad {{ padding-left:20px !important; padding-right:20px !important; }}
  .ap-button {{ display:block !important; text-align:center !important; }}
}}
</style>
</head>
<body style="margin:0;padding:0;width:100%;background:#f4f5f7;font-family:-apple-system,BlinkMacSystemFont,'Segoe UI',Roboto,Helvetica,Arial,sans-serif;color:#1a1c1f;-webkit-font-smoothing:antialiased;">
<div style="display:none;font-size:0;line-height:0;max-height:0;overflow:hidden;opacity:0;">{lead}</div>
<table role="presentation" cellpadding="0" cellspacing="0" border="0" width="100%" style="background:#f4f5f7;">
<tr><td align="center" style="padding:24px 12px;">
<table role="presentation" cellpadding="0" cellspacing="0" border="0" width="600" style="width:100%;max-width:600px;background:#ffffff;border-radius:14px;border:1px solid #e3e6ea;overflow:hidden;">

<tr><td class="ap-pad" style="padding:22px 32px;background:#14161a;">
<table role="presentation" cellpadding="0" cellspacing="0" border="0"><tr>
{banner_cells}
</tr></table>
</td></tr>

<tr><td class="ap-pad" style="padding:32px 32px 8px;font-size:16px;line-height:1.6;color:#1a1c1f;">{lead}</td></tr>

<tr><td class="ap-pad" style="padding:20px 32px 8px;">
<table role="presentation" cellpadding="0" cellspacing="0" border="0"><tr><td style="border-radius:8px;background:#14161a;">
<a class="ap-button" href="{href}" style="display:inline-block;padding:13px 26px;border-radius:8px;background:#14161a;color:#ffffff;text-decoration:none;font-size:15px;font-weight:600;line-height:1;">{call}</a>
</td></tr></table>
</td></tr>

<tr><td class="ap-pad" style="padding:16px 32px 0;font-size:13px;line-height:1.6;color:#6b7280;">Or paste this link into your browser:</td></tr>
<tr><td class="ap-pad" style="padding:4px 32px 0;font-size:13px;line-height:1.6;word-break:break-all;"><a href="{href}" style="color:#4b5563;text-decoration:underline;">{href}</a></td></tr>

<tr><td class="ap-pad" style="padding:24px 32px 0;"><div style="height:1px;background:#e3e6ea;line-height:1px;font-size:0;">&nbsp;</div></td></tr>
<tr><td class="ap-pad" style="padding:16px 32px 28px;font-size:13px;line-height:1.6;color:#6b7280;">{note}</td></tr>

</table>
<table role="presentation" cellpadding="0" cellspacing="0" border="0" width="600" style="width:100%;max-width:600px;">
<tr><td class="ap-pad" style="padding:16px 32px 0;font-size:12px;line-height:1.6;color:#9aa1ab;text-align:center;">{footer}</td></tr>
</table>
</td></tr>
</table>
</body></html>"#
    )
}

/// Escape the five characters that would otherwise close a tag or an attribute.
fn escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

/// "7 days", "24 hours", "1 hour" — a duration written the way the sentence
/// "it stops working in ___" needs it.
pub fn humanise(secs: u64) -> String {
    let plural = |n: u64, unit: &str| {
        if n == 1 {
            format!("1 {unit}")
        } else {
            format!("{n} {unit}s")
        }
    };
    match secs {
        0..=90 => plural(secs.max(1), "second"),
        91..=3599 => plural((secs + 30) / 60, "minute"),
        3600..=172_800 => plural((secs + 1800) / 3600, "hour"),
        _ => plural((secs + 43200) / 86400, "day"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An app root with one template in it, and `Links` pointed at it.
    fn links_with(name: &str, body: &str) -> Links {
        let root = std::env::temp_dir().join(format!(
            "apiplant-emails-{name}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let dir = root.join(crate::email_templates::TEMPLATE_DIR);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(format!("{name}.liquid")), body).unwrap();
        let templates = crate::email_templates::EmailTemplates::load(&root).unwrap();
        links().with_templates(Arc::new(templates))
    }

    #[test]
    fn an_app_template_replaces_the_built_in_message_and_keeps_its_link() {
        // The whole contract: the app writes the words, the framework still
        // decides where the link goes and how long it lasts — those are not the
        // template's to get wrong.
        let links = links_with(
            "verification",
            "---\nsubject = \"Welcome to {{ app_name }}\"\n---\n             <p>Tap <a href=\"{{ url }}\">here</a> within {{ expires_in }}.</p>",
        );
        let message = verification(&links, "tok_abc", "24 hours");

        assert_eq!(message.subject, "Welcome to Acme");
        assert!(message.html.contains("Tap"));
        assert!(message.html.contains("verify-email?token=tok_abc"));
        assert!(message.html.contains("24 hours"));
        // The built-in wording is gone, not merely added to.
        assert!(!message.html.contains("Confirm this address to finish"));
        // …and a text half was derived, so the message is not HTML-only.
        assert!(message.text.contains("Tap"));
    }

    #[test]
    fn a_template_for_one_message_leaves_the_others_alone() {
        let links = links_with("verification", "<p>ours</p>");
        assert!(verification(&links, "t", "1 hour").html.contains("ours"));
        // The reset was not overridden, so it is still the framework's.
        let reset = password_reset(&links, "t", "1 hour");
        assert!(reset.html.contains("Choose a new password"));
        assert_eq!(reset.subject, "Reset your Acme password");
    }

    #[test]
    fn an_invitation_template_is_given_the_organisation_and_the_inviter() {
        // The two facts that message has and the other two do not.
        let links = links_with(
            "invitation",
            "<p>{{ inviter }} invited you to {{ organization }}.</p>",
        );
        let message = invitation(&links, "Acme Ltd", Some("Bo"), "inv_1", "7 days");
        assert!(message.html.contains("Bo invited you to Acme Ltd."));
        // No front matter, so the subject is the one it is replacing.
        assert_eq!(message.subject, "You're invited to join Acme Ltd");
    }

    #[test]
    fn an_app_with_no_templates_sends_the_built_in_messages() {
        let message = verification(&links(), "t", "1 hour");
        assert!(message.html.contains("Confirm this address to finish"));
    }

    fn links() -> Links {
        Links {
            base: "https://example.com/admin".into(),
            app_name: "Acme".into(),
            logo_url: Some("https://example.com/logo.png".into()),
            templates: None,
        }
    }

    #[test]
    fn the_banner_carries_the_logo_and_the_name_and_survives_having_neither() {
        let with_mark = verification(&links(), "t", "1 hour");
        assert!(with_mark
            .html
            .contains(r#"src="https://example.com/logo.png""#));
        assert!(with_mark.html.contains("Acme"));

        // No file behind `[email] logo`: the banner is still there, showing the
        // name alone rather than a broken image.
        let bare = Links {
            logo_url: None,
            ..links()
        };
        let without = verification(&bare, "t", "1 hour");
        assert!(!without.html.contains("<img"));
        assert!(without.html.contains("background:#14161a"));
        assert!(without.html.contains("Acme"));
    }

    #[test]
    fn the_logo_is_a_path_inside_public_that_has_to_exist() {
        let dir = std::env::temp_dir().join("apiplant-email-logo");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("public").join("img")).unwrap();
        std::fs::write(dir.join("public").join("img").join("mark.png"), b"png").unwrap();

        let mut app = apiplant_core::App::load(&dir).expect("a directory is a valid app");
        app.config.server.public_url = "https://example.com".into();

        // The default names a file most apps don't have, so it stays silent.
        assert_eq!(Links::from_app(&app).logo_url, None);

        // A leading slash is allowed and means the same file.
        for path in ["img/mark.png", "/img/mark.png"] {
            app.config.email.logo = path.into();
            assert_eq!(
                Links::from_app(&app).logo_url.as_deref(),
                Some("https://example.com/img/mark.png")
            );
        }

        // Nothing outside the public directory is reachable, so nothing outside
        // it can be linked.
        app.config.email.logo = "../secret.png".into();
        assert_eq!(Links::from_app(&app).logo_url, None);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_link_points_at_the_dashboard_screen_that_spends_the_token() {
        let message = invitation(&links(), "Acme Ltd", Some("Ann"), "inv_abc", "7 days");
        assert!(message
            .text
            .contains("https://example.com/admin/#/accept-invite?token=inv_abc"));
        assert!(message.html.contains("accept-invite?token=inv_abc"));
        // The person who sent it is named when we know them.
        assert!(message.text.contains("Ann has invited you"));

        let anonymous = invitation(&links(), "Acme Ltd", None, "inv_abc", "7 days");
        assert!(anonymous.text.contains("You have been invited"));
    }

    #[test]
    fn every_message_carries_the_url_as_readable_text_too() {
        // A button is a link a mail client may refuse to render; the URL under
        // it is what makes the message recoverable when that happens.
        for message in [
            verification(&links(), "verify_abc", "24 hours"),
            password_reset(&links(), "reset_abc", "1 hour"),
        ] {
            let url = message
                .text
                .lines()
                .find(|line| line.starts_with("https://"))
                .expect("a bare URL on its own line");
            assert!(message.html.contains(url));
        }
    }

    #[test]
    fn markup_in_a_name_cannot_escape_into_the_message() {
        let hostile = invitation(
            &links(),
            "<script>alert(1)</script>",
            None,
            "inv_abc",
            "7 days",
        );
        assert!(!hostile.html.contains("<script>"));
        assert!(hostile.html.contains("&lt;script&gt;"));
    }

    #[test]
    fn durations_read_like_a_sentence() {
        assert_eq!(humanise(60 * 60), "1 hour");
        assert_eq!(humanise(60 * 60 * 24), "24 hours");
        assert_eq!(humanise(60 * 60 * 24 * 7), "7 days");
        assert_eq!(humanise(60 * 30), "30 minutes");
    }

    #[test]
    fn the_dashboard_path_is_where_links_go_and_the_origin_is_the_fallback() {
        let mut app = apiplant_core::App::load(std::env::temp_dir().join("apiplant-no-such-app"))
            .expect("an empty directory is a valid app");
        app.config.server.public_url = "https://api.example.com/".into();
        assert_eq!(
            Links::from_app(&app).to("verify-email", "t"),
            "https://api.example.com/admin/#/verify-email?token=t"
        );

        // With no dashboard there is no form of ours to send anyone to, so the
        // app's own origin is the best we can do.
        app.config.admin.enabled = false;
        assert_eq!(
            Links::from_app(&app).to("verify-email", "t"),
            "https://api.example.com/#/verify-email?token=t"
        );
    }
}
