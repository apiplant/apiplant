# Embedding apiplant in your own program

The `apiplant` binary is one way to run apiplant. The other is to depend on
`apiplant-server` from a Rust program of your own. That program gets the
accounts, organisations, permissions, generated API and admin dashboard, and
mounts routes of its own **on the same server, behind the same sign-in**.

```toml
[dependencies]
apiplant-core   = "0.11"
apiplant-server = { version = "0.11", default-features = false, features = ["email", "oauth"] }   # see "Cargo features"
ntex            = "4"   # the same major apiplant-server is built on
```

```rust
use apiplant_server::{AppState, Caller, Platform, Shared, State};
use ntex::web::{self, HttpRequest, HttpResponse};

async fn list_files(caller: Caller, state: State<AppState>, req: HttpRequest) -> HttpResponse {
    if let Err(refusal) = caller.require(&state, &req, "member").await {
        return refusal; // 401 / 403 / 404, as apiplant's own routes answer
    }
    let org = caller.active_org.unwrap(); // `member` guarantees one
    HttpResponse::Ok().json(&serde_json::json!({ "org": org }))
}

#[ntex::main]
async fn main() -> anyhow::Result<()> {
    let app = apiplant_core::App::load("./platform")?; // main.toml + resources/
    let platform = Platform::boot(app, Default::default()).await?;

    // Between boot and serve: the database pool, the authenticator, the
    // loaded resources are all on `platform.state()`.

    platform
        .serve_with(|cfg: &mut web::ServiceConfig<Shared>| {
            cfg.route("/files", web::get().to(list_files));
        })
        .await
}
```

## `Platform`

| | |
|---|---|
| `Platform::boot(app, options)` | Everything `apiplant run` does before listening: refuses features the build lacks, connects and migrates, builds the mailer, OAuth providers, rate limits, dashboard manifest, OpenAPI document, and starts the queue subscriber. |
| `platform.state()` | The `AppState` every handler shares: `db`, `auth`, `app` (config and resources), `mailer`, `queue`, … |
| `platform.serve()` | Serve apiplant's routes only. `run_with` is exactly `boot` + `serve`. |
| `platform.serve_with(extra)` | Serve apiplant's routes and yours. `extra` runs once per worker and receives a `ServiceConfig<Shared>`. |

### State in your handlers

ntex 4 has no state extractor of its own. Every extractor receives the
application's state, which here is `Shared` (the `AppState` behind an `Arc`).
`apiplant_server::State<AppState>` is the extractor that hands it to a handler.
It derefs to `AppState` and costs one reference count. Extractors of your own
implement `ntex::web::FromRequest<Shared>`, as `Caller` does.

Your routes are registered after the dashboard and stored files and before the
public site and the API scope, so a literal path of yours beats the API's
generic `/{resource}` match. Give the API a `[server] base_path` of its own
(`/api/platform`) to keep the namespaces apart. The dashboard's path
(`[admin] path`) and stored files' prefix (`[storage]`, `/files` by default)
win over a route of yours with the same prefix. Move them, pick another prefix, or build without the `storage` / `admin` feature.

## `Caller`

An ntex extractor. It never fails (an anonymous request is a `Caller` with no
principal) and resolves exactly as apiplant's own routes do: bearer token, API
key, or the [session cookie](authentication.md#the-session-in-a-cookie);
memberships read fresh; `X-Organization` accepted only for an organisation the
caller belongs to.

| | |
|---|---|
| `caller.principal` | `Option<Principal>`: user id and every membership with its roles. |
| `caller.active_org` | The organisation the request acts in, never guessed. |
| `caller.global_admin` | Whether `[organization] global_admin_role` names them. |
| `caller.user_id()`, `is_authenticated()`, `membership()`, `has_role(r)` | Shorthands. |
| `caller.require(&state, &req, "role:editor")` | Check a permission in the [resource grammar](permissions.md): `public`, `authenticated`, `member`, `role:<name>`, `private`, each with an optional `@org_class=`. `owner` counts as `private` (there is no row). `Err` is the response to send. |

## Cargo features

`apiplant-server` compiles every subsystem by default. An embedder can leave
out the ones it doesn't use, along with their dependency trees:

| Feature | Brings | Without it |
|---|---|---|
| `typescript` | V8 (`deno_core`), TypeScript functions | `.js` functions are reported and not loaded |
| `payments` | Stripe (`async-stripe`), `/billing`, a function's `payments` call | `[payments]` refuses to boot |
| `ai` | `apiplant-ai`, `/ai`, agents, a function's `chat` call | `[ai]` (or an agent's) refuses to boot |
| `cache` | Redis, a function's `cache` call | `[cache] url` refuses to boot |
| `telemetry` | OpenTelemetry + OTLP exporter (and a second `reqwest`) | logs still go through `tracing`; `[observability] enabled` refuses to boot |
| `email` | lettre, liquid, `emails/` templates, invitations, address confirmation, password reset, `send_email` | `[email]` refuses to boot |
| `oauth` | `apiplant-oauth`, `/auth/oauth/*` | `[oauth]` refuses to boot |
| `storage` | `apiplant-storage`, `/uploads`, stored files at `/files` | the default `backend = "local"` is read as `none`; `backend = "s3"` refuses to boot |
| `queues` | `apiplant-queue`, `/queues/{topic}`, `[publish]`, the subscriber | `[queues]` or any `[publish]` refuses to boot |
| `admin` | the embedded dashboard (`apiplant-assets/admin`) | not served; `[admin] enabled` (on by default) only warns |

The default is all of them. Measured with `cargo tree` (unique crates): about
560 with everything, about 400 with `email`, `oauth`, `queues` and `admin`
only, about 330 with nothing.

The admin manifest (`apiplant-admin.json`) lists the compiled features as
`features`, so a dashboard can tell "not configured" from "not in this build".

**Configuration is checked against the build.** An app whose `main.toml` asks
for something the build lacks is refused at boot, and the error names the
feature to turn on. Nothing is served with a section quietly ignored. The two
exceptions are the subsystems that are on by default in config (`[storage]`,
`[admin]`), as the table says.

The routes a missing feature would have mounted are simply absent, so their
paths are free for your own routes. With `storage` off, for example, `/files`
is yours.

The `apiplant` binary always turns on everything except `typescript`, which
its own `typescript` feature controls (the slim build), so `apiplant run`
behaves the same as before.
