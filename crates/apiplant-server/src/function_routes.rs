//! HTTP surface for dynamically-loaded functions, mounted at
//! `<base>/functions/{name}` — and, for a function that produces its answer in
//! pieces, `<base>/functions/{name}/stream`.
//!
//! ## The two endpoints are one function
//!
//! The same handler answers both. A function streams by calling `emit` as it
//! goes; on the plain endpoint those chunks go nowhere and the caller gets the
//! return value, on `/stream` each one is forwarded the moment it is produced
//! and the return value arrives last as the `done` event. So "can this be
//! streamed" is a question about the *caller*, not about the function — which
//! is what lets one function serve a browser that wants tokens as they land and
//! a cron job that wants a JSON document.

use apiplant_abi::HttpMethod;
use ntex::http::Method;
use ntex::util::Bytes;
use ntex::web::types::{Path, State};
use ntex::web::{HttpRequest, HttpResponse};
use serde_json::json;

use crate::functions::HostBridge;
use crate::response::error;
use crate::sse;
use crate::state::AppState;

fn expected_method(m: HttpMethod) -> Method {
    match m {
        HttpMethod::Get => Method::GET,
        HttpMethod::Post => Method::POST,
        HttpMethod::Put => Method::PUT,
        HttpMethod::Delete => Method::DELETE,
    }
}

/// What both endpoints need before they can run anything: the function exists,
/// the method is right, and the caller is allowed.
///
/// `Err` is the response to send.
async fn admit(
    req: &HttpRequest,
    state: &State<AppState>,
    name: &str,
    body: String,
) -> Result<Ready, HttpResponse> {
    // Read manifest fields we need before crossing into the blocking task.
    let (method, access, config_json) = match state.functions.get(name) {
        Some(f) => (
            f.manifest.method,
            f.manifest.access(),
            f.config_json.clone(),
        ),
        None => return Err(error(404, format!("unknown function `{name}`"))),
    };

    if req.method() != expected_method(method) {
        return Err(error(405, "method not allowed"));
    }

    // The same vocabulary a resource's `[permissions]` uses, minus `owner`: a
    // function call has no row to own.
    let principal = crate::access::check(state, req, &access, "unknown function").await?;

    Ok(Ready {
        input: match body.trim().is_empty() {
            true => "{}".to_string(),
            false => body,
        },
        config_json,
        principal_id: principal
            .as_ref()
            .map(|p| p.user_id.to_string())
            .unwrap_or_default(),
    })
}

/// An admitted call, ready to run.
struct Ready {
    input: String,
    /// The function's `functions/<name>.toml`, read here so the blocking
    /// worker needs nothing from the registry but the function itself.
    config_json: String,
    principal_id: String,
}

/// Everything the blocking worker needs, pulled off the state while we are
/// still on the async side.
fn bridge(state: &State<AppState>, ready: &Ready) -> HostBridge {
    HostBridge::new(
        state.db.clone(),
        tokio::runtime::Handle::current(),
        ready.config_json.clone(),
        ready.principal_id.clone(),
    )
    .with_services(
        state.mailer.clone(),
        state.cache.clone(),
        state.payments.clone(),
        state.ai.clone(),
    )
}

/// Dispatch a request to a loaded function. Enforces the manifest's method and
/// visibility, then runs `invoke` on a blocking worker (the host bridge blocks
/// on the async database from there).
pub async fn invoke(
    req: HttpRequest,
    state: State<AppState>,
    path: Path<String>,
    body: String,
) -> HttpResponse {
    let name = path.into_inner();
    let ready = match admit(&req, &state, &name, body).await {
        Ok(ready) => ready,
        Err(response) => return response,
    };

    let functions = state.functions.clone();
    let name2 = name.clone();
    let bridge = bridge(&state, &ready);
    let input = ready.input;

    let result = tokio::task::spawn_blocking(move || {
        let f = functions.get(&name2).expect("checked above");
        f.invoke(bridge, &input)
    })
    .await;

    match result {
        Ok(Ok(json)) => HttpResponse::Ok()
            .content_type("application/json")
            .body(json),
        // The function faulted rather than faulting the caller. Log the detail and
        // answer generically — it names internals the caller shouldn't see.
        Ok(Err(msg)) => match msg.strip_prefix(apiplant_abi::INTERNAL_ERROR_PREFIX) {
            Some(detail) => {
                tracing::error!(function = %name, detail, "function faulted");
                error(500, "internal function error")
            }
            None => error(400, msg),
        },
        // Reached only if the *host* side of the blocking closure panicked;
        // panics inside the function are caught before they cross the ABI.
        Err(_) => {
            tracing::error!(function = %name, "function task panicked");
            error(500, "internal function error")
        }
    }
}

/// `<base>/functions/{name}/stream` — the same call, answered as it happens.
///
/// Every `emit` the function makes is forwarded immediately as a `delta`
/// event; what it finally returns arrives as the `done` event's `result`. A
/// function that never emits still works here — it produces one `done` and
/// nothing else, which is a slow endpoint rather than a broken one.
///
/// The status code is decided before the function starts, so everything after
/// it — including the function faulting halfway — is reported inside the stream
/// as an `error` event. That is not a compromise: a failure that arrives after
/// three paragraphs of text *is* a mid-stream event, and there is no status
/// code left to spend on it.
pub async fn stream(
    req: HttpRequest,
    state: State<AppState>,
    path: Path<String>,
    body: String,
) -> HttpResponse {
    let name = path.into_inner();
    let ready = match admit(&req, &state, &name, body).await {
        Ok(ready) => ready,
        Err(response) => return response,
    };

    // Unbounded on purpose. The alternative is a function blocking mid-chunk
    // on a slow reader, inside a blocking worker, holding a thread — and a
    // function's output is bounded by the work it is doing anyway.
    let (chunks, receiver) = tokio::sync::mpsc::unbounded_channel::<String>();
    let functions = state.functions.clone();
    let name2 = name.clone();
    let bridge = bridge(&state, &ready).streaming(chunks);
    let input = ready.input;

    let finished = tokio::task::spawn_blocking(move || {
        let f = functions.get(&name2).expect("checked above");
        f.invoke(bridge, &input)
        // `bridge` drops here, closing the channel, which is what ends the
        // stream below — so the ordering is a consequence of the ownership
        // rather than of a flag anyone has to remember to set.
    });

    let deltas = futures_util::stream::unfold(receiver, |mut recv| async move {
        recv.recv().await.map(|chunk| {
            let frame: Result<Bytes, sse::Never> = Ok(sse::delta(&chunk));
            (frame, recv)
        })
    });

    // Polled only once the deltas are exhausted, which is exactly when the
    // function has returned.
    let ending = futures_util::stream::once(async move {
        let frame = match finished.await {
            Ok(Ok(output)) => {
                // The return value is JSON when the function produced JSON, and
                // a string when it produced anything else — a client should not
                // have to double-parse to find out which.
                let result =
                    serde_json::from_str(&output).unwrap_or(serde_json::Value::String(output));
                sse::done(&json!({ "result": result }))
            }
            Ok(Err(msg)) => match msg.strip_prefix(apiplant_abi::INTERNAL_ERROR_PREFIX) {
                Some(detail) => {
                    tracing::error!(function = %name, detail, "streaming function faulted");
                    sse::failure("internal function error")
                }
                None => sse::failure(&msg),
            },
            Err(_) => {
                tracing::error!(function = %name, "streaming function task panicked");
                sse::failure("internal function error")
            }
        };
        Ok::<Bytes, sse::Never>(frame)
    });

    let mut response = HttpResponse::Ok();
    sse::headers(&mut response);
    // Boxed because the response body has to be `Unpin` and a chain of
    // `async` blocks is not.
    response.streaming(Box::pin(futures_util::StreamExt::chain(deltas, ending)))
}
