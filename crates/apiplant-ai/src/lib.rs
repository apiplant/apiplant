//! # apiplant-ai
//!
//! One way to ask a chat assistant something, whichever service answers.
//!
//! An app names a provider in `main.toml`:
//!
//! ```toml
//! [ai]
//! provider = "custom"
//! endpoint = "http://localhost:8080"
//! model    = "local"
//! ```
//!
//! …and gets `POST <base>/ai/chat`, which streams a reply back token by token,
//! plus a `chat` call every function can make. Changing `provider` to `openai`
//! or `anthropic` changes the request shape, the authentication scheme and the
//! event format — and changes nothing a caller can see.
//!
//! ## Supported providers
//!
//! | `provider` | Endpoint | Credentials |
//! |------------|----------|-------------|
//! | `openai` | `api.openai.com/v1/chat/completions` | `api_key` (Bearer) |
//! | `anthropic` | `api.anthropic.com/v1/messages` | `api_key` (`x-api-key`) |
//! | `custom` | whatever `endpoint` says, OpenAI chat-completions shape | `api_key`, or none |
//!
//! `custom` is not a lesser third option: llama.cpp, vLLM, Ollama, LM Studio and
//! most gateways all speak the OpenAI shape, so pointing `endpoint` at one is
//! the whole integration. The key is optional there, and an empty key sends no
//! authorization header rather than an empty one — which is the difference
//! between a local server answering and a local server refusing.
//!
//! ## Streaming
//!
//! [`Ai::stream`] is the primary call and [`Ai::chat`] is written in terms of
//! it, because a chat completion is a slow thing whose value arrives gradually:
//! the first token lands in a fraction of the time the last one does, and an
//! interface that waits for the whole answer throws that away.

use std::time::Duration;

pub mod summary;

use apiplant_core::AiConfig;
use futures_util::{Stream, StreamExt};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

/// What went wrong while talking to the assistant.
#[derive(Debug, thiserror::Error)]
pub enum AiError {
    /// The app's `[ai]` section can't produce a working client — an unknown
    /// provider, a `custom` provider with no endpoint. Raised at startup where
    /// possible, so a deployment fails to boot rather than failing at the first
    /// question.
    #[error("ai configuration: {0}")]
    Config(String),

    /// The conversation itself is unusable: no messages, an unknown role.
    #[error("invalid request: {0}")]
    Request(String),

    /// The provider could not be reached, or timed out.
    #[error("ai transport: {0}")]
    Transport(String),

    /// The provider answered, and said no.
    #[error("{provider} refused the request ({status}): {body}")]
    Provider {
        provider: String,
        status: u16,
        body: String,
    },
}

/// Who said a thing.
///
/// `system` is accepted in the message list even for providers that carry the
/// system prompt out of band (Anthropic): it is lifted out on the way to the
/// wire, so a caller writes the same conversation for every provider.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    System,
    #[default]
    User,
    Assistant,
    Tool,
}

impl Role {
    fn as_str(&self) -> &'static str {
        match self {
            Role::System => "system",
            Role::User => "user",
            Role::Assistant => "assistant",
            Role::Tool => "tool",
        }
    }
}

/// One model tool call.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub input: Value,
}

/// One tool the model may call.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ToolDefinition {
    pub name: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub description: String,
    #[serde(default)]
    pub input_schema: Value,
}

/// One turn of the conversation.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Message {
    pub role: Role,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub content: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tool_calls: Vec<ToolCall>,
}

impl Message {
    pub fn user(content: impl Into<String>) -> Message {
        Message {
            role: Role::User,
            content: content.into(),
            ..Message::default()
        }
    }

    pub fn assistant(content: impl Into<String>) -> Message {
        Message {
            role: Role::Assistant,
            content: content.into(),
            ..Message::default()
        }
    }

    pub fn system(content: impl Into<String>) -> Message {
        Message {
            role: Role::System,
            content: content.into(),
            ..Message::default()
        }
    }

    pub fn assistant_tool_calls(tool_calls: Vec<ToolCall>) -> Message {
        Message {
            role: Role::Assistant,
            tool_calls,
            ..Message::default()
        }
    }

    pub fn tool_result(id: impl Into<String>, content: impl Into<String>) -> Message {
        Message {
            role: Role::Tool,
            content: content.into(),
            tool_call_id: Some(id.into()),
            ..Message::default()
        }
    }
}

/// A question to put to the assistant.
///
/// Deserialised straight from the JSON a client posts to `<base>/ai/chat` and
/// from the JSON a function hands the host, so the field names here are the
/// ones people write. Everything except `messages` falls back to the app's
/// `[ai]` configuration.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(default)]
pub struct ChatRequest {
    /// The conversation so far, oldest first.
    pub messages: Vec<Message>,
    /// Overrides `[ai] model`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// Overrides `[ai] system`. A `system` message inside `messages` wins over
    /// both, since it is the more specific place to have said it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub system: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u32>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tools: Vec<ToolDefinition>,
}

impl ChatRequest {
    /// A one-question conversation.
    pub fn ask(prompt: impl Into<String>) -> ChatRequest {
        ChatRequest {
            messages: vec![Message::user(prompt)],
            ..ChatRequest::default()
        }
    }

    /// Split the request into the system prompt and the dialogue, filling in
    /// what it didn't say from the app's configuration.
    ///
    /// The system prompt is separated here rather than at each provider because
    /// exactly one of the three wants it out of band, and a caller should not
    /// have to know which.
    fn resolve(&self, config: &AiConfig) -> Result<Resolved, AiError> {
        let mut system = Vec::new();
        let mut turns = Vec::new();
        for message in &self.messages {
            if message.content.trim().is_empty() && message.tool_calls.is_empty() {
                continue;
            }
            match message.role {
                Role::System => system.push(message.content.clone()),
                _ => turns.push(message.clone()),
            }
        }
        if turns.is_empty() {
            return Err(AiError::Request(
                "no messages: give at least one `user` message".to_string(),
            ));
        }

        // Most specific wins: a system message in the conversation, then the
        // request's own field, then the app's default.
        if system.is_empty() {
            let fallback = self.system.clone().unwrap_or_else(|| config.system.clone());
            if !fallback.trim().is_empty() {
                system.push(fallback);
            }
        }

        let model = self
            .model
            .clone()
            .filter(|m| !m.trim().is_empty())
            .unwrap_or_else(|| config.model.clone());

        Ok(Resolved {
            system: system.join("\n\n"),
            messages: turns,
            model,
            temperature: self.temperature.or_else(|| config.default_temperature()),
            max_tokens: self.max_tokens.unwrap_or(config.max_tokens).max(1),
        })
    }
}

/// A [`ChatRequest`] with the app's defaults filled in and its invariants
/// checked. Providers only ever see one of these.
#[derive(Debug, Clone)]
struct Resolved {
    system: String,
    messages: Vec<Message>,
    model: String,
    temperature: Option<f32>,
    max_tokens: u32,
}

/// One thing that happened while the assistant was answering.
#[derive(Debug, Clone, PartialEq)]
pub enum Event {
    /// More text, to append to what came before. Never the whole answer.
    Delta(String),
    /// More of the model's *thinking*, on a provider that streams it
    /// separately (`reasoning_content` — llama.cpp, vLLM and DeepSeek-shaped
    /// APIs all do, and OpenAI's `o` models do their own version).
    ///
    /// Kept apart from [`Delta`](Event::Delta) rather than merged into it,
    /// because it is not part of the answer: a caller assembling the reply
    /// must not end up with the reasoning in it, and a caller that wants to
    /// show "thinking…" needs to be able to tell which is which. Ignoring this
    /// variant entirely is a correct and complete way to consume the stream.
    Reasoning(String),
    /// The answer is complete; nothing follows.
    Done(Done),
}

/// How an answer ended.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Done {
    /// The provider's own word for why it stopped — `stop`, `length`,
    /// `max_tokens`. Empty when it didn't say.
    #[serde(skip_serializing_if = "String::is_empty", default)]
    pub finish_reason: String,
    /// Tokens the prompt cost, when the provider reports it.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub input_tokens: Option<u64>,
    /// Tokens the answer cost, when the provider reports it.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub output_tokens: Option<u64>,
}

/// A complete answer, for a caller that has nothing to do with a partial one.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ChatReply {
    /// The whole message, assembled from every delta.
    pub text: String,
    /// Provider reasoning kept separate from the visible answer.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub reasoning: String,
    /// The provider that answered.
    pub provider: String,
    /// The model that was asked for.
    pub model: String,
    #[serde(flatten)]
    pub done: Done,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tool_calls: Vec<ToolCall>,
}

/// Which service answers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Provider {
    OpenAi,
    Anthropic,
    /// Anything speaking the OpenAI chat-completions shape at an endpoint of
    /// its own.
    Custom,
}

impl Provider {
    /// Parse the `[ai] provider` string.
    pub fn parse(value: &str) -> Option<Provider> {
        match value.trim().to_ascii_lowercase().as_str() {
            "openai" | "open-ai" | "gpt" => Some(Provider::OpenAi),
            "anthropic" | "claude" => Some(Provider::Anthropic),
            // Every self-hosted server people arrive with speaks the same
            // dialect, so naming yours is a shortcut, not a separate provider.
            "custom" | "local" | "openai-compatible" | "ollama" | "vllm" | "llamacpp"
            | "llama.cpp" | "lmstudio" => Some(Provider::Custom),
            _ => None,
        }
    }

    /// The canonical name, used in logs and in a [`ChatReply`].
    pub fn as_str(&self) -> &'static str {
        match self {
            Provider::OpenAi => "openai",
            Provider::Anthropic => "anthropic",
            Provider::Custom => "custom",
        }
    }

    /// Every accepted spelling, for error messages.
    pub fn names() -> &'static str {
        "none, openai, anthropic, custom"
    }

    /// Where this provider's completions live when the app named only an
    /// origin — or nothing at all.
    fn default_origin(&self) -> &'static str {
        match self {
            Provider::OpenAi => "https://api.openai.com",
            Provider::Anthropic => "https://api.anthropic.com",
            Provider::Custom => "",
        }
    }

    fn path(&self) -> &'static str {
        match self {
            Provider::Anthropic => "/v1/messages",
            _ => "/v1/chat/completions",
        }
    }
}

/// A configured, ready-to-use assistant.
///
/// Built once at boot and shared by every worker: the HTTP client pools its
/// connections, which matters more here than anywhere else in the framework —
/// a chat completion holds one open for as long as the answer takes.
#[derive(Clone)]
pub struct Ai {
    provider: Provider,
    config: AiConfig,
    /// Parsed once at boot rather than per reply: it is a deployment fact.
    reasoning_format: ReasoningFormat,
    url: String,
    client: reqwest::Client,
}

impl std::fmt::Debug for Ai {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Deliberately does not print `config`: it holds the API key.
        f.debug_struct("Ai")
            .field("provider", &self.provider.as_str())
            .field("url", &self.url)
            .field("model", &self.config.model)
            .finish()
    }
}

impl Ai {
    /// Build the assistant an app's `[ai]` section describes.
    ///
    /// `Ok(None)` means the app has none (`provider = "none"`, the default) —
    /// not an error. `Err` means it *asked* for one and the request can't be
    /// honoured, which is worth failing the boot over.
    pub fn from_config(config: &AiConfig) -> Result<Option<Ai>, AiError> {
        if !config.enabled() {
            return Ok(None);
        }
        let provider = Provider::parse(&config.provider).ok_or_else(|| {
            AiError::Config(format!(
                "unknown provider `{}`; expected one of: {}",
                config.provider,
                Provider::names()
            ))
        })?;

        let url = resolve_url(provider, &config.endpoint)?;

        // No check on `api_key`: an assistant that needs none is the common
        // local case, and a hosted provider says `401` clearly enough that
        // guessing on its behalf would only be wrong for somebody.
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(config.timeout_secs.max(1)))
            .user_agent(concat!("apiplant/", env!("CARGO_PKG_VERSION")))
            .build()
            .map_err(|e| AiError::Config(e.to_string()))?;

        Ok(Some(Ai {
            provider,
            // Anthropic streams thinking in a block of its own and never
            // writes a tag into the text, so there is nothing to read out of
            // it — and reading anyway would only find a tag the user typed.
            reasoning_format: match provider {
                Provider::Anthropic => ReasoningFormat::Native,
                _ => ReasoningFormat::parse(&config.reasoning_format),
            },
            config: config.clone(),
            url,
            client,
        }))
    }

    /// Which provider answers.
    pub fn provider(&self) -> Provider {
        self.provider
    }

    /// The URL requests go to. Worth logging at boot: "which model am I
    /// actually talking to" is the first question of every debugging session.
    pub fn url(&self) -> &str {
        &self.url
    }

    /// The model asked for when a request doesn't name one.
    pub fn model(&self) -> &str {
        &self.config.model
    }

    /// Ask, and get the answer as it arrives.
    ///
    /// The stream ends after exactly one [`Event::Done`], or early with an
    /// error. A transport failure *mid-answer* arrives as an error item rather
    /// than a `Result` on the whole call, because by then the caller already
    /// has text on screen and needs to know it stopped rather than that it
    /// never started.
    pub async fn stream(
        &self,
        request: &ChatRequest,
    ) -> Result<impl Stream<Item = Result<Event, AiError>>, AiError> {
        let resolved = request.resolve(&self.config)?;
        let body = self.body(&resolved, true, &request.tools);
        let response = self.send(body).await?;

        let provider = self.provider;
        let mut buffer = String::new();
        let mut finished = false;
        // One per stream: inline thinking is separated here, so no consumer
        // downstream has to know that some servers tag it and some do not.
        let mut thinking = ThinkingSplit::new(self.reasoning_format);
        let bytes = response.bytes_stream();

        // One SSE frame does not map to one chunk of the response body: a
        // reply may arrive split mid-frame or several frames at a time, so the
        // buffer is what makes the parse correct rather than usually correct.
        let events = bytes.flat_map(move |chunk| {
            let mut out: Vec<Result<Event, AiError>> = Vec::new();
            match chunk {
                Ok(bytes) => {
                    buffer.push_str(&String::from_utf8_lossy(&bytes));
                    for frame in take_frames(&mut buffer) {
                        if finished {
                            break;
                        }
                        match parse_frame(provider, &frame) {
                            Some(event @ Event::Done(_)) => {
                                finished = true;
                                out.extend(thinking.flush().map(Ok));
                                out.push(Ok(event));
                            }
                            // Only `content` can hide thinking in it; a native
                            // reasoning field is already what it says it is.
                            Some(Event::Delta(chunk)) => {
                                out.extend(thinking.push(&chunk).into_iter().map(Ok));
                            }
                            // A native reasoning field settles the question:
                            // this server separates the thinking itself, so
                            // the content is the answer and a block assumed
                            // open in the text was never open at all.
                            Some(event @ Event::Reasoning(_)) => {
                                thinking.server_separates_reasoning();
                                out.push(Ok(event));
                            }
                            None => {}
                        }
                    }
                }
                Err(e) => out.push(Err(AiError::Transport(e.to_string()))),
            }
            futures_util::stream::iter(out)
        });

        Ok(events)
    }

    /// Ask, and wait for the whole answer.
    ///
    /// Still streamed on the wire — a provider that has been generating for two
    /// minutes has not timed out, and a connection that goes quiet for two
    /// minutes has. This is the call a function makes, since a function returns
    /// one value.
    pub async fn chat(&self, request: &ChatRequest) -> Result<ChatReply, AiError> {
        if !request.tools.is_empty() {
            return self.complete(request).await;
        }
        let resolved = request.resolve(&self.config)?;
        let model = resolved.model.clone();
        let mut stream = Box::pin(self.stream(request).await?);

        let mut text = String::new();
        let mut reasoning = String::new();
        let mut done = Done::default();
        while let Some(event) = stream.next().await {
            match event? {
                Event::Delta(delta) => text.push_str(&delta),
                // Not part of the answer — see `Event::Reasoning`.
                Event::Reasoning(chunk) => reasoning.push_str(&chunk),
                Event::Done(end) => {
                    done = end;
                    break;
                }
            }
        }

        tracing::debug!(
            provider = self.provider.as_str(),
            model = %model,
            characters = text.len(),
            "chat completion"
        );
        Ok(ChatReply {
            text,
            reasoning,
            provider: self.provider.as_str().to_string(),
            model,
            done,
            tool_calls: Vec::new(),
        })
    }

    /// Ask without streaming, preserving provider tool-call requests.
    pub async fn complete(&self, request: &ChatRequest) -> Result<ChatReply, AiError> {
        let resolved = request.resolve(&self.config)?;
        let model = resolved.model.clone();
        let body = self.body(&resolved, false, &request.tools);
        let value: Value = self
            .send(body)
            .await?
            .json()
            .await
            .map_err(|e| AiError::Transport(e.to_string()))?;
        parse_complete(self.provider, self.reasoning_format, &value)
            .map(|mut reply| {
                reply.provider = self.provider.as_str().to_string();
                reply.model = model;
                reply
            })
            .ok_or_else(|| AiError::Provider {
                provider: self.provider.as_str().to_string(),
                status: 502,
                body: brief(&value.to_string()),
            })
    }

    /// POST the body, and refuse anything that isn't a `2xx`.
    async fn send(&self, body: Value) -> Result<reqwest::Response, AiError> {
        let mut request = self.client.post(&self.url).json(&body);

        // An empty key means "this endpoint wants no credential" — so send no
        // header at all. A local llama.cpp rejects an empty bearer token, which
        // would make "leave api_key out" the one thing that doesn't work.
        let key = self.config.api_key.trim();
        if !key.is_empty() {
            request = match self.provider {
                Provider::Anthropic => request.header("x-api-key", key),
                _ => request.header("authorization", format!("Bearer {key}")),
            };
        }
        if self.provider == Provider::Anthropic {
            request = request.header("anthropic-version", "2023-06-01");
        }

        let response = request
            .send()
            .await
            .map_err(|e| AiError::Transport(e.to_string()))?;

        let status = response.status();
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            return Err(AiError::Provider {
                provider: self.provider.as_str().to_string(),
                status: status.as_u16(),
                // Providers answer with a JSON object whose useful part is one
                // sentence; the rest is request ids nobody reads in a log.
                body: brief(&body),
            });
        }
        Ok(response)
    }

    /// The provider's own request shape.
    fn body(&self, resolved: &Resolved, stream: bool, tools: &[ToolDefinition]) -> Value {
        let mut messages: Vec<Value> = Vec::with_capacity(resolved.messages.len() + 1);
        if self.provider != Provider::Anthropic && !resolved.system.is_empty() {
            messages.push(json!({ "role": "system", "content": resolved.system }));
        }
        messages.extend(resolved.messages.iter().map(|m| self.message(m)));

        let mut body = json!({
            "model": resolved.model,
            "messages": messages,
            "stream": stream,
            "max_tokens": resolved.max_tokens,
        });

        if self.provider == Provider::Anthropic && !resolved.system.is_empty() {
            body["system"] = json!(resolved.system);
        }
        if let Some(temperature) = resolved.temperature {
            body["temperature"] = json!(temperature);
        }
        if !tools.is_empty() {
            body["tools"] = match self.provider {
                Provider::Anthropic => json!(tools
                    .iter()
                    .map(|tool| json!({
                        "name": tool.name,
                        "description": tool.description,
                        "input_schema": tool.input_schema,
                    }))
                    .collect::<Vec<_>>()),
                _ => json!(tools
                    .iter()
                    .map(|tool| json!({
                        "type": "function",
                        "function": {
                            "name": tool.name,
                            "description": tool.description,
                            "parameters": tool.input_schema,
                        }
                    }))
                    .collect::<Vec<_>>()),
            };
        }
        // OpenAI reports token usage on a streamed completion only when asked;
        // a `custom` server that doesn't know the option ignores it.
        if stream && self.provider != Provider::Anthropic {
            body["stream_options"] = json!({ "include_usage": true });
        }
        self.apply_thinking(&mut body, resolved);
        body
    }

    /// Ask the provider to think, or not to, using its own parameter for it.
    ///
    /// Nothing is sent unless the app said something, because every one of
    /// these is a request a server may reject if it has never heard of it.
    fn apply_thinking(&self, body: &mut Value, resolved: &Resolved) {
        let Some(thinking) = self.config.thinking else {
            return;
        };
        match self.provider {
            Provider::Anthropic => {
                if thinking {
                    // The budget is part of `max_tokens`, not extra, so it has
                    // to leave room for an answer — and Anthropic rejects a
                    // budget under 1024 or a temperature alongside thinking.
                    let budget = (resolved.max_tokens / 2).max(1_024);
                    if resolved.max_tokens > budget {
                        body["thinking"] = json!({ "type": "enabled", "budget_tokens": budget });
                        body.as_object_mut().map(|body| body.remove("temperature"));
                    } else {
                        tracing::warn!(
                            max_tokens = resolved.max_tokens,
                            "thinking was asked for but max_tokens leaves no room for it"
                        );
                    }
                } else {
                    body["thinking"] = json!({ "type": "disabled" });
                }
            }
            // What the Qwen-family chat templates read, and what llama.cpp,
            // vLLM, SGLang and Ollama all pass through to them. A server whose
            // template ignores it is unharmed by it.
            Provider::Custom => {
                body["chat_template_kwargs"] = json!({ "enable_thinking": thinking });
            }
            // OpenAI's reasoning models take `reasoning_effort` and cannot be
            // told not to think at all; their other models reject the field
            // outright. Neither is served by guessing, so nothing is sent.
            Provider::OpenAi => {
                tracing::debug!(
                    "`thinking` is not sent to openai: its reasoning models cannot be \
switched off and its other models reject the parameter"
                );
            }
        }
    }

    fn message(&self, message: &Message) -> Value {
        match self.provider {
            Provider::Anthropic if message.role == Role::Tool => json!({
                "role": "user",
                "content": [{
                    "type": "tool_result",
                    "tool_use_id": message.tool_call_id,
                    "content": message.content,
                }]
            }),
            Provider::Anthropic if !message.tool_calls.is_empty() => {
                let mut content = Vec::new();
                if !message.content.is_empty() {
                    content.push(json!({ "type": "text", "text": message.content }));
                }
                content.extend(message.tool_calls.iter().map(|call| {
                    json!({
                        "type": "tool_use",
                        "id": call.id,
                        "name": call.name,
                        "input": call.input,
                    })
                }));
                json!({ "role": "assistant", "content": content })
            }
            _ if message.role == Role::Tool => json!({
                "role": "tool",
                "tool_call_id": message.tool_call_id,
                "content": message.content,
            }),
            _ if !message.tool_calls.is_empty() => json!({
                "role": "assistant",
                "content": message.content,
                "tool_calls": message.tool_calls.iter().map(|call| json!({
                    "id": call.id,
                    "type": "function",
                    "function": {
                        "name": call.name,
                        "arguments": call.input.to_string(),
                    }
                })).collect::<Vec<_>>(),
            }),
            _ => json!({ "role": message.role.as_str(), "content": message.content }),
        }
    }
}

/// Work out the URL to POST to from what the app wrote.
///
/// A full path is used as given; anything shorter is treated as an origin (or
/// an origin plus a prefix, for a gateway mounted under one) and gets the
/// provider's standard path appended.
fn resolve_url(provider: Provider, endpoint: &str) -> Result<String, AiError> {
    let endpoint = endpoint.trim().trim_end_matches('/');
    if endpoint.is_empty() {
        let origin = provider.default_origin();
        if origin.is_empty() {
            return Err(AiError::Config(
                "[ai] endpoint is required for provider `custom` — the URL of the server that answers"
                    .to_string(),
            ));
        }
        return Ok(format!("{origin}{}", provider.path()));
    }
    if !endpoint.contains("://") {
        return Err(AiError::Config(format!(
            "[ai] endpoint `{endpoint}` is not a URL — write the scheme too, e.g. http://localhost:8080"
        )));
    }
    // Already the completions endpoint: leave it exactly as written, so a
    // gateway can mount it wherever it likes.
    if endpoint.ends_with("/chat/completions") || endpoint.ends_with("/messages") {
        return Ok(endpoint.to_string());
    }
    // `…/v1` is the other thing people paste, and appending the full path to it
    // would produce `/v1/v1/…`.
    if let Some(base) = endpoint.strip_suffix("/v1") {
        return Ok(format!("{base}{}", provider.path()));
    }
    Ok(format!("{endpoint}{}", provider.path()))
}

/// Pull every complete SSE frame out of the buffer, leaving any partial tail.
/// The tags a model writes around its thinking when the server in front of it
/// does not parse them into a field of their own.
const THINKING_TAGS: [(&str, &str); 3] = [
    ("<think>", "</think>"),
    ("<thinking>", "</thinking>"),
    ("<reasoning>", "</reasoning>"),
];

/// Every closing tag, for a block whose opening tag was never sent.
const CLOSING_TAGS: [&str; 3] = ["</think>", "</thinking>", "</reasoning>"];

/// Where the thinking is, in the shape the *server* chose to send it.
///
/// This is a property of the deployment, not of the model: the same Qwen3
/// weights answer with `reasoning_content`, with a matched `<think>…</think>`
/// pair, or with a bare `</think>` and no opening tag, depending on the
/// server's reasoning parser and chat template. Naming it turns "look at the
/// text and guess" into "read it the way it is written".
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ReasoningFormat {
    /// Native fields first; failing that, read whatever tag shape is in the
    /// text, including a pre-opened block.
    #[default]
    Auto,
    /// The server always fills `reasoning_content`; never scan the text.
    Native,
    /// A matched `<think>…</think>` pair inside the content.
    Tags,
    /// The chat template opened the block before generation started, so the
    /// reply *begins* inside the thinking and the first closing tag ends it.
    Implicit,
}

impl ReasoningFormat {
    /// Parse the config string. Anything unrecognised is [`Auto`], with a
    /// warning: a typo should not silently turn thinking into the answer.
    pub fn parse(value: &str) -> Self {
        match value.trim().to_ascii_lowercase().as_str() {
            "" | "auto" => ReasoningFormat::Auto,
            "native" | "field" | "reasoning_content" => ReasoningFormat::Native,
            "tags" | "tag" | "inline" => ReasoningFormat::Tags,
            "implicit" | "open" | "pre-opened" | "deepseek" => ReasoningFormat::Implicit,
            other => {
                tracing::warn!(
                    reasoning_format = other,
                    "unknown ai.reasoning_format; treating it as `auto`"
                );
                ReasoningFormat::Auto
            }
        }
    }
}

/// Separates inline thinking from the answer, once, at the transport boundary.
///
/// The provider-native fields (`reasoning_content`, Anthropic's
/// `thinking_delta`) are the interface this crate prefers, and where a server
/// populates them nothing here does any work. But a local server whose
/// template emits `<think>…</think>` straight into `content` — llama.cpp
/// without a reasoning format, Ollama, any gateway passing the raw template
/// through — has already merged the two, and only a reader of the text can
/// take them apart again.
///
/// Doing it here means every consumer gets the same guarantee: `Event::Delta`
/// is the answer and `Event::Reasoning` is the thinking, whatever the server
/// did. The alternative — each caller sniffing for tags in text it was handed
/// — is the same string matching done repeatedly, inconsistently, and after
/// the text has already been shown to somebody.
///
/// Stateful because a stream splits wherever it likes: an opening tag can
/// arrive one character per chunk.
#[derive(Debug, Default)]
struct ThinkingSplit {
    /// The closing tags that would end the current thinking block. Empty means
    /// the text being read is the answer. More than one means the block was
    /// opened by the template rather than by the model, so any of them ends it.
    closing: &'static [&'static str],
    /// Text held back because it may yet turn out to be the start of a tag.
    pending: String,
    /// Still at the very start of a pre-opened block, where a template that
    /// *also* emitted the opening tag has to be told apart from one that only
    /// opened it in the prompt.
    leading_open: bool,
}

impl ThinkingSplit {
    /// Read the content the way `format` says the server writes it.
    ///
    /// A pre-opened block is only ever assumed on `implicit`, never on `auto`.
    /// Mid-stream there is nothing to tell one apart from an ordinary answer —
    /// the tag that would settle it has not arrived, and may never — and a
    /// wrong guess hides the whole reply behind the reasoning toggle. `auto`
    /// therefore guesses only where it can be checked: [`split_thinking`], on
    /// text that has finished arriving.
    fn new(format: ReasoningFormat) -> Self {
        let implicit = matches!(format, ReasoningFormat::Implicit);
        ThinkingSplit {
            closing: if implicit { &CLOSING_TAGS } else { &[] },
            pending: String::new(),
            leading_open: implicit,
        }
    }

    /// Feed one chunk of `content`; get back what is safely classified.
    fn push(&mut self, chunk: &str) -> Vec<Event> {
        self.pending.push_str(chunk);
        let mut out = Vec::new();
        // A pre-opened block whose server sends the opening tag anyway would
        // otherwise keep `<think>` at the head of the reasoning text. Consume
        // it — but only once, and only while nothing but whitespace has
        // arrived, so a tag later in the thinking is left alone.
        if self.leading_open && !self.consume_leading_open() {
            return out;
        }
        loop {
            if self.closing.is_empty() {
                let opening = THINKING_TAGS
                    .iter()
                    .filter_map(|(open, close)| {
                        self.pending.find(open).map(|at| (at, *open, *close))
                    })
                    .min_by_key(|(at, _, _)| *at);
                let Some((at, open, close)) = opening else {
                    break;
                };
                let before = self.pending[..at].to_string();
                if !before.is_empty() {
                    out.push(Event::Delta(before));
                }
                self.pending = self.pending[at + open.len()..].to_string();
                self.closing = closing_tag(close);
            } else {
                let found = self
                    .closing
                    .iter()
                    .filter_map(|close| self.pending.find(close).map(|at| (at, *close)))
                    .min_by_key(|(at, _)| *at);
                let Some((at, close)) = found else {
                    break;
                };
                let thinking = self.pending[..at].to_string();
                if !thinking.is_empty() {
                    out.push(Event::Reasoning(thinking));
                }
                self.pending = self.pending[at + close.len()..].to_string();
                self.closing = &[];
            }
        }

        // Whatever cannot yet be a tag is safe to emit; the rest waits for the
        // next chunk. Without this a tag split across chunks is never seen,
        // and with too much of it a slow stream stops being a stream.
        let keep = self.partial_tag_length();
        let ready = self.pending[..self.pending.len() - keep].to_string();
        self.pending = self.pending[self.pending.len() - keep..].to_string();
        if !ready.is_empty() {
            out.push(self.classify(ready));
        }
        out
    }

    /// The answer ended: nothing more is coming, so held-back text is just text.
    fn flush(&mut self) -> Option<Event> {
        self.leading_open = false;
        if self.pending.is_empty() {
            return None;
        }
        let rest = std::mem::take(&mut self.pending);
        // An unterminated block means the model was cut off mid-thought. That
        // is reasoning, not an answer, however it ends.
        Some(self.classify(rest))
    }

    /// The server sent thinking in a field of its own, so it is not also
    /// hiding it in the content: abandon a block only assumed to be open.
    ///
    /// This is the net under a pre-opened guess. `auto` infers "the reply
    /// starts inside `<think>`" from the app having asked for thinking, which
    /// is wrong on a server that has a reasoning parser after all — and this
    /// is that server saying so, before any content has been misfiled.
    fn server_separates_reasoning(&mut self) {
        if self.leading_open {
            self.leading_open = false;
            self.closing = &[];
        }
    }

    /// Which side of the split text read in the current state belongs to.
    fn classify(&self, text: String) -> Event {
        if self.closing.is_empty() {
            Event::Delta(text)
        } else {
            Event::Reasoning(text)
        }
    }

    /// Eat the opening tag of a pre-opened block if the server sent one too.
    ///
    /// Returns whether the decision is made. `false` means the leading text so
    /// far is still a possible prefix of an opening tag and the rest of `push`
    /// must wait for another chunk — a wait bounded by the longest tag, since
    /// anything longer either matches or cannot.
    fn consume_leading_open(&mut self) -> bool {
        let head = self.pending.trim_start();
        if let Some((open, close)) = THINKING_TAGS
            .iter()
            .find(|(open, _)| head.starts_with(open))
        {
            let at = self.pending.len() - head.len() + open.len();
            self.pending = self.pending[at..].to_string();
            self.closing = closing_tag(close);
            self.leading_open = false;
            return true;
        }
        // Still short of a decision: nothing but whitespace so far, or `<thi`,
        // which could become `<think>` or could be the thinking itself.
        if head.is_empty() || THINKING_TAGS.iter().any(|(open, _)| open.starts_with(head)) {
            return false;
        }
        self.leading_open = false;
        true
    }

    /// How many trailing characters could still become a tag.
    fn partial_tag_length(&self) -> usize {
        let candidates: Vec<&str> = if self.closing.is_empty() {
            THINKING_TAGS.iter().map(|(open, _)| *open).collect()
        } else {
            self.closing.to_vec()
        };
        let max = candidates.iter().map(|tag| tag.len()).max().unwrap_or(0);
        let start = self.pending.len().saturating_sub(max - 1);
        for at in start..self.pending.len() {
            if !self.pending.is_char_boundary(at) {
                continue;
            }
            let tail = &self.pending[at..];
            if candidates.iter().any(|tag| tag.starts_with(tail)) {
                return tail.len();
            }
        }
        0
    }
}

/// The one-element slice naming the tag that ends an explicitly opened block.
fn closing_tag(close: &'static str) -> &'static [&'static str] {
    match close {
        "</think>" => &CLOSING_TAGS[0..1],
        "</thinking>" => &CLOSING_TAGS[1..2],
        _ => &CLOSING_TAGS[2..3],
    }
}

/// Split a whole (non-streamed) `content` the same way.
///
/// Reading the finished text is the easy case: a closing tag with no opening
/// one before it *is* a pre-opened block, and there is no guessing to do — so
/// `auto` settles this without help from the config, whatever `thinking` says.
fn split_thinking(content: &str, format: ReasoningFormat) -> (String, String) {
    if format == ReasoningFormat::Native {
        return (content.to_string(), String::new());
    }
    let implicit = match format {
        ReasoningFormat::Implicit => true,
        ReasoningFormat::Auto => dangling_close(content),
        _ => false,
    };
    let mut split = ThinkingSplit::new(if implicit {
        ReasoningFormat::Implicit
    } else {
        ReasoningFormat::Tags
    });
    let mut text = String::new();
    let mut reasoning = String::new();
    for event in split.push(content).into_iter().chain(split.flush()) {
        match event {
            Event::Delta(chunk) => text.push_str(&chunk),
            Event::Reasoning(chunk) => reasoning.push_str(&chunk),
            Event::Done(_) => {}
        }
    }
    (text, reasoning)
}

/// Whether the text closes a thinking block it never opened — the signature of
/// a chat template that opened it in the prompt (Qwen3, DeepSeek-R1) on a
/// server with no reasoning parser in front of it.
fn dangling_close(content: &str) -> bool {
    let close = CLOSING_TAGS
        .iter()
        .filter_map(|close| content.find(close).map(|at| (at, *close)))
        .min_by_key(|(at, _)| *at);
    let Some((at, _)) = close else {
        return false;
    };
    // An opening tag before it means the pair is matched and ordinary.
    !THINKING_TAGS
        .iter()
        .any(|(open, _)| content[..at].contains(open))
}

fn take_frames(buffer: &mut String) -> Vec<String> {
    let mut frames = Vec::new();
    // `\r\n\r\n` is the same separator over a connection that normalises line
    // endings; both appear in the wild.
    while let Some(end) = buffer.find("\n\n").or_else(|| buffer.find("\r\n\r\n")) {
        let width = if buffer[end..].starts_with("\r\n\r\n") {
            4
        } else {
            2
        };
        let frame: String = buffer[..end].to_string();
        buffer.replace_range(..end + width, "");
        frames.push(frame);
    }
    frames
}

/// Turn one SSE frame into an [`Event`], or `None` for the frames that carry
/// nothing a caller cares about — keep-alive comments, an event type this
/// provider uses for bookkeeping, an unparseable payload.
fn parse_frame(provider: Provider, frame: &str) -> Option<Event> {
    let mut data = String::new();
    for line in frame.lines() {
        if let Some(rest) = line.strip_prefix("data:") {
            if !data.is_empty() {
                data.push('\n');
            }
            data.push_str(rest.trim_start());
        }
    }
    let data = data.trim();
    if data.is_empty() {
        return None;
    }
    // OpenAI's terminator, and every compatible server's.
    if data == "[DONE]" {
        return Some(Event::Done(Done::default()));
    }

    let value: Value = serde_json::from_str(data).ok()?;
    match provider {
        Provider::Anthropic => parse_anthropic(&value),
        _ => parse_openai(&value),
    }
}

fn parse_openai(value: &Value) -> Option<Event> {
    // A provider may report an error mid-stream, after a `200`.
    if let Some(error) = value.get("error") {
        let message = error
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or("the provider reported an error mid-answer");
        return Some(Event::Done(Done {
            finish_reason: message.to_string(),
            ..Done::default()
        }));
    }

    let choices = value.get("choices").and_then(Value::as_array);
    let choice = choices.and_then(|c| c.first());
    let delta = choice.and_then(|c| c.get("delta"));

    let text = |field: &str| {
        delta
            .and_then(|d| d.get(field))
            .and_then(Value::as_str)
            .filter(|text| !text.is_empty())
            .map(str::to_string)
    };

    if let Some(content) = text("content") {
        return Some(Event::Delta(content));
    }
    // What a reasoning model emits before it starts answering. Several names
    // are in circulation for the same field and a server picks one.
    if let Some(thinking) = text("reasoning_content").or_else(|| text("reasoning")) {
        return Some(Event::Reasoning(thinking));
    }

    let finish = choice
        .and_then(|c| c.get("finish_reason"))
        .and_then(Value::as_str)
        .filter(|reason| !reason.is_empty());

    let usage = value.get("usage").and_then(|usage| {
        let counts = Done {
            finish_reason: String::new(),
            input_tokens: usage.get("prompt_tokens").and_then(Value::as_u64),
            output_tokens: usage.get("completion_tokens").and_then(Value::as_u64),
        };
        (counts.input_tokens.is_some() || counts.output_tokens.is_some()).then_some(counts)
    });

    match (finish, usage) {
        // The provider said why it stopped: that is the end, with whatever
        // token counts came alongside.
        (Some(reason), counts) => Some(Event::Done(Done {
            finish_reason: reason.to_string(),
            ..counts.unwrap_or_default()
        })),
        // OpenAI's trailing usage frame carries no choice at all. A server
        // that repeats `usage` on *every* chunk (llama.cpp does) must not be
        // read as ending the answer on its first one — which is why the empty
        // choice list, and not the presence of usage, is what decides here.
        (None, Some(counts)) if choice.is_none() => Some(Event::Done(counts)),
        _ => None,
    }
}

fn parse_complete(provider: Provider, format: ReasoningFormat, value: &Value) -> Option<ChatReply> {
    match provider {
        Provider::Anthropic => parse_anthropic_complete(value),
        _ => parse_openai_complete(format, value),
    }
}

fn parse_openai_complete(format: ReasoningFormat, value: &Value) -> Option<ChatReply> {
    if value.get("error").is_some() {
        return None;
    }
    let choice = value
        .get("choices")
        .and_then(Value::as_array)
        .and_then(|choices| choices.first())?;
    let message = choice.get("message")?;
    let (text, inline_reasoning) = split_thinking(
        message
            .get("content")
            .and_then(Value::as_str)
            .unwrap_or_default(),
        format,
    );
    // The native field is the interface; inline tags are what a server that
    // does not populate it leaves behind. A server can produce both.
    let reasoning = message
        .get("reasoning_content")
        .and_then(Value::as_str)
        .filter(|text| !text.is_empty())
        .or_else(|| {
            message
                .get("reasoning")
                .and_then(Value::as_str)
                .filter(|text| !text.is_empty())
        })
        .unwrap_or(inline_reasoning.as_str())
        .to_string();
    let tool_calls = message
        .get("tool_calls")
        .and_then(Value::as_array)
        .map(|calls| {
            calls
                .iter()
                .filter_map(|call| {
                    let function = call.get("function")?;
                    let name = function.get("name").and_then(Value::as_str)?;
                    let arguments = function
                        .get("arguments")
                        .and_then(Value::as_str)
                        .and_then(|raw| serde_json::from_str(raw).ok())
                        .unwrap_or(Value::Object(Default::default()));
                    Some(ToolCall {
                        id: call
                            .get("id")
                            .and_then(Value::as_str)
                            .unwrap_or(name)
                            .to_string(),
                        name: name.to_string(),
                        input: arguments,
                    })
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let usage = value.get("usage");
    Some(ChatReply {
        text,
        reasoning,
        provider: String::new(),
        model: String::new(),
        done: Done {
            finish_reason: choice
                .get("finish_reason")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
            input_tokens: usage
                .and_then(|u| u.get("prompt_tokens"))
                .and_then(Value::as_u64),
            output_tokens: usage
                .and_then(|u| u.get("completion_tokens"))
                .and_then(Value::as_u64),
        },
        tool_calls,
    })
}

fn parse_anthropic_complete(value: &Value) -> Option<ChatReply> {
    if value.get("type").and_then(Value::as_str) == Some("error") {
        return None;
    }
    let mut text = String::new();
    let mut tool_calls = Vec::new();
    for block in value
        .get("content")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default()
    {
        match block.get("type").and_then(Value::as_str) {
            Some("text") => {
                if let Some(chunk) = block.get("text").and_then(Value::as_str) {
                    text.push_str(chunk);
                }
            }
            Some("tool_use") => {
                let Some(name) = block.get("name").and_then(Value::as_str) else {
                    continue;
                };
                tool_calls.push(ToolCall {
                    id: block
                        .get("id")
                        .and_then(Value::as_str)
                        .unwrap_or(name)
                        .to_string(),
                    name: name.to_string(),
                    input: block
                        .get("input")
                        .cloned()
                        .unwrap_or(Value::Object(Default::default())),
                });
            }
            _ => {}
        }
    }
    let usage = value.get("usage");
    Some(ChatReply {
        text,
        reasoning: String::new(),
        provider: String::new(),
        model: String::new(),
        done: Done {
            finish_reason: value
                .get("stop_reason")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
            input_tokens: usage
                .and_then(|u| u.get("input_tokens"))
                .and_then(Value::as_u64),
            output_tokens: usage
                .and_then(|u| u.get("output_tokens"))
                .and_then(Value::as_u64),
        },
        tool_calls,
    })
}

fn parse_anthropic(value: &Value) -> Option<Event> {
    match value.get("type").and_then(Value::as_str)? {
        "content_block_delta" => {
            let delta = value.get("delta")?;
            let text = |field: &str| {
                delta
                    .get(field)
                    .and_then(Value::as_str)
                    .filter(|text| !text.is_empty())
                    .map(str::to_string)
            };
            // An extended-thinking block streams as `thinking_delta`, which is
            // the same distinction `Event::Reasoning` exists for.
            match text("text") {
                Some(text) => Some(Event::Delta(text)),
                None => text("thinking").map(Event::Reasoning),
            }
        }
        // The stop reason and the output token count both land here, one frame
        // before `message_stop`.
        "message_delta" => Some(Event::Done(Done {
            finish_reason: value
                .get("delta")
                .and_then(|d| d.get("stop_reason"))
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
            input_tokens: value
                .get("usage")
                .and_then(|u| u.get("input_tokens"))
                .and_then(Value::as_u64),
            output_tokens: value
                .get("usage")
                .and_then(|u| u.get("output_tokens"))
                .and_then(Value::as_u64),
        })),
        "message_stop" => Some(Event::Done(Done::default())),
        "error" => Some(Event::Done(Done {
            finish_reason: value
                .get("error")
                .and_then(|e| e.get("message"))
                .and_then(Value::as_str)
                .unwrap_or("the provider reported an error mid-answer")
                .to_string(),
            ..Done::default()
        })),
        // `message_start`, `content_block_start`, `ping`, …
        _ => None,
    }
}

/// The sentence out of an error body worth putting in a log line.
fn brief(body: &str) -> String {
    let message = serde_json::from_str::<Value>(body)
        .ok()
        .and_then(|v| {
            v.get("error")
                .and_then(|e| e.get("message"))
                .or_else(|| v.get("message"))
                .and_then(Value::as_str)
                .map(str::to_string)
        })
        .unwrap_or_else(|| body.trim().to_string());
    if message.len() > 500 {
        format!("{}…", &message[..500])
    } else {
        message
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config(provider: &str) -> AiConfig {
        AiConfig {
            provider: provider.to_string(),
            model: "test-model".to_string(),
            api_key: "key".to_string(),
            ..AiConfig::default()
        }
    }

    #[test]
    fn provider_names_include_the_ones_people_actually_type() {
        assert_eq!(Provider::parse("OpenAI"), Some(Provider::OpenAi));
        assert_eq!(Provider::parse(" claude "), Some(Provider::Anthropic));
        // Naming your local server is a shortcut to `custom`, not a fourth
        // provider — they all speak the same dialect.
        assert_eq!(Provider::parse("ollama"), Some(Provider::Custom));
        assert_eq!(Provider::parse("vllm"), Some(Provider::Custom));
        assert_eq!(Provider::parse("bedrock"), None);
    }

    #[test]
    fn an_endpoint_is_completed_unless_it_already_names_the_path() {
        // Nothing at all: the provider's own API.
        assert_eq!(
            resolve_url(Provider::OpenAi, "").unwrap(),
            "https://api.openai.com/v1/chat/completions"
        );
        assert_eq!(
            resolve_url(Provider::Anthropic, "").unwrap(),
            "https://api.anthropic.com/v1/messages"
        );
        // A bare origin — what a local server is.
        assert_eq!(
            resolve_url(Provider::Custom, "http://localhost:8080").unwrap(),
            "http://localhost:8080/v1/chat/completions"
        );
        assert_eq!(
            resolve_url(Provider::Custom, "http://localhost:8080/").unwrap(),
            "http://localhost:8080/v1/chat/completions"
        );
        // The two things people paste that would otherwise double up.
        assert_eq!(
            resolve_url(Provider::Custom, "http://localhost:8080/v1").unwrap(),
            "http://localhost:8080/v1/chat/completions"
        );
        assert_eq!(
            resolve_url(
                Provider::Custom,
                "http://gw.internal/llm/v1/chat/completions"
            )
            .unwrap(),
            "http://gw.internal/llm/v1/chat/completions"
        );
    }

    #[test]
    fn a_custom_provider_without_an_endpoint_cannot_boot() {
        let mut config = config("custom");
        config.endpoint.clear();
        let err = Ai::from_config(&config).unwrap_err().to_string();
        assert!(err.contains("endpoint"), "{err}");

        // …and one that isn't a URL says so, rather than failing at the first
        // question with a confusing relative-URL error.
        config.endpoint = "localhost:8080".to_string();
        assert!(Ai::from_config(&config)
            .unwrap_err()
            .to_string()
            .contains("not a URL"));
    }

    #[test]
    fn a_disabled_ai_section_builds_no_client() {
        assert!(Ai::from_config(&AiConfig::default()).unwrap().is_none());
        assert!(matches!(
            Ai::from_config(&config("mistral-ai")),
            Err(AiError::Config(_))
        ));
    }

    #[test]
    fn the_system_prompt_is_taken_from_the_most_specific_place_that_named_one() {
        let mut config = config("openai");
        config.system = "from config".to_string();

        // The app's default, when nothing else says otherwise.
        let plain = ChatRequest::ask("hi").resolve(&config).unwrap();
        assert_eq!(plain.system, "from config");

        // The request's own field beats it.
        let mut named = ChatRequest::ask("hi");
        named.system = Some("from request".to_string());
        assert_eq!(named.resolve(&config).unwrap().system, "from request");

        // And a system message in the conversation beats both.
        let inline = ChatRequest {
            messages: vec![Message::system("from messages"), Message::user("hi")],
            system: Some("from request".to_string()),
            ..ChatRequest::default()
        };
        let resolved = inline.resolve(&config).unwrap();
        assert_eq!(resolved.system, "from messages");
        // …and it does not stay in the dialogue, or it would be sent twice.
        assert_eq!(resolved.messages.len(), 1);
    }

    #[test]
    fn a_conversation_with_nothing_in_it_is_refused() {
        let config = config("openai");
        assert!(matches!(
            ChatRequest::default().resolve(&config),
            Err(AiError::Request(_))
        ));
        // Whitespace is nothing, and a system prompt is not a question.
        let system_only = ChatRequest {
            messages: vec![Message::system("be terse"), Message::user("   ")],
            ..ChatRequest::default()
        };
        assert!(matches!(
            system_only.resolve(&config),
            Err(AiError::Request(_))
        ));
    }

    #[test]
    fn anthropic_carries_the_system_prompt_out_of_band_and_openai_does_not() {
        let request = ChatRequest {
            messages: vec![Message::system("be terse"), Message::user("hi")],
            ..ChatRequest::default()
        };

        let openai = Ai::from_config(&config("openai")).unwrap().unwrap();
        let body = openai.body(&request.resolve(&openai.config).unwrap(), true, &[]);
        assert_eq!(body["messages"][0]["role"], "system");
        assert_eq!(body["messages"][1]["content"], "hi");
        assert!(body.get("system").is_none());

        let anthropic = Ai::from_config(&config("anthropic")).unwrap().unwrap();
        let body = anthropic.body(&request.resolve(&anthropic.config).unwrap(), true, &[]);
        assert_eq!(body["system"], "be terse");
        assert_eq!(body["messages"][0]["role"], "user");
        // Anthropic refuses a request without one, so it is always sent.
        assert_eq!(body["max_tokens"], 2048);
    }

    #[test]
    fn openai_plain_user_message_is_not_a_tool_result() {
        let ai = Ai::from_config(&config("openai")).unwrap().unwrap();
        let request = ChatRequest::ask("hi");
        let body = ai.body(&request.resolve(&ai.config).unwrap(), false, &[]);
        assert_eq!(
            body["messages"][0],
            json!({ "role": "user", "content": "hi" })
        );
    }

    #[test]
    fn frames_are_only_taken_once_they_are_complete() {
        let mut buffer = String::from("data: one\n\ndata: two\n\ndata: par");
        assert_eq!(take_frames(&mut buffer), ["data: one", "data: two"]);
        // The partial tail stays put until the rest of it arrives.
        assert_eq!(buffer, "data: par");
        buffer.push_str("tial\n\n");
        assert_eq!(take_frames(&mut buffer), ["data: partial"]);
        assert!(buffer.is_empty());

        // Same bytes, CRLF line endings.
        let mut crlf = String::from("data: one\r\n\r\n");
        assert_eq!(take_frames(&mut crlf), ["data: one"]);
    }

    #[test]
    fn openai_frames_become_deltas_and_one_ending() {
        let frame = |json: &str| parse_frame(Provider::OpenAi, &format!("data: {json}"));

        assert_eq!(
            frame(r#"{"choices":[{"delta":{"content":"Hel"}}]}"#),
            Some(Event::Delta("Hel".to_string()))
        );
        // The opening frame names the role and carries no text.
        assert_eq!(
            frame(r#"{"choices":[{"delta":{"role":"assistant"}}]}"#),
            None
        );
        // Keep-alive comments and blank frames carry nothing.
        assert_eq!(parse_frame(Provider::OpenAi, ": ping"), None);

        assert_eq!(
            frame(r#"{"choices":[{"delta":{},"finish_reason":"stop"}]}"#),
            Some(Event::Done(Done {
                finish_reason: "stop".to_string(),
                ..Done::default()
            }))
        );
        // The usage-only frame is where the token counts live.
        assert_eq!(
            frame(r#"{"choices":[],"usage":{"prompt_tokens":9,"completion_tokens":4}}"#),
            Some(Event::Done(Done {
                finish_reason: String::new(),
                input_tokens: Some(9),
                output_tokens: Some(4),
            }))
        );
        assert_eq!(
            parse_frame(Provider::OpenAi, "data: [DONE]"),
            Some(Event::Done(Done::default()))
        );
    }

    #[test]
    fn openai_tool_history_uses_string_content() {
        let ai = Ai::from_config(&config("openai")).unwrap().unwrap();
        let request = ChatRequest {
            messages: vec![
                Message::assistant_tool_calls(vec![ToolCall {
                    id: "call_1".to_string(),
                    name: "current_user_notes".to_string(),
                    input: serde_json::json!({}),
                }]),
                Message::tool_result("call_1", r#"{"notes":[]}"#),
            ],
            tools: vec![ToolDefinition {
                name: "current_user_notes".to_string(),
                description: "Fetch notes.".to_string(),
                input_schema: serde_json::json!({ "type": "object", "properties": {} }),
            }],
            ..ChatRequest::default()
        };
        let body = ai.body(&request.resolve(&ai.config).unwrap(), false, &request.tools);
        assert_eq!(body["messages"][0]["content"], "");
        assert_eq!(body["messages"][1]["role"], "tool");
        assert_eq!(body["messages"][1]["tool_call_id"], "call_1");
        assert_eq!(body["messages"][1]["content"], r#"{"notes":[]}"#);
    }

    #[test]
    fn openai_complete_keeps_reasoning_separate_from_response_text() {
        let reply = parse_complete(
            Provider::OpenAi,
            ReasoningFormat::Auto,
            &json!({
                "choices": [{
                    "message": {
                        "role": "assistant",
                        "content": "",
                        "reasoning_content": "A compact summary."
                    },
                    "finish_reason": "stop"
                }],
                "usage": { "prompt_tokens": 9, "completion_tokens": 4 }
            }),
        )
        .unwrap();

        assert_eq!(reply.text, "");
        assert_eq!(reply.reasoning, "A compact summary.");
        assert_eq!(reply.done.input_tokens, Some(9));
        assert_eq!(reply.done.output_tokens, Some(4));
    }

    /// llama.cpp repeats the running `usage` on *every* chunk. Reading that as
    /// the trailing usage frame ends the answer on its first token — which is
    /// exactly what happened before the empty-choice-list rule below.
    #[test]
    fn usage_on_every_chunk_does_not_end_the_answer() {
        let frame = |json: &str| parse_frame(Provider::OpenAi, &format!("data: {json}"));

        // The opening frame: a role, no text, and a token count already.
        assert_eq!(
            frame(
                r#"{"choices":[{"finish_reason":null,"index":0,"delta":{"role":"assistant","content":null}}],"usage":{"completion_tokens":1,"prompt_tokens":20}}"#
            ),
            None
        );
        // Text still arrives as text, usage or no usage.
        assert_eq!(
            frame(
                r#"{"choices":[{"delta":{"content":"red"}}],"usage":{"completion_tokens":2,"prompt_tokens":20}}"#
            ),
            Some(Event::Delta("red".to_string()))
        );
        // And the end is the frame that says why it stopped.
        assert_eq!(
            frame(
                r#"{"choices":[{"delta":{},"finish_reason":"stop"}],"usage":{"completion_tokens":9,"prompt_tokens":20}}"#
            ),
            Some(Event::Done(Done {
                finish_reason: "stop".to_string(),
                input_tokens: Some(20),
                output_tokens: Some(9),
            }))
        );
    }

    /// A reasoning model streams its thinking first. It is not the answer, so
    /// it arrives as its own event and never lands in `text`.
    fn split_stream(chunks: &[&str]) -> (String, String) {
        split_stream_as(ReasoningFormat::Tags, chunks)
    }

    /// The same, for a server whose shape has been named.
    fn split_stream_as(format: ReasoningFormat, chunks: &[&str]) -> (String, String) {
        let mut split = ThinkingSplit::new(format);
        let (mut text, mut reasoning) = (String::new(), String::new());
        for event in chunks
            .iter()
            .flat_map(|chunk| split.push(chunk))
            .collect::<Vec<_>>()
            .into_iter()
            .chain(split.flush())
        {
            match event {
                Event::Delta(chunk) => text.push_str(&chunk),
                Event::Reasoning(chunk) => reasoning.push_str(&chunk),
                Event::Done(_) => {}
            }
        }
        (text, reasoning)
    }

    /// The bug this guards: a server with no reasoning parser puts the whole
    /// `<think>` block in `content`, and it used to reach the user as answer.
    #[test]
    fn inline_thinking_never_reaches_the_answer() {
        let (text, reasoning) = split_stream(&["<think>Let me see.</think>The sea is cold."]);
        assert_eq!(text, "The sea is cold.");
        assert_eq!(reasoning, "Let me see.");
    }

    #[test]
    fn a_tag_split_across_chunks_is_still_a_tag() {
        // A stream splits wherever it likes, including mid-tag.
        let (text, reasoning) = split_stream(&["<thi", "nk>Let me", " see.</th", "ink>Cold."]);
        assert_eq!(text, "Cold.");
        assert_eq!(reasoning, "Let me see.");

        // …and text that only looks like the start of one is not held back.
        let (text, reasoning) = split_stream(&["1 < 2 and 3 > 2"]);
        assert_eq!(text, "1 < 2 and 3 > 2");
        assert!(reasoning.is_empty());
    }

    #[test]
    fn an_unterminated_block_is_all_thinking() {
        // Cut off mid-thought: there is no answer in there to salvage.
        let (text, reasoning) = split_stream(&["<think>I should start by"]);
        assert!(text.is_empty());
        assert_eq!(reasoning, "I should start by");
    }

    #[test]
    fn text_without_tags_is_passed_through_untouched() {
        let (text, reasoning) = split_stream(&["The sea ", "is cold."]);
        assert_eq!(text, "The sea is cold.");
        assert!(reasoning.is_empty());
    }

    #[test]
    fn thinking_is_asked_for_with_each_providers_own_parameter() {
        let body = |provider: &str, thinking: Option<bool>| {
            let mut config = config(provider);
            config.thinking = thinking;
            config.max_tokens = 8_000;
            config.endpoint = "http://localhost:8080".to_string();
            let ai = Ai::from_config(&config).unwrap().unwrap();
            ai.body(
                &ChatRequest::ask("hello").resolve(&config).unwrap(),
                false,
                &[],
            )
        };

        // Local OpenAI-compatible servers read the chat template kwarg.
        assert_eq!(
            body("custom", Some(false))["chat_template_kwargs"],
            json!({ "enable_thinking": false })
        );
        assert_eq!(
            body("custom", Some(true))["chat_template_kwargs"],
            json!({ "enable_thinking": true })
        );

        // Anthropic has a parameter of its own, and rejects a temperature
        // alongside it.
        let anthropic = body("anthropic", Some(true));
        assert_eq!(anthropic["thinking"]["type"], "enabled");
        assert!(anthropic["thinking"]["budget_tokens"].as_u64().unwrap() >= 1_024);
        assert!(anthropic.get("temperature").is_none());
        assert_eq!(
            body("anthropic", Some(false))["thinking"]["type"],
            "disabled"
        );

        // Saying nothing sends nothing: a server that has never heard of the
        // parameter must not be handed one.
        assert!(body("custom", None).get("chat_template_kwargs").is_none());
        assert!(body("anthropic", None).get("thinking").is_none());
        // OpenAI's models either cannot be switched off or reject the field.
        assert!(body("openai", Some(false))
            .get("chat_template_kwargs")
            .is_none());
        assert!(body("openai", Some(false))
            .get("reasoning_effort")
            .is_none());
    }

    #[test]
    fn thinking_arrives_separately_from_the_answer() {
        assert_eq!(
            parse_frame(
                Provider::OpenAi,
                r#"data: {"choices":[{"delta":{"reasoning_content":"Let me see"}}]}"#
            ),
            Some(Event::Reasoning("Let me see".to_string()))
        );
        assert_eq!(
            parse_frame(
                Provider::Anthropic,
                r#"data: {"type":"content_block_delta","delta":{"type":"thinking_delta","thinking":"Let me see"}}"#
            ),
            Some(Event::Reasoning("Let me see".to_string()))
        );
    }

    #[test]
    fn anthropic_frames_become_deltas_and_one_ending() {
        let frame = |json: &str| parse_frame(Provider::Anthropic, &format!("data: {json}"));

        assert_eq!(
            frame(r#"{"type":"content_block_delta","delta":{"type":"text_delta","text":"Hel"}}"#),
            Some(Event::Delta("Hel".to_string()))
        );
        assert_eq!(frame(r#"{"type":"message_start","message":{}}"#), None);
        assert_eq!(frame(r#"{"type":"ping"}"#), None);
        assert_eq!(
            frame(
                r#"{"type":"message_delta","delta":{"stop_reason":"end_turn"},"usage":{"output_tokens":12}}"#
            ),
            Some(Event::Done(Done {
                finish_reason: "end_turn".to_string(),
                input_tokens: None,
                output_tokens: Some(12),
            }))
        );
    }

    /// A frame may arrive with its event name on one line and its payload on
    /// another, and the payload may itself be split across `data:` lines.
    #[test]
    fn a_frames_data_lines_are_joined_before_being_read() {
        let frame = "event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\ndata: \"delta\":{\"text\":\"hi\"}}";
        assert_eq!(
            parse_frame(Provider::Anthropic, frame),
            Some(Event::Delta("hi".to_string()))
        );
    }

    /// The API key must not reach a log line by way of a debug print.
    #[test]
    fn debug_does_not_leak_credentials() {
        let mut config = config("custom");
        config.endpoint = "http://localhost:8080".to_string();
        config.api_key = "sk-secret-value".to_string();
        let printed = format!("{:?}", Ai::from_config(&config).unwrap().unwrap());
        assert!(!printed.contains("sk-secret"), "{printed}");
        assert!(printed.contains("localhost:8080"), "{printed}");
    }

    #[test]
    fn an_error_body_is_reduced_to_the_sentence_in_it() {
        assert_eq!(
            brief(r#"{"error":{"message":"model not found","type":"invalid_request_error"}}"#),
            "model not found"
        );
        assert_eq!(brief("plain text failure"), "plain text failure");
    }

    /// The failure this whole thing exists for: Qwen3 and DeepSeek-R1 chat
    /// templates open `<think>` in the *prompt*, so the model never generates
    /// an opening tag and the reply is `thinking…</think>answer`. Read as a
    /// matched pair that is no reasoning at all, and the thinking becomes the
    /// message.
    #[test]
    fn a_pre_opened_block_is_thinking_not_the_answer() {
        let whole = "The user greeted me.</think>Hello!";
        let (text, reasoning) = split_thinking(whole, ReasoningFormat::Auto);
        assert_eq!(text, "Hello!");
        assert_eq!(reasoning, "The user greeted me.");

        // Streamed, it takes the deployment's shape being named: mid-stream
        // there is nothing yet to tell a pre-opened block from an answer.
        let (text, reasoning) = split_stream_as(
            ReasoningFormat::Implicit,
            &["The user gre", "eted me.</thi", "nk>Hello!"],
        );
        assert_eq!(text, "Hello!");
        assert_eq!(reasoning, "The user greeted me.");
    }

    /// A pre-opened block whose server emits the opening tag as well must not
    /// leave `<think>` at the head of the reasoning.
    #[test]
    fn a_pre_opened_block_tolerates_the_opening_tag_arriving_too() {
        let (text, reasoning) = split_stream_as(
            ReasoningFormat::Implicit,
            &["<think>\nWhy.</think>Because."],
        );
        assert_eq!(text, "Because.");
        assert_eq!(reasoning, "\nWhy.");
    }

    /// Nothing to split: a model that was told not to think, on a server that
    /// pre-opens nothing, must not have its answer swallowed as reasoning.
    #[test]
    fn an_answer_with_no_tags_is_all_answer() {
        for format in [ReasoningFormat::Auto, ReasoningFormat::Tags] {
            let (text, reasoning) = split_thinking("Just the answer.", format);
            assert_eq!(text, "Just the answer.");
            assert!(reasoning.is_empty(), "{format:?}");
        }
        let (text, reasoning) = split_stream_as(ReasoningFormat::Auto, &["Just the ", "answer."]);
        assert_eq!(text, "Just the answer.");
        assert!(reasoning.is_empty());
    }

    /// `native` is a promise that the server fills `reasoning_content`, so the
    /// text is left exactly as it came — tags in a code block included.
    #[test]
    fn native_never_reads_the_text() {
        let (text, reasoning) =
            split_thinking("Write `<think>` to open one.", ReasoningFormat::Native);
        assert_eq!(text, "Write `<think>` to open one.");
        assert!(reasoning.is_empty());
    }

    /// A matched pair is still a matched pair when the reply also opens with
    /// text, which is what tells `auto` not to reach for the implicit rule.
    #[test]
    fn a_matched_pair_after_text_is_not_a_pre_opened_block() {
        let (text, reasoning) =
            split_thinking("Sure. <think>brief</think> Done.", ReasoningFormat::Auto);
        assert_eq!(text, "Sure.  Done.");
        assert_eq!(reasoning, "brief");
    }

    #[test]
    fn reasoning_format_parses_what_the_config_can_say() {
        assert_eq!(ReasoningFormat::parse(""), ReasoningFormat::Auto);
        assert_eq!(ReasoningFormat::parse(" Auto "), ReasoningFormat::Auto);
        assert_eq!(ReasoningFormat::parse("native"), ReasoningFormat::Native);
        assert_eq!(ReasoningFormat::parse("tags"), ReasoningFormat::Tags);
        assert_eq!(
            ReasoningFormat::parse("implicit"),
            ReasoningFormat::Implicit
        );
        // A typo is not a licence to show the thinking as the answer.
        assert_eq!(ReasoningFormat::parse("deepseek-r1"), ReasoningFormat::Auto);
    }

    /// The guess `auto` makes has to lose to evidence: a server that fills
    /// `reasoning_content` is not also pre-opening a block in the content, and
    /// its answer must not disappear behind the reasoning toggle.
    #[test]
    fn a_native_reasoning_field_cancels_the_pre_opened_guess() {
        let mut split = ThinkingSplit::new(ReasoningFormat::Implicit);
        split.server_separates_reasoning();
        let events: Vec<Event> = split
            .push("Hello!")
            .into_iter()
            .chain(split.flush())
            .collect();
        assert_eq!(events, vec![Event::Delta("Hello!".to_string())]);
    }
}
