//! # apiplant-server
//!
//! Turns a loaded [`App`] into a running HTTP service on [`ntex`]:
//!
//! * generic CRUD routes for every resource (`<base>/<resource>[/<id>]`),
//! * built-in auth routes (`<base>/auth/...`),
//! * one route per loaded function (`<base>/functions/<name>`),
//! * [lifecycle hooks](hooks) running functions around each CRUD operation,
//! * the [admin](admin) dashboard, embedded in the binary and served at
//!   `/admin/` for every app unless `[admin] enabled = false`,
//! * [file uploads](storage_routes) (`<base>/uploads`) served back from
//!   `/files/...`, over a directory or an S3-compatible bucket,
//! * the app's `public/` directory served at the site root, with a 404 page,
//! * TLS inferred from the app's `https/` directory.

/// Build the whole `ntex` application from an [`AppState`].
///
/// A macro rather than a function because the type of a fully-assembled `ntex`
/// app is a tower of generics that can't reasonably be written down. Every
/// route the server answers is registered here, and only here — `run` and the
/// tests both go through it, so what the tests exercise is what ships.
///
/// Order matters: the dashboard and the public site are registered *before* the
/// API scope, so their literal paths beat its generic `/{resource}` match. Only
/// paths that name a real file get a route, which is why `/products` still
/// reaches the API while `/about.html` reaches the static site.
macro_rules! build_app {
    ($state:expr) => {{
        let state = $state.clone();
        let config = &state.app.config;
        let domain = config.server.domain.clone();
        let statics = state.statics.clone();

        // An empty `base_path` means "mount the API at the root" — but ntex's
        // `scope("")` matches nothing at all, so it has to be spelled `/`.
        let base_path = match config.server.base_path.as_str() {
            "" => "/",
            path => path,
        };
        let mut scope = $crate::ntex_web::scope(base_path);
        if let Some(g) = $crate::host_guard(&domain) {
            scope = scope.guard(g);
        }
        // Docs routes (literal segments) are registered before the generic
        // `/{resource}` routes so they win.
        if config.docs.enabled {
            scope = scope
                .route(
                    "/openapi.json",
                    $crate::ntex_web::get().to($crate::openapi_spec),
                )
                .route(
                    config.docs.path.as_str(),
                    $crate::ntex_web::get().to($crate::docs_page),
                );
        }
        let mut scope = scope
            .route("/_health", $crate::ntex_web::get().to($crate::health))
            .route(
                "/auth/register",
                $crate::ntex_web::post().to($crate::auth_routes::register),
            )
            .route(
                "/auth/login",
                $crate::ntex_web::post().to($crate::auth_routes::login),
            )
            .route(
                "/auth/me",
                $crate::ntex_web::get().to($crate::auth_routes::me),
            )
            .route(
                "/auth/apikeys",
                $crate::ntex_web::post().to($crate::auth_routes::create_api_key),
            );

        // Acting as somebody else. Mounted only where some door into it is
        // open, so an app that has switched both off has no endpoint to probe.
        if config.impersonation_enabled() {
            scope = scope
                .route(
                    "/auth/impersonate",
                    $crate::ntex_web::post().to($crate::impersonation::start),
                )
                .route(
                    "/auth/impersonate/stop",
                    $crate::ntex_web::post().to($crate::impersonation::stop),
                );
        }

        // Uploads carry their own payload limit, because the framework-wide
        // default is a JSON body's worth and this route exists to take files.
        if let Some(storage) = &state.storage {
            scope = scope.service(
                $crate::ntex_web::resource("/uploads")
                    .state($crate::ntex_web::types::PayloadConfig::new(
                        storage.max_bytes() as usize,
                    ))
                    .route($crate::ntex_web::post().to($crate::storage_routes::upload)),
            );
        }

        // Always mounted, even though `[queues] publish` defaults to `private`
        // and a private policy answers 404. Leaving it unmounted would let
        // `/queues/{topic}` fall through to the generic `/{resource}/{id}`
        // routes and come back 405 — which says "wrong method" about an
        // endpoint that does not exist. The handler's own check gives the 404.
        scope = scope.route(
            "/queues/{topic}",
            $crate::ntex_web::post().to($crate::queue_routes::publish),
        );

        // The flows that reach somebody through their mailbox exist only where
        // this app can actually send mail. An unmounted route answers 404,
        // which is the honest answer: there is no password reset here. The
        // admin manifest carries the same three facts, so no interface offers a
        // button that would land on one.
        if state.invitations_enabled() {
            scope = scope
                .route(
                    "/auth/invitations",
                    $crate::ntex_web::post().to($crate::email_auth::create_invitation),
                )
                .route(
                    "/auth/invitations/{token}",
                    $crate::ntex_web::get().to($crate::email_auth::preview_invitation),
                )
                .route(
                    "/auth/invitations/{token}/accept",
                    $crate::ntex_web::post().to($crate::email_auth::accept_invitation),
                );
        }
        if state.requires_email_verification() {
            scope = scope
                .route(
                    "/auth/verify-email",
                    $crate::ntex_web::post().to($crate::email_auth::verify_email),
                )
                .route(
                    "/auth/verify-email/resend",
                    $crate::ntex_web::post().to($crate::email_auth::resend_verification),
                );
        }
        if state.password_reset_enabled() {
            scope = scope
                .route(
                    "/auth/password/forgot",
                    $crate::ntex_web::post().to($crate::email_auth::forgot_password),
                )
                .route(
                    "/auth/password/reset",
                    $crate::ntex_web::post().to($crate::email_auth::reset_password),
                );
        }

        // Signing in with somebody else's account exists only where a
        // provider is configured. The `GET` pair is a browser following a link
        // — a redirect out and a redirect back — and the `POST` pair is the
        // same handshake for a front end that would rather hold the browser
        // itself; see `oauth_routes`.
        if state.oauth_enabled() {
            scope = scope
                .route(
                    "/auth/oauth",
                    $crate::ntex_web::get().to($crate::oauth_routes::providers),
                )
                .route(
                    "/auth/oauth/{provider}/start",
                    $crate::ntex_web::get().to($crate::oauth_routes::start_redirect),
                )
                .route(
                    "/auth/oauth/{provider}/start",
                    $crate::ntex_web::post().to($crate::oauth_routes::start_json),
                )
                .route(
                    "/auth/oauth/{provider}/callback",
                    $crate::ntex_web::get().to($crate::oauth_routes::callback_redirect),
                )
                .route(
                    "/auth/oauth/{provider}/callback",
                    $crate::ntex_web::post().to($crate::oauth_routes::callback_json),
                )
                .route(
                    "/auth/oauth/{provider}",
                    $crate::ntex_web::delete().to($crate::oauth_routes::unlink),
                );
        }

        // Billing exists only where a provider does. The `billing_*`
        // resources are absent in the same case, so an app that takes no
        // money has neither the endpoints nor the tables.
        if state.payments_enabled() {
            scope = scope
                .route(
                    "/billing/config",
                    $crate::ntex_web::get().to($crate::billing::config),
                )
                .route(
                    "/billing/checkout",
                    $crate::ntex_web::post().to($crate::billing::checkout),
                )
                .route(
                    "/billing/portal",
                    $crate::ntex_web::post().to($crate::billing::portal),
                )
                // Stripe's own deliveries. Not authenticated in the ordinary
                // sense — the body carries a signature — and mounted even
                // without a `webhook_secret`, where it refuses everything and
                // says so, rather than 404ing and leaving an operator to
                // wonder which half is misconfigured.
                .route(
                    "/billing/webhook",
                    $crate::ntex_web::post().to($crate::billing::webhook),
                );
        }

        // The assistant exists only where a provider does, like billing and
        // like the mailbox flows: an app with no `[ai]` section has no
        // endpoint to 404 on and no button offering one.
        if state.ai_enabled() {
            scope = scope
                .route(
                    "/ai/config",
                    $crate::ntex_web::get().to($crate::ai_routes::config),
                )
                .route(
                    "/ai/agents/{name}/chat",
                    $crate::ntex_web::post().to($crate::agent_routes::chat),
                )
                .route(
                    "/ai/chat",
                    $crate::ntex_web::post().to($crate::ai_routes::chat),
                );
        }

        let mut scope = scope
            // Literal `functions` segment is registered before the generic
            // resource routes so it wins over `/{resource}/{id}`.
            // The streaming form is registered first: `/functions/{name}`
            // would otherwise match `/functions/summarise/stream` with a name
            // of `summarise` and lose the suffix.
            .route(
                "/functions/{name}/stream",
                $crate::ntex_web::route().to($crate::function_routes::stream),
            )
            .route(
                "/functions/{name}",
                $crate::ntex_web::route().to($crate::function_routes::invoke),
            )
            .service(
                $crate::ntex_web::resource("/{resource}")
                    .route($crate::ntex_web::get().to($crate::crud::list))
                    .route($crate::ntex_web::post().to($crate::crud::create)),
            )
            .service(
                $crate::ntex_web::resource("/{resource}/{id}")
                    .route($crate::ntex_web::get().to($crate::crud::get))
                    .route($crate::ntex_web::patch().to($crate::crud::update))
                    .route($crate::ntex_web::put().to($crate::crud::update))
                    .route($crate::ntex_web::delete().to($crate::crud::delete)),
            )
            // Nested has_many: GET /parent/{id}/child
            .route(
                "/{parent}/{id}/{child}",
                $crate::ntex_web::get().to($crate::crud::nested_list),
            );

        // With the API mounted at the root, its scope swallows every unmatched
        // path, so the 404 page has to be its default too — not just the app's.
        if statics.not_found_page.is_some() {
            scope = scope.default_service($crate::ntex_web::to($crate::not_found_route));
        }

        // Rate limiting wraps the API and only the API: the dashboard's own
        // assets and the public site are files, and counting a page's images
        // against the allowance for its data is how a limit that was set for
        // an API ends up breaking a browser.
        let scope = scope.wrap($crate::rate_limit::RateLimit::new(::std::sync::Arc::clone(
            &state.rate_limit,
        )));

        // Outside the rate limiter, so a 429 is still counted and still
        // traced: "we started refusing traffic at 14:02" is precisely the
        // thing you want the graph to show, and a limiter that returns before
        // the span exists is a limiter whose effects are invisible.
        let scope = scope.wrap($crate::telemetry::Telemetry::new(::std::sync::Arc::clone(
            &state.telemetry,
        )));

        let mut app = $crate::ntex_web::App::new().state(state.clone());

        // Root-level routes answer for the configured domain only, exactly as
        // the API scope does.
        macro_rules! guarded {
            ($path:expr) => {{
                let resource = $crate::ntex_web::resource($path);
                match $crate::host_guard(&domain) {
                    Some(g) => resource.guard(g),
                    None => resource,
                }
            }};
        }

        if let Some(admin_path) = &statics.admin_path {
            app = app
                .service(
                    guarded!(format!("{admin_path}/"))
                        .route($crate::ntex_web::get().to($crate::admin_index)),
                )
                .service(
                    guarded!(format!("{admin_path}/{{path:.*}}"))
                        .route($crate::ntex_web::get().to($crate::admin_asset)),
                )
                // `/admin` without the slash would otherwise 404; the page loads
                // its assets relatively, so it has to resolve as a directory.
                .service(
                    guarded!(admin_path.as_str())
                        .route($crate::ntex_web::get().to($crate::admin_redirect)),
                );
        }

        // Stored files answer above the API and the static site, on the prefix
        // the stored links carry. Registered before them so a `/files` path in
        // `public/` cannot shadow an upload.
        if let Some(base) = &statics.storage_base {
            // `{key}*` and not `{key:.*}`: ntex's per-segment regex stops at a
            // `/`, and a storage key is dated — `2026/08/…` — so it always has
            // one. The tail form is the only spelling that matches.
            app = app.service(
                guarded!(format!("{base}/{{key}}*"))
                    .route($crate::ntex_web::get().to($crate::storage_routes::serve)),
            );
        }

        for route in &statics.public_routes {
            app = app.service(
                guarded!(route.as_str()).route($crate::ntex_web::get().to($crate::public_asset)),
            );
        }

        app = app.service(scope);
        if statics.not_found_page.is_some() {
            app = app.default_service($crate::ntex_web::to($crate::not_found_route));
        }
        app
    }};
}

/// A `Host:` guard matching any of the configured domains, or `None` when no
/// domains are configured and every host should be answered.
pub(crate) fn host_guard(domains: &[String]) -> Option<ntex_guard::AnyGuard> {
    if domains.is_empty() {
        return None;
    }
    Some(ntex_guard::AnyGuard(
        domains
            .iter()
            .map(|d| Box::new(ntex_guard::Host(d.clone())) as Box<dyn ntex_guard::Guard>)
            .collect(),
    ))
}

pub mod access;
pub mod admin;
mod agent_routes;
mod ai_routes;
mod auth_routes;
mod banner;
mod billing;
pub mod bind;
pub mod builtins;
pub mod cabi;
pub mod call;
mod crud;
pub mod email_auth;
pub mod email_templates;
mod emails;
mod function_routes;
pub mod functions;
pub mod hooks;
pub mod impersonation;
mod oauth_routes;
mod openapi;
mod queue_routes;
pub mod queues;
pub mod rate_limit;
mod response;
mod sse;
mod state;
mod storage_routes;
pub mod telemetry;
#[cfg(test)]
mod tests;

use std::sync::Arc;
use std::{fs, path::Component, path::Path, path::PathBuf};

use apiplant_auth::Authenticator;
use apiplant_core::{App, TlsPaths};
use apiplant_db::Db;
use ntex::web::{self, HttpRequest, HttpResponse, HttpServer};

// Re-exported under crate-local names so `build_app!` can name them absolutely
// and expand anywhere in the crate, tests included.
pub(crate) use ntex::web as ntex_web;
pub(crate) use ntex::web::guard as ntex_guard;
use uuid::Uuid;

use functions::FunctionRegistry;
use state::{AppState, Statics};

async fn health() -> HttpResponse {
    HttpResponse::Ok().json(&serde_json::json!({ "status": "ok", "framework": "apiplant" }))
}

/// Serve the pre-rendered OpenAPI document.
async fn openapi_spec(state: web::types::State<AppState>) -> HttpResponse {
    HttpResponse::Ok()
        .content_type("application/json")
        .body(state.openapi_json.as_str().to_owned())
}

/// Serve the Swagger UI page.
async fn docs_page(state: web::types::State<AppState>) -> HttpResponse {
    HttpResponse::Ok()
        .content_type("text/html; charset=utf-8")
        .body(state.docs_html.as_str().to_owned())
}

async fn admin_index(state: web::types::State<AppState>) -> HttpResponse {
    serve_admin(&state, "index.html")
}

async fn admin_asset(
    state: web::types::State<AppState>,
    path: web::types::Path<String>,
) -> HttpResponse {
    let path = path.into_inner();
    serve_admin(&state, &path)
}

/// Serve one file of the dashboard.
///
/// Everything comes out of the binary: the files from the embedded build, the
/// manifest from memory — it describes *this* app, and is built on boot. There
/// is no directory to generate and none to go stale.
fn serve_admin(state: &AppState, requested: &str) -> HttpResponse {
    let requested = requested.trim_start_matches('/');

    if requested == admin::MANIFEST_FILE {
        return HttpResponse::Ok()
            .content_type("application/json")
            .body(state.admin_manifest.as_str().to_owned());
    }

    match admin::asset(requested) {
        Some(bytes) => HttpResponse::Ok()
            .content_type(apiplant_assets::content_type(requested))
            .body(bytes.into_owned()),
        None => HttpResponse::NotFound().finish(),
    }
}

/// Serve a file from the app's `public/` directory.
///
/// Routes are registered per file at boot, so the path always names something
/// that existed then; it is re-resolved here so edits are picked up without a
/// restart, and a file deleted since boot answers with the 404 page.
async fn public_asset(state: web::types::State<AppState>, req: HttpRequest) -> HttpResponse {
    let Some(root) = state.statics.public_dir.as_deref() else {
        return not_found(&state);
    };
    serve_file(root, req.path()).unwrap_or_else(|| not_found(&state))
}

/// Anything that matched no route at all: the app's 404 page, or a bare 404.
async fn not_found_route(state: web::types::State<AppState>) -> HttpResponse {
    not_found(&state)
}

fn not_found(state: &AppState) -> HttpResponse {
    let Some(page) = state.statics.not_found_page.as_deref() else {
        return HttpResponse::NotFound().finish();
    };
    match fs::read(page) {
        Ok(bytes) => HttpResponse::NotFound()
            .content_type(content_type_for(page))
            .body(bytes),
        Err(error) => {
            tracing::error!(path = %page.display(), error = %error, "failed to read 404 page");
            HttpResponse::NotFound().finish()
        }
    }
}

/// Read a file under `root`, or `None` when it isn't there.
fn serve_file(root: &Path, requested: &str) -> Option<HttpResponse> {
    let path = resolve_static_path(root, requested)?;
    match fs::read(&path) {
        Ok(bytes) => Some(
            HttpResponse::Ok()
                .content_type(content_type_for(&path))
                .body(bytes),
        ),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => {
            tracing::error!(path = %path.display(), error = %error, "failed to read static file");
            Some(HttpResponse::InternalServerError().finish())
        }
    }
}

fn resolve_static_path(root: &Path, requested: &str) -> Option<PathBuf> {
    let mut path = root.to_path_buf();
    let requested = requested.trim_matches('/');

    if requested.is_empty() {
        path.push("index.html");
        return Some(path);
    }

    for component in Path::new(requested).components() {
        match component {
            Component::Normal(segment) => path.push(segment),
            Component::CurDir => {}
            _ => return None,
        }
    }

    if path.is_dir() {
        path.push("index.html");
    }
    Some(path)
}

fn content_type_for(path: &Path) -> &'static str {
    apiplant_assets::content_type(&path.to_string_lossy())
}

/// `/admin` → `/admin/`, so relative asset URLs resolve.
async fn admin_redirect(req: HttpRequest) -> HttpResponse {
    HttpResponse::PermanentRedirect()
        .header("location", format!("{}/", req.path()))
        .finish()
}

/// The route patterns one public file answers on.
///
/// A file is served at its own path; an `index.html` additionally answers for
/// the directory holding it, with and without the trailing slash. Returns
/// nothing for names ntex would read as a path pattern (`{`, `}`) or that would
/// escape the root — those are skipped rather than mis-registered.
fn public_routes(relative: &str) -> Vec<String> {
    if relative
        .split('/')
        .any(|segment| segment.is_empty() || segment.contains(['{', '}']) || segment == "..")
    {
        tracing::warn!(
            file = relative,
            "skipping public file: its name can't be a route"
        );
        return Vec::new();
    }

    let mut routes = vec![format!("/{relative}")];
    if let Some(directory) = relative.strip_suffix("index.html") {
        let directory = directory.trim_end_matches('/');
        if directory.is_empty() {
            routes.push("/".to_string());
        } else {
            routes.push(format!("/{directory}/"));
            routes.push(format!("/{directory}"));
        }
    }
    routes
}

/// Every file under `root`, as site-root-relative paths (`css/app.css`).
///
/// Used to register one route per public file, which is what lets a static site
/// share the root with the API: an explicit `/about.html` route is matched
/// before the generic `/{resource}` CRUD route, while `/products` still reaches
/// the API because no such file exists.
fn walk_public(root: &Path, prefix: &str, into: &mut Vec<String>) {
    let entries = match fs::read_dir(root) {
        Ok(entries) => entries,
        Err(error) => {
            tracing::error!(path = %root.display(), error = %error, "failed to read public directory");
            return;
        }
    };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        let relative = if prefix.is_empty() {
            name
        } else {
            format!("{prefix}/{name}")
        };
        if entry.path().is_dir() {
            walk_public(&entry.path(), &relative, into);
        } else {
            into.push(relative);
        }
    }
}

/// Boot the server for a loaded app and serve until shut down.
pub async fn run(app: App) -> anyhow::Result<()> {
    run_with(app, Options::default()).await
}

/// How to boot.
#[derive(Debug, Clone, Default)]
pub struct Options {
    /// Load the app's `seed/` directory after migrating. Off by default: a
    /// fixture belongs to a fresh database and a development machine, not to
    /// every restart of a production server.
    pub seed: bool,
}

/// Boot the server for a loaded app, with boot-time options.
pub async fn run_with(app: App, options: Options) -> anyhow::Result<()> {
    // 1. Database + migrations.
    let db_url = app.config.database.resolved_url();
    tracing::info!("connecting to database");
    let db = Db::connect(&db_url, app.config.database.max_connections).await?;
    if app.config.database.auto_migrate {
        tracing::info!("running migrations");
        apiplant_db::migrate(db.connection(), &app).await?;
    }
    if options.seed {
        // After the migrations, because the fixture needs its tables — and
        // before anything is served, because a request that arrives mid-seed
        // would see half a fixture.
        let report = apiplant_db::seed::seed(db.connection(), &app).await?;
        if report.is_empty() {
            tracing::warn!("--seed was given but there is no seed/ directory to load");
        } else {
            tracing::info!(
                inserted = report.inserted(),
                already_present = report.skipped(),
                "seeded"
            );
        }
    }

    // 2. Authenticator (ephemeral secret if none configured).
    let secret = if app.config.auth.jwt_secret.is_empty() {
        tracing::warn!(
            "auth.jwt_secret is empty — using an ephemeral secret; sessions won't survive a restart"
        );
        format!("{}{}", Uuid::new_v4(), Uuid::new_v4()).into_bytes()
    } else {
        app.config.auth.jwt_secret.clone().into_bytes()
    };
    let authr = Authenticator::new(secret, app.config.auth.session_ttl_secs);

    // 2b. Optional services a function can reach: the email provider and the
    //     cache. Both are built here, once, and shared by every worker — and
    //     both fail the boot when the app asked for one it can't have, rather
    //     than at the first send or the first lookup.
    let mailer = apiplant_email::Mailer::from_config(&app.config.email)?;
    match &mailer {
        Some(mailer) => tracing::info!(
            "  email -> {} (from {})",
            mailer.provider().as_str(),
            app.config.email.from
        ),
        None => tracing::debug!("no email provider configured"),
    }

    let cache = apiplant_cache::Cache::connect(&app.config.cache).await?;
    match &cache {
        Some(_) => tracing::info!(
            "  cache -> redis (prefix {:?})",
            app.config.cache.prefix.as_str()
        ),
        None => tracing::debug!("no cache configured"),
    }

    let storage = apiplant_storage::Storage::connect(&app.config.storage, &app.root)
        .map_err(|e| apiplant_core::Error::Message(e.to_string()))?;
    match &storage {
        Some(storage) => tracing::info!(
            "  storage -> {} ({}), served at {}/",
            storage.kind(),
            storage.location(),
            storage.public_base()
        ),
        None => tracing::debug!("no storage configured"),
    }

    // The queue is not optional and cannot fail to build: `publish` writes to a
    // built-in table, so it works in an app whose main.toml never mentions
    // queues. What `[queues]` turns on is the subscriber half, below.
    let queue = apiplant_queue::Queue::new(&db, &app);
    if app.config.queues.is_active() {
        for (topic, subscribers) in &app.config.queues.subscribe {
            tracing::info!("  topic {topic} -> {}", subscribers.join(", "));
        }
    }

    let ai = apiplant_ai::Ai::from_config(&app.config.ai)?;
    let agent_ais = app
        .agents
        .values()
        .filter_map(|agent| {
            agent.ai.as_ref().map(|_| {
                apiplant_ai::Ai::from_config(&agent.merged_ai_config(&app.config.ai))
                    .map(|ai| (agent.meta.name.clone(), ai))
            })
        })
        .collect::<Result<Vec<_>, _>>()?
        .into_iter()
        .filter_map(|(name, ai)| ai.map(|ai| (name, ai)))
        .collect();
    match &ai {
        Some(ai) => tracing::info!(
            "  ai -> {} ({} at {})",
            ai.provider().as_str(),
            match ai.model() {
                "" => "the server's own model",
                model => model,
            },
            ai.url()
        ),
        None => tracing::debug!("no ai provider configured"),
    }

    // A buyer Stripe returns to needs somewhere to land, and this crate is
    // the only thing that knows where that is: the dashboard's billing
    // screen, or the app's own origin when the dashboard is switched off.
    let billing_landing = match app.config.admin.enabled {
        true => format!(
            "{}{}/#/billing",
            app.config.server.public_origin(),
            app.config.admin.path.trim_end_matches('/')
        ),
        false => app.config.server.public_origin(),
    };
    let payments =
        apiplant_payments::Payments::from_config(&app.config.payments, &billing_landing)?;
    match &payments {
        Some(payments) => tracing::info!(
            "  payments -> {} ({}, automatic tax {})",
            payments.provider().as_str(),
            app.config.payments.default_currency(),
            match app.config.payments.automatic_tax {
                true => "on",
                false => "off",
            }
        ),
        None => tracing::debug!("no payment provider configured"),
    }

    // Sign-in with somebody else's account. Both failures here are startup
    // failures on purpose: a provider missing its secret, or an app that
    // replaced `oauth_connection` and dropped a column it needs, would
    // otherwise surface as a 500 in front of the first person to press the
    // button — and be discovered by them rather than by whoever deployed it.
    oauth_routes::check_resources(&app).map_err(apiplant_core::Error::Message)?;
    let callback_base = format!(
        "{}{}/auth/oauth",
        app.config.server.public_origin(),
        app.config.server.base_path.trim_end_matches('/'),
    );
    let oauth = apiplant_oauth::Providers::from_config(&app.config.oauth, &callback_base)
        .map_err(|e| apiplant_core::Error::Message(e.to_string()))?;
    match &oauth {
        Some(providers) => {
            for provider in providers.iter() {
                tracing::info!(
                    "  oauth {} -> {}/auth/oauth/{}/start  (redirect URI: {})",
                    provider.label,
                    app.config.server.base_path,
                    provider.key,
                    provider.redirect_uri,
                );
            }
        }
        None => tracing::debug!("no oauth providers configured"),
    }

    // 3. Load dynamic functions.
    let registry = FunctionRegistry::load(&app);
    for f in registry.iter() {
        // A `Private` function has no route — it exists to be called from a
        // hook — so don't advertise one it would answer 404 on.
        if f.manifest.visibility == apiplant_abi::Visibility::Private {
            tracing::info!("  fn {} (private — no endpoint)", f.manifest.name);
        } else {
            tracing::info!(
                "  fn {} -> {}/functions/{}",
                f.manifest.name,
                app.config.server.base_path,
                f.manifest.name
            );
        }
    }

    // 3b. A subscription pointing at a function that isn't loaded is a topic
    //     whose messages will queue up and fail their way to the dead-letter,
    //     one retry cycle at a time. Say so now, at boot, rather than letting it
    //     be discovered as a growing `failed` count.
    for name in app.config.queues.subscribed_functions() {
        if registry.get(name).is_none() {
            tracing::error!(
                function = name,
                "a [queues.subscribe] entry names a function that is not loaded — \
                 messages on its topic will retry and then fail"
            );
        }
    }

    // 4. Report the resource hooks, loudly flagging any that can't resolve —
    //    a missing hook function fails its requests closed at runtime.
    for resource in app.resources.values() {
        for (event, function) in resource.hooks.iter() {
            if registry.get(function).is_some() {
                tracing::info!(
                    "  hook {}.{} -> {}",
                    resource.meta.name,
                    event.as_str(),
                    function
                );
            } else {
                tracing::error!(
                    resource = %resource.meta.name,
                    hook = event.as_str(),
                    function = function,
                    "hook function is not loaded — this resource's {} requests will fail with 500",
                    event.action()
                );
            }
        }
        for (event, function) in resource.hooks.auth_iter() {
            if registry.get(function).is_some() {
                tracing::info!("  hook auth.{} -> {}", event.as_str(), function);
            } else {
                tracing::error!(
                    hook = event.as_str(),
                    function = function,
                    "auth hook function is not loaded — {} requests will fail with 500",
                    event.action()
                );
            }
        }
    }

    // 5. Generate the OpenAPI document + Swagger UI (once; static per boot).
    let base_path = app.config.server.base_path.clone();
    let spec_url = format!("{base_path}/openapi.json");
    let spec = openapi::build(&app, &registry, mailer.is_some());
    let openapi_json = serde_json::to_string(&spec).unwrap_or_else(|_| "{}".to_string());
    let docs_html = openapi::swagger_ui_html(&spec_url, &app.docs_title());
    if app.config.docs.enabled {
        tracing::info!(
            "  docs -> {base_path}{}  (spec: {spec_url})",
            app.config.docs.path
        );
    }

    // 6. Assemble shared state and pull out what the closure needs.
    let host = app.config.server.host.clone();
    let port = app.config.server.port;
    let banner_docs_path = app
        .config
        .docs
        .enabled
        .then(|| app.config.docs.path.clone());
    let banner_domains = app.config.server.domain.clone();
    let banner_name = app.display_name();
    let workers = app.config.server.workers;
    let tls = app.tls.clone();

    // 7. Work out what is served alongside the API — the dashboard, the public
    //    site, the 404 page — and build the dashboard's manifest.
    //
    //    The dashboard ships inside the binary, so every app has one without
    //    generating anything; an `admin/` directory in the app (from `apiplant
    //    admin`) overrides the embedded build file for file. Either way the
    //    manifest is derived here, from the app being served, and the dashboard
    //    talks to its own origin — no CORS, and no rebuild after a resource change.
    let statics = Statics::resolve(&app);
    let banner_admin_path = statics.admin_path.clone();
    let banner_site = !statics.public_routes.is_empty();
    let admin_manifest = match &statics.admin_path {
        Some(path) => {
            tracing::info!("  admin -> {path}/");
            admin::manifest_json(&app, &registry, base_path.clone(), mailer.is_some()).unwrap_or_else(|error| {
                tracing::error!(%error, "failed to build the admin manifest — the dashboard will not load");
                "{}".to_string()
            })
        }
        None => String::new(),
    };
    if let Some(dir) = &statics.public_dir {
        tracing::info!(
            routes = statics.public_routes.len(),
            "  public -> /  (from {})",
            dir.display()
        );
    }
    if let Some(page) = &statics.not_found_page {
        tracing::info!("  404 -> {}", page.display());
    }

    // Resolved before the state is assembled, and from inside the runtime:
    // every limit gets a bucket with a task sweeping the clients that stopped
    // calling.
    let rate_limit = rate_limit::RateLimitPolicy::build(&app, &registry);
    if rate_limit.is_active() {
        tracing::info!(
            overrides = rate_limit.overrides(),
            "  rate limit -> {}",
            app.config.rate_limit.default.as_string()
        );
    }

    // Compiled before the state is assembled, and fatally: a template that does
    // not parse was written to be used, and an app that boots without it sends
    // the built-in message instead, which looks exactly like the override
    // working until somebody reads their mail.
    let email_templates = Arc::new(email_templates::EmailTemplates::load(&app.root)?);
    if !email_templates.names().is_empty() {
        tracing::info!("  emails -> {}", email_templates.names().join(", "));
    }

    let telemetry =
        telemetry::TelemetryPolicy::build(&app.config.observability, &app.config.server.base_path);
    if telemetry.is_active() {
        tracing::info!(
            traces = app.config.observability.traces.enabled,
            metrics = app.config.observability.metrics.enabled,
            "  observability -> {}",
            app.config
                .observability
                .endpoint()
                .unwrap_or_else(|| "in-process".to_string())
        );
    }

    let state = AppState {
        app: Arc::new(app),
        db,
        auth: authr,
        functions: Arc::new(registry),
        mailer,
        email_templates,
        cache,
        storage,
        payments,
        ai,
        oauth: oauth.map(Arc::new),
        queue: queue.clone(),
        agent_ais: Arc::new(agent_ais),
        rate_limit: Arc::new(rate_limit),
        telemetry: Arc::new(telemetry),
        statics: Arc::new(statics),
        admin_manifest: Arc::new(admin_manifest),
        openapi_json: Arc::new(openapi_json),
        docs_html: Arc::new(docs_html),
    };

    // The subscriber runs once per *process*, not once per HTTP worker: each
    // worker gets its own runtime and its own copy of the app, and starting a
    // subscriber in each would have four of them competing over the same rows.
    // `SKIP LOCKED` would keep that correct, but it would also mean four idle
    // `LISTEN` connections per replica for no extra throughput.
    //
    // Deliberately spawned even when nothing is subscribed — the loop exits
    // immediately in that case — so there is one place this is decided.
    if state.app.config.queues.is_active() {
        let subscriber = queues::Subscriber {
            db: state.db.clone(),
            queue: queue.clone(),
            functions: Arc::clone(&state.functions),
            mailer: state.mailer.clone(),
            email_templates: state.email_templates.clone(),
            cache: state.cache.clone(),
            payments: state.payments.clone(),
            ai: state.ai.clone(),
            database_url: db_url.clone(),
            worker: format!("{}:{}", hostname(), std::process::id()),
        };
        tokio::spawn(queues::run(subscriber));
    }

    let base_path_log = base_path.clone();
    let mut server = HttpServer::new(move || build_app!(state));

    if let Some(w) = workers {
        server = server.workers(w);
    }

    // The socket is claimed here rather than through `bind` so that a port
    // collision can be reported by name — and another port offered — instead of
    // surfacing as a bare `Address already in use`.
    let (listener, port) = bind::listener(&host, port)?;
    let addr = format!("{host}:{port}");
    let scheme = if tls.is_some() { "https" } else { "http" };
    let server = match tls {
        Some(paths) => server.listen_rustls(listener, load_tls(&paths)?)?,
        None => server.listen(listener)?,
    };

    tracing::info!("apiplant listening on {scheme}://{addr}{base_path_log}");
    banner::Banner {
        name: banner_name,
        scheme,
        addr: addr.clone(),
        base_path: base_path_log.clone(),
        docs_path: banner_docs_path,
        admin_path: banner_admin_path,
        site: banner_site,
        domains: banner_domains,
    }
    .print();
    server.run().await?;
    Ok(())
}

/// This machine's name, for `queue_message.claimed_by`.
///
/// Best-effort and never fatal: it is there so that "which replica keeps dying
/// holding messages" is answerable from the table, and an unknown host is
/// merely a less useful answer than a wrong one would be.
fn hostname() -> String {
    std::env::var("HOSTNAME")
        .ok()
        .filter(|h| !h.trim().is_empty())
        .unwrap_or_else(|| "unknown".to_string())
}

/// Build a rustls server config from PEM cert + key files.
fn load_tls(paths: &TlsPaths) -> anyhow::Result<rustls::ServerConfig> {
    use std::io::BufReader;

    // Install a default crypto provider once (ring); ignore "already set".
    let _ = rustls::crypto::ring::default_provider().install_default();

    let mut cert_reader = BufReader::new(std::fs::File::open(&paths.cert)?);
    let certs = rustls_pemfile::certs(&mut cert_reader).collect::<Result<Vec<_>, _>>()?;

    let mut key_reader = BufReader::new(std::fs::File::open(&paths.key)?);
    let key = rustls_pemfile::private_key(&mut key_reader)?
        .ok_or_else(|| anyhow::anyhow!("no private key in {}", paths.key.display()))?;

    let config = rustls::ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(certs, key)?;
    Ok(config)
}

#[cfg(test)]
mod route_tests {
    use super::*;

    #[test]
    fn an_index_answers_for_its_directory_too() {
        assert_eq!(public_routes("index.html"), ["/index.html", "/"]);
        assert_eq!(
            public_routes("guide/index.html"),
            ["/guide/index.html", "/guide/", "/guide"]
        );
        assert_eq!(public_routes("css/app.css"), ["/css/app.css"]);
    }

    #[test]
    fn names_that_cannot_be_routes_are_skipped() {
        assert!(public_routes("weird{name}.html").is_empty());
        assert!(public_routes("../escape.html").is_empty());
    }

    #[test]
    fn static_paths_resolve_under_the_root_and_never_above_it() {
        let root = Path::new("/srv/app/public");
        assert_eq!(
            resolve_static_path(root, "/css/app.css"),
            Some(root.join("css/app.css"))
        );
        assert_eq!(
            resolve_static_path(root, "/"),
            Some(root.join("index.html"))
        );
        assert_eq!(resolve_static_path(root, "/../main.toml"), None);
    }
}
