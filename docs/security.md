# Security model

This page states what the framework enforces on its own, what it hands to you,
and what you have to decide before a deployment is exposed. Everything here is
enforced **server-side, on every request**; the dashboard's own rules are
presentation and are never what keeps anyone out.

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
| Organisation memberships and roles are read from the database per request, never carried in the token, so a revoked role takes effect immediately. | — |
| Every value reaches Postgres as a bind parameter; only validated, double-quoted identifiers are ever interpolated into SQL. | [`apiplant-db`](../crates/apiplant-db/src/lib.rs) |
| Static files resolve under `public/` only — a path that would escape the root is refused, not served. | [Configuration](configuration.md) |

A `before_*` hook is subject to the same rules as the caller: the body it
returns goes through the same stripping, so a hook cannot spoof a tenant, an
owner or a password hash. See [Hooks](hooks.md).

## What you have to decide

### `auth.jwt_secret`

Leave it empty and the server generates an ephemeral secret at boot, warns
about it, and every session dies at the next restart — fine for development,
wrong for production. Set it from your secret store:

```toml
[auth]
jwt_secret = "…at least 32 random bytes…"
session_ttl_secs = 604800   # a week; shorten it if sessions are sensitive
```

Rotating the secret invalidates every outstanding session, which is also the way
to force everyone to sign in again.

### TLS

Drop a certificate and key in `https/` and the server serves HTTPS instead of
HTTP — see [Configuration](configuration.md). Terminating TLS in front of
apiplant (a load balancer, a reverse proxy) is equally fine; what is not fine is
neither, since session tokens and API keys travel in headers.

### The admin dashboard

It ships in the binary and is served at `/admin/` for every app. It is an
operator console, not a second API: it talks to the same endpoints with the
same credentials and can do nothing a signed-in caller could not already do.
Still, it advertises the shape of your app to anyone who loads it. Turn it off
for a deployment that should not expose one:

```toml
[admin]
enabled = false
```

### Registration

`auth.allow_registration = false` closes self-service signup — both
`POST <base>/auth/register` and anonymous `POST <base>/user`. Accounts then come
only from an authenticated caller with `create` on `user`, or from a hook.

### Database credentials

`[database]` defaults to `postgres:postgres@localhost`, which exists to make the
first run work. Point `url` at a real connection string in anything else, with a
role that owns only this database — the framework runs DDL at boot when
`auto_migrate` is on.

### Functions are trusted code

A function is a shared library loaded into the server process. It runs with the
server's privileges and its host bridge can execute arbitrary SQL against the
app's database — deliberately, since that is what makes a function useful. Treat
`functions/` exactly like the rest of your source: review it, build it yourself,
and never load a library you did not produce. Functions are *not* a sandbox.

## Reporting a vulnerability

Open a security advisory on the [repository](https://github.com/apiplant/apiplant)
rather than a public issue.
