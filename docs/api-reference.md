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
| `<field>` | `?published=true` | Exact-match filter on any column. |
| `via` | `?via=sender_id` | (nested only) pick the reference field when ambiguous. |

Results are ordered newest-first by `created_at` when the resource has
timestamps. `expand` also works on the single read endpoint.

## Request/response shape

* Rows are flat JSON objects: `id`, your fields, and `created_at`/`updated_at`.
* `hidden` fields never appear.
* Timestamps are RFC 3339 strings; ids and references are UUID strings.
* On create, server-managed columns (`id`, timestamps, owner column) are set for
  you; supplying them is unnecessary (owner is always overwritten).

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
| `500` | unexpected server/database error |

Errors are JSON: `{ "error": "<message>" }`.

## Domain / host filtering

When `server.domain` is set, requests whose `Host` header doesn't match get no
route (effectively `404`). Leave it unset to answer any host.
