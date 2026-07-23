//! The `apiplant` executable.
//!
//! ```text
//! apiplant [APP_DIR]      # serve the app in APP_DIR (default: current dir)
//! apiplant --check [DIR]  # load & validate the app, then exit
//! ```
//!
//! An *app directory* holds an optional `main.toml`, an optional `models/`
//! directory of resource definitions, an optional `functions/` directory of
//! compiled function libraries, and — to enable TLS — an `https/` directory
//! with a certificate and key. Every piece is optional; an empty directory is a
//! valid (if bare) app.

use apiplant_core::App;

#[ntex::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info,apiplant=debug".into()),
        )
        .init();

    let mut check_only = false;
    let mut dir = String::from(".");
    for arg in std::env::args().skip(1) {
        match arg.as_str() {
            "--check" => check_only = true,
            "-h" | "--help" => {
                println!("usage: apiplant [--check] [APP_DIR]");
                return Ok(());
            }
            other => dir = other.to_string(),
        }
    }

    tracing::info!("loading app from {dir}");
    let app = App::load(&dir)?;
    tracing::info!(
        resources = app.resources.len(),
        tls = app.tls.is_some(),
        "app loaded"
    );

    if check_only {
        for name in app.resources.keys() {
            println!("resource: {name}");
        }
        println!("ok");
        return Ok(());
    }

    apiplant_server::run(app).await
}
