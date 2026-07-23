# OpenAPI & Swagger UI

apiplant generates an **OpenAPI 3.0** document from your app and serves an
interactive **Swagger UI** — no annotations, no build step. Both update
automatically as you add resources, functions or relationships.

| Endpoint | What |
|----------|------|
| `GET <base>/openapi.json` | the generated OpenAPI 3.0 spec |
| `GET <base>/docs` | Swagger UI (try requests from the browser) |

Enable/configure via [`[docs]`](configuration.md#docs). Both are on by default.

## What's in the spec

* **Resources** → CRUD paths with `operationId`s, path/query parameters, request
  bodies and typed responses.
* **Two component schemas per resource**: a read schema (id + fields +
  timestamps, `readOnly` where server-managed) and an `Input` schema (writable
  fields only). `hidden` fields are excluded from both; the auto-stamped owner
  column is excluded from inputs.
* **Query parameters**: `limit`, `offset`, `expand` (documenting the available
  relations), and one exact-match filter per column.
* **Nested `has_many` paths**: `/{parent}/{id}/{child}` for every reverse
  relation.
* **Auth endpoints** and **every non-private function**. Functions written with
  the `function!` macro whose input/output derive `JsonSchema` get **typed
  request/response schemas** (registered as `Fn<Name>Input` / `Fn<Name>Output`
  components), so their bodies render as typed forms rather than opaque objects.
* Each operation carries a `description` stating its permission
  (e.g. *"Requires the `admin` role."*).

## Auth in the UI

Two security schemes are declared and attached to every operation that requires
authentication, so Swagger's **Authorize** button is live:

| Scheme | Type | How |
|--------|------|-----|
| `bearerAuth` | HTTP bearer (JWT) | paste a token from `POST /auth/login` |
| `apiKeyAuth` | API key in `X-Api-Key` | paste a key from `POST /auth/apikeys` |

Public operations carry **no** security requirement, so they stay callable while
signed out. `Authorize` persists across page reloads.

### Typical flow in the UI

1. Open `<base>/docs`.
2. Expand `POST /auth/login`, **Try it out**, send your credentials, copy the
   `token`.
3. Click **Authorize**, paste the token under `bearerAuth`, authorize.
4. Every protected endpoint now sends your token. (Or authorize `apiKeyAuth`
   with an API key instead.)

## Notes

* The spec is generated once at boot and cached (static for the process
  lifetime).
* The Swagger UI page loads assets from a CDN (jsDelivr); it needs internet at
  view time. Ask if you'd prefer the assets vendored into the binary for
  offline use.
