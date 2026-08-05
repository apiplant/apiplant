# Security model

This page describes what the framework enforces on its own, what it delegates to
the application, and what must be decided before a deployment is exposed.
Everything here is enforced **server-side, on every request**; the dashboard's
own rules affect presentation only and are never the mechanism of enforcement.

## What the server guarantees

| Guarantee | Where |
|-----------|-------|
| Every action is checked against the resource's `[permissions]` policy before the database is touched. | [Permissions](permissions.md) |
| Org-scoped resources require membership in the active organisation and filter every query to it. | [Multitenancy](multitenancy.md) |
| `organization_id`, the owner column and the password column are stamped by the server and stripped from any request body, on create and update. | [API reference](api-reference.md#server-owned-columns) |
| `?expand=` runs the *target* resource's `read` policy; an unreadable relation inlines as `null`. | [Relationships](relationships.md) |
| `hidden` fields are stripped from responses and refused as filters. | [Resources](resources.md) |
| Passwords are argon2id-hashed with a per-password random salt; the plaintext is never stored, and a login for an unknown identity costs the same as one for a wrong password. | [Authentication](authentication.md) |
| API keys are stored as SHA-256 of a 256-bit random token; the plaintext is shown once, at creation. | [Authentication](authentication.md) |
| Organisation memberships and roles are read from the database per request, never carried in the token, so a revoked role takes effect immediately. | |
| Every value reaches Postgres as a bind parameter; only validated, double-quoted identifiers are ever interpolated into SQL. | [`apiplant-db`](../crates/apiplant-db/src/lib.rs) |
| Static files resolve under `public/` only; a path that would escape the root is refused rather than served. | [Configuration](configuration.md) |
| An OAuth sign-in is bound to a `state` this server issued, stored as a SHA-256 hash and spendable once, with PKCE wherever the provider supports it; and a provider account reaches an existing user only through an address that provider says it **verified**. | [Authentication](authentication.md#signing-in-with-somebody-elses-account) |

A `before_*` hook is subject to the same rules as the caller: the body it
returns goes through the same stripping, so a hook cannot spoof a tenant, an
owner or a password hash. See [Hooks](hooks.md).

## What you have to decide

### `auth.jwt_secret`

If left empty, the server generates an ephemeral secret at boot, logs a warning,
and invalidates every session on restart. That is acceptable for development but
not for production. Set it from your secret store:

```toml
[auth]
jwt_secret = "…at least 32 random bytes…"
session_ttl_secs = 604800   # one week; shorten for sensitive sessions
```

Rotating the secret invalidates every outstanding session, which is also how to
force all users to sign in again.

Write it as `"$JWT_SECRET"` and it is read from the environment at boot, so the
committed `main.toml` holds no secret. Every string in every app TOML file works
the same way; see [Configuration → Environment
variables](configuration.md#environment-variables).

### OAuth credentials, and the URL they are registered against

`[oauth]` is disabled unless a provider block carries a `client_id`, so an app
that signs nobody in this way is unaffected. If you do configure it:

* keep both credentials in the environment, as above
  (`client_secret = "$GITHUB_CLIENT_SECRET"`). The secret leaves this process in
  exactly one request — the token exchange, server to provider — and is never in
  a URL, a redirect or a page;
* **`[server] public_url` is security-relevant here.** The redirect URI
  registered with each provider is derived from it, and a provider will only
  send a code to a URI that matches. Getting it wrong breaks sign-in; pointing
  it at a host you do not control would be worse;
* `token_delivery = "query"` puts a session token in a URL, where proxies,
  access logs and `Referer` headers can record it. The default, `fragment`,
  does not — prefer it unless a server-rendered page has to read the token;
* `link_by_verified_email = false` is the stricter setting, not the safer one
  in every sense: it refuses an automatic match that would have been correct.
  Neither value ever matches an *unverified* address.

### Email and cache credentials

`[email]` and `[cache]` are disabled unless configured, so an app that uses
neither is unaffected by this section. If you do configure them:

* keep the provider key and the Redis URL in the environment, as above
  (`api_key = "$SENDGRID_API_KEY"`);
* `[email.smtp] encryption = "none"` sends credentials and messages in
  cleartext and is logged as a warning at boot; it is intended only for a relay
  on localhost;
* the cache holds whatever a function stores there, under a shared prefix. Do
  not store session tokens, password hashes or any data whose loss would matter:
  Redis is usually unauthenticated on a private network, and cached data is
  disposable by definition.

### TLS

Place a certificate and key in `https/` and the server serves HTTPS instead of
HTTP; see [Configuration](configuration.md). Terminating TLS in front of
apiplant, at a load balancer or reverse proxy, is equally valid. What is not
acceptable is serving plain HTTP over an untrusted network, since session tokens
and API keys travel in headers.

### The admin dashboard

The dashboard ships in the binary and is served at `/admin/` for every app. It
is an operator console rather than a second API: it calls the same endpoints
with the same credentials and can do nothing a signed-in caller could not
already do. It does, however, expose the structure of your app to anyone who
loads it. Disable it for a deployment that should not:

```toml
[admin]
enabled = false
```

### Registration

`auth.allow_registration = false` disables self-service signup, covering both
`POST <base>/auth/register` and anonymous `POST <base>/user`. Accounts can then
be created only by an authenticated caller with `create` on `user`, or by a
hook.

### Database credentials

`[database]` defaults to `postgres:postgres@localhost`, which exists to make the
first run work. In any other environment, point `url` at a real connection
string using a role that owns only this database, since the framework runs DDL
at boot when `auto_migrate` is enabled.

### Functions are trusted code

A function is a shared library loaded into the server process. It runs with the
server's privileges, and its host bridge can execute arbitrary SQL against the
app's database by design. Treat `functions/` like the rest of your source code:
review it, build it yourself, and never load a library you did not produce.
Functions are *not* a sandbox.

## Reporting a vulnerability

Open a security advisory on the [repository](https://github.com/apiplant/apiplant)
rather than a public issue.
