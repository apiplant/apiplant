# API reference

Conventions shared by every generated endpoint. `<base>` is `server.base_path`,
which is empty by default and often set to `/api`. The generated
[OpenAPI spec](openapi.md) at `<base>/openapi.json` is the authoritative,
always-current version of this reference.

## CRUD endpoints

For a resource named `post`:

| Method | Path | Action | Success | Body |
|--------|------|--------|---------|------|
| GET | `<base>/post` | list | `200` array | none |
| POST | `<base>/post` | create | `201` object | JSON object |
| GET | `<base>/post/{id}` | read | `200` object | none |
| PATCH | `<base>/post/{id}` | update | `200` object | JSON object (partial) |
| PUT | `<base>/post/{id}` | update | `200` object | JSON object (partial) |
| DELETE | `<base>/post/{id}` | delete | `204` no content | none |
| GET | `<base>/post/{id}/{child}` | nested list (has_many) | `200` array | none |

`{id}` is a UUID. `PATCH` and `PUT` behave identically, applying a partial
update of the supplied fields. Each is gated by the resource's
[permissions](permissions.md).

## Query parameters (list & nested list)

| Parameter | Example | Effect |
|-----------|---------|--------|
| `limit` | `?limit=100` | Page size, clamped to `1..=500` (default `50`). |
| `offset` | `?offset=50` | Rows to skip (default `0`). |
| `expand` | `?expand=owner,post` | Inline `belongs_to` [relations](relationships.md). |
| `<field>` | `?published=true` | Exact-match filter on any visible column. |
| `<field>~` | `?title~=depot` | Case-insensitive **substring** match on a `string` or `text` column, as a search box would perform. |
| `search` | `?search=depot` | The same match, across every field the resource's [`[admin] search_fields`](admin.md#admin-on-a-resource) names. |
| `search_fields` | `?search=depot&search_fields=title,body` | Search these columns instead of the configured ones. |
| `order` | `?order=-created_at,title` | Sort keys, applied left to right. |
| `via` | `?via=sender_id` | Nested lists only: selects the reference field when more than one applies. |

Results are ordered newest first by `created_at` when the resource has
timestamps and `order` is not specified. `expand` also applies to the single
read endpoint.

### Ordering

`?order=` takes one or more column names separated by commas. A leading `-`, or
a `:desc` suffix such as `?order=title:desc`, reverses that key; empty values
sort last in both directions. Sortable columns are `id`, the timestamps, and any
visible field:

```
?order=status,-created_at      # status A→Z, newest first within each group
```

A column that does not exist, or that is `hidden`, returns `400` rather than
being silently dropped, since returning a page in an unrequested order without
indicating so is worse than rejecting the request.

### Searching several fields at once

`?field~=` searches one column. `?search=` searches the set declared by the
model, so a search box needs no knowledge of the underlying fields:

```toml
# models/order.toml
[admin]
search_fields = ["reference", "customer_note"]
```

```
?search=depot        # reference ILIKE %depot% OR customer_note ILIKE %depot%
```

When undeclared, the set is the single `search_field`, which itself defaults to
the `display_field`, so behaviour is unchanged for a resource that does not
configure one. A caller with knowledge of the model can override the set per
request with `?search_fields=`, an API-only refinement; the dashboard always
uses the configured set. Both forms follow the same rules as `~`: only `string`
and `text` columns, never a `hidden` one, `%` and `_` matched literally, and
anything else returns `400`.

## Request/response shape

* Rows are flat JSON objects: `id`, your fields, and `created_at`/`updated_at`.
* `hidden` fields never appear.
* Timestamps are RFC 3339 strings; ids and references are UUID strings.
* On create, server-managed columns (`id`, the timestamps, the owner column,
  and `organization_id` on org-scoped resources) are set automatically.
* Server-managed columns are **not writable**, on create or update. A body
  carrying `organization_id`, the owner column or the password column has those
  keys dropped before the statement is built, and the rest of the body is
  applied normally. See [Server-owned columns](#server-owned-columns).
* `hidden` fields are also not filterable: `?password_hash=…` is ignored rather
  than answered, so a list endpoint cannot be used to probe a value it does not
  return. `~` provides no way around this; `?password_hash~=$argon` is ignored
  identically.
* `~` is a separate parameter form rather than a change to `?field=`, because a
  filter that silently matched substrings would break existing callers. `%` and
  `_` in the term are matched literally rather than as wildcards, and `~` on a
  non-text column returns `400` rather than searching its text representation.

## Server-owned columns

Three groups of columns are determined by the server and ignored when a client
sends them:

| Column | Reason |
|--------|--------|
| `organization_id` (org-scoped resources) | The tenant is stamped from the active organisation. Accepting it on update would allow a caller to *move* a row into an organisation they do not belong to, since the `WHERE` clause only establishes that they may modify the row in its current organisation. |
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
| POST | `<base>/auth/register` | create a user, returning `{token, user}` |
| POST | `<base>/auth/login` | exchange credentials for `{token}` |
| POST | `<base>/auth/apikeys` | issue a key, returning `{api_key, id}` |

## Billing endpoints

Mounted only where `[payments]` names a provider. See [Payments](payments.md).

| Method | Path | Who | Purpose |
|--------|------|-----|---------|
| GET | `<base>/billing/config` | anyone | publishable key, currency, whether tax is added |
| POST | `<base>/billing/checkout` | org **admin** | start a purchase, returning `{url}` |
| POST | `<base>/billing/portal` | org **admin** | self-service billing, returning `{url}` |
| POST | `<base>/billing/webhook` | the provider | signed deliveries; the only writer of `billing_subscription` and `billing_payment` |

The two operator-initiated `POST`s take the organisation from `X-Organization`,
as every other org-scoped request does, and require the `admin` role in it,
since starting a subscription commits the organisation to a recurring charge.

## AI endpoints

Mounted only where `[ai]` names a provider. See [AI](ai.md).

| Method | Path | Who | Purpose |
|--------|------|-----|---------|
| GET | `<base>/ai/config` | anyone | provider, default model, access level and the configured agents. Never the API key |
| POST | `<base>/ai/chat` | `[ai] access` (default **authenticated**) | accepts a conversation and streams back the reply |
| POST | `<base>/ai/agents/<name>/chat` | the named agent's `chat` permission | one turn with a configured agent; optionally resumes persisted history |

`POST /ai/chat` responds as `text/event-stream` unless the body sets
`"stream": false`, in which case it returns a single JSON document. A provider
that rejects the request or cannot be reached produces a `502`, indicating the
failure occurred upstream of this server.

A stored agent (`agents/*.toml` with `storage.enabled = true`) also creates the
read-only history resources `ai_<name>_thread` and `ai_<name>_message`. The chat
route writes them; ordinary CRUD may list and read them according to the agent's
`history` permission.

## Function endpoints

| Method | Path | Notes |
|--------|------|-------|
| *(manifest method)* | `<base>/functions/<name>` | Body forwarded to the function; JSON returned. |
| *(manifest method)* | `<base>/functions/<name>/stream` | The same call, answered as `text/event-stream`: one `delta` event per chunk the function emits, then a `done` event carrying its return value as `result`. |

Both endpoints enforce the function's own method and access policy; `/stream`
grants no additional access. The built-in admin dashboard uses the `/stream`
variant for actions, so emitted chunks are visible while the function is still
running.

See [Functions](functions.md).

## Server-sent events

Both streaming endpoints use the same three events, and every payload is JSON:

| event | data | means |
|-------|------|-------|
| `delta` | `{"text": "…"}` | more output; append it |
| `reasoning` | `{"text": "…"}` | a model's reasoning, where the provider streams it separately from the answer. Not part of the reply |
| `error` | `{"error": "…"}` | generation stopped early. Always followed by `done` |
| `done` | `{…}` | the end of the stream: the completion's `finish_reason` and token counts, or a function's `result`. Sent exactly once |

The status code is determined before the first byte, so any later failure
arrives as an `error` event rather than as a status.

## Lifecycle hooks

A resource with a `[hooks]` section runs functions around its CRUD operations,
which can change what these endpoints do:

* a `before_*` hook may reject the request with **any** `4xx`/`5xx` status it
  chooses (`400` when it returns a plain error), so an endpoint can answer with
  codes not listed below;
* a `before_create`/`before_update` hook may rewrite the body that gets stored;
* an `after_*` hook may replace the response body, including turning the usual
  `204` from `DELETE` into a `200` with content;
* a hook naming a function that is not loaded causes its operation to fail with
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
| `400` | invalid body, invalid filter or id, a foreign key referencing an unknown record, or a missing required field |
| `401` | authentication required: an anonymous caller reached a protected action |
| `403` | authenticated but not permitted, for example holding the wrong role |
| `404` | resource, route or row not found; also a `private` action, or an owner-scoped row belonging to someone else |
| `405` | wrong HTTP method for a function |
| `409` | uniqueness conflict |
| `500` | unexpected server or database error, or a declared hook whose function is not loaded |

Errors are JSON: `{ "error": "<message>" }`. A [hook](hooks.md) can return any
status in `400..=599` with its own message.

## Domain / host filtering

When `server.domain` is set, requests whose `Host` header does not match are
routed nowhere, producing a `404`. Leave it unset to accept any host.
