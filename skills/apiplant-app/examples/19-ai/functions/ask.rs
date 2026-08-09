//! A function in front of the assistant.
//!
//! `POST /api/ai/chat` already streams a reply, so a function that only
//! forwards a prompt would be pure overhead. This one earns its place by doing
//! the three things a real app puts in front of a model, none of which belong
//! in a browser:
//!
//! * it fetches context out of the database and puts it in the prompt,
//! * it fixes the instructions rather than letting the caller write them,
//! * it records what was asked and what came back.
//!
//! And it still streams, because [`chat_streaming`] hands every token to the
//! caller on its way through — which is what makes wrapping the model free
//! rather than a downgrade. Call it at
//! `POST /api/functions/ask/stream` for tokens as they land, or at
//! `POST /api/functions/ask` for the finished answer as JSON. Same code.
//!
//! [`chat_streaming`]: apiplant_function::Context::chat_streaming

use apiplant_function::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Deserialize, Default)]
struct Config {
    /// What the assistant is told it is, before it is told anything else.
    #[serde(default = "default_persona")]
    persona: String,
    /// How many earlier notes to put in the prompt as context.
    #[serde(default = "default_context")]
    context_notes: usize,
}

fn default_persona() -> String {
    "You are a terse assistant answering questions about a person's notes.".to_string()
}

fn default_context() -> usize {
    5
}

#[derive(Deserialize, JsonSchema)]
struct Input {
    /// What to ask.
    question: String,
}

#[derive(Serialize, JsonSchema)]
struct Output {
    /// The complete answer. On the streaming endpoint this arrives last, after
    /// every token of it has already been delivered.
    answer: String,
    /// The model that produced it.
    model: String,
    /// How many notes went into the prompt.
    context_notes: usize,
}

#[derive(Deserialize, JsonSchema)]
struct EmptyInput {}

#[derive(Serialize, JsonSchema)]
struct NoteSummary {
    id: String,
    body: String,
    created_at: String,
}

#[derive(Serialize, JsonSchema)]
struct NotesOutput {
    notes: Vec<NoteSummary>,
}

fn ask(ctx: &Context<Config>, input: Input) -> Result<Output, String> {
    if input.question.trim().is_empty() {
        return Err("ask a question".to_string());
    }

    // Context the caller could not have supplied and should not have to: their
    // own recent notes, read with their own id, never from the request body.
    let notes = ctx.query(
        "SELECT body FROM apiplant_note WHERE owner_id = $1::uuid \
         ORDER BY created_at DESC LIMIT $2",
        &[
            serde_json::json!(ctx.principal_id()),
            serde_json::json!(ctx.config().context_notes as i64),
        ],
    )?;

    let context = notes
        .iter()
        .filter_map(|row| row.get("body").and_then(|b| b.as_str()))
        .map(|body| format!("- {body}"))
        .collect::<Vec<_>>()
        .join("\n");

    let prompt = match context.is_empty() {
        true => input.question.clone(),
        false => format!(
            "Here are my most recent notes:\n{context}\n\nQuestion: {}",
            input.question
        ),
    };

    // The one line that talks to a model. `chat_streaming` emits each token to
    // this function's caller as it arrives *and* returns the whole reply, so
    // the endpoint streams and the code below still has something to log.
    let reply = ctx.chat_streaming(Chat::ask(prompt).system(&ctx.config().persona))?;

    ctx.info(&format!(
        "answered {} characters with {} ({} output tokens)",
        reply.text.len(),
        reply.model,
        reply.output_tokens.unwrap_or(0)
    ));

    Ok(Output {
        answer: reply.text,
        model: reply.model,
        context_notes: notes.len(),
    })
}

fn current_user_notes(ctx: &Context<()>, _input: EmptyInput) -> Result<NotesOutput, String> {
    if ctx.principal_id().trim().is_empty() {
        return Err("authentication required".to_string());
    }

    let rows = ctx.query(
        "SELECT id::text AS id, body, created_at::text AS created_at \
         FROM apiplant_note WHERE owner_id = $1::uuid ORDER BY created_at DESC",
        &[serde_json::json!(ctx.principal_id())],
    )?;

    let notes = rows
        .iter()
        .map(|row| NoteSummary {
            id: row
                .get("id")
                .and_then(|value| value.as_str())
                .unwrap_or_default()
                .to_string(),
            body: row
                .get("body")
                .and_then(|value| value.as_str())
                .unwrap_or_default()
                .to_string(),
            created_at: row
                .get("created_at")
                .and_then(|value| value.as_str())
                .unwrap_or_default()
                .to_string(),
        })
        .collect();

    Ok(NotesOutput { notes })
}

apiplant_function::functions! {
    {
        name: "ask",
        description: "Answers a question about the caller's own notes, streaming as it goes.",
        method: Post,
        // The model costs something to run, and the prompt is built from the
        // caller's own rows — both of which need to know who is asking.
        visibility: Authenticated,
        handler: ask,
    },
    {
        name: "current_user_notes",
        description: "Tool-only: returns every note owned by the current chatter.",
        method: Post,
        // Available to configured agent tools, but not callable from HTTP or the admin panel.
        permission: "none",
        handler: current_user_notes,
    },
}
