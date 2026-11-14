//! `POST <base>/queues/{topic}` — publishing from outside the app.
//!
//! One endpoint, and it is off unless the app asks for it. `[queues] publish`
//! defaults to `private`, which means this route answers `404`: a topic is an
//! internal name wired to real work, and an app that exposes one should have
//! decided to rather than discovered it.
//!
//! What it is for is the publisher that is not a function — a webhook from a
//! service that has no apiplant SDK, a script, another service in the same
//! system. Everything already inside the app should publish through a
//! function's `publish`, which needs no HTTP round trip and no credential.
//!
//! It answers `202 Accepted`, not `200`: the message is written down, and that
//! is the entire promise. Whether the handler succeeds is not knowable yet and
//! will never be reported here.

use crate::state::State;
use ntex::web::types::Path;
use ntex::web::{HttpRequest, HttpResponse};
use serde_json::Value;

use crate::response::error;
use crate::state::AppState;

/// `POST <base>/queues/{topic}` — queue the request body as a message.
pub async fn publish(
    req: HttpRequest,
    state: State<AppState>,
    topic: Path<String>,
    body: ntex::util::Bytes,
) -> HttpResponse {
    let access = state.app.config.queues.publish_access();
    // Matches every other `private` endpoint in the framework: not forbidden,
    // absent. Probing must not reveal which topics an app has.
    let missing = "this app does not accept published messages";
    if let Err(response) = crate::access::check(
        &state,
        &req,
        &crate::access::to_function_policy(&access),
        missing,
    )
    .await
    {
        return response;
    }

    let topic = topic.into_inner();
    // An empty body is a signal — "this happened" with nothing to add — and a
    // signal is a perfectly good message.
    let message: Value = match body.is_empty() {
        true => Value::Object(Default::default()),
        false => match serde_json::from_slice(&body) {
            Ok(value) => value,
            Err(e) => return error(400, format!("the body is not valid JSON: {e}")),
        },
    };

    // Attributing the message to whoever published it is the point of requiring
    // a credential at all: the handler, running later and elsewhere, still
    // knows who caused the work.
    let principal = state
        .resolve_principal(&req)
        .await
        .map(|p| p.user_id.to_string())
        .unwrap_or_default();

    match state.queue.publish(&topic, &message, &principal).await {
        Ok(publication) => HttpResponse::Accepted().json(&serde_json::json!({
            "id": publication.id,
            "topic": publication.topic,
            "delivered": publication.delivered,
        })),
        // The only errors reaching here are a malformed topic (the caller's
        // fault, and safe to explain) and a database failure (ours, and not).
        Err(apiplant_queue::QueueError::Request(message)) => error(400, message),
        Err(e) => {
            crate::telemetry::record_error("queue_publish", &e);
            tracing::error!(error = %e, %topic, "could not queue a published message");
            error(500, "the message could not be queued")
        }
    }
}
