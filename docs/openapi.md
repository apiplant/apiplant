# OpenAPI & Swagger UI

apiplant generates an **OpenAPI 3.0** document from the app and serves an
interactive **Swagger UI**, with no annotations and no build step. Both update
automatically as resources, functions or relationships are added.

| Endpoint | Contents |
|----------|----------|
| `GET <base>/openapi.json` | the generated OpenAPI 3.0 spec |
| `GET <base>/docs` | Swagger UI, which can issue requests from the browser |

Enable and configure these via [`[docs]`](configuration.md#docs). Both are
enabled by default.

## What the spec contains

* **Resources**, as CRUD paths with `operationId`s, path and query parameters,
  request bodies and typed responses.
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
* Each operation carries a `description` stating its permission, for example
  *"Requires the `admin` role in the active organisation."*

## Auth in the UI

Two security schemes are declared and attached to every operation that requires
authentication, so Swagger's **Authorize** button is live:

| Scheme | Type | Usage |
|--------|------|-------|
| `bearerAuth` | HTTP bearer (JWT) | paste a token from `POST /auth/login` |
| `apiKeyAuth` | API key in `X-Api-Key` | paste a key from `POST /auth/apikeys` |

Public operations carry **no** security requirement, so they remain callable
while signed out. The `Authorize` setting persists across page reloads.

### Typical flow in the UI

1. Open `<base>/docs`.
2. Expand `POST /auth/login`, **Try it out**, send your credentials, copy the
   `token`.
3. Click **Authorize**, paste the token under `bearerAuth`, authorize.
4. Every protected endpoint now sends the token. Alternatively, authorize
   `apiKeyAuth` with an API key.

## Notes

* The spec is generated once at boot and cached for the lifetime of the process.
* The Swagger UI page loads its assets from a CDN (jsDelivr), so it requires
  internet access when viewed. Open an issue if vendoring the assets into the
  binary for offline use would be useful.
