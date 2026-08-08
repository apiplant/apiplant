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
resource, so a search box needs no knowledge of the underlying fields:

```toml
# resources/order.toml
[admin]
search_fields = ["reference", "customer_note"]
```

```
?search=depot        # reference ILIKE %depot% OR customer_note ILIKE %depot%
```

When undeclared, the set is the single `search_field`, which itself defaults to
the `display_field`, so behaviour is unchanged for a resource that does not
configure one. A caller with knowledge of the resource can override the set per
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
| The `user` resource's `password_field` | Holds an argon2 hash. The only endpoint that writes it is `POST <base>/auth/register`, which hashes a plaintext first. |

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

## OAuth endpoints

Mounted only where `[oauth]` names a provider. See
[Authentication](authentication.md#signing-in-with-somebody-elses-account).

| Method | Path | Who | Purpose |
|--------|------|-----|---------|
| GET | `<base>/auth/oauth` | anyone | the providers this deployment offers, each with its `start_url` |
| GET | `<base>/auth/oauth/{provider}/start` | anyone | **302** to the provider; `?return_to=/path` chooses the landing page, `?token_delivery=fragment\|query\|json` overrides `[oauth] token_delivery` for this one flow |
| POST | `<base>/auth/oauth/{provider}/start` | anyone | the same as `{authorize_url, state, expires_in, linking}`; **with a session it links** rather than signs in |
| GET | `<base>/auth/oauth/{provider}/callback` | the provider | the registered redirect URI; **302** to the landing page carrying the token |
| POST | `<base>/auth/oauth/{provider}/callback` | a front end | the same from `{code, state}`, answering `{token, user, created, linked}` |
| DELETE | `<base>/auth/oauth/{provider}` | the account | unlink; **409** when it is the only way in. The only way to remove one: `oauth_connection`'s own `delete` is `private`, so the check cannot be walked around |

The redirect URI to register is
`<public_url><base_path>/auth/oauth/<provider>/callback`, printed for each
provider at boot. The token these issue is the one `POST <base>/auth/login`
issues.

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

## Storage endpoints

Mounted unless `[storage] backend = "none"`, which is to say by default. See
[File storage](storage.md).

| Method | Path | Who | Purpose |
|--------|------|-----|---------|
| POST | `<base>/uploads` | **authenticated** | store one file, returning `{url, key, size, content_type}` |
| GET | `/files/<key>` | anyone | read a stored file back |

The upload takes the file as the **raw request body** — not multipart — with its
name in `?filename=` and its type in `Content-Type`:

```bash
curl -X POST 'localhost:8080/api/uploads?filename=chair.png' \
  -H "authorization: Bearer $TOKEN" \
  -H 'content-type: image/png' \
  --data-binary @chair.png
```

`url` is a **relative** link (`/files/2026/08/…`) and is what goes into a
[`file` field](resources.md). `413` means the body exceeded `max_size_mb`, and
`415` a content type outside `allowed_types`.

`GET /files/<key>` sits outside `base_path`, alongside the dashboard and the
static site, because it is the URL stored in rows and served in `<img>` tags. It
is unauthenticated: a stored link is unguessable, but anyone holding it can read
the file. Responses carry `nosniff` and a one-year immutable cache header, and
no upload is ever served as HTML.

## Queue endpoints

| Method | Path | Who | Purpose |
|--------|------|-----|---------|
| POST | `<base>/queues/<topic>` | `[queues] publish` | queue the request body as a message |

Off unless `[queues] publish` names a policy — the default, `private`, answers
`404`, because a topic is an internal name wired to real work rather than
something to expose by accident. Everything inside the app should publish
through a function's `publish`, which needs no endpoint and no credential.

```bash
curl -X POST localhost:8080/api/queues/order.paid \
  -H "authorization: Bearer $TOKEN" \
  -H 'content-type: application/json' \
  -d '{"order_id": "..."}'
```

```json
{ "id": "...", "topic": "order.paid", "delivered": 2 }
```

`202 Accepted`, not `200`: the message is written down, and that is the whole
promise. Whether the handler succeeds is not knowable yet and is never reported
here — look at `queue_message`. `delivered` is how many subscribers it was
queued for; `0` means nothing subscribes to that topic, which is recorded rather
than refused. An empty body is a valid message (`{}`).

See [Queues](queues.md).

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
| `413` | an upload larger than `[storage] max_size_mb` |
| `415` | an upload whose content type is outside `[storage] allowed_types` |
| `429` | more requests than `[rate_limit]` allows; carries `Retry-After` and the `X-RateLimit-*` headers |
| `500` | unexpected server or database error, or a declared hook whose function is not loaded |

Errors are JSON: `{ "error": "<message>" }`. A [hook](hooks.md) can return any
status in `400..=599` with its own message.

## Domain / host filtering

When `server.domain` is set, requests whose `Host` header does not match are
routed nowhere, producing a `404`. Leave it unset to accept any host.
