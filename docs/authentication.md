# Authentication & authorization

apiplant ships a complete identity system built out of ordinary resources.
`organization`, `membership`, `user`, `api_key`, and `oauth_connection` exist by
default, and any of them can be extended by adding a same-named file in
`models/`.

## Built-in resources

| Resource | Purpose | Key fields |
|----------|---------|-----------|
| `organization` | tenants | `name`, `slug` (unique) |
| `membership` | joins users to organisations, carrying a per-org role | `user_id → user`, `organization_id → organization`, `role` |
| `user` | accounts | `email` (unique), `password_hash` (hidden), `display_name` |
| `api_key` | long-lived credentials | `name`, `token_hash` (hidden, unique), `owner_id → user` |
| `oauth_connection` | linked third-party identities | `provider`, `provider_user_id`, `owner_id → user` |
| `invitation` | a pending invitation to an organisation, for an address that may not yet have an account | `email`, `role`, `token_hash` (hidden, unique), `organization_id`, `expires_at`, `accepted_at` |
| `auth_token` | single-use tokens sent by email | `user_id → user`, `kind`, `token_hash` (hidden, unique), `expires_at`, `used_at` |

These are real resources: they get tables, CRUD endpoints (gated by their own
permissions), migrations and relationships like any other.

## The `[auth]` section

The `user` model carries an optional `[auth]` block controlling how login works:

```toml
# models/users.toml
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

Without a `models/users.toml`, the defaults are `identity_field = "email"`,
`password_field = "password_hash"`, no OAuth providers and no hooks.

### Auth hooks

The endpoints below are extension points, not fixed behaviour: the `user`
model's ordinary `[hooks]` section carries an event for each point in their
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
* Honours `auth.allow_registration` (`403` when disabled). The `user` model
  ships with `create = "public"` so that this endpoint works, so the same switch
  also closes anonymous `POST <base>/user`; otherwise signup would simply move
  to that route.
* Returns `{ "token": "...", "user": { ... } }` (with hidden fields stripped).
* The new account is given a [personal organisation](multitenancy.md) it
  administers, so no session begins without an organisation to work in.
* Registering is a `create` on the `user` resource, so the `user` model's
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

Permission to issue one is read from the **`membership` model's `create`
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
model's `before_register`, `before_create`, `after_create` and `after_register`
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

## OAuth

The `oauth_connection` resource and the `auth.oauth_providers` list model
third-party identities (a `provider` and `provider_user_id` linked to a user).
**The provider redirect and callback handshake is not implemented yet**; this is
the scaffolding for it. Password and API-key authentication are fully
functional.

## Extending the user model

Because `user` is an ordinary resource, you can:

* add profile fields (`[fields.bio]`, `[fields.avatar_url]`, …),
* change its permissions (who may list/read/update users),
* switch the login identifier (`identity_field = "username"`),
* relate it to other resources with `reference` fields,
* hook the auth endpoints themselves with [auth hooks](#auth-hooks).

Create `models/users.toml` to replace the default; authentication remains
wired up.
