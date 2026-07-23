//! Small helpers for JSON responses and error mapping.

use ntex::http::StatusCode;
use ntex::web::HttpResponse;
use serde_json::json;

/// A JSON `{ "error": ... }` body with the given status.
pub fn error(status: u16, message: impl Into<String>) -> HttpResponse {
    HttpResponse::build(StatusCode::from_u16(status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR))
        .json(&json!({ "error": message.into() }))
}

/// Map a database error to an HTTP response. Bad input and common constraint
/// violations become 4xx; everything else is a logged 500.
pub fn db_error(e: apiplant_db::Error) -> HttpResponse {
    match e {
        apiplant_db::Error::BadInput(msg) => error(400, msg),
        apiplant_db::Error::Db(ref dberr) => {
            let msg = dberr.to_string();
            let lower = msg.to_lowercase();
            if lower.contains("foreign key") {
                error(400, "references a record that does not exist")
            } else if lower.contains("unique") || lower.contains("duplicate key") {
                error(409, "a record with these values already exists")
            } else if lower.contains("not-null") || lower.contains("not null") {
                error(400, "a required field is missing")
            } else {
                tracing::error!(error = %msg, "database error");
                error(500, "internal error")
            }
        }
        other => {
            tracing::error!(error = %other, "database error");
            error(500, "internal error")
        }
    }
}

/// 200 with a JSON body.
pub fn ok(value: &serde_json::Value) -> HttpResponse {
    HttpResponse::Ok().json(value)
}
