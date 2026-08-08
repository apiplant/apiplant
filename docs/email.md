# Sending email

An app names a provider in `main.toml`, and functions send mail through it.
Nothing else changes: the same `ctx.send_email(...)` goes out through Amazon
SES, SendGrid, Brevo, Mailjet or a plain SMTP relay depending on one line of
configuration.

```toml
[email]
provider = "sendgrid"
from     = "no-reply@example.com"
api_key  = "${SENDGRID_API_KEY}"
```

```rust
ctx.send_email(
    Email::to("ann@example.com")
        .subject("Welcome to Example")
        .text("Glad you're here.")
        .html("<p>Glad you're here.</p>"),
)?;
```

Email is **off by default** (`provider = "none"`). An app that never sends mail
carries no configuration, no credentials and no client.

The framework never sends email on its own. There is no built-in "password
reset" or "verify your address" message, because the wording, the link and the
timing belong to the application. The framework provides the transport.

## Providers

| `provider` | Reaches | Credentials it needs |
|------------|---------|----------------------|
| `smtp` | any SMTP relay | `[email.smtp]`, see below |
| `ses` (`aws`) | Amazon SES v2 API | `api_key` = access key id, `api_secret` = secret access key, `region` |
| `sendgrid` | `api.sendgrid.com` | `api_key` |
| `brevo` (`sendinblue`, `mailinblue`) | `api.brevo.com` | `api_key` |
| `mailjet` | `api.mailjet.com` | `api_key` (public key) + `api_secret` (private key) |
| `mailgun` | `api.mailgun.net` | `api_key` + `domain` |
| `postmark` | `api.postmarkapp.com` | `api_key` (server token) |
| `resend` | `api.resend.com` | `api_key` |

All of these also support SMTP, so `provider = "smtp"` serves both as the
fallback for services not listed here and as a normal way to use one that is.

Switching providers is a configuration change. The message a function builds,
the receipt it gets back and the errors it has to handle are identical across
all of them.

## Configuration

```toml
[email]
provider     = "ses"                   # none | smtp | ses | sendgrid | brevo |
                                       # mailjet | mailgun | postmark | resend
from         = "no-reply@example.com"  # required once enabled
from_name    = "Example"               # display name beside `from`
reply_to     = "help@example.com"      # optional default Reply-To
api_key      = "${AWS_ACCESS_KEY_ID}"
api_secret   = "${AWS_SECRET_ACCESS_KEY}"
region       = "eu-west-1"             # ses only
domain       = "mg.example.com"        # mailgun only
timeout_secs = 15                      # per send
logo         = "logo.png"              # banner mark, a path inside public/

[email.smtp]                           # provider = "smtp" only
host       = "smtp.example.com"
port       = 0                         # 0 = pick from `encryption`
username   = "apikey"
password   = "${SMTP_PASSWORD}"
encryption = "starttls"                # starttls | tls | none
```

`port = 0` resolves to 465 for `tls`, 587 for `starttls` and 25 for `none`.
`encryption = "none"` sends credentials in cleartext and logs a warning at
boot. It is intended only for a relay on localhost.

### Keys belong in the environment

Every string in an app's TOML files may reference the environment using `$VAR`,
`${VAR}` or `${VAR:-default}`, so a committed `main.toml` names the variable
rather than the key itself:

```toml
[email]
provider = "sendgrid"
api_key  = "$SENDGRID_API_KEY"
```

See [Configuration → Environment
variables](configuration.md#environment-variables).

### Misconfiguration fails the boot

An unusable provider configuration (an unknown name, a missing `api_key`,
`mailgun` without a `domain`, or no `from` address) stops the server at startup
with a message naming the field. The alternative would be discovering the
problem at the first password reset.

## Sending from a function

`Email` builds the message; `ctx.send_email` sends it and returns the
provider's receipt.

```rust
use apiplant_function::prelude::*;

fn invite(ctx: &Context<Config>, input: Input) -> Result<Output, String> {
    let sent = ctx.send_email(
        Email::to(format!("{} <{}>", input.name, input.email))
            .cc("records@example.com")
            .subject("You've been invited")
            .text(format!("Hello {}, join us at {}", input.name, input.link))
            .html(format!("<p>Hello {}, <a href=\"{}\">join us</a></p>",
                          input.name, input.link))
            .reply_to("help@example.com"),
    )?;

    ctx.info(&format!("invite sent via {} as {}", sent.provider, sent.id));
    Ok(Output { message_id: sent.id })
}
```

Addresses use the usual mail-client forms: either `"ann@example.com"` or
`"Ann Lee <ann@example.com>"`. `Email::to_all` takes
several at once, and `.cc(…)` / `.bcc(…)` can be chained more than once.

`from` and `reply_to` come from `[email]` unless the message overrides them,
so a function that sends as the app itself never mentions a sender.

Send at least one of `.text(…)` and `.html(…)`. When both are given, providers
that build the MIME themselves receive a `multipart/alternative`, with the plain
part serving clients that cannot render HTML.

### The receipt

```rust
pub struct Sent {
    pub provider: String,    // "ses", "sendgrid", …
    pub id: String,          // the provider's message id, "" if it returns none
    pub recipients: usize,   // to + cc + bcc
}
```

`id` is the reference to quote in provider support requests, and is worth
logging. SendGrid returns it in a header and SMTP relays usually return a queue
id; in both cases it arrives in the same field.

### When sending fails

`send_email` returns `Err` when no provider is configured, when the message
can't be sent (no recipient, no sender) or when the provider refuses it. The
error names the provider and its status:

```
sendgrid rejected the message (401): {"errors":[{"message":"…"}]}
```

Whether that should fail the request is application-specific, and usually it
should not:

```rust
if let Err(error) = ctx.send_email(welcome) {
    // The account exists regardless; do not roll back a signup over an email.
    ctx.error(&format!("welcome email failed: {error}"));
}
```

Sending happens inline, on the function's blocking worker, and takes as long as
the provider does (bounded by `timeout_secs`). A function that sends several
messages holds its worker for the total duration, so for a large batch, write
the recipients to a resource and process them from a separate job.

## Sending from a hook

Hooks are functions, so they send mail the same way. `after_create` is the usual
place, since the row already exists by then:

```toml
# resources/invoice.toml
[hooks]
after_create = "invoice_after_create"
```

```rust
fn invoice_after_create(ctx: &Context<()>, row: serde_json::Value) -> Result<Value, String> {
    let to = row["customer_email"].as_str().unwrap_or_default();
    if !to.is_empty() {
        let _ = ctx.send_email(
            Email::to(to)
                .subject(format!("Invoice {}", row["number"])) 
                .text("Your invoice is attached to your account."),
        );
    }
    Ok(reply::proceed())
}
```

A `before_*` hook can also send, but the write has not happened yet, so a
failure after the message has gone out announces something that does not exist.
Prefer `after_*` for anything the recipient will act on.

## From C, Zig or Go

The C ABI carries the same call. `host->send_email(ctx, request_json)` takes the
message as JSON and returns the receipt (or `{"error": "…"}`), which you release
with `host->free_string`:

```c
char *receipt = host->send_email(host->ctx,
    "{\"to\":\"ann@example.com\",\"subject\":\"Hi\",\"text\":\"Hello\"}");
/* … use it … */
host->free_string(host->ctx, receipt);
```

See [Functions → the C ABI](functions.md) for the whole contract.

## What a provider switches on by itself

Configuring `[email]` does more than let a function call `send_email`. Three
parts of the built-in identity system need a mailbox, and all three appear the
moment there is one to use:

* **organisation invitations**, allowing an admin to add someone with no account,
* **address confirmation** on registration,
* **password reset**.

Without a provider they are not merely disabled: their endpoints are not
mounted, and neither the dashboard nor the console offers a control that would
reach one. Each can be toggled individually through `[auth]`, and all three
default to following this section. See
[Authentication → Reaching people by email](authentication.md#reaching-people-by-email)
for the endpoints, and remember to set `[server] public_url` so the links in
those messages point somewhere a browser can reach.

The messages themselves are intentionally plain: a dark banner with your logo
and the app's name, a sentence, a button, and the URL as readable text below it.
The banner image is `[email] logo`, a path inside `public/`:

```toml
[email]
logo = "logo.png"    # or "/img/mark.svg"; the leading slash is optional
```

It defaults to `logo.png` and is used only when that file exists, so an app
without one gets a banner showing just its name rather than a broken image. The
URL is built against `[server] public_url`, because the image is fetched from a
mail client rather than from a page.

An app that wants its own wording and letterhead should send its own messages
from a hook. `after_register` is intended for this, and a function has access to
the full API described below.

## What isn't here

* **Attachments and templates.** A message is a subject, a text part and an
  HTML part. Provider-side templates remain available by calling the provider's
  own API from a function, which is the appropriate place for a
  provider-specific feature.
* **Queueing and retries.** A send is one HTTP request or one SMTP session, and
  it either succeeds or returns an error. Durable delivery belongs to a queue,
  not a request handler.
* **Inbound mail.** apiplant sends mail; it does not receive it.

## See also

* [Example 15 · email](../examples/15-email): a working app that sends on
  registration and from an endpoint.
* [Configuration](configuration.md): the full `main.toml` reference.
* [Functions](functions.md): writing the code that sends.
* [Caching](caching.md): the other optional service a function can reach.
