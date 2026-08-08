//! The `<base>/ai` endpoints: talking to the app's assistant.
//!
//! Two routes, mounted only in an app whose `[ai]` section names a provider:
//!
//! | Route | What it does |
//! |-------|--------------|
//! | `GET  /ai/config` | which provider and model answer, and who may ask |
//! | `POST /ai/chat` | takes a conversation, streams the reply back |
//!
//! ## Why the reply streams by default
//!
//! Because a chat completion is slow in a way that is entirely visible to the
//! person waiting: the first token arrives in a fraction of the time the last
//! one does. An endpoint that buffers the whole answer throws that away and
//! turns a responsive interface into a spinner. So `POST /ai/chat` answers as
//! [server-sent events](crate::sse) unless the caller asks for JSON with
//! `{"stream": false}` — which is the right thing for a script that has
//! nothing to do with half an answer.
//!
//! ## Who may ask
//!
//! Whatever `[ai] access` says, defaulting to `authenticated`. The endpoint
//! spends money (or a GPU) on behalf of whoever calls it, so a public one is an
//! open proxy to your provider account — a thing an app should have to write
//! down rather than get by leaving a line out.

use apiplant_abi::{FunctionAccess, FunctionPolicy};
use apiplant_ai::{ChatRequest, Event};
use futures_util::StreamExt;
use ntex::util::Bytes;
use ntex::web::types::{Json, State};
use ntex::web::{HttpRequest, HttpResponse};
use serde::Deserialize;
use serde_json::json;

use crate::response::{error, ok};
use crate::sse;
use crate::state::AppState;

/// A posted conversation: [`ChatRequest`] plus how the caller wants it back.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct Body {
    #[serde(flatten)]
    chat: ChatRequest,
    /// `true` (the default) streams the answer as SSE; `false` answers with one
    /// JSON document once it is complete.
    stream: Option<bool>,
}

/// `GET <base>/ai/config` — what a front end needs to render a chat box.
///
/// Nothing here is a secret: the provider's name, the default model, and the
/// access level, which is what lets an interface show a sign-in prompt instead
/// of a chat box that would answer `401`. The API key is not in it and never
/// will be — unlike a payment provider's publishable key, an AI key is a
/// spending credential with no browser-safe half.
pub async fn config(state: State<AppState>) -> HttpResponse {
    let Some(ai) = &state.ai else {
        return error(404, "this app has no ai assistant");
    };
    ok(&json!({
        "provider": ai.provider().as_str(),
        "model": ai.model(),
        "access": state.app.config.ai.access,
        "streaming": true,
        "agents": state.app.agents.values().map(|agent| {
            json!({
                "name": agent.meta.name,
                "description": agent.meta.description,
                "access": agent.permissions.chat.as_string(),
                "scope": match agent.meta.scope {
                    apiplant_core::Scope::Global => "global",
                    apiplant_core::Scope::Organization => "organization",
                },
                "storage": agent.meta.storage.enabled,
                "tools": agent.tools.iter().map(|tool| {
                    json!({
                        "name": tool.name,
                        "description": tool.description,
                        "input_schema": tool.input_schema,
                        "output_schema": tool.output_schema,
                    })
                }).collect::<Vec<_>>(),
            })
        }).collect::<Vec<_>>(),
    }))
}

/// `POST <base>/ai/chat` — ask, and get the answer as it is written.
///
/// Body: `{ "messages": [{ "role": "user", "content": "…" }] }`, plus optional
/// `model`, `system`, `temperature`, `max_tokens` and `stream`. Everything but
/// the messages falls back to `[ai]`.
///
/// Answers as [SSE](crate::sse) — `delta` events carrying text, then one
/// `done` — or, with `"stream": false`, as `{ "text": …, "provider": …,
/// "model": …, "finish_reason": … }`.
pub async fn chat(req: HttpRequest, state: State<AppState>, body: Json<Body>) -> HttpResponse {
    let Some(ai) = state.ai.clone() else {
        return error(404, "this app has no ai assistant");
    };

    let access = FunctionPolicy::parse(&state.app.config.ai.access)
        // A typo in `[ai] access` closes the door rather than opening it, which
        // is how every other access string in the framework behaves.
        .unwrap_or_else(|| FunctionAccess::Private.into());
    if let Err(response) = crate::access::check(&state, &req, &access, "no ai assistant").await {
        return response;
    }

    let body = body.into_inner();
    let request = body.chat;

    // A request that wants the whole thing at once gets one document, and any
    // failure is an ordinary status code — the useful shape for a script.
    if body.stream == Some(false) {
        return match ai.chat(&request).await {
            Ok(reply) => match serde_json::to_value(&reply) {
                Ok(value) => ok(&value),
                Err(e) => {
                    tracing::error!(error = %e, "unserialisable chat reply");
                    error(500, "internal error")
                }
            },
            Err(e) => refused(e),
        };
    }

    // Everything before the first byte can still be a status code; after it,
    // the only channel left is the stream itself.
    let stream = match ai.stream(&request).await {
        Ok(stream) => stream,
        Err(e) => return refused(e),
    };

    // Whether the provider's own ending has already gone out, so the fallback
    // below doesn't send a second one. A plain `Cell` because an ntex worker
    // is single-threaded and the two halves of this stream are polled in turn.
    let ended = std::rc::Rc::new(std::cell::Cell::new(false));
    let closing = ended.clone();

    let events = stream
        .map(move |event| -> Result<Bytes, sse::Never> {
            Ok(match event {
                Ok(Event::Delta(text)) => sse::delta(&text),
                // The model's thinking, on a provider that streams it apart
                // from the answer. Forwarded under its own event name so a
                // client can show it, ignore it, or collapse it — and never
                // append it to the reply by accident.
                Ok(Event::Reasoning(text)) => sse::event("reasoning", &json!({ "text": text })),
                Ok(Event::Done(done)) => {
                    ended.set(true);
                    sse::done(&serde_json::to_value(&done).unwrap_or_else(|_| json!({})))
                }
                // Mid-answer, with a `200` long since sent: the caller has text
                // on screen and needs to know it stopped, not that it never
                // started.
                Err(e) => {
                    tracing::warn!(error = %e, "ai stream failed mid-answer");
                    sse::failure(&e.to_string())
                }
            })
        })
        // A provider that closes without a terminator still ends the stream the
        // same way for the client, so there is one place to stop listening —
        // and exactly one, which is what `ended` is for.
        .chain(futures_util::stream::once(async move {
            Ok(match closing.get() {
                true => Bytes::new(),
                false => sse::done(&json!({})),
            })
        }));

    let mut response = HttpResponse::Ok();
    sse::headers(&mut response);
    // Boxed because the response body has to be `Unpin` and a chain of
    // `async` blocks is not.
    response.streaming(Box::pin(events))
}

/// Turn a failure that happened *before* anything was sent into a status code.
///
/// A refusal from the provider is reported as a `502`, not as our own error:
/// the request reached us fine and the thing behind us said no, and an operator
/// reading `500` would go looking in the wrong process. The provider's own
/// status is passed through in the body for the same reason.
fn refused(e: apiplant_ai::AiError) -> HttpResponse {
    match e {
        apiplant_ai::AiError::Request(message) => error(400, message),
        apiplant_ai::AiError::Provider {
            provider,
            status,
            body,
        } => {
            tracing::warn!(provider, status, body = %body, "the ai provider refused a request");
            HttpResponse::BadGateway().json(&json!({
                "error": format!("the ai provider refused this request: {body}"),
                "provider": provider,
                "provider_status": status,
            }))
        }
        other => {
            tracing::error!(error = %other, "ai request failed");
            HttpResponse::BadGateway().json(&json!({ "error": other.to_string() }))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_posted_body_is_a_conversation_plus_how_to_answer_it() {
        let body: Body = serde_json::from_str(
            r#"{"messages":[{"role":"user","content":"hi"}],"model":"m","stream":false}"#,
        )
        .unwrap();
        assert_eq!(body.chat.messages.len(), 1);
        assert_eq!(body.chat.model.as_deref(), Some("m"));
        assert_eq!(body.stream, Some(false));

        // Streaming is what a body that doesn't say gets: the answer is worth
        // more as it arrives, and a caller who disagrees says so.
        let default: Body = serde_json::from_str(r#"{"messages":[]}"#).unwrap();
        assert_eq!(default.stream, None);
    }

    #[test]
    fn a_provider_refusal_is_a_502_naming_the_provider() {
        let response = refused(apiplant_ai::AiError::Provider {
            provider: "openai".to_string(),
            status: 429,
            body: "rate limited".to_string(),
        });
        assert_eq!(response.status().as_u16(), 502);

        // An unusable conversation is the caller's fault, not the provider's.
        let bad = refused(apiplant_ai::AiError::Request("no messages".to_string()));
        assert_eq!(bad.status().as_u16(), 400);
    }

    #[test]
    fn the_done_event_carries_the_ending_as_json() {
        let done = apiplant_ai::Done {
            finish_reason: "stop".to_string(),
            input_tokens: Some(9),
            output_tokens: Some(4),
        };
        let value = serde_json::to_value(&done).unwrap();
        assert!(value.is_object());
        assert_eq!(value["finish_reason"], "stop");
        assert_eq!(value["output_tokens"], 4);
    }
}
