# Authentication & authorization

apiplant ships a complete identity system built out of ordinary resources.
`organization`, `membership`, `user`, `api_key`, and `oauth_connection` exist by
default, and any of them can be extended by adding a same-named file in
`resources/`.

## Built-in resources

| Resource | Purpose | Key fields |
|----------|---------|-----------|
| `organization` | tenants | `name`, `slug` (unique), `avatar_url` |
| `membership` | joins users to organisations, carrying a per-org role | `user_id → user`, `organization_id → organization`, `role` |
| `user` | accounts | `email` (unique), `password_hash` (hidden), `display_name`, `avatar_url`, `email_placeholder` |
| `api_key` | long-lived credentials | `name`, `token_hash` (hidden, unique), `owner_id → user` |
| `oauth_connection` | linked third-party identities | `provider`, `provider_user_id`, `provider_key` (unique), `owner_id → user`, and what the provider last said about them |
| `invitation` | a pending invitation to an organisation, for an address that may not yet have an account | `email`, `role`, `token_hash` (hidden, unique), `organization_id`, `expires_at`, `accepted_at` |
| `auth_token` | single-use tokens sent by email | `user_id → user`, `kind`, `token_hash` (hidden, unique), `expires_at`, `used_at` |

These are real resources: they get tables, CRUD endpoints (gated by their own
permissions), migrations and relationships like any other.

## The `[auth]` section

The `user` resource carries an optional `[auth]` block controlling how login works:

```toml
# resources/users.toml
[resource]
name = "user"

[auth]
identity_field  = "email"          # the login identifier column
password_field  = "password_hash"  # where the argon2 hash is stored
oauth_providers = ["google", "github"]

[fields.email]
type = "string"
required = true
unique = true

[fields.password_hash]
type = "string"
hidden = true

# add your own fields freely:
[fields.display_name]
type = "string"
```

Without a `resources/users.toml`, the defaults are `identity_field = "email"`,
`password_field = "password_hash"`, no OAuth providers and no hooks.

### Auth hooks

The endpoints below are extension points, not fixed behaviour: the `user`
resource's ordinary `[hooks]` section carries an event for each point in their
lifecycle, alongside the resource's CRUD hooks.

```toml
[hooks]
before_register = "invite_only"    # reject a signup without a valid invite
after_register  = "send_welcome"
before_login    = "check_lockout"  # return 423 after too many failed attempts
after_login     = "record_attempt" # every attempt, successful or not
before_api_key  = "stamp_expiry"
after_api_key   = "audit_key"
```

They follow the same protocol as [lifecycle hooks](hooks.md), returning
`{"error": …}` to abort or `{"data": …}` to replace, and never receive a
plaintext password. `after_login` is worth noting: it fires on failures as well
as successes, carrying `success`, `identity` and a `reason`, which is what a
lockout counter uses without needing a separate event. See
[Auth hooks](hooks.md#auth-hooks) for the payload each one receives.

## Endpoints

| Method | Path | Purpose |
|--------|------|---------|
| `POST` | `<base>/auth/register` | Create a user, return a session token |
| `POST` | `<base>/auth/login` | Exchange credentials for a session token |
| `GET` | `<base>/auth/me` | Check the caller's credential is valid and their user still exists |
| `POST` | `<base>/auth/apikeys` | Issue an API key for the caller |

These are always mounted. The seven below are mounted **only when the app can
send email**; see [Reaching people by email](#reaching-people-by-email).

| Method | Path | Purpose |
|--------|------|---------|
| `POST` | `<base>/auth/invitations` | Invite an address into the active organisation |
| `GET` | `<base>/auth/invitations/{token}` | What an invitation link is for (anonymous) |
| `POST` | `<base>/auth/invitations/{token}/accept` | Accept it, creating an account if needed |
| `POST` | `<base>/auth/verify-email` | Spend a confirmation token |
| `POST` | `<base>/auth/verify-email/resend` | Send the confirmation again |
| `POST` | `<base>/auth/password/forgot` | Email a reset link |
| `POST` | `<base>/auth/password/reset` | Set a new password from one |

### Register

```http
POST /api/auth/register
{ "email": "a@b.com", "password": "hunter2", "display_name": "Ann" }
```

* Requires a `password`; all other properties map to `user` fields.
* The password is argon2id-hashed into the `password_field`; the plaintext is
  never stored.
* Honours `auth.allow_registration` (`403` when disabled). The `user` resource
  ships with `create = "public"` so that this endpoint works, so the same switch
  also closes anonymous `POST <base>/user`; otherwise signup would simply move
  to that route.
* Returns `{ "token": "...", "user": { ... } }` (with hidden fields stripped).
* The new account is given a [personal organisation](multitenancy.md) it
  administers, so no session begins without an organisation to work in.
* Registering is a `create` on the `user` resource, so the `user` resource's
  `before_create` / `after_create` [hooks](hooks.md#registration-fires-the-user-create-hooks)
  also fire here. This is the usual place to assign a new account a starting
  organisation, as [`examples/14-email-domains`](../examples/14-email-domains)
  demonstrates.

### Login

```http
POST /api/auth/login
{ "email": "a@b.com", "password": "hunter2" }
```

The identity property name is whatever `identity_field` is set to. Returns
`{ "token": "..." }` on success, `401` otherwise. The token embeds the user's id;
organisation memberships are loaded fresh on each request.

### Check a credential

```http
GET /api/auth/me             (authenticated)
```

Returns `{ "user_id": "…" }` when the credential verifies *and* the account it
names still exists, and `401` when either fails. A signature check alone cannot
establish the second condition, since a token signed before the user was deleted
still verifies, so this endpoint gives a client holding a stored token a single
call to determine whether to keep it. The admin dashboard calls this on load and
discards the token on a `401`.

### Issue an API key

```http
POST /api/auth/apikeys       (authenticated)
{ "name": "ci-server" }
```

Returns `{ "api_key": "apik_…", "id": "…" }`. **The plaintext key is shown
once**; only its SHA-256 is stored, in `token_hash`. Delete a key by removing
its `api_key` row.

## Reaching people by email

An identity system needs email for three things:

* **inviting someone** who does not yet have an account,
* **confirming** that an address belongs to the person who entered it,
* **resetting** a forgotten password.

All three become available as soon as an app configures an
[`[email]` provider](email.md), and none are available without one:

```toml
[email]
provider = "resend"
from     = "no-reply@example.com"
api_key  = "${RESEND_API_KEY}"

[server]
public_url = "https://example.com"   # the origin emailed links point at
```

That is the complete setup. `[auth] require_email_verification`,
`allow_invitations` and `allow_password_reset` each default to following
`[email]`, so configuring a provider enables them, and any of them can be
disabled individually; see [Configuration](configuration.md#auth).

**Without a provider, the endpoints are not registered at all.** A server that
cannot send mail does not fail partway through a password reset; it has no
password reset endpoint. The dashboard and the console read the same three facts
from the admin manifest, so neither offers a control that would reach a missing
route.

The destination of the links matters: `public_url` provides the origin, and the
path is the admin dashboard, whose `#/accept-invite`, `#/verify-email` and
`#/reset-password` screens are the forms that consume the tokens. The request's
own `Host:` header is not used, since a message is composed once and read
elsewhere, possibly days later and behind a proxy that rewrote it. Set
`public_url` in any deployment not reached at `http://<host>:<port>`.

### Invitations

```http
POST /api/auth/invitations       (role:admin in the active organisation)
X-Organization: <org id>
{ "email": "new@example.com", "role": "member" }
```

Emails a link and creates an `invitation` row. The address does not need an
existing account, which is why this endpoint exists alongside
`POST <base>/membership`, which can only add users who have already registered.

Permission to issue one is read from the **`membership` resource's `create`
policy** (`role:admin` by default), so an app that has changed who manages its
team gets consistent behaviour rather than a second, independent rule.

Inviting the same address twice replaces the pending invitation, matching the
intent of re-sending, and the earlier link stops working. Revoke one by deleting
its `invitation` row (`DELETE <base>/invitation/{id}`, `role:admin`). An
invitation that could not be *sent* leaves no row behind, so an admin never sees
a pending entry for a message that was never delivered.

Accepting is two calls, both anonymous, both authorised by the token alone:

```http
GET  /api/auth/invitations/{token}
→ { "email": "…", "organization": "Acme Ltd", "role": "member",
    "expires_at": "…", "has_account": false }

POST /api/auth/invitations/{token}/accept
{ "password": "hunter2" }        # only when has_account is false
→ { "token": "<session>", "organization_id": "…" }
```

Without an existing account, this *is* a registration: it fires the `user`
resource's `before_register`, `before_create`, `after_create` and `after_register`
[hooks](hooks.md) exactly as `POST <base>/auth/register` does, so rules about
who may sign up cannot be bypassed via an invitation. The new account is marked
as having a confirmed address without a second email, since opening the link
already demonstrates control of the mailbox.

With an existing account, nothing is created and no password is required: the
token proves control of the address the account is registered to.

### Confirming an address

With `require_email_verification` on, `POST <base>/auth/register` creates the
account and returns **no session**:

```json
{ "user": { … }, "verification_required": true,
  "message": "Check your email to confirm your address, then sign in." }
```

Signing in before confirming is rejected with `403` and
`{"reason": "email_unverified"}`, which is distinct from the `401` returned for
a wrong password: the credentials were correct, and the user has a specific
action available. `after_login` receives the same `reason`, so a lockout counter
can exclude these attempts.

`POST <base>/auth/verify-email` with `{"token": "…"}` marks the address
confirmed and returns a session token, since requiring a separate sign-in
immediately after proving mailbox access adds nothing.

Confirming is the end of the sign-up detour, and the place to land afterwards is
usually the app rather than the screen that spent the token. `[auth]
verify_email_redirect` names it:

```toml
[auth]
verify_email_redirect = "https://app.example.com/welcome"   # or "/welcome"
```

The response then carries it beside the token:

```json
{ "token": "…", "verified": true, "redirect_to": "https://app.example.com/welcome" }
```

The dashboard signs the user in and then sends the browser there, so the app is
reached already authenticated. Unset, the key is **absent** from the response
rather than empty, so a client can tell "go here" from "nowhere in particular".

`POST <base>/auth/verify-email/resend` always returns `202`.

### Resetting a password

`POST <base>/auth/password/forgot` with `{"email": "…"}` always returns `202`,
whether or not the address has an account. Distinguishing the two cases would
turn the endpoint into a way of enumerating registered addresses; the account's
owner learns the outcome in their mailbox.

`POST <base>/auth/password/reset` with `{"token": "…", "password": "…"}` sets
the password and returns a session. It also invalidates every *other*
outstanding reset for that account, so requesting several links does not leave
earlier ones usable, and marks the address as confirmed.

### About the tokens

Invitations and both kinds of link carry a random 256-bit token, sent once and
stored **only as a SHA-256 hash**, as API keys are, so a leaked database does
not yield working links. Each token is single-use and expires (see the
`*_ttl_secs` keys in [Configuration](configuration.md#auth)); consuming one
stamps its row, leaving the copy in the mailbox inert.

Expired, already used and never issued all produce the same `404` with identical
wording, so the endpoint cannot be used to probe which tokens existed.

## Presenting credentials

| Header | Meaning |
|--------|---------|
| `Authorization: Bearer <jwt>` | A session token from register/login. |
| `Authorization: ApiKey <key>` | An API key. |
| `X-Api-Key: <key>` | An API key (equivalent; used by the Swagger UI). |

An API key **authenticates as its owning user**, with the same identity,
organisation memberships and permissions. A key therefore inherits everything
that user can do.

## Implementation details

* **Passwords**: argon2id with a random salt (`argon2` crate). Verification is
  constant-time.
* **Sessions**: HMAC-signed JWTs (`jsonwebtoken`) carrying `sub` (user id) and
  `exp`. Signed with `auth.jwt_secret`; lifetime `session_ttl_secs`.
  Memberships are resolved fresh from the database on each request so role/org
  changes take effect immediately.
* **API keys**: random 32-byte tokens prefixed `apik_`, stored as SHA-256 to
  allow indexed lookup, which argon2 would prevent. They are never
  recoverable.

## Roles

Roles are per-organisation. The built-in `membership` resource carries a
member's `role`, so `role:admin` means **admin of the active organisation**.
Create or update memberships through the API to assign roles.

## Acting as somebody else

Support work needs somebody's screen, not their password. Two doors lead to it,
deliberately different sizes:

```toml
# main.toml
[auth]
allow_impersonation = true                            # on by default

[organization]
global_admin_role = "role:admin@org_class=staff"      # nobody, unless named
```

**An organisation's admin** may act as one of its members. The session they get
back is **pinned** to that organisation: the `X-Organization` header cannot move
it, and none of the borrowed account's other memberships are loaded at all — so
somebody who belongs to two organisations cannot have one admin see the other.
That property is what makes this safe to leave on by default.

**The back office** — whoever `[organization] global_admin_role` names, which is
nobody unless an app says so — may act as anyone in any organisation, and
their session is not pinned, because moving around the account's organisations
is what support access is for.

```http
POST /api/auth/impersonate        (an admin of the active organisation,
{ "user_id": "…" }                 or a global admin)

POST /api/auth/impersonate/stop   (a session that is acting as somebody)
```

Both answer with a session: `{ "token", "user_id", "impersonator",
"organization_id" }`, where `organization_id` is the pin and is `null` for an
unpinned one. `GET /auth/me` reports the same three facts, so a page that has
been reloaded still knows whose account it is holding.

Some things that follow, none of them optional:

* **Impersonation does not nest.** A borrowed session cannot borrow again
  (`409`), so the actor named in a token is always a real person.
* **Stopping needs no stored credential.** The way back is minted from the
  borrowed token's own actor claim.
* **A borrowed session is never a back office.** `global_admin_role` is
  answered `false` while impersonating, whoever is behind it — otherwise somebody could wear another name and keep their own
  powers, which is the one arrangement no audit trail can untangle.
* **Nothing else changes.** Every permission is then answered against the
  borrowed account exactly as it would be for its owner.

Neither endpoint is mounted when both doors are shut, so an app that wants
none of this has none of it to probe. In the [dashboard](admin.md), **Act as**
appears on the team screen beside each member an admin may borrow — and, for a
global admin, on every row of the `user` list as well as on the record, since
they may reach people they share no organisation with whichever organisation
they are standing in. A strip across the top of every screen says whose account is
in use and holds the way out. The [console](cli.md#acting-as-somebody-else) has
the same two doors on `I`, from the Team screen and from the `user` table, with
a banner in the header and the way out on the Session screen.

## Signing in with somebody else's account

Two credentials per provider, and nothing else:

```toml
# main.toml
[oauth.github]
client_id     = "${GITHUB_CLIENT_ID}"
client_secret = "${GITHUB_CLIENT_SECRET}"

[oauth.google]
client_id     = "${GOOGLE_CLIENT_ID}"
client_secret = "${GOOGLE_CLIENT_SECRET}"
```

apiplant ships **GitHub, Google, LinkedIn and X**, and knows for each of them
the authorize URL, the token URL, where the profile lives, which scopes reach a
verified address, whether it wants PKCE and whether it insists on the client
secret as HTTP Basic — four things they disagree about, and the reason a
handshake written once by hand is rarely written twice.

Naming a provider mounts the endpoints below and adds the `oauth_state`
[resource](resources.md). Naming none leaves an app with neither.

| Method | Path | Purpose |
|--------|------|---------|
| `GET` | `<base>/auth/oauth` | the providers this deployment offers, each with the URL that starts it |
| `GET` | `<base>/auth/oauth/{provider}/start` | **302** to the provider's consent screen |
| `POST` | `<base>/auth/oauth/{provider}/start` | the same as JSON — and, with a session, this is how an account **links** a provider |
| `GET` | `<base>/auth/oauth/{provider}/callback` | the redirect URI to register; finishes the sign-in |
| `POST` | `<base>/auth/oauth/{provider}/callback` | the same from a body, for a front end with its own callback route |
| `DELETE` | `<base>/auth/oauth/{provider}` | unlink, unless it is the last way into the account |

### The redirect URI

Derived, not configured:

```
<public_url><base_path>/auth/oauth/<provider>/callback
```

`apiplant run` prints the exact string for each provider on the way up — copy it
into the provider's dashboard, which compares it byte for byte. Getting
`[server] public_url` wrong behind a proxy or a tunnel is the single most
common OAuth failure, and it is the only value that has to be right.

### The dashboard already has it

The built-in [admin dashboard](admin.md) reads `[oauth]` from the manifest and
draws a button per provider — with its own mark — above the password form on
both the sign-in and create-account tabs, and grows a *Linked accounts* card on
*Your account* for connecting and disconnecting. There is nothing to switch on:
configuring a provider is what puts it there.

### The client side, in full

```html
<a href="/api/auth/oauth/github/start">Sign in with GitHub</a>
```

That is a working sign-in. The start endpoint redirects to the provider, the
provider redirects back into the API, and the API sends the browser to
`[oauth] success_redirect` with a session token — in the URL fragment by
default, since a fragment never reaches a server and so stays out of access
logs and `Referer` headers. A single-page app that would rather hold the
browser itself uses the `POST` pair and gets the same handshake as JSON.

The token is the same HS256 JWT `POST <base>/auth/login` issues, signed with the
same `auth.jwt_secret` and carrying the same claims. Every permission, hook and
`owner`-scoped query accepts it without knowing it came from GitHub.

### Whose account is this?

The one decision worth reading carefully. In order:

1. **A connection already exists** — somebody signing in again. Keyed on the
   provider's immutable id, never a username or an address, so changing either
   changes nothing.
2. **The flow was started from a session** — "connect my GitHub". Recorded when
   the flow began, while the caller held the credential that proves whose
   account it is, and never read from the callback.
3. **A verified address matches an existing account** — the "registered with a
   password, came back through Google" case. Governed by
   `[oauth] link_by_verified_email`.
4. **Otherwise a new account**, created through the same path
   `POST <base>/auth/register` uses: the `user` resource's `before_register`,
   `before_create`, `after_create` and `after_register` hooks all fire, and the
   account gets its own organisation. An OAuth sign-up is not a second kind of
   registration with a second set of rules.

Step 3 is safe **only** because the provider says it *verified* the address. An
unverified one is never matched — if it were, anybody could set their address at
a careless provider to somebody else's and sign in as them, which is how several
real "sign in with" takeovers worked. A match that cannot be made automatically
is refused with an answer that says what to do instead: sign in the way you
already can, then link the provider from that session.

`allow_registration = false` closes step 4 too. A provider button is not a side
door into an app that has closed signup — but it still signs in the people who
are already there.

### Two more things it takes care of

* **A provider with no email address.** X releases none to most apps, so those
  accounts get a placeholder at `oauth.invalid` — a domain RFC 2606 reserves so
  it can never resolve — and `email_placeholder` set, so "there is a string in
  the email column" stops meaning "we can write to this person". An app with
  `require_email_verification` on refuses such a provider at the door instead,
  since an account it created could never sign in.
* **Locking somebody out.** Unlinking the last connection from an account with
  no password would make it permanently unreachable, so `DELETE
  <base>/auth/oauth/{provider}` refuses that one and says so.

### What lands on the account

Three columns, all of them in the built-in `user` resource, so a sign-in fills them
in an app with no `resources/users.toml` at all:

| Field | Filled with |
|-------|-------------|
| `display_name` | the provider's name for them |
| `avatar_url` | their picture |
| `email_placeholder` | `true` when apiplant invented the address rather than being given one |

The first two are written on **every** sign-in, not only the first: people
change their name and their picture, and a copy that is only right on the day
the account was created is worse than none. Which columns they are is
configurable:

```toml
[oauth]
name_field   = "display_name"   # "" to write no name
avatar_field = "avatar_url"     # "" to write no picture
```

A resource that does not declare a column simply does not get it filled, so
removing one from `resources/users.toml` is a complete way to opt out.

`email_placeholder` is not configurable, and is in the built-in resource rather
than left to apps, because apiplant is the one *inventing* the value it
describes. An app that has never heard of the flag is exactly the app that would
otherwise mail an address the framework made up. It is also the one field here
the framework reads: nothing is sent to an address at a `.invalid` domain, so a
password reset or a confirmation for such an account is quietly not attempted
rather than handed to a provider that cannot deliver it.

The provider's own view of somebody — address, verified flag, name, picture,
last sign-in — is kept on their `oauth_connection` row regardless. That resource
is live and ordinary: `GET <base>/oauth_connection` is "my linked accounts",
`owner`-scoped, so the question needs no filter and cannot answer anybody
else's. It is what the dashboard's *Linked accounts* card reads.

Its `delete` is `private`, though, and deliberately: `DELETE
<base>/auth/oauth/{provider}` is the way to unlink, because that is the one that
knows an account with no password and no other provider must keep this one. A
`delete = "owner"` on the resource would be a second door beside that check with
nothing behind it but the same row.

### A provider apiplant does not ship

Three URLs and the scopes it wants. Anything speaking OpenID Connect — which is
most things — needs no code:

```toml
[oauth.gitlab]
client_id     = "${GITLAB_CLIENT_ID}"
client_secret = "${GITLAB_CLIENT_SECRET}"
authorize_url = "https://gitlab.com/oauth/authorize"
token_url     = "https://gitlab.com/oauth/token"
userinfo_url  = "https://gitlab.com/oauth/userinfo"
scopes        = "openid email profile"
icon          = "/oauth/gitlab.svg"   # a file in public/
```

`icon` is the only line about appearance. apiplant draws GitHub, Google,
LinkedIn and X itself — those are trademarks, drawn to their owners'
guidelines, and a configured icon deliberately cannot replace them — while any
other provider's button shows its initial on a plain tile until an image is
named. [Super Tiny Icons](https://github.com/edent/SuperTinyIcons) is where to
get one: several hundred brand marks, each a few hundred bytes of hand-drawn
SVG, MIT licensed, and the source apiplant's own four are drawn from. Save the
file into `public/` and point `icon` at it; both the sign-in page you write and
the [dashboard](admin.md) will use it.

### What it does not do

* **Refresh tokens.** This signs people in; it never acts on their behalf
  afterwards, so the access token is used once to read a profile and dropped.
  An app that needs to keep calling a provider adds columns for them itself and
  encrypts them at rest.
* **`id_token` verification.** The profile is read from the userinfo endpoint
  over a fresh TLS connection instead, which needs no signature check. An
  `id_token` that arrived any other way would need full JWKS verification.
* **Cookies.** Sessions are Bearer tokens here as everywhere else.

Every setting is in [Configuration](configuration.md#oauth);
[`examples/22-oauth`](../examples/22-oauth) is the whole thing running.

## Extending the user resource

Because `user` is an ordinary resource, you can:

* add profile fields (`[fields.bio]`, `[fields.avatar_url]`, …),
* change its permissions (who may list/read/update users),
* switch the login identifier (`identity_field = "username"`),
* relate it to other resources with `reference` fields,
* hook the auth endpoints themselves with [auth hooks](#auth-hooks).

Create `resources/users.toml` to replace the default; authentication remains
wired up.
