# 05 · Authentication

`user` and `api_key` are ordinary resources with built-in defaults — so you
change how authentication works by **redefining them**, not by configuring a
black box.

```
05-auth/
├── main.toml
└── models/
    ├── users.toml      # replaces the built-in `user`: log in with a username
    └── api_keys.toml   # extends `api_key` with a name and last_used_at
```

A model named `user` replaces the default one, and the framework keeps using it
for login, ownership, API keys and organisation membership.

## Run it

```bash
createdb -h 127.0.0.1 -p 55432 -U postgres apiplant_auth
cargo run -p apiplant -- examples/05-auth
```

**Register** — `identity_field = "username"`, so that's what registration takes:

```bash
curl -s -XPOST localhost:8099/api/auth/register -H 'content-type: application/json' \
  -d '{"username":"ada","password":"hunter2","display_name":"Ada L."}'
# → {"token":"…","user":{"id":"…","username":"ada","display_name":"Ada L."}}
```

Note what's missing from the response: `password_hash` is `hidden`, so it never
leaves the server. It's stored as an argon2 hash.

**Log in** for a fresh JWT:

```bash
TOKEN=$(curl -s -XPOST localhost:8099/api/auth/login -H 'content-type: application/json' \
  -d '{"username":"ada","password":"hunter2"}' | jq -r .token)
```

**Issue an API key** — the plaintext is shown exactly once; only its hash is kept:

```bash
KEY=$(curl -s -XPOST localhost:8099/api/auth/apikeys \
  -H "authorization: Bearer $TOKEN" -H 'content-type: application/json' \
  -d '{"name":"ci"}' | jq -r .api_key)
```

## Three ways to identify a caller

All equivalent; an API key acts as its owning user:

```bash
curl -s localhost:8099/api/user -H "authorization: Bearer $TOKEN"
curl -s localhost:8099/api/user -H "authorization: ApiKey $KEY"
curl -s localhost:8099/api/user -H "x-api-key: $KEY"
```

## Things worth trying

* `update = "owner"` on `user` — you can `PATCH` your own row, but someone
  else's returns `404` (an owned row you don't own is invisible, not forbidden).
* Set `allow_registration = false` under `[auth]` in `main.toml` and registration
  starts returning `403`, while login keeps working — handy for invite-only apps.
* Add a field to `models/users.toml`, restart, and it appears on the resource;
  existing rows keep working.

## OAuth

`oauth_connection` is a built-in resource that models linked third-party
identities, and `auth.oauth_providers` lists which providers a deployment
accepts. **The provider redirect/callback handshake is not implemented yet** —
this is scaffolding. Password and API-key auth are complete.

Details in [Authentication](../../docs/authentication.md).

**Next:** [06 · Permissions](../06-permissions) decides who may do what.
