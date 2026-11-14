//! Uploading a file, and serving it back.
//!
//! Two endpoints and no ceremony:
//!
//! * `POST <base>/uploads` takes the file as the **raw request body**, with its
//!   name in `?filename=` and its type in `Content-Type`, and answers
//!   `{"url": "/files/…"}`. Not multipart: an upload is one file, a browser
//!   sends a `File` object as a body without any encoding step, and the
//!   framework avoids a parser it would otherwise have to carry.
//! * `GET <public_base>/{key}` reads it back out of whichever backend
//!   `[storage]` names, which is what lets a stored link stay relative.
//!
//! The read side is deliberately unauthenticated. A stored URL is unguessable
//! (a UUID per object) and ends up in `<img src>` tags, style sheets and mail —
//! places that cannot send a bearer token. Files that must not be readable by
//! anyone holding the link do not belong in a `file` field.

use crate::state::State;
use apiplant_abi::FunctionAccess;
use ntex::web::{HttpRequest, HttpResponse};
use serde_json::json;

use crate::response::{error, ok};
use crate::state::AppState;

/// `POST <base>/uploads` — store one file, and answer with the link to it.
///
/// Authenticated: an upload spends disk (or somebody's S3 bill), so the caller
/// has to be somebody. It is not org-scoped, because the two things every app
/// uploads — a user's avatar and an organisation's logo — are set from either
/// side of that line.
pub async fn upload(
    req: HttpRequest,
    state: State<AppState>,
    mut payload: ntex::web::types::Payload,
) -> HttpResponse {
    let Some(storage) = state.storage.clone() else {
        return error(404, "this app does not store files");
    };
    if let Err(response) = crate::access::check(
        &state,
        &req,
        &FunctionAccess::Authenticated.into(),
        "this app does not store files",
    )
    .await
    {
        return response;
    }

    // Read only now, after the caller is known: an anonymous upload is turned
    // away before a byte of it is buffered. The limit is enforced chunk by
    // chunk as the body arrives, so an oversized file is refused at the first
    // chunk past the limit rather than after it has been held in memory. (The
    // app-wide payload limit is for JSON bodies and does not apply here.)
    let limit = storage.max_bytes();
    let mut buffer = ntex::util::BytesMut::new();
    while let Some(chunk) = payload.recv().await {
        let chunk = match chunk {
            Ok(chunk) => chunk,
            Err(_) => return error(400, "the upload was interrupted"),
        };
        if (buffer.len() + chunk.len()) as u64 > limit {
            return error(
                413,
                format!(
                    "the file is larger than the {} MB limit",
                    limit / (1024 * 1024)
                ),
            );
        }
        buffer.extend_from_slice(&chunk);
    }
    let body = buffer.freeze();
    if body.is_empty() {
        return error(400, "the request body is empty");
    }

    let content_type = req
        .headers()
        .get("content-type")
        .and_then(|value| value.to_str().ok())
        .unwrap_or("application/octet-stream")
        .to_string();
    if !storage.allows_type(&content_type) {
        return error(
            415,
            format!("this app does not accept {content_type} uploads"),
        );
    }

    let filename = query_param(req.query_string(), "filename").unwrap_or_default();
    let key = storage.key_for(&filename);

    match storage.put(&key, body.to_vec(), &content_type).await {
        Ok(()) => ok(&json!({
            "url": storage.url_for(&key),
            "key": key,
            "size": body.len(),
            "content_type": content_type,
        })),
        Err(e) => {
            // The caller gets a flat 500: a bucket name or a filesystem path in
            // an error body tells them about our deployment, not about theirs.
            crate::telemetry::record_error("storage", &e);
            tracing::error!(error = %e, %key, "failed to store an upload");
            error(500, "the file could not be stored")
        }
    }
}

/// `GET <public_base>/{key}` — read a stored file back.
pub async fn serve(req: HttpRequest, state: State<AppState>) -> HttpResponse {
    let Some(storage) = state.storage.clone() else {
        return HttpResponse::NotFound().build();
    };
    let Some(key) = storage.key_from_path(req.path()) else {
        return HttpResponse::NotFound().build();
    };

    match storage.get(&key).await {
        Ok(Some(object)) => HttpResponse::Ok()
            .content_type(object.content_type.as_str())
            // Immutable in the literal sense: a key is minted per upload and
            // never written twice, so nothing behind this URL can change.
            .header("cache-control", "public, max-age=31536000, immutable")
            // Belt to `content_type_for`'s braces: whatever the type says, the
            // browser must not sniff an uploaded file into something scriptable.
            .header("x-content-type-options", "nosniff")
            .header("content-disposition", "inline")
            .body(object.bytes),
        Ok(None) => HttpResponse::NotFound().build(),
        Err(e) => {
            crate::telemetry::record_error("storage", &e);
            tracing::error!(error = %e, %key, "failed to read a stored file");
            HttpResponse::InternalServerError().build()
        }
    }
}

/// One value out of a query string, percent-decoded.
///
/// The filename is decoration — it is appended to a key that is already unique,
/// and sanitised after this — so a malformed escape is dropped rather than
/// refused.
fn query_param(query: &str, name: &str) -> Option<String> {
    query.split('&').find_map(|pair| {
        let (key, value) = pair.split_once('=')?;
        (key == name).then(|| percent_decode(value))
    })
}

fn percent_decode(raw: &str) -> String {
    let bytes = raw.replace('+', " ").into_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'%' if i + 2 < bytes.len() => {
                match u8::from_str_radix(&String::from_utf8_lossy(&bytes[i + 1..i + 3]), 16) {
                    Ok(byte) => {
                        out.push(byte);
                        i += 3;
                    }
                    Err(_) => {
                        out.push(bytes[i]);
                        i += 1;
                    }
                }
            }
            byte => {
                out.push(byte);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_filename_survives_the_query_string() {
        assert_eq!(
            query_param("filename=Logo%20Final.png", "filename").as_deref(),
            Some("Logo Final.png")
        );
        assert_eq!(
            query_param("a=1&filename=x.png&b=2", "filename").as_deref(),
            Some("x.png")
        );
        assert_eq!(query_param("a=1", "filename"), None);
        // A broken escape is a worse name, not an error.
        assert_eq!(
            query_param("filename=a%zz", "filename").as_deref(),
            Some("a%zz")
        );
    }
}
