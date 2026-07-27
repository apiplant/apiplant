# Authentication & authorization

apiplant ships a complete identity system built out of ordinary resources.
`organization`, `membership`, `user`, `api_key`, and `oauth_connection` exist by
default; you can extend any of them by dropping a same-named file in `models/`.

## Built-in resources

| Resource | Purpose | Key fields |
|----------|---------|-----------|
| `organization` | tenants | `name`, `slug` (unique) |
| `membership` | joins users to organisations, carrying a per-org role | `user_id → user`, `organization_id → organization`, `role` |
| `user` | accounts | `email` (unique), `password_hash` (hidden), `display_name` |
| `api_key` | long-lived credentials | `name`, `token_hash` (hidden, unique), `owner_id → user` |
| `oauth_connection` | linked third-party identities | `provider`, `provider_user_id`, `owner_id → user` |

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

Defaults if you don't provide `models/users.toml`: `identity_field = "email"`,
`password_field = "password_hash"`, no OAuth providers.

## Endpoints

| Method | Path | Purpose |
|--------|------|---------|
| `POST` | `<base>/auth/register` | Create a user, return a session token |
| `POST` | `<base>/auth/login` | Exchange credentials for a session token |
| `GET` | `<base>/auth/me` | Check the caller's credential is valid and their user still exists |
| `POST` | `<base>/auth/apikeys` | Issue an API key for the caller |

### Register

```http
POST /api/auth/register
{ "email": "a@b.com", "password": "hunter2", "display_name": "Ann" }
```

* Requires a `password`; all other properties map to `user` fields.
* The password is argon2id-hashed into the `password_field`; the plaintext is
  never stored.
* Honours `auth.allow_registration` (`403` when disabled). The `user` model
  ships with `create = "public"` so this endpoint works, so the same switch also
  closes anonymous `POST <base>/user` — otherwise signup would just move.
* Returns `{ "token": "...", "user": { ... } }` (with hidden fields stripped).
* Registering is a `create` on the `user` resource, so the `user` model's
  `before_create` / `after_create` [hooks](hooks.md#registration-fires-the-user-create-hooks)
  fire here too — the usual place to give a new account a starting
  organisation, as [`examples/14-email-domains`](../examples/14-email-domains)
  does.

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
names still exists; `401` when either fails. A signature check alone cannot tell
you the second — a token signed before the user was deleted still verifies — so
a client holding a stored token has one call to ask whether to keep it. The
admin dashboard calls this on load and discards the token on a `401`.

### Issue an API key

```http
POST /api/auth/apikeys       (authenticated)
{ "name": "ci-server" }
```

Returns `{ "api_key": "apik_…", "id": "…" }`. **The plaintext key is shown
once** — only its SHA-256 is stored (`token_hash`). Delete a key by removing its
`api_key` row.

## Presenting credentials

| Header | Meaning |
|--------|---------|
| `Authorization: Bearer <jwt>` | A session token from register/login. |
| `Authorization: ApiKey <key>` | An API key. |
| `X-Api-Key: <key>` | An API key (equivalent; used by the Swagger UI). |

An API key **authenticates as its owning user** — same identity, same
organisation memberships, same permissions. This means a key inherits everything
the user can do.

## How it works under the hood

* **Passwords**: argon2id with a random salt (`argon2` crate). Verification is
  constant-time.
* **Sessions**: HMAC-signed JWTs (`jsonwebtoken`) carrying `sub` (user id) and
  `exp`. Signed with `auth.jwt_secret`; lifetime `session_ttl_secs`.
  Memberships are resolved fresh from the database on each request so role/org
  changes take effect immediately.
* **API keys**: random 32-byte tokens prefixed `apik_`, stored as SHA-256 for
  O(1) indexed lookup (argon2 would prevent lookup). Never recoverable.

## Roles

Roles are per-organisation. The built-in `membership` resource carries a
member's `role`, so `role:admin` means **admin of the active organisation**.
Create or update memberships through the API to assign roles.

## OAuth

The `oauth_connection` resource and the `auth.oauth_providers` list model
third-party identities (a `provider` + `provider_user_id` linked to a user).
**The provider redirect/callback handshake is not implemented yet** — this is the
scaffolding for it. Password and API-key auth are fully functional today.

## Extending the user model

Because `user` is a normal resource, you can:

* add profile fields (`[fields.bio]`, `[fields.avatar_url]`, …),
* change its permissions (who may list/read/update users),
* switch the login identifier (`identity_field = "username"`),
* relate it to other resources with `reference` fields.

Just create `models/users.toml`; it replaces the default while keeping auth
wired up.
