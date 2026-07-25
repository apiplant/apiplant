//! # apiplant-server
//!
//! Turns a loaded [`App`] into a running HTTP service on [`ntex`]:
//!
//! * generic CRUD routes for every resource (`<base>/<resource>[/<id>]`),
//! * built-in auth routes (`<base>/auth/...`),
//! * one route per loaded function (`<base>/functions/<name>`),
//! * [lifecycle hooks](hooks) running functions around each CRUD operation,
//! * TLS inferred from the app's `https/` directory.

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
use ntex::web::{self, guard, App as WebApp, HttpResponse, HttpServer};
use uuid::Uuid;

use functions::FunctionRegistry;
use state::AppState;

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

fn serve_admin(state: &AppState, requested: &str) -> HttpResponse {
    let Some(root) = state.admin_dir.as_deref() else {
        return HttpResponse::NotFound().finish();
    };
    let Some(path) = resolve_admin_path(root, requested) else {
        return HttpResponse::NotFound().finish();
    };
    let body = match fs::read(&path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return HttpResponse::NotFound().finish();
        }
        Err(error) => {
            tracing::error!(path = %path.display(), error = %error, "failed to read admin asset");
            return HttpResponse::InternalServerError().finish();
        }
    };
    HttpResponse::Ok()
        .content_type(content_type_for(&path))
        .body(body)
}

fn resolve_admin_path(root: &Path, requested: &str) -> Option<PathBuf> {
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
    match path.extension().and_then(|ext| ext.to_str()) {
        Some("html") => "text/html; charset=utf-8",
        Some("css") => "text/css; charset=utf-8",
        Some("js") => "application/javascript; charset=utf-8",
        Some("json") => "application/json",
        Some("png") => "image/png",
        Some("svg") => "image/svg+xml",
        Some("txt") => "text/plain; charset=utf-8",
        _ => "application/octet-stream",
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
    let docs_enabled = app.config.docs.enabled;
    let docs_path = app.config.docs.path.clone();
    let spec_url = format!("{base_path}/openapi.json");
    let spec = openapi::build(&app, &registry);
    let openapi_json = serde_json::to_string(&spec).unwrap_or_else(|_| "{}".to_string());
    let docs_html = openapi::swagger_ui_html(&spec_url, &app.config.docs.title);
    if docs_enabled {
        tracing::info!("  docs -> {base_path}{docs_path}  (spec: {spec_url})");
    }

    // 6. Assemble shared state and pull out what the closure needs.
    let host = app.config.server.host.clone();
    let port = app.config.server.port;
    let workers = app.config.server.workers;
    let domain = app.config.server.domain.clone();
    let tls = app.tls.clone();
    let admin_dir = app.root.join("admin");
    let admin_enabled = admin_dir.is_dir();
    if admin_enabled {
        tracing::info!("  admin -> /admin/");
    }

    let state = AppState {
        app: Arc::new(app),
        db,
        auth: authr,
        functions: Arc::new(registry),
        admin_dir: admin_enabled.then_some(admin_dir),
        openapi_json: Arc::new(openapi_json),
        docs_html: Arc::new(docs_html),
    };

    let base_path_log = base_path.clone();
    let mut server = HttpServer::new(move || {
        let mut scope = web::scope(base_path.as_str());
        if let Some(d) = &domain {
            scope = scope.guard(guard::Host(d.clone()));
        }
        // Docs routes (literal segments) are registered before the generic
        // `/{resource}` routes so they win.
        if docs_enabled {
            scope = scope
                .route("/openapi.json", web::get().to(openapi_spec))
                .route(&docs_path, web::get().to(docs_page));
        }
        let scope = scope
            .route("/_health", web::get().to(health))
            .route("/auth/register", web::post().to(auth_routes::register))
            .route("/auth/login", web::post().to(auth_routes::login))
            .route("/auth/apikeys", web::post().to(auth_routes::create_api_key))
            // Literal `functions` segment is registered before the generic
            // resource routes so it wins over `/{resource}/{id}`.
            .route(
                "/functions/{name}",
                web::route().to(function_routes::invoke),
            )
            .service(
                web::resource("/{resource}")
                    .route(web::get().to(crud::list))
                    .route(web::post().to(crud::create)),
            )
            .service(
                web::resource("/{resource}/{id}")
                    .route(web::get().to(crud::get))
                    .route(web::patch().to(crud::update))
                    .route(web::put().to(crud::update))
                    .route(web::delete().to(crud::delete)),
            )
            // Nested has_many: GET /parent/{id}/child
            .route("/{parent}/{id}/{child}", web::get().to(crud::nested_list));
        let mut app = WebApp::new().state(state.clone());
        if admin_enabled {
            let mut admin_index_route = web::resource("/admin/");
            let mut admin_asset_route = web::resource("/admin/{path:.*}");
            if let Some(d) = &domain {
                admin_index_route = admin_index_route.guard(guard::Host(d.clone()));
                admin_asset_route = admin_asset_route.guard(guard::Host(d.clone()));
            }
            app = app
                .service(admin_index_route.route(web::get().to(admin_index)))
                .service(admin_asset_route.route(web::get().to(admin_asset)));
        }
        app.service(scope)
    });

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
