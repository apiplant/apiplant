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
        if let Some(d) = &domain {
            scope = scope.guard($crate::ntex_guard::Host(d.clone()));
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
                "/auth/apikeys",
                $crate::ntex_web::post().to($crate::auth_routes::create_api_key),
            )
            // Literal `functions` segment is registered before the generic
            // resource routes so it wins over `/{resource}/{id}`.
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

        let mut app = $crate::ntex_web::App::new().state(state.clone());

        // Root-level routes answer for the configured domain only, exactly as
        // the API scope does.
        macro_rules! guarded {
            ($path:expr) => {{
                let resource = $crate::ntex_web::resource($path);
                match &domain {
                    Some(d) => resource.guard($crate::ntex_guard::Host(d.clone())),
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

pub mod admin;
mod auth_routes;
pub mod cabi;
mod crud;
mod function_routes;
pub mod functions;
pub mod hooks;
mod openapi;
mod response;
mod state;
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
/// The manifest is answered from memory — it describes *this* app, and is built
/// on boot rather than generated into a directory first. Everything else comes
/// from the app's own `admin/` build when it has one (so `apiplant admin`
/// output still wins, customisations and all) and from the embedded copy
/// otherwise.
fn serve_admin(state: &AppState, requested: &str) -> HttpResponse {
    let requested = requested.trim_start_matches('/');

    if requested == admin::MANIFEST_FILE {
        return HttpResponse::Ok()
            .content_type("application/json")
            .body(state.admin_manifest.as_str().to_owned());
    }

    if let Some(root) = state.statics.admin_dir.as_deref() {
        return serve_file(root, requested).unwrap_or_else(|| HttpResponse::NotFound().finish());
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
    // 1. Database + migrations.
    let db_url = app.config.database.resolved_url();
    tracing::info!("connecting to database");
    let db = Db::connect(&db_url, app.config.database.max_connections).await?;
    if app.config.database.auto_migrate {
        tracing::info!("running migrations");
        apiplant_db::migrate(db.connection(), &app).await?;
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

    // 3. Load dynamic functions.
    let registry = FunctionRegistry::load_dir(&app.functions_dir);
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
    }

    // 5. Generate the OpenAPI document + Swagger UI (once; static per boot).
    let base_path = app.config.server.base_path.clone();
    let spec_url = format!("{base_path}/openapi.json");
    let spec = openapi::build(&app, &registry);
    let openapi_json = serde_json::to_string(&spec).unwrap_or_else(|_| "{}".to_string());
    let docs_html = openapi::swagger_ui_html(&spec_url, &app.config.docs.title);
    if app.config.docs.enabled {
        tracing::info!(
            "  docs -> {base_path}{}  (spec: {spec_url})",
            app.config.docs.path
        );
    }

    // 6. Assemble shared state and pull out what the closure needs.
    let host = app.config.server.host.clone();
    let port = app.config.server.port;
    let workers = app.config.server.workers;
    let tls = app.tls.clone();

    // 7. Work out what is served alongside the API — the dashboard, the public
    //    site, the 404 page — and build the dashboard's manifest.
    //
    //    The dashboard ships inside the binary, so every app has one without
    //    generating anything; an `admin/` directory in the app (from `apiplant
    //    admin`) overrides the embedded build file for file. Either way the
    //    manifest is derived here, from the app being served, and the dashboard
    //    talks to its own origin — no CORS, and no rebuild after a model change.
    let statics = Statics::resolve(&app);
    let admin_manifest = match &statics.admin_path {
        Some(path) => {
            tracing::info!(
                "  admin -> {path}/{}",
                match &statics.admin_dir {
                    Some(dir) => format!("  (from {})", dir.display()),
                    None => String::new(),
                }
            );
            admin::manifest_json(&app, &registry, base_path.clone()).unwrap_or_else(|error| {
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

    let state = AppState {
        app: Arc::new(app),
        db,
        auth: authr,
        functions: Arc::new(registry),
        statics: Arc::new(statics),
        admin_manifest: Arc::new(admin_manifest),
        openapi_json: Arc::new(openapi_json),
        docs_html: Arc::new(docs_html),
    };

    let base_path_log = base_path.clone();
    let mut server = HttpServer::new(move || build_app!(state));

    if let Some(w) = workers {
        server = server.workers(w);
    }

    let addr = format!("{host}:{port}");
    let scheme = if tls.is_some() { "https" } else { "http" };
    let server = match tls {
        Some(paths) => server.bind_rustls(&addr, load_tls(&paths)?)?,
        None => server.bind(&addr)?,
    };

    tracing::info!("apiplant listening on {scheme}://{addr}{base_path_log}");
    server.run().await?;
    Ok(())
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
