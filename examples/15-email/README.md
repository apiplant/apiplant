# 15 · Email (one provider, named in config)

Registering sends a welcome message. An operator endpoint mails an existing
account. Neither piece of code knows which service actually delivers it — that
is one line of `main.toml`, and changing it changes nothing else.

```
15-email/
├── main.toml                     # [email] provider = "smtp" (+ every other provider, commented)
├── models/
│   └── users.toml                # the built-in + [hooks] after_create
└── functions/
    ├── mail.rs                   # welcome_email (a hook) + notify (an endpoint)
    └── welcome_email.toml         # …the hook's config: subject, sign-in URL
```

[Example 14](../14-email-domains) showed a hook reacting to registration by
writing a row. This one reacts by sending a message — the same event, the same
`after_create` shape, a different service on the other end.

## Run it

The example ships pointed at a local mail catcher, so it runs without an
account anywhere. Any of these will do:

```bash
mailpit                       # or: MailHog
python3 -m aiosmtpd -n -l 127.0.0.1:1025   # prints messages to the terminal
```

```bash
createdb -h 127.0.0.1 -p 55432 -U postgres apiplant_email
cargo run -p apiplant -- build examples/15-email   # needs cargo on PATH
cargo run -p apiplant -- run examples/15-email
```

```
INFO apiplant_server:   email -> smtp (from no-reply@example.com)
INFO apiplant_server:   fn welcome_email (private — no endpoint)
INFO apiplant_server:   fn notify -> /api/functions/notify
INFO apiplant_server:   hook user.after_create -> welcome_email
```

That first line is worth noticing: the mailer is built at **boot**, not at the
first send. A missing key or an unknown provider stops the server there and
then, with a message naming the field — rather than at 3am, inside somebody's
password reset.

## Registering sends the welcome mail

```bash
curl -s -X POST http://127.0.0.1:8099/api/auth/register \
  -H 'content-type: application/json' \
  -d '{"email":"ann@example.com","password":"hunter2","display_name":"Ann"}'
```

The response is the usual `{ "token": …, "user": … }` — the mail is a side
effect, and the log says what happened to it:

```
INFO apiplant::function: welcome mail to ann@example.com accepted by smtp as 250 Ok
```

Your catcher now holds a `multipart/alternative` message: `.text(...)` and
`.html(...)` both given, so a client that can't render HTML has something to
show.

### A failed send does not fail the signup

Stop the catcher and register again. The account is still created, the token is
still issued, and the log carries the failure instead:

```
ERROR apiplant::function: welcome mail to bo@example.com failed: email transport: smtp: Connection refused
```

That's a choice the hook makes, and the interesting line in `mail.rs`:

```rust
match ctx.send_email(message) {
    Ok(sent) => ctx.info(&format!("…accepted by {} as {}", sent.provider, sent.id)),
    // The signup succeeded. Telling the caller it didn't would be a lie.
    Err(error) => ctx.error(&format!("welcome mail to {email} failed: {error}")),
}
```

Returning `Err` from an `after_create` hook fails the request that triggered it.
For a welcome email that is the wrong trade: the account exists either way.

## Mailing an account on purpose

`notify` is an ordinary function endpoint, gated to admins. Creating an
organisation makes you one — roles live on the membership, so an account with
no organisation has no role and gets a `403`:

```bash
TOKEN=$(curl -s -X POST http://127.0.0.1:8099/api/auth/login \
  -H 'content-type: application/json' \
  -d '{"email":"ann@example.com","password":"hunter2"}' | jq -r .token)

curl -s -X POST http://127.0.0.1:8099/api/organization \
  -H "authorization: Bearer $TOKEN" -H 'content-type: application/json' \
  -d '{"name":"Example Co"}'          # the creator joins as admin

curl -s -X POST http://127.0.0.1:8099/api/functions/notify \
  -H "authorization: Bearer $TOKEN" -H 'content-type: application/json' \
  -d '{"user_id":"<uuid>","subject":"Maintenance","body":"We are back."}'
```

```json
{ "provider": "smtp", "message_id": "Ok: queued as CATCHER-1" }
```

Two details:

* The address is read **from the database**, not taken from the request. An
  endpoint that mails whatever address it is handed is an open relay with extra
  steps.
* Here the error *is* the answer, so `ctx.send_email(...)?` propagates it and
  the caller gets a `400` naming the provider's complaint. Same call, opposite
  handling from the hook — because the caller can act on this one.

`notify` also appears in the admin dashboard, under "Communication", because
the `admin { … }` block in its manifest asks for it.

## Sending for real

Comment out `[email.smtp]` and replace the `[email]` block. SendGrid:

```toml
[email]
provider = "sendgrid"
from     = "no-reply@example.com"
api_key  = "${SENDGRID_API_KEY}"
```

Amazon SES:

```toml
[email]
provider   = "ses"
from       = "no-reply@example.com"
api_key    = "${AWS_ACCESS_KEY_ID}"
api_secret = "${AWS_SECRET_ACCESS_KEY}"
region     = "eu-west-1"
```

`main.toml` has the rest commented out: Brevo, Mailjet, Mailgun, Postmark and
Resend. Rebuild nothing — the library never mentioned a provider. `$VAR` is read
from the environment at boot — as it is in every app TOML file — which is how a
`main.toml` you commit holds no secrets.

## What to read next

* [Sending email](../../docs/email.md) — every provider, the whole `Email`
  builder, and what deliberately isn't there (attachments, queueing, retries).
* [Example 16 · caching](../16-caching) — the other optional service a function
  can reach.
