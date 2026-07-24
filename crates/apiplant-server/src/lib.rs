//! # apiplant-server
//!
//! Turns a loaded [`App`] into a running HTTP service on [`ntex`]:
//!
//! * generic CRUD routes for every resource (`<base>/<resource>[/<id>]`),
//! * built-in auth routes (`<base>/auth/...`),
//! * one route per loaded function (`<base>/functions/<name>`),
//! * TLS inferred from the app's `https/` directory.

mod auth_routes;
mod crud;
mod function_routes;
pub mod functions;
mod openapi;
mod response;
mod state;
#[cfg(test)]
mod tests;

use std::sync::Arc;

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
        tracing::info!(
            "  fn {} -> {}/functions/{}",
            f.manifest.name,
            app.config.server.base_path,
            f.manifest.name
        );
    }

    // 4. Generate the OpenAPI document + Swagger UI (once; static per boot).
    let base_path = app.config.server.base_path.clone();
    let docs_enabled = app.config.docs.enabled;
    let docs_path = app.config.docs.path.clone();
    let spec_url = format!("{base_path}/openapi.json");
    let spec = openapi::build(&app, &registry);
    let openapi_json = serde_json::to_string(&spec).unwrap_or_else(|_| "{}".to_string());
    let docs_html = openapi::swagger_ui_html(&spec_url, &app.config.docs.title);
    if docs_enabled {
        tracing::info!(
            "  docs -> {base_path}{docs_path}  (spec: {spec_url})"
        );
    }

    // 5. Assemble shared state and pull out what the closure needs.
    let host = app.config.server.host.clone();
    let port = app.config.server.port;
    let workers = app.config.server.workers;
    let domain = app.config.server.domain.clone();
    let tls = app.tls.clone();

    let state = AppState {
        app: Arc::new(app),
        db,
        auth: authr,
        functions: Arc::new(registry),
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
            .route("/functions/{name}", web::route().to(function_routes::invoke))
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
            .route(
                "/{parent}/{id}/{child}",
                web::get().to(crud::nested_list),
            );
        WebApp::new().state(state.clone()).service(scope)
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
