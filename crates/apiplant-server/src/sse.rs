//! Server-sent events: the wire format both streaming endpoints answer in.
//!
//! SSE rather than a WebSocket or a raw chunked body, for three reasons that
//! all come down to it being the boring choice: it is one-directional, which is
//! what a generated answer is; it survives proxies, because it is an ordinary
//! HTTP response; and `EventSource` is in every browser, so the client half is
//! four lines and no library.
//!
//! Every stream this server produces has the same three event types, so one
//! client handles both `<base>/ai/chat` and a function's `/stream`:
//!
//! ```text
//! event: delta
//! data: {"text":"Hel"}
//!
//! event: error
//! data: {"error":"the provider refused the request"}
//!
//! event: done
//! data: {"finish_reason":"stop"}
//! ```
//!
//! `done` is always last and always sent — including after an `error`, so a
//! client has exactly one place to stop listening.

use ntex::util::Bytes;
use serde_json::{json, Value};

/// The content type an SSE response is served as.
pub const CONTENT_TYPE: &str = "text/event-stream";

/// Format one event. `data` is always JSON, so a client parses every frame the
/// same way rather than switching on the event name first.
pub fn event(name: &str, data: &Value) -> Bytes {
    // A JSON document is serialised without newlines, so it needs no
    // multi-line `data:` handling — but a string with an escaped newline in it
    // is still one line here, which is exactly why the payload is JSON.
    Bytes::from(format!("event: {name}\ndata: {data}\n\n"))
}

/// More text, to append to what came before.
pub fn delta(text: &str) -> Bytes {
    event("delta", &json!({ "text": text }))
}

/// Something went wrong. Always followed by [`done`].
pub fn failure(message: &str) -> Bytes {
    event("error", &json!({ "error": message }))
}

/// The stream is over. Nothing follows.
pub fn done(data: &Value) -> Bytes {
    event("done", data)
}

/// Headers that keep a stream a stream.
///
/// Buffering is the failure mode here and it is invisible: a proxy that holds
/// the response until it is complete turns a token-by-token answer back into a
/// single slow one, and everything still "works". `X-Accel-Buffering` is
/// nginx's opt-out and is ignored by everything else.
pub fn headers(response: &mut ntex::web::HttpResponseBuilder) {
    response
        .content_type(CONTENT_TYPE)
        .header("cache-control", "no-cache, no-transform")
        .header("connection", "keep-alive")
        .header("x-accel-buffering", "no");
}

/// The error type a streaming body reports. Nothing produces one — every
/// failure is delivered *inside* the stream as an `error` event, because by the
/// time a stream is failing its status code has long since been sent.
#[derive(Debug)]
pub enum Never {}

impl std::fmt::Display for Never {
    fn fmt(&self, _: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match *self {}
    }
}

impl std::error::Error for Never {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_event_is_one_frame_with_a_json_payload() {
        assert_eq!(
            delta("Hel"),
            Bytes::from("event: delta\ndata: {\"text\":\"Hel\"}\n\n")
        );
    }

    /// The one thing that would corrupt the framing is a newline in the
    /// payload, and JSON encoding is what prevents it.
    #[test]
    fn a_newline_in_the_text_cannot_break_the_framing() {
        let frame = delta("one\ntwo");
        let text = std::str::from_utf8(&frame).unwrap();
        assert_eq!(text.matches("\n\n").count(), 1);
        assert!(text.ends_with("\n\n"));
        assert!(text.contains(r#"{"text":"one\ntwo"}"#));
    }
}
