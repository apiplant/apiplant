//! Two ways an app sends mail, in one library.
//!
//! * `welcome_email` is a **hook**: `resources/users.toml` points the `user`
//!   resource's `after_create` event at it, so registering triggers it. Its
//!   body comes from `emails/welcome.liquid` — the function names a template
//!   and the values it needs, and never spells out a sentence.
//! * `notify` is an **endpoint**: `POST /api/functions/notify` mails an address
//!   the caller names, through a template the caller names, with the values it
//!   should be filled in with.
//!
//! Neither one names a provider. `ctx.send_email(...)` goes out through
//! whatever `[email] provider` says in `main.toml` — SMTP here, SES or SendGrid
//! in production — and the code below doesn't change when that does.

use apiplant_function::prelude::*;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Config from `functions/welcome_email.toml`.
#[derive(Deserialize)]
#[serde(default)]
struct WelcomeSettings {
    /// Subject line, when this app wants one per message. Empty — the default —
    /// leaves it to the front matter of `emails/welcome.liquid`, which is where
    /// the rest of the wording lives.
    subject: String,
    /// Where the mail tells people to go.
    sign_in_url: String,
}

impl Default for WelcomeSettings {
    fn default() -> Self {
        WelcomeSettings {
            subject: String::new(),
            sign_in_url: "http://127.0.0.1:8099/".to_string(),
        }
    }
}

/// `after_create` on `user` — the stored row arrives as `input`, so the address
/// and the display name are both in hand.
///
/// The interesting decision here is the error handling: a failed welcome email
/// must not undo a successful signup. The account exists either way, so a
/// failure is logged and the hook still returns `proceed()`.
fn welcome_email(ctx: &Context<WelcomeSettings>, input: Value) -> Result<Value, String> {
    let settings = ctx.config();

    let email = input["email"].as_str().unwrap_or_default();
    if email.is_empty() {
        ctx.warn("a user was created with no email address; nothing to send to");
        return Ok(reply::proceed());
    }
    let name = input["display_name"].as_str().unwrap_or("there");

    // The body is not here. `emails/welcome.liquid` holds it — subject, markup
    // and the plain-text half beside it — so changing the wording is editing a
    // file and restarting, with no Rust involved. This function's job is to
    // know *who* is being written to and *what facts* the message needs.
    let recipient = format!("{name} <{email}>");
    let mut message = Email::to(recipient)
        .template("welcome")
        .var("name", name)
        .var("sign_in_url", settings.sign_in_url.as_str());

    // A subject spelled out here wins over the template's own, which is how one
    // template serves several messages that differ only in their subject line.
    if !settings.subject.is_empty() {
        message = message.subject(&settings.subject);
    }

    match ctx.send_email(message) {
        Ok(sent) => ctx.info(&format!(
            "welcome mail to {email} accepted by {} as {}",
            sent.provider, sent.id
        )),
        // Deliberately not an error: the signup succeeded, and telling the
        // caller it failed would be a lie they can't act on.
        Err(error) => ctx.error(&format!("welcome mail to {email} failed: {error}")),
    }

    Ok(reply::proceed())
}

#[derive(Default, Deserialize, JsonSchema)]
#[serde(default)]
struct NotifyInput {
    /// Where to send it: `bo@example.com` or `Bo <bo@example.com>`.
    to: String,
    /// A template in `emails/` to render — `welcome` here. Leave it out to send
    /// `body` as written instead.
    template: String,
    /// What the template reads: `{"name": "Bo", "sign_in_url": "…"}`. A value
    /// the template never mentions is harmless; one it mentions without being
    /// given renders as empty.
    vars: serde_json::Map<String, Value>,
    /// Overrides the template's own subject. Required when sending a `body`.
    subject: String,
    /// The message, when no template is named.
    body: String,
}

#[derive(Serialize, JsonSchema)]
struct NotifyOutput {
    /// The provider that accepted the message.
    provider: String,
    /// Its identifier for the message; empty when the provider returns none.
    message_id: String,
}

/// `POST /api/functions/notify` — mail an address, through a template.
///
/// The recipient comes off the request, so this endpoint will mail anybody it
/// is told to. That is what `permission: "role:admin"` in the manifest below is
/// holding: an operator action, not something an account can aim at a stranger.
/// An app that mails its *own users* should look the address up by id instead —
/// then no caller can name one.
///
/// Here the failure *is* the answer, so it is returned rather than swallowed.
fn notify(ctx: &Context<()>, input: NotifyInput) -> Result<NotifyOutput, String> {
    // Not an address validator — the provider is the authority on that — just
    // enough to turn the common mistake into a sentence instead of a rejected
    // send and a bill for an API call.
    if !input.to.contains('@') {
        return Err("`to` must be an email address".to_string());
    }

    let mut message = Email::to(&input.to);

    if !input.template.is_empty() {
        // The body is `emails/<template>.liquid`, rendered with whatever the
        // caller passed. Naming a template the app does not have fails the
        // send, and the error says which name.
        message = message.template(&input.template).vars(input.vars);
    } else if !input.body.is_empty() {
        message = message.text(&input.body);
    } else {
        return Err("give either a `template` or a `body`".to_string());
    }

    // A subject spelled out here wins over the template's front matter, which
    // is how one template serves several messages that differ only in it.
    if !input.subject.is_empty() {
        message = message.subject(&input.subject);
    } else if input.template.is_empty() {
        return Err("a message with no template needs a `subject`".to_string());
    }

    let sent = ctx.send_email(message)?;

    Ok(NotifyOutput {
        provider: sent.provider,
        message_id: sent.id,
    })
}

apiplant_function::functions! {
    {
        name: "welcome_email",
        description: "Mails a new account its welcome message.",
        method: Post,
        permission: "private",     // a hook needs no endpoint of its own
        handler: welcome_email,
    },
    {
        name: "notify",
        description: "Sends a message to an address, rendering one of the app's templates.",
        method: Post,
        permission: "role:admin",  // mailing other people is an operator action
        admin: {
            label: "Send a message",
            group: "Communication",
            description: "Emails one address, rendering a template from emails/ with the values you give it.",
            run_label: "Send",
        },
        handler: notify,
    },
}
