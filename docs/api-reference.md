# API reference

Conventions shared by every generated endpoint. `<base>` is `server.base_path`
(empty by default, e.g. `/api`). A live, always-accurate version of this is the
generated [OpenAPI spec](openapi.md) at `<base>/openapi.json`.

## CRUD endpoints

For a resource named `post`:

| Method | Path | Action | Success | Body |
|--------|------|--------|---------|------|
| GET | `<base>/post` | list | `200` array | — |
| POST | `<base>/post` | create | `201` object | JSON object |
| GET | `<base>/post/{id}` | read | `200` object | — |
| PATCH | `<base>/post/{id}` | update | `200` object | JSON object (partial) |
| PUT | `<base>/post/{id}` | update | `200` object | JSON object (partial) |
| DELETE | `<base>/post/{id}` | delete | `204` no content | — |
| GET | `<base>/post/{id}/{child}` | nested list (has_many) | `200` array | — |

`{id}` is a UUID. `PATCH` and `PUT` behave identically (partial update of the
supplied fields). Each is gated by the resource's [permissions](permissions.md).

## Query parameters (list & nested list)

| Parameter | Example | Effect |
|-----------|---------|--------|
| `limit` | `?limit=100` | Page size, clamped to `1..=500` (default `50`). |
| `offset` | `?offset=50` | Rows to skip (default `0`). |
| `expand` | `?expand=owner,post` | Inline `belongs_to` [relations](relationships.md). |
| `<field>` | `?published=true` | Exact-match filter on any visible column. |
| `<field>~` | `?title~=depot` | Case-insensitive **substring** match on a `string` or `text` column — what a search box means. |
| `via` | `?via=sender_id` | (nested only) pick the reference field when ambiguous. |

Results are ordered newest-first by `created_at` when the resource has
timestamps. `expand` also works on the single read endpoint.

## Request/response shape

* Rows are flat JSON objects: `id`, your fields, and `created_at`/`updated_at`.
* `hidden` fields never appear.
* Timestamps are RFC 3339 strings; ids and references are UUID strings.
* On create, server-managed columns (`id`, timestamps, owner column, and on
  org-scoped resources `organization_id`) are set for you.
* Server-managed columns are **not writable**, on create or on update. A body
  that carries `organization_id`, the owner column or the password column has
  those keys dropped before the statement is built — the rest of the body is
  applied as normal. See [Server-owned columns](#server-owned-columns).
* `hidden` fields are not filterable either: `?password_hash=…` is ignored
  rather than answered, so a list endpoint cannot be used to probe a value it
  refuses to return. `~` is no way around that — `?password_hash~=$argon` is
  ignored the same way.
* `~` is a separate spelling rather than a change to `?field=`, because a
  filter that quietly matched substrings would surprise everything that relies
  on it. `%` and `_` in the term are matched literally, not as wildcards, and
  `~` on a non-text column is a `400` rather than a search of its text
  rendering.

## Server-owned columns

Three groups of columns are decided by the server and ignored when a client
sends them:

| Column | Why |
|--------|-----|
| `organization_id` (org-scoped resources) | The tenant is stamped from the active organisation. Accepting it on update would let a caller *move* a row into an organisation they are not in — the `WHERE` clause proves only that they may touch the row where it is now. |
| The resource's `owner_field` | Stamped from the caller, so ownership cannot be assigned or given away. |
| The `user` model's `password_field` | Holds an argon2 hash. The only endpoint that writes it is `POST <base>/auth/register`, which hashes a plaintext first. |

This applies to hook replacements too: a `before_create`/`before_update` hook
that returns one of these keys has it dropped the same way, so a hook cannot
spoof the tenant or the owner either.

## Authentication

| Header | Meaning |
|--------|---------|
| `Authorization: Bearer <jwt>` | Session token. |
| `Authorization: ApiKey <key>` | API key (acts as its owning user). |
| `X-Api-Key: <key>` | API key (equivalent). |

See [Authentication](authentication.md).

## Auth endpoints

| Method | Path | Purpose |
|--------|------|---------|
| POST | `<base>/auth/register` | create user → `{token, user}` |
| POST | `<base>/auth/login` | credentials → `{token}` |
| POST | `<base>/auth/apikeys` | issue key → `{api_key, id}` |

## Function endpoints

| Method | Path | Notes |
|--------|------|-------|
| *(manifest method)* | `<base>/functions/<name>` | Body forwarded to the function; JSON returned. |

See [Functions](functions.md).

## Lifecycle hooks

A resource with a `[hooks]` section runs functions around its CRUD operations,
which can change what these endpoints do:

* a `before_*` hook may reject the request with **any** `4xx`/`5xx` status it
  chooses (`400` when it returns a plain error), so an endpoint can answer with
  codes not listed below;
* a `before_create`/`before_update` hook may rewrite the body that gets stored;
* an `after_*` hook may replace the response body — including turning the usual
  `204` from `DELETE` into a `200` with content;
* a hook naming a function that isn't loaded makes its operation fail with
  `500`, and nothing is written.

See [Lifecycle hooks](hooks.md).

## Utility endpoints

| Method | Path | Purpose |
|--------|------|---------|
| GET | `<base>/_health` | `{"status":"ok","framework":"apiplant"}` |
| GET | `<base>/openapi.json` | generated OpenAPI 3.0 spec (if docs enabled) |
| GET | `<base>/docs` | Swagger UI (if docs enabled) |

## Status codes

| Code | When |
|------|------|
| `200` | successful read/list/update |
| `201` | successful create |
| `204` | successful delete |
| `400` | invalid body, bad filter/id, unknown-record reference (FK), missing required field |
| `401` | authentication required (anonymous caller hit a protected action) |
| `403` | authenticated but not permitted (wrong role, etc.) |
| `404` | resource/route/row not found (also: a `private` action, or an owned row you don't own) |
| `405` | wrong HTTP method for a function |
| `409` | uniqueness conflict |
| `500` | unexpected server/database error, or a declared hook whose function isn't loaded |

Errors are JSON: `{ "error": "<message>" }`. A [hook](hooks.md) can return any
status in `400..=599` with its own message.

## Domain / host filtering

When `server.domain` is set, requests whose `Host` header doesn't match get no
route (effectively `404`). Leave it unset to answer any host.
