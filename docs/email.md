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

Nothing in the framework sends email on its own — there is no built-in
"password reset" or "verify your address" mail, because the wording, the link
and the timing are all yours. What the framework provides is the transport.

## Providers

| `provider` | Reaches | Credentials it needs |
|------------|---------|----------------------|
| `smtp` | any SMTP relay | `[email.smtp]` — see below |
| `ses` (`aws`) | Amazon SES v2 API | `api_key` = access key id, `api_secret` = secret access key, `region` |
| `sendgrid` | `api.sendgrid.com` | `api_key` |
| `brevo` (`sendinblue`, `mailinblue`) | `api.brevo.com` | `api_key` |
| `mailjet` | `api.mailjet.com` | `api_key` (public key) + `api_secret` (private key) |
| `mailgun` | `api.mailgun.net` | `api_key` + `domain` |
| `postmark` | `api.postmarkapp.com` | `api_key` (server token) |
| `resend` | `api.resend.com` | `api_key` |

Every one of these also speaks SMTP, so `provider = "smtp"` is both the escape
hatch for a service that isn't listed and a perfectly ordinary way to use one
that is.

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

[email.smtp]                           # provider = "smtp" only
host       = "smtp.example.com"
port       = 0                         # 0 = pick from `encryption`
username   = "apikey"
password   = "${SMTP_PASSWORD}"
encryption = "starttls"                # starttls | tls | none
```

`port = 0` resolves to 465 for `tls`, 587 for `starttls` and 25 for `none`.
`encryption = "none"` sends credentials in cleartext and logs a warning at
boot; it exists for a relay on localhost and nothing else.

### Keys belong in the environment

Every string in an app's TOML files may reference the environment — `$VAR`,
`${VAR}`, or `${VAR:-default}` — so a `main.toml` you commit names the variable
rather than the key:

```toml
[email]
provider = "sendgrid"
api_key  = "$SENDGRID_API_KEY"
```

See [Configuration → Environment
variables](configuration.md#environment-variables).

### Misconfiguration fails the boot

A provider that can't work — an unknown name, a missing `api_key`, `mailgun`
without a `domain`, no `from` address — stops the server at startup with a
message naming the field. The alternative is finding out at the first password
reset, which is both later and someone else's problem.

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

Addresses are written the way you'd write them in a mail client — either
`"ann@example.com"` or `"Ann Lee <ann@example.com>"`. `Email::to_all` takes
several at once, and `.cc(…)` / `.bcc(…)` can be chained more than once.

`from` and `reply_to` come from `[email]` unless the message overrides them,
so a function that sends as the app itself never mentions a sender.

Send at least one of `.text(…)` and `.html(…)`. Given both, providers that
build the MIME themselves receive a `multipart/alternative` — the plain part
for clients that can't render HTML.

### The receipt

```rust
pub struct Sent {
    pub provider: String,    // "ses", "sendgrid", …
    pub id: String,          // the provider's message id, "" if it returns none
    pub recipients: usize,   // to + cc + bcc
}
```

`id` is what you quote at the provider's support desk, and worth logging.
SendGrid returns it in a header and SMTP relays usually return a queue id;
either way it comes back in the same field.

### When sending fails

`send_email` returns `Err` when no provider is configured, when the message
can't be sent (no recipient, no sender) or when the provider refuses it. The
error names the provider and its status:

```
sendgrid rejected the message (401): {"errors":[{"message":"…"}]}
```

Whether that should fail the request is your call, and usually it shouldn't:

```rust
if let Err(error) = ctx.send_email(welcome) {
    // The account exists either way; don't undo a signup over an email.
    ctx.error(&format!("welcome email failed: {error}"));
}
```

Sending happens inline, on the function's blocking worker, and takes as long as
the provider does (bounded by `timeout_secs`). A function that sends several
messages holds its worker for the sum of them — for a large batch, write the
recipients to a resource and drain it from a separate job.

## Sending from a hook

Hooks are functions, so they send mail the same way. Sending on `after_create`
is the usual shape — the row exists, so the email is about something real:

```toml
# models/invoice.toml
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

A `before_*` hook can send too, but the write hasn't happened yet — and a
failure after the email has gone out leaves you having announced something that
doesn't exist. Prefer `after_*` for anything the recipient will act on.

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

## What isn't here

* **Attachments and templates.** A message is a subject, a text part and an
  HTML part. Provider-side templates are reachable by calling the provider's
  own API from a function, which is the honest place for something that only
  one provider supports.
* **Queueing and retries.** A send is one HTTP request or one SMTP session, and
  it either works or returns an error. Durable delivery is a queue's job, not a
  request handler's.
* **Inbound mail.** apiplant sends; nothing here receives.

## See also

* [Example 15 · email](../examples/15-email) — a working app that sends on
  registration and from an endpoint.
* [Configuration](configuration.md) — the full `main.toml` reference.
* [Functions](functions.md) — writing the code that sends.
* [Caching](caching.md) — the other optional service a function can reach.
