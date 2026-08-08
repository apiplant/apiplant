//! Two ways an app sends mail, in one library.
//!
//! * `welcome_email` is a **hook**: `resources/users.toml` points the `user`
//!   resource's `after_create` event at it, so registering triggers it.
//! * `notify` is an **endpoint**: `POST /api/functions/notify` mails a message
//!   to an existing account.
//!
//! Neither one names a provider. `ctx.send_email(...)` goes out through
//! whatever `[email] provider` says in `main.toml` — SMTP here, SES or SendGrid
//! in production — and the code below doesn't change when that does.

use apiplant_function::prelude::*;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

/// Config from `functions/welcome_email.toml`.
#[derive(Deserialize)]
#[serde(default)]
struct WelcomeSettings {
    /// Subject line. Kept in config so the wording changes with a restart
    /// rather than a rebuild.
    subject: String,
    /// Where the mail tells people to go.
    sign_in_url: String,
}

impl Default for WelcomeSettings {
    fn default() -> Self {
        WelcomeSettings {
            subject: "Welcome".to_string(),
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

    let recipient = format!("{name} <{email}>");
    let message = Email::to(recipient)
        .subject(&settings.subject)
        .text(format!(
            "Hello {name},\n\nYour account is ready. Sign in at {}.\n",
            settings.sign_in_url
        ))
        .html(format!(
            "<p>Hello {name},</p><p>Your account is ready. \
             <a href=\"{}\">Sign in</a>.</p>",
            settings.sign_in_url
        ));

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

#[derive(Deserialize, JsonSchema)]
struct NotifyInput {
    /// Id of the account to write to.
    user_id: String,
    subject: String,
    body: String,
}

#[derive(Serialize, JsonSchema)]
struct NotifyOutput {
    /// The provider that accepted the message.
    provider: String,
    /// Its identifier for the message; empty when the provider returns none.
    message_id: String,
}

/// `POST /api/functions/notify` — mail an existing account.
///
/// The address comes out of the database rather than off the request, so this
/// endpoint can't be used to send mail to an arbitrary stranger. Here the
/// failure *is* the answer, so it is returned rather than swallowed.
fn notify(ctx: &Context<()>, input: NotifyInput) -> Result<NotifyOutput, String> {
    let row = ctx
        .query_one(
            "SELECT email, display_name FROM apiplant_user WHERE id = $1::uuid",
            &[json!(input.user_id)],
        )?
        .ok_or("no such user")?;

    let email = row["email"].as_str().unwrap_or_default();
    if email.is_empty() {
        return Err("that account has no email address".to_string());
    }
    let name = row["display_name"].as_str().unwrap_or_default();

    let recipient = if name.is_empty() {
        email.to_string()
    } else {
        format!("{name} <{email}>")
    };

    let sent = ctx.send_email(
        Email::to(recipient)
            .subject(input.subject)
            .text(input.body),
    )?;

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
        description: "Sends a message to an existing account.",
        method: Post,
        permission: "role:admin",  // mailing other people is an operator action
        admin: {
            label: "Send a message",
            group: "Communication",
            description: "Emails one account, using the app's configured provider.",
            run_label: "Send",
        },
        handler: notify,
    },
}
