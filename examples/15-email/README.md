# 15 · Email (one provider, named in config)

Registering sends a welcome message. An operator endpoint mails any address,
rendering the same template with values it is handed. Neither piece of code
knows which service actually delivers it — that is one line of `main.toml`, and
changing it changes nothing else.

```
15-email/
├── main.toml                     # [email] provider = "smtp" (+ every other provider, commented)
├── emails/
│   ├── welcome.liquid            # this app's own message, sent by the hook
│   ├── welcome.text.liquid       # …its plain-text half, written rather than derived
│   ├── verification.liquid       # replaces the framework's "confirm your address"
│   ├── invitation.liquid         # …and its "you're invited", which names the org
│   └── invitation.text.liquid    # …its plain-text half
├── public/
│   └── thank-you.html            # where a confirmed address lands, served at /
├── resources/
│   └── users.toml                # the built-in + [hooks] after_create
└── functions/
    ├── mail.rs                   # welcome_email (a hook) + notify (an endpoint)
    └── welcome_email.toml         # …the hook's config: the sign-in URL it passes in
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
createdb -h 127.0.0.1 -p 5432 -U postgres apiplant_email
cargo run -p apiplant -- build examples/15-email   # needs cargo on PATH
cargo run -p apiplant -- run examples/15-email
```

```
INFO apiplant_server:   email -> smtp (from no-reply@example.com)
INFO apiplant_server:   emails -> verification, welcome
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

Two messages land: the welcome one this app wrote, and the framework's own
confirmation of the address. The response is the usual `{ "token": …, "user": … }`
— the mail is a side effect, and the log says what happened to it:

```
INFO apiplant::function: welcome mail to ann@example.com accepted by smtp as 250 Ok
```

Your catcher now holds a `multipart/alternative` message — both halves given, so
a client that can't render HTML has something to show. Neither half is in
`mail.rs`:

```rust
let message = Email::to(recipient)
    .template("welcome")                      // emails/welcome.liquid
    .var("name", name)
    .var("sign_in_url", settings.sign_in_url.as_str());
```

## The wording lives in `emails/`

`emails/welcome.liquid` is [Liquid](https://shopify.github.io/liquid/) with TOML
front matter carrying the subject:

```liquid
---
subject = "Welcome to {{ app_name }}, {{ name }}"
---
<p>Hello {{ name }} — your account is ready.</p>
<a href="{{ sign_in_url }}">Sign in</a>
```

Two things follow from that. Changing what the message *says* is editing a file
and restarting — no `cargo build`, no Rust — which is the difference between the
copy belonging to whoever writes copy and belonging to whoever compiles. And the
function is left holding only what it actually knows: who is being written to,
and the facts the message needs.

`emails/welcome.text.liquid` beside it is the plain-text half, written out. Skip
that file and one is derived from the rendered HTML instead — tags dropped,
links kept as their URL — which is what `verification` does here.

A `subject` given in `functions/welcome_email.toml` still wins over the
template's own, so one template can serve several messages that differ only in
their subject line. It ships commented out.

### Replacing a message the framework sends

`emails/verification.liquid` is named after one of the three messages the
framework sends by itself — `verification`, `password_reset`, `invitation` — so
it **replaces** that message and keeps the flow around it. Delete the file and
the built-in comes back.

An override is given facts, not prose: `app_name`, `logo_url`, `url` and
`expires_in` (plus `organization` and `inviter` for an invitation). The link and
how long it lasts stay the framework's to decide — they are not the template's
to get wrong — while every sentence around them is yours.

`emails/invitation.liquid` is the second override here, and the one with
something extra to say: only this message knows which organisation is being
joined and who did the asking, so it is the only one that can put both in the
subject line.

```liquid
---
subject = "{{ inviter }} would like you in {{ organization }}"
---
```

`inviter` is empty when the invite came from an account with no name on it, so
the body branches on it rather than assuming — `{% if inviter != '' %}`. Try it
by inviting an address into an organisation:

```bash
curl -s -XPOST $API/auth/invitations -H "authorization: Bearer $TOKEN" \
  -H "X-Organization: $ORG" -H 'content-type: application/json' \
  -d '{"email":"someone@example.com","role":"member"}'
```

An address with no account yet is invited all the same: opening the link is
where they choose a password, and the account, the confirmed address and the
membership are all made at once. See
[Authentication](../../docs/authentication.md).

`app_name` and `logo_url` are the app's own facts, so they are in scope for
*every* template — an app's own `welcome` included, which is why the function
above hands over only `name` and `sign_in_url` and the banner still renders.

A template that does not **parse** stops the app at boot, naming the file: it
was written to be used, and quietly sending the built-in instead would look
exactly like the override working.

Both files are editable with a live preview in `apiplant studio`, which renders
them as the mail client will show them and lets you fill in the values.

## Confirming an address, and where it lands

Configuring a provider switches address confirmation on, so registering sends
the message above and the account cannot sign in until it is used. `[auth]
verify_email_redirect` says where somebody goes afterwards:

```toml
[auth]
verify_email_redirect = "/thank-you.html"
```

`POST /auth/verify-email` then returns it beside the session token:

```json
{ "token": "…", "verified": true, "redirect_to": "/thank-you.html" }
```

That page is `public/thank-you.html` — an ordinary static file, served at the
root of this origin because `[public]` is on by default. Public is the point: a
confirmation link is opened from a mailbox, by somebody whose browser may be
signed into nothing, so the page it lands on has to be one anybody can fetch.
An absolute URL works too, and is what an app whose front end lives somewhere
else would write instead.

The dashboard signs the user in and sends the browser there, so the app is
reached already authenticated. Confirming is the end of the sign-up detour, and
the place to land is the app — not the screen that happened to spend the token.
Unset, the key is **absent** from the response rather than empty, so a client
can tell "go here" from "nowhere in particular".

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

## Sending a template on purpose

`notify` is an ordinary function endpoint, gated to admins. It takes the
address, the template to render, and the values to render it with:

```json
{
  "to": "bo@example.com",
  "template": "welcome",
  "vars": { "name": "Bo", "sign_in_url": "http://127.0.0.1:8099/api/docs" },
  "subject": "Good to have you, Bo"
}
```

Creating an organisation makes you an admin — roles live on the membership, so
an account with no organisation has no role and gets a `403`. Sign in first,
which means confirming the address above, since a provider is configured:

```bash
TOKEN=$(curl -s -X POST http://127.0.0.1:8099/api/auth/login \
  -H 'content-type: application/json' \
  -d '{"email":"ann@example.com","password":"hunter2"}' | jq -r .token)

curl -s -X POST http://127.0.0.1:8099/api/organization \
  -H "authorization: Bearer $TOKEN" -H 'content-type: application/json' \
  -d '{"name":"Example Co"}'          # the creator joins as admin

curl -s -X POST http://127.0.0.1:8099/api/functions/notify \
  -H "authorization: Bearer $TOKEN" -H 'content-type: application/json' \
  -d '{"to":"bo@example.com","template":"welcome",
       "vars":{"name":"Bo","sign_in_url":"http://127.0.0.1:8099/api/docs"}}'
```

```json
{ "provider": "smtp", "message_id": "Ok: queued as CATCHER-1" }
```

The same template the signup hook sends, filled in by hand — which is the point
of the copy living in `emails/` rather than inside whichever function happens to
send it. Omit `template` and pass a `body` instead to send a message written on
the spot; a `subject` given here overrides the template's front matter either
way.

Three details:

* **The recipient comes off the request**, so this endpoint mails whoever it is
  told to. `permission: "role:admin"` is what makes that acceptable — it is an
  operator action. An app mailing its *own* users should take a user id and look
  the address up instead, so no caller can name one.
* **Unknown variables are not an error.** A value the template never mentions is
  ignored, and one it mentions without being given renders as empty — Liquid's
  own answer, and the reason a template can gain a variable without breaking
  every caller that predates it.
* Here the error *is* the answer, so `ctx.send_email(...)?` propagates it and
  the caller gets a `400` naming the provider's complaint — or the template that
  does not exist. Same call, opposite handling from the hook, because the caller
  can act on this one.

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
