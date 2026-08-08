# Configuration (`main.toml`)

`main.toml` lives at the root of the app directory. **Every key is optional**: a
missing file, section or key falls back to a safe default. The only setting that
is *inferred* rather than declared is TLS (see [HTTPS](#https)).

```toml
[app]
name = "Acme Logistics"      # displayed name; defaults to the directory name

[server]
host      = "0.0.0.0"        # optional: interface to bind; unset means all interfaces
port      = 8080             # TCP port
domain    = "api.example.com" # optional: only answer this Host header
                              # (or a list: ["api.example.com", "www.example.com"])
base_path = "/api"          # mount the whole API under a sub-path
workers   = 8               # worker threads (default: one per CPU)

[database]
url             = "postgres://user:pass@localhost:5432/mydb"  # full URL...
# ...or assemble from parts (used only when `url` is empty):
host            = "localhost"
port            = 5432
name            = "apiplant"
user            = "postgres"
password        = "postgres"
max_connections = 16
auto_migrate    = true       # run additive migrations on boot

[auth]
jwt_secret         = "change-me"  # signs session tokens; empty means ephemeral
session_ttl_secs   = 604800       # 7 days
allow_registration = true         # enable POST /auth/register
# The three below follow [email]: enabled with a provider, disabled without one.
# require_email_verification = true
# allow_invitations          = true
# allow_password_reset       = true

[rate_limit]                 # optional: how often one client may call; off by default
default             = "100/1m"  # every endpoint, unless something narrower says otherwise
trust_proxy_headers = false  # true only behind a proxy that writes X-Forwarded-For

[docs]
enabled = true               # serve OpenAPI spec + Swagger UI
path    = "/docs"            # where Swagger UI mounts (under base_path)
# title = "My API"           # only when the docs name differs from [app] name

[admin]
enabled = true               # serve the built-in admin dashboard
path    = "/admin"           # where it mounts (outside base_path)
# logo  = "/logo.png"        # your logo instead of the apiplant one
gravatar = false             # fall back to Gravatar for accounts with no avatar_url

[organization]               # optional: rules about the tenant itself
org_class_editors = "member@org_class=admin"   # who may class an organisation
default_org_class = "customer"                 # what a new one starts as

[public]
enabled   = true             # serve `dir` at the site root when it exists
dir       = "public"         # static site directory, relative to the app root
not_found = "404.html"       # page for unmatched requests (default when present)

[email]                      # optional: outbound mail, disabled unless configured
provider = "sendgrid"        # none | smtp | ses | sendgrid | brevo | mailjet |
                             # mailgun | postmark | resend
from     = "no-reply@example.com"
api_key  = "${SENDGRID_API_KEY}"
logo     = "logo.png"        # banner image, a path inside `public/`

[cache]                      # optional: Redis, disabled unless a url is given
url    = "redis://127.0.0.1:6379"
prefix = "my-app:"

[storage]                    # uploaded files; `local` unless told otherwise
backend       = "local"      # local | s3 | none
dir           = "storage"    # local: a directory (a mounted volume, in a container)
allowed_types = ["image/*"]  # empty accepts anything

[queues]                     # background work; on, with nothing subscribed
[queues.subscribe]
"order.paid" = "fulfilOrder" # topic -> the function(s) that handle it

[payments]                   # optional: Stripe, disabled unless a provider is named
provider   = "stripe"
secret_key = "${STRIPE_SECRET_KEY}"

[oauth.github]               # optional: sign in with GitHub, off unless credentialed
client_id     = "${GITHUB_CLIENT_ID}"
client_secret = "${GITHUB_CLIENT_SECRET}"

[observability]              # optional: OpenTelemetry, off unless enabled
enabled     = true
environment = "production"
[observability.logs]
format = "json"              # pretty | compact | json
[observability.otlp]
endpoint = "http://collector:4318"   # OTLP/HTTP; unset keeps everything in-process

[ai]                         # optional: a chat assistant, disabled unless named
provider = "custom"          # none | openai | anthropic | custom
endpoint = "http://localhost:8080"
model    = "local"
access   = "authenticated"   # who may call <base>/ai/chat
```

## `[app]`

| Key | Default | Notes |
|-----|---------|-------|
| `name` | the app directory's name | The name shown to users: the [dashboard](admin.md) header (`<name> admin`), the browser title, and the [API docs](openapi.md). A directory name such as `07-functions`, `backend` or `api-v2` reflects how the source is organised and is rarely appropriate to display. |

## `[server]`

| Key | Default | Notes |
|-----|---------|-------|
| `host` | `0.0.0.0` | Bind address. Omit it to listen on every interface; unset, `""` and `*` all normalise to `0.0.0.0`. Set it only to *narrow* the bind, for example `127.0.0.1` for localhost only. |
| `port` | `8080` | TCP port. |
| `domain` | *(none)* | When set, requests whose `Host` header matches none of these return 404. Useful for virtual hosting. Takes a single string or a list. Unset, `[]`, `""`, `*` and `_` all mean any host is accepted, and a wildcard anywhere in a list applies to the whole list. |
| `base_path` | `/` (empty) | Sub-path prefix for **all** routes. Normalised to start with `/` and not end with one. `/api` ⇒ endpoints live at `/api/...`. |
| `workers` | one per CPU | Number of OS worker threads. |
| `public_url` | first `domain`, else `http://<host>:<port>` | The origin at which this server is reached externally, for example `https://api.example.com`. It is needed only for links that leave the process, such as those in invitation and password-reset emails. **Set it behind a proxy or load balancer**, where the fallback is incorrect. |

## `[database]`

| Key | Default | Notes |
|-----|---------|-------|
| `url` | *(empty)* | Full Postgres URL. When empty, built from the parts below. |
| `host` / `port` / `name` / `user` / `password` | `localhost` / `5432` / `apiplant` / `postgres` / `postgres` | Used only if `url` is empty. |
| `max_connections` | `16` | Connection-pool size. |
| `auto_migrate` | `true` | Create missing tables, columns and foreign keys on boot. Set `false` to manage the schema yourself. |

apiplant targets **PostgreSQL**, using `to_jsonb`, `gen_random_uuid()`,
`jsonb_agg` and real foreign keys.

## `[auth]`

| Key | Default | Notes |
|-----|---------|-------|
| `jwt_secret` | *(empty)* | HMAC secret for session JWTs. **Set this in production**: an empty value generates a random secret at boot, so tokens do not survive a restart. |
| `session_ttl_secs` | `604800` (7d) | Lifetime of issued session tokens. |
| `allow_registration` | `true` | Whether self-service signup is open. `false` closes `POST /auth/register` *and* anonymous `POST <base>/user`. |
| `require_email_verification` | follows `[email]` | New accounts must confirm their address before they can sign in. |
| `allow_invitations` | follows `[email]` | Offer `POST /auth/invitations`, so an admin can invite someone who has no account yet. |
| `allow_password_reset` | follows `[email]` | Offer `POST /auth/password/forgot` and `/auth/password/reset`. |
| `invite_ttl_secs` | `604800` (7d) | How long an invitation link stays valid. |
| `verification_ttl_secs` | `86400` (24h) | How long a confirmation link stays valid. |
| `password_reset_ttl_secs` | `3600` (1h) | How long a reset link stays valid. Deliberately short, since it is a live credential sitting in a mailbox. |

The three "follows `[email]`" flags are the only defaults here that are not
constant. Configuring a provider enables all three; configuring none leaves them
disabled. None of them can function without a way to send mail, and an app that
can send mail usually wants all three. Set one explicitly to override this. An
explicit `true` still requires a provider, since otherwise every new account
would be locked behind a confirmation that cannot be delivered.

See [Authentication](authentication.md) for the flows themselves.

## `[rate_limit]`

| Key | Default | Notes |
|-----|---------|-------|
| `enabled` | `true` | The kill switch. `false` drops every limit — this section's, the resources' and the functions' — without editing any of them. |
| `default` | `"off"` | The rule every endpoint gets unless a resource or a function names its own. `"off"` limits nothing, which is why an app that says nothing here is limited nowhere. |
| `trust_proxy_headers` | `false` | Read the client address from `X-Forwarded-For` / `X-Real-IP` instead of the peer socket. |
| `cleanup_interval_secs` | `60` | How often tracked clients are swept for buckets nobody is using. |
| `stale_after_secs` | `600` | How long a client's bucket is kept after their last request. |

A rule is written `"<requests>/<window>"`: `"100/1m"`, `"30/30s"`, `"1000/1h"`,
`"5/1d"`, or the bare-seconds form `"100/60"`. Two words stand in for a number —
`"off"` lifts the limit and `"inherit"` defers to the level above. Anything else
fails the load, naming the file: a rate limit that quietly parsed as "no limit"
is the one failure mode worth being noisy about.

Three levels decide, narrowest last:

```toml
# main.toml — everything, unless something says otherwise
[rate_limit]
default = "100/1m"
```

```toml
# models/order.toml — this resource, per action
[rate_limit]
all    = "60/1m"
create = "5/1m"    # expensive, so stricter
list   = "off"     # cached upstream, so unlimited
```

```toml
# functions/summarise.toml — this function, both its endpoints
rate_limit = "10/1m"
```

A function's limit lives in its config file rather than in its manifest because
the manifest is compiled into the library: how often a deployment lets people
call a function is the deployment's decision, and it should not need a rebuild.

A limited response carries `X-RateLimit-Limit`, `X-RateLimit-Remaining` and
`X-RateLimit-Reset`; a refused one is `429` with `Retry-After` beside them.
Limits apply to the API only — the admin dashboard's own assets and the
`public/` site are files, and counting a page's images against the allowance for
its data is how a limit set for an API ends up breaking a browser.

### Who "one client" is

The peer socket address, which a caller cannot forge. Behind a reverse proxy
that is the *proxy's* address for every request — one bucket for everybody,
throttling all callers together — so a deployment behind one has to set
`trust_proxy_headers = true` **and** make sure the proxy overwrites
`X-Forwarded-For` rather than appending to it. Trusting that header with nothing
in front of the server hands every caller their own limit for the price of a
header line, which is the same as having none.

Buckets live in the process, so each replica limits against its own count: two
replicas behind a load balancer allow roughly twice `default` between them.

## `[docs]`

| Key | Default | Notes |
|-----|---------|-------|
| `enabled` | `true` | Serve `GET <base>/openapi.json` and the Swagger UI. |
| `path` | `/docs` | UI mount path (under `base_path`). |
| `title` | the app's `[app] name` | Shown in the UI and in the spec's `info.title`. Set it only when the published API uses a different name from the app. |

See [OpenAPI & Swagger UI](openapi.md).

## `[admin]`

| Key | Default | Notes |
|-----|---------|-------|
| `enabled` | `true` | Serve the admin dashboard. Every app has one: the interface is built into the `apiplant` binary and its manifest is derived from the app on boot, so there is nothing to generate. Set `false` for a deployment that should expose no operator console. |
| `path` | `/admin` | Where it mounts. Outside `base_path`, so `/admin/` is unaffected when the API moves to `/api`. Normalised to start with `/` and not end with one. |
| `logo` | unset | Image shown beside the app name, given as a URL the browser can fetch, usually a file in [`public/`](#public) such as `/logo.png`. Unset keeps the apiplant logo. |
| `gravatar` | `false` | Draw an account that has no `avatar_url` with its [Gravatar](https://gravatar.com), looked up from the hash of its email address. Off by default: it is a request to a third party for every face the dashboard draws, and the hash of an address is enough to confirm a guess at it. An address with no Gravatar — or any picture that fails to load — falls back to the account's initials, which is also what happens with this left off. |

Gravatar needs a **secure context**. The dashboard hashes the address with
WebCrypto, and `crypto.subtle` is only exposed on `https://`, `localhost`, or
`127.0.0.1` — so a dashboard opened on `http://0.0.0.0:8099/admin/`, or on a LAN
address like `http://192.168.1.20:8099/admin/`, quietly draws initials no matter
what this key says. Binding to `0.0.0.0` is fine; it is the address in the
browser's URL bar that decides. Reach a local app through
`http://localhost:8099/admin/` instead, and serve over HTTPS in production.

`[admin.ai_assistance]` adds a small "fill with AI" helper to every writable
text input and textarea in the dashboard, including the Markdown/HTML editor.
It only appears when both this block enables it *and* the app's [`[ai]`](#ai)
section names a provider.

| Key | Default | Notes |
|-----|---------|-------|
| `enabled` | `false` | Show the helper controls in the dashboard. Without an `[ai]` provider it stays absent even when this is `true`. |
| `system` | empty | Optional system prompt sent only by the dashboard's field helper. Empty uses the app-wide `[ai] system` alone. |
| `prompt_placeholder` | `Describe what you want AI to write for this field.` | Placeholder shown in the helper's floating prompt box. |

The dashboard is served entirely from the binary: the files come from the
embedded build and the manifest from the app being served. Nothing on disk feeds
into it, so it requires no CORS and cannot become stale after a model change.
For a custom console, set `enabled = false` and serve one from `public/admin/`.
To host a copy of this one on another origin, build it with `apiplant admin`.
See [Admin dashboard](admin.md).

## `[organization]`

| Key | Default | Notes |
|-----|---------|-------|
| `org_class_editors` | `private` | Who may write `organization.org_class`, in the [permissions](permissions.md) grammar. The default writes it for nobody: the column is server-owned and classes come from seed data or SQL. |
| `default_org_class` | unset | The class stamped on a new organisation that has none — every organisation the API creates, including the personal one each account is given. Unset leaves them unclassed, which no `@org_class=` qualifier matches. A class editor who names a class on create keeps theirs. |

An organisation's `org_class` is what a `@org_class=` permission is checked
against, so an organisation able to set its own class could grant itself
whatever those permissions guard. The column is therefore stripped from every
request body — like `organization_id` — except for callers this setting names.

The policy is answered against the organisation the caller has **selected**, not
the one being edited, which is how one back-office organisation comes to
administer everybody's classes:

```toml
[organization]
org_class_editors = "member@org_class=admin"
```

See [Organisation classes](permissions.md#organisation-classes).

## `[public]`

Add a `public/` directory to the app and it is served at the site root:
`public/index.html` serves `/`, `public/style.css` serves `/style.css`, and
`public/guide/index.html` serves both `/guide` and `/guide/`.

| Key | Default | Notes |
|-----|---------|-------|
| `enabled` | `true` | Serve `dir` when it exists. A missing directory is not an error; it simply means no static site. |
| `dir` | `public` | Directory holding the site, relative to the app root. |
| `not_found` | *(none)* | Page for requests that match nothing, relative to `dir`. When unset, `404.html` is used if it exists. Served with a `404` status. |

One route is registered per file at boot, so the site and the API can share the
root: `/about.html` serves the file while `/products` still reaches the API's
CRUD routes. A path with neither a file nor a route, such as `/no/such/page`,
serves the 404 page. Two-segment paths belong to the API's `/{resource}/{id}`
and continue to respond in JSON.

## `[email]`

Outbound mail for functions to send. Disabled by default
(`provider = "none"`), and never used by the framework itself; no message is
sent that the application did not compose.

| Key | Default | Notes |
|-----|---------|-------|
| `provider` | `none` | `smtp`, `ses` (`aws`), `sendgrid`, `brevo` (`sendinblue`), `mailjet`, `mailgun`, `postmark` or `resend`. |
| `from` | *(empty)* | Envelope sender. Required once a provider is named; a message may override it. |
| `from_name` | *(empty)* | Display name beside `from`. |
| `reply_to` | *(empty)* | Default `Reply-To`. |
| `api_key` | *(empty)* | The provider's key: the AWS access key id for `ses`, the public key for `mailjet`, the server token for `postmark`. |
| `api_secret` | *(empty)* | The second half of a two-part credential: `ses`, `mailjet`. |
| `region` | *(empty)* | `ses` only, for example `eu-west-1`. |
| `domain` | *(empty)* | `mailgun` only, for example `mg.example.com`. |
| `timeout_secs` | `15` | How long one send may take. |
| `logo` | `logo.png` | The image shown in the banner of framework-sent messages, given as a path inside [`[public] dir`](#public); `logo.png` and `/logo.png` refer to the same file. It is resolved to an absolute URL against `public_url`, since a mail client fetches it over the internet. If no such file exists, which is the case for the default until one is added, the banner shows the app's name alone rather than a broken image. |

`[email.smtp]`, for `provider = "smtp"`:

| Key | Default | Notes |
|-----|---------|-------|
| `host` | *(empty)* | Required for `smtp`. |
| `port` | `0` | `0` picks from `encryption`: 465 (`tls`), 587 (`starttls`), 25 (`none`). |
| `username` / `password` | *(empty)* | Omit `username` for a relay that authenticates by IP. |
| `encryption` | `starttls` | `starttls`, `tls` (implicit) or `none` (cleartext, which logs a warning at boot). |

An unusable provider configuration (unknown name, missing key, or no `from`)
fails the boot rather than the first send. See [Sending email](email.md).

## `[cache]`

An optional Redis, reachable only from functions. Disabled unless `url` is set,
and never used by the framework itself. See [Caching](caching.md).

| Key | Default | Notes |
|-----|---------|-------|
| `enabled` | `true` | Disable the cache without removing the rest of the section. |
| `url` | *(empty)* | `redis://…`, `rediss://…`; may carry a password and a database index. Empty means no cache. |
| `prefix` | *(empty)* | Prepended to every key, so apps can share one Redis. |
| `default_ttl_secs` | `0` | Expiry for a write that does not specify one. `0` means keys persist. |
| `timeout_secs` | `5` | How long one operation may take. |

A `url` that cannot be reached fails the boot.

## `[storage]`

Where uploaded files go, and what backs the [`file` field type](resources.md).
A `file` column holds a **relative** URL — `/files/2026/08/…` — which the server
answers from whichever backend is named here, so switching between them is a
configuration change and not a data migration. On by default: with no section at
all, uploads land in a `storage/` directory. See [File storage](storage.md).

| Key | Default | Notes |
|-----|---------|-------|
| `backend` | `local` | `local`, `s3`, or `none` to refuse uploads outright. |
| `dir` | `storage` | `local` only. Relative to the app root unless absolute; created on boot. Must be a mounted volume in a container. |
| `public_base` | `/files` | URL prefix the stored links carry and the server answers on. |
| `max_size_mb` | `10` | Largest upload accepted. |
| `allowed_types` | *(empty)* | `image/png`, `image/*`. Empty accepts anything. |
| `bucket` | *(empty)* | `s3` only, required. |
| `region` | `auto` | `s3` only. R2 and most S3-compatibles want `auto`. |
| `endpoint` | *(empty)* | `s3` only. Empty uses AWS's own; set it for R2, MinIO, B2. |
| `access_key_id` / `secret_access_key` | *(empty)* | `s3` only, both required. |
| `path_style` | *(set when `endpoint` is)* | `<endpoint>/<bucket>/<key>` addressing. Required by MinIO and R2. |
| `prefix` | *(empty)* | Key prefix, so several apps can share one bucket or directory. |
| `base_url` | *(empty)* | Store absolute links under this origin (a CDN) instead of proxying reads. |

A misconfigured backend — `s3` with no bucket, a `dir` that cannot be created —
fails the boot rather than the first upload.

[`examples/26-file-upload`](../examples/26-file-upload) is this section pointed
at Cloudflare R2, with the local block beside it for comparison.

## `[queues]`

Background work: a message published now, handled by a function shortly after,
outside the request that caused it. The transport is the app's own Postgres —
`publish` writes a row to `queue_message` and fires a `NOTIFY`, and a subscriber
claims the row with `FOR UPDATE SKIP LOCKED`. There is nothing to install and no
second service that can be down. See [Queues](queues.md).

Publishing needs no configuration at all: `queue_message` is a built-in
resource, so `ctx.publish` works in an app whose `main.toml` never mentions
queues. What this section configures is the *subscriber* half.

| Key | Default | Notes |
|-----|---------|-------|
| `enabled` | `true` | Pause handling without deleting the subscriptions. Publishing still records rows, so nothing is lost while it is off. |
| `subscribe` | *(empty)* | `[queues.subscribe]`: topic → the function(s) that handle it. One name or a list; each subscriber gets its own row and its own retries. |
| `prefix` | `apiplant` | `NOTIFY` channel prefix, so two apps sharing a database don't wake each other. |
| `poll_secs` | `30` | Sweep interval. The `NOTIFY` is what makes delivery immediate; this is the safety net beneath it. |
| `batch` | `10` | Messages claimed in one go. |
| `max_attempts` | `5` | Then the message is left `failed` for a person to look at. `1` means no retries. |
| `retry_backoff_secs` | `10` | Doubling, capped at an hour: 10s, 20s, 40s, 80s. |
| `lease_secs` | `300` | How long a claimed message may be worked on before another subscriber may take it. Set it above your slowest handler. |
| `retain_hours` | `24` | Delete *handled* messages after this. `0` keeps them. `failed` rows are never swept. |
| `publish` | `private` | Who may `POST <base>/queues/{topic}`, in the resource permission grammar. The default means no such endpoint. |

```toml
[queues]
max_attempts = 3

[queues.subscribe]
"order.paid"      = ["fulfilOrder", "notifyOps"]
"order.cancelled" = "releaseStock"
```

A subscription naming a function that isn't loaded is reported at boot, loudly:
its messages would otherwise queue up and retry their way to the dead-letter one
cycle at a time.

Delivery is **at-least-once** — write handlers that can be run twice.

## `[payments]`

Payment processing. Disabled unless `provider` names one; naming a provider also
adds the `billing_*` resources and the `/billing` endpoints. See
[Payments](payments.md).

| Key | Default | Notes |
|-----|---------|-------|
| `provider` | `none` | `none` or `stripe`. |
| `secret_key` | *(empty)* | `sk_…`. Required once a provider is named; a `pk_…` here is detected by its prefix at boot. |
| `publishable_key` | *(empty)* | `pk_…`. Served to browsers by `GET <base>/billing/config`; it is not a secret. |
| `webhook_secret` | *(empty)* | `whsec_…`. Without it every delivery is rejected and no purchase is recorded. |
| `currency` | `usd` | ISO 4217, for prices that do not name one. |
| `automatic_tax` | `true` | Let Stripe Tax compute and add tax. Requires registrations configured at Stripe. |
| `tax_id_collection` | *(follows `automatic_tax`)* | Ask business buyers for a VAT or GST number. |
| `billing_address` | `auto` | `auto` or `required`. |
| `success_url` / `cancel_url` | *(empty)* | Where a buyer is returned to. Empty uses the dashboard's billing screen. |
| `portal_return_url` | *(empty)* | Where the customer portal returns to. |
| `timeout_secs` | `20` | Per provider call. |

An unusable provider configuration (unknown name or missing key) fails the
boot.

## `[oauth]`

Signing in with somebody else's account. Off until a provider block carries a
`client_id`; naming one mounts the `<base>/auth/oauth` endpoints and adds the
`oauth_state` resource. See
[Authentication](authentication.md#signing-in-with-somebody-elses-account).

```toml
[oauth.github]
client_id     = "${GITHUB_CLIENT_ID}"
client_secret = "${GITHUB_CLIENT_SECRET}"
```

apiplant ships `github`, `google`, `linkedin` and `x`, and knows each one's
endpoints, scopes and quirks. The keys below the table are only for a provider
it does not ship, or for pointing one somewhere else (a GitHub Enterprise host,
say).

| Key | Default | Notes |
|-----|---------|-------|
| `link_by_verified_email` | `true` | May a **verified** address from a provider sign somebody in to an existing account with that address? An *unverified* one never can, whatever this says. Off means such a match is refused with an answer telling the caller to sign in the way they already can and link from that session. |
| `state_ttl_secs` | `600` | How long somebody has to get through a consent screen. Clamped to 60–3600. |
| `success_redirect` | `/` | Where the redirecting callback lands, as a path. A caller may override it per flow with `?return_to=/somewhere`, accepted only as a path — never a full URL. |
| `failure_redirect` | *(empty)* | Where a failed sign-in lands. Empty answers with a JSON error instead, which is what you want while setting a provider up. |
| `token_delivery` | `fragment` | `fragment` (`…/#token=…`, never sent to a server, so it stays out of logs and `Referer`), `query` (`…?token=…`, easier for a server-rendered page), or `json` (no redirect: the callback answers with the body). A client that knows what it can read may override this per flow with `?token_delivery=` on `…/start` — the admin dashboard asks for `fragment` whatever an app has configured for its own front end. |
| `name_field` | `display_name` | The `user` column a provider's name is written to on every sign-in. `""` writes none. |
| `avatar_field` | `avatar_url` | The `user` column its picture is written to. `""` writes none. Both columns are in the built-in `user` model; a model that does not declare one simply does not get it filled. `email_placeholder` is filled too and is not configurable — see [Authentication](authentication.md#what-lands-on-the-account). |

And per provider, under `[oauth.<name>]`:

| Key | Default | Notes |
|-----|---------|-------|
| `client_id` | *(empty)* | What the provider issued. Empty leaves the provider off, so a committed config can name all four and a deployment supply only what it has. |
| `client_secret` | *(empty)* | Required once `client_id` is set. A client id with no secret fails the boot rather than the first sign-in. |
| `enabled` | `true` | Set false to take a button away without deleting the credentials. |
| `label` | the built-in name | What the button says. |
| `scopes` | the built-in list | Widen only for scopes the app will use: each is another line on a consent screen. **Required** for a provider apiplant does not ship. |
| `authorize_url`, `token_url`, `userinfo_url` | the built-in URLs | **Required** for a provider apiplant does not ship. |
| `style` | `oidc` | How to read the profile: `oidc` (standard `sub`/`email`/`email_verified`/`name`/`picture` claims) or `github`. Providers apiplant ships already know. |
| `redirect_uri` | *(derived)* | `<public_url><base_path>/auth/oauth/<provider>/callback`. Override only when something in front of this server rewrites paths. |
| `pkce` | *(what the provider supports)* | Rarely set by hand: X requires PKCE, GitHub does not offer it. |
| `icon` | *(none)* | A logo for the sign-in button, as a URL a browser can fetch — usually a file in [`public/`](#public), such as `/oauth/gitlab.svg`. Only for a provider apiplant does not draw itself; without one the button shows the provider's initial. [Super Tiny Icons](https://github.com/edent/SuperTinyIcons) is a good source — several hundred brand marks, a few hundred bytes each, MIT licensed, and where apiplant's own four come from. |

The redirect URI is derived from [`[server] public_url`](#server), which is
therefore the one value that has to be right — a provider compares it byte for
byte. `apiplant run` prints the string to register for each provider on the way
up.

An unusable configuration fails the boot: a `client_id` without a secret, an
unknown provider without endpoints, or a replaced `oauth_connection` model
missing a column the handshake writes.

## `[ai]`

A chat assistant. Disabled unless `provider` names one; naming a provider mounts
`<base>/ai/chat` and `<base>/ai/config`, and gives every function a `chat` call.
See [AI](ai.md).

| Key | Default | Notes |
|-----|---------|-------|
| `provider` | `none` | `none`, `openai`, `anthropic` or `custom`. `custom` covers anything implementing the OpenAI chat-completions shape, such as llama.cpp, vLLM, Ollama, LM Studio or a gateway. |
| `endpoint` | *(provider's own API)* | An origin (`http://localhost:8080`), a base (`…/v1`) or the full path (`…/v1/chat/completions`, used as written). Required for `custom`. |
| `model` | *(empty)* | Requested when a call does not name one. A server hosting a single model ignores it. |
| `api_key` | *(empty)* | **Optional.** Empty sends no authorization header at all rather than an empty one, which is what most local models require. |
| `system` | *(empty)* | System prompt for conversations that do not carry their own. |
| `max_tokens` | `2048` | Cap per reply, sent to every provider (Anthropic requires one). A reasoning model consumes this budget while reasoning, so a small value can produce an empty reply. |
| `temperature` | *(unset)* | Sent only when `>= 0`; otherwise the provider decides. |
| `reasoning` | `false` | Whether provider reasoning is surfaced to callers, via `reasoning` stream events, reasoning retained on stored messages, and a **Show reasoning** toggle in admin. When disabled it is discarded before any caller sees it. Individual agents override this with their own `[ai] reasoning`. |
| `access` | `authenticated` | Who may call `<base>/ai/chat`, using the same grammar as `[permissions]`: `public`, `authenticated`, `member`, `role:<name>`. Not `public` by default, since the endpoint consumes provider credit or GPU time on the caller's behalf. |
| `timeout_secs` | `300` | Per completion. Intentionally generous, since a long answer from a local model is slow rather than failed. |

An unknown provider, or `custom` without an endpoint, fails the boot.

## `[observability]`

Logs, traces and metrics, over [OpenTelemetry]. Turning `enabled` on gives every
request a span; adding an `[observability.otlp] endpoint` sends those spans and
the request metrics to a collector. The two are separate on purpose — a
deployment with no collector still gets structured, trace-correlated logs, which
is most of the value for none of the infrastructure.

OTLP is the only export format, because it is the one every backend speaks:
point it at the [OpenTelemetry Collector], Jaeger, Tempo, Honeycomb, Datadog,
Grafana Cloud, New Relic or anything else that accepts OTLP/HTTP on `:4318`.
Transport is HTTP, not gRPC.

[`examples/25-observability`](../examples/25-observability) is the whole thing
running, against a Grafana stack in one container.

| Key | Default | Notes |
|-----|---------|-------|
| `enabled` | `false` | Arms the section. Off means: log to the terminal as always, build no spans, export nothing. |
| `service_name` | the app's `[app] name` | What this service is called in a trace. Falls back to `OTEL_SERVICE_NAME`, then the app name, then the directory name. |
| `service_version` | the `apiplant` version | The build being traced. |
| `environment` | *(unset)* | Exported as `deployment.environment.name` — the attribute every backend groups by first. |
| `resource_attributes` | *(empty)* | Extra attributes on every span and metric: `{ region = "eu-west-1", tenant = "$TENANT" }`. |

### `[observability.logs]`

Applies whether or not `enabled` is set: a server writes logs before anyone asks
it to be observable.

| Key | Default | Notes |
|-----|---------|-------|
| `format` | `pretty` | `pretty` for a terminal, `compact` for a log file a person still reads, `json` for anything that parses the line. |
| `level` | `info,apiplant=debug,ntex_server=warn` | The `RUST_LOG` filter used when the environment does not set one. **`RUST_LOG` always wins** — it is what you reach for inside a running container. |
| `span_fields` | `true` | JSON only: include the current span's fields (route, method, trace id) on every line written inside it. This is what makes "every log line from the request that failed" a filter rather than a search. |

### `[observability.traces]`

| Key | Default | Notes |
|-----|---------|-------|
| `enabled` | `true` | Build spans. On even with no exporter — the trace id and error fields are worth having in the logs alone. |
| `sample_ratio` | `1.0` | Fraction of root requests recorded. Sampling is parent-based, so a trace is kept or dropped whole and a request arriving with a `traceparent` follows its caller's decision. Out-of-range values clamp rather than silently sampling nothing. |
| `response_header` | `true` | Return `X-Trace-Id` on every response, so a bug report becomes a lookup. |
| `capture_headers` | *(empty)* | Request headers to copy onto the span, e.g. `["x-request-id"]`. `authorization`, `proxy-authorization`, `cookie`, `set-cookie` and `x-api-key` are **refused even if listed** — a captured credential is a credential in your log aggregator. |
| `exclude_paths` | `["/_health"]` | Prefixes (under `base_path`) never traced. Health checks are noise you pay per span for. |

### `[observability.metrics]`

| Key | Default | Notes |
|-----|---------|-------|
| `enabled` | `true` | Needs an OTLP endpoint to go anywhere — metrics, unlike spans, are worth nothing in-process. |
| `interval_secs` | `60` | How often measurements are pushed. |

Two instruments, on the standard HTTP semantic conventions so a stock dashboard
reads them without being taught your app:

- `http.server.request.duration` (histogram, seconds), labelled by
  `http.request.method`, `http.route` and `http.response.status_code`
- `http.server.active_requests` (up-down counter)

`http.route` is a template, not a path: `/products/7` and
`/products/2f8a…` both report as `/products/{id}`. A metric labelled with row
ids grows a time series per row, which is the standard way to turn a monitoring
bill from tens of dollars into thousands.

### `[observability.otlp]`

| Key | Default | Notes |
|-----|---------|-------|
| `endpoint` | `$OTEL_EXPORTER_OTLP_ENDPOINT` | Base URL of an OTLP/HTTP receiver; `/v1/traces` and `/v1/metrics` are appended. Unset in both places exports nothing. |
| `protocol` | `http/protobuf` | Or `http/json` for a receiver that only speaks JSON. |
| `headers` | *(empty)* | Sent with every export — where a vendor API key goes. Use `$VAR`. |
| `timeout_secs` | `10` | Per export. The exporter drops a batch rather than blocking behind a collector that stopped answering. |

The standard `OTEL_*` variables are read when the matching key is unset —
`OTEL_EXPORTER_OTLP_ENDPOINT`, `OTEL_EXPORTER_OTLP_HEADERS`, `OTEL_SERVICE_NAME`
— because that is how a sidecar-injected collector configures the pods around
it, and an app should not need rebuilding to be scraped.

### Errors and failures

Every request span carries the OpenTelemetry HTTP attributes
(`http.request.method`, `http.route`, `url.path`, `http.response.status_code`).
A request that fails carries three more — `error.type`, `exception.type` and
`exception.message` — and is marked `ERROR`, which is what a backend colours red
and what an alert counts.

Any `5xx` is marked failed even if nothing reported it, and the failures the
server knows the shape of name themselves in `error.type`: `database`,
`function_fault`, `function_panic`, `hook_fault`, `hook_panic`, `storage`,
`queue_publish`, `payments`, `oauth_unreachable`. Group by that attribute and
"the API is throwing 500s" becomes "one function is panicking".

Rate limiting sits *inside* the tracing middleware, so a `429` is a traced,
counted request: "we started refusing traffic at 14:02" is exactly what you want
the graph to show.

### A worked example

Run a collector next to the app and look at traces in Jaeger:

```toml
[observability]
enabled     = true
environment = "production"
service_name = "checkout-api"

[observability.logs]
format = "json"

[observability.traces]
sample_ratio = 1.0        # sample everything here; drop what you don't
                          # want in the collector, where the outcome is known

[observability.otlp]
endpoint = "http://otel-collector:4318"
headers  = { authorization = "$OTEL_TOKEN" }
```

Traces stop at the edge of a service that does not propagate context, so if
requests reach this server through a gateway or another service, make sure it
forwards the `traceparent` header — this server reads it, continues the caller's
trace, and passes its own id back as `X-Trace-Id`.

[OpenTelemetry]: https://opentelemetry.io
[OpenTelemetry Collector]: https://opentelemetry.io/docs/collector/

## Environment variables

**Any** string value in **any** of the app's TOML files (`main.toml`, every
`models/*.toml` and every `functions/*.toml`) may reference the environment:

```toml
[database]
url = "$DATABASE_URL"

[auth]
jwt_secret = "${JWT_SECRET}"

[email]
api_key = "$SENDGRID_API_KEY"
```

This is what allows a committed `main.toml` to hold no credentials: the file
records *where* each secret comes from, and the deployment supplies it.

| Written | Means |
|---------|-------|
| `$VAR`, `${VAR}` | the variable's value, or `""` (with a warning) when unset |
| `${VAR:-default}` | the variable's value, or `default` when unset or empty |
| `$$` | a literal `$` |
| `$` followed by anything else | itself, unchanged |

A name is a letter or `_` followed by letters, digits or `_`. Anything else is
left exactly as written, so `$19.99`, `100 US$` and `a $ b` need no escaping;
`$$` is only required for a genuine ambiguity such as `$$USD`.

References can appear anywhere in a string, and a string can hold several:

```toml
[database]
url = "postgres://$DB_USER:$DB_PASSWORD@$DB_HOST:${DB_PORT:-5432}/$DB_NAME"

[server]
domain = "${APP_DOMAIN:-api.example.com}"
```

Defaults are what allow one file to work in both development and production:
name the variable, give the local value as the default, and set it explicitly
where it matters.

Two intentional limits:

* **Values only, never keys.** A table or field named by the environment would
  make a file's *shape* depend on the deployment, which is out of scope.
* **Substitution happens after parsing**, into a string TOML has already
  produced. A password containing `"` or a newline stays one string value — it
  cannot become a syntax error, and it cannot inject extra TOML.

An unset variable with no default expands to the empty string and logs a warning
naming the variable and the file. Leaving `$DATABASE_URL` in place would instead
pass the literal text to whatever consumes it, failing later and less clearly.

## HTTPS

TLS is **not** configured in `main.toml`. Instead, if the app directory contains
an `https/` folder with a certificate and a private key, the server serves HTTPS
automatically. Recognised filenames:

* **cert**: `cert.pem`, `fullchain.pem`, `certificate.pem`, or `server.crt`
* **key**: `key.pem`, `privkey.pem`, `server.key`, or `private.pem`

```
my-app/
└── https/
    ├── fullchain.pem
    └── privkey.pem
```

No `https/` directory means plain HTTP.

## Precedence and safety

* An absent file means all defaults.
* An absent section or key means that key's default; other keys are still read.
* `url` takes precedence over the individual `[database]` parts.
* An empty `jwt_secret` is allowed but logs a warning.
* `[email]`, `[cache]`, `[payments]` and `[ai]` are opt-in; a misconfigured one
  fails the boot rather than failing quietly at first use. `[storage]` is the
  exception among the optional services: it is on by default, and `backend =
  "none"` is how an app opts out.
* Removing `[payments]` removes the routes and the resources but not the tables,
  since migrations are additive and records of money changing hands should
  outlive a configuration change.
* `$VAR` is expanded in every app TOML file, before any of the above is read.
