//! `apiplant studio` — serve the visual editor out of this binary.
//!
//! Studio is a local-first editor: it opens an app directory through the
//! browser's File System Access API and reads and writes it directly, so this
//! command is only a static file server for the embedded build. Nothing is
//! uploaded anywhere, and there is no state on this side.
//!
//! It binds loopback by default, because the page it serves is a licence to
//! edit whichever directory the person at the keyboard grants it — and because
//! the File System Access API needs a secure context, which `localhost` is and
//! a LAN address is not.

use ntex::web::{self, App as WebApp, HttpRequest, HttpResponse, HttpServer};

pub const DEFAULT_HOST: &str = "127.0.0.1";
pub const DEFAULT_PORT: u16 = 5273;

async fn asset(req: HttpRequest) -> HttpResponse {
    let path = req.path().trim_start_matches('/');
    match apiplant_assets::find(apiplant_assets::STUDIO, path) {
        Some(bytes) => HttpResponse::Ok()
            .content_type(apiplant_assets::content_type(if path.is_empty() {
                "index.html"
            } else {
                path
            }))
            .body(bytes),
        None if !path.contains('.') => HttpResponse::Ok()
            .content_type(apiplant_assets::content_type("index.html"))
            .body(apiplant_assets::find(apiplant_assets::STUDIO, "index.html").unwrap()),
        None => HttpResponse::NotFound().finish(),
    }
}

pub async fn serve(host: &str, port: u16) -> anyhow::Result<()> {
    let (listener, port) = apiplant_server::bind::listener(host, port)?;
    let addr = format!("{host}:{port}");
    let server = HttpServer::new(|| WebApp::new().default_service(web::to(asset))).listen(listener)?;

    println!("apiplant studio -> http://{addr}/");
    server.run().await?;
    Ok(())
}
