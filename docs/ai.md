# AI

An app names a provider in `main.toml` and gets four things: a chat endpoint
that streams its reply, optional configured agents from `agents/*.toml`, a
`chat` call inside every function, and a `/stream` endpoint on every function
so a function can pass an answer through as it arrives.

```toml
[ai]
provider = "openai"
model    = "gpt-4o-mini"
api_key  = "$OPENAI_API_KEY"
```

AI support is off by default. An app that does not configure a provider has no
AI configuration, no client and no AI endpoints.

---

## Providers

| `provider` | Endpoint | Credentials |
|------------|----------|-------------|
| `openai` | `api.openai.com/v1/chat/completions` | `api_key`, sent as `Authorization: Bearer` |
| `anthropic` | `api.anthropic.com/v1/messages` | `api_key`, sent as `x-api-key` |
| `custom` | whatever `endpoint` says, in the OpenAI chat-completions shape | `api_key`, or none at all |

`custom` is a first-class option. llama.cpp, vLLM, Ollama, LM Studio, LiteLLM,
OpenRouter, Together, Groq and most corporate gateways implement the OpenAI
request shape, so pointing `endpoint` at one is the entire integration:

```toml
[ai]
provider = "custom"
endpoint = "http://localhost:8080"        # llama.cpp, vLLM, LM Studio
model    = "local"
```

`api_key` is optional for `custom`. An empty key sends **no** authorization
header rather than an empty one, which is what most local servers require.

The three providers differ in URL, authentication, request shape and event
format. Everything above that layer is identical: the endpoint, the function
call and the event types do not change, so switching provider is a
configuration edit.

---

## `[ai]` in full

| Key | Default | Means |
|-----|---------|-------|
| `provider` | `"none"` | `none`, `openai`, `anthropic`, `custom` |
| `endpoint` | provider's own API | where to POST. Required for `custom` |
| `model` | `""` | model to ask for when a request doesn't name one |
| `api_key` | `""` | the credential. Empty sends no auth header |
| `system` | `""` | system prompt for conversations that don't carry one |
| `max_tokens` | `2048` | cap on generated tokens. Anthropic requires one, so it is always sent |
| `temperature` | unset | sent only when `>= 0`; otherwise the provider decides |
| `reasoning` | `false` | whether the model's thinking is surfaced to callers |
| `thinking` | unset | whether to ask the model to think, using the provider's own switch |
| `reasoning_format` | `"auto"` | how the server hands the thinking back: `auto`, `native`, `tags`, `implicit` |
| `access` | `"authenticated"` | who may call `<base>/ai/chat` |
| `timeout_secs` | `300` | how long one completion may take |

### `endpoint`

Written as an origin (`http://localhost:8080`), as a base
(`https://gw.internal/llm/v1`) or as the full path
(`…/v1/chat/completions`, `…/v1/messages`). The first two get the provider's
standard path appended; the last is used exactly as written, so a gateway can
mount it wherever it likes.

### `max_tokens` and reasoning models

A reasoning model (Qwen, DeepSeek, an `o`-series model) consumes tokens on
internal reasoning before producing any output. If the budget is exhausted
during that phase the result is an **empty answer**, which is a configuration
problem rather than a bug. Set the limit generously for these models.

### `reasoning_format` — where the thinking actually is

`reasoning` and `thinking` are about the model. `reasoning_format` is about the
**server**: the same Qwen3 weights hand their thinking back in three different
shapes depending on the reasoning parser and chat template in front of them,
and only one of them is a field of its own.

| Value | The server sends |
|-------|------------------|
| `auto` (default) | whichever of the three it finds |
| `native` | `reasoning_content` alongside the answer — llama.cpp `--reasoning-format deepseek`, vLLM `--reasoning-parser` |
| `tags` | a matched `<think>…</think>` pair inside the content |
| `implicit` | the template opened the block in the *prompt*, so every reply starts inside the thinking and the first `</think>` ends it |

`implicit` is the one that surprises people. Qwen3 and DeepSeek-R1 templates
append `<think>` to the assistant turn themselves, so the model never generates
an opening tag and a raw llama.cpp reply looks like:

```
The user is greeting me, so a short reply.</think>Hello!
```

Read as a matched pair that is no pair at all, the thinking *is* the message —
which is exactly what you see when a local model's answer arrives with its
whole train of thought in front of it.

`auto` settles this on its own for a reply read whole (`stream: false`): a
closing tag with nothing opening it before it can only be a pre-opened block.
While a reply is still **streaming** it cannot, and does not try — the first
token of a pre-opened block is indistinguishable from the first token of an
answer, and guessing wrong hides the whole reply behind the reasoning toggle.
Name the shape instead:

```toml
[ai]
reasoning_format = "implicit"
```

Then the thinking arrives as `reasoning` events and never as `delta`, so it
shows up behind the **Show reasoning** toggle rather than in the message body.

Most llama.cpp builds need none of this: `--reasoning-format deepseek` is the
default, and they fill `reasoning_content` like any other server. Check before
reaching for `implicit` — a raw `curl` at the endpoint answers it in one line.

### An answer that is all thinking

A reasoning model can spend its entire `max_tokens` budget thinking and stop
with nothing left to say. The turn then has a full reasoning trace and an empty
answer, and how that is reported depends on `reasoning`:

- **shown** — the trace stays *reasoning*. The message is empty, and a
  `warning` event says the model ran out of tokens before answering. The thinking
  never becomes the reply.
- **hidden** — the trace is about to be discarded and nothing else survives the
  turn, so it is promoted to the answer with a `warning` saying so.

Either way the fix is a larger `max_tokens`: a local Qwen can spend three
thousand tokens deciding how to say hello.

### `access`

The same grammar a resource's `[permissions]` uses: `public`,
`authenticated` (the default), `member`, `role:<name>`, `private`.

The default is intentionally not `public`. The endpoint consumes provider
credit or GPU time on behalf of the caller, so a public setting makes it an open
proxy to your provider account. That should be an explicit choice rather than an
omission. As with every other access string in the framework, an unrecognised
value denies access rather than granting it.

---

## `POST <base>/ai/chat`

```json
{
  "messages": [{ "role": "user", "content": "Summarise this." }],
  "model": null, "system": null, "temperature": null,
  "max_tokens": null, "stream": true
}
```

Everything but `messages` falls back to `[ai]`. Roles are `system`, `user` and
`assistant`; a `system` message may sit in the list for every provider, and is
lifted onto its own field on the way to the ones that want it there.

The system prompt is taken from the most specific place that named one: a
`system` message in the conversation, then the request's `system` field, then
`[ai] system`.

### Streamed (the default)

The response is `text/event-stream`:

```
event: delta
data: {"text":"Hel"}

event: reasoning
data: {"text":"The question asks…"}

event: error
data: {"error":"the provider stopped answering"}

event: done
data: {"finish_reason":"stop","input_tokens":42,"output_tokens":96}
```

| event | means |
|-------|-------|
| `delta` | more of the answer; append it |
| `reasoning` | more of the model's reasoning, where the provider streams it separately from the answer. It is not part of the reply, and ignoring this event entirely is a valid way to consume the stream |
| `error` | generation stopped early. Always followed by `done` |
| `done` | the stream is complete. Sent exactly once in all cases |

Every payload is JSON, so a client parses each frame the same way rather than
branching on the event name first, and a newline inside the text cannot break
the framing.

Streaming is the default because completions are slow enough for latency to be
visible: the first token arrives far sooner than the last, and buffering the
whole answer discards that advantage.

### Whole (`"stream": false`)

```json
{ "text": "…", "provider": "openai", "model": "gpt-4o-mini",
  "finish_reason": "stop", "input_tokens": 42, "output_tokens": 96 }
```

This is the appropriate shape for non-interactive callers that need the
complete answer, and the one where failures are reported as ordinary status
codes.

### Failures

| Status | When |
|--------|------|
| `400` | the conversation is unusable, for example no messages |
| `401` / `403` / `404` | `[ai] access` said no |
| `502` | the provider refused it, or could not be reached. The body carries the provider's own status |

Once the stream has started the status code has already been sent, so any
failure after the first byte is reported as an `error` event.

## `GET <base>/ai/config`

```json
{
  "provider": "custom",
  "model": "local",
  "access": "authenticated",
  "streaming": true,
  "agents": [{ "name": "coach", "scope": "global", "storage": true }]
}
```

This endpoint is public and intentionally minimal: enough for a front end to
choose between a chat box and a sign-in prompt, and to discover the configured
agents. It never includes the API key. Unlike a payment provider's publishable
key, an AI key is a spending credential with no browser-safe equivalent.

## Configured agents

An `agents/<name>.toml` file declares one named chat surface under the app's
single `[ai]` provider:

```toml
[agent]
name = "coach"
description = "A persisted assistant for note-based conversations."
system = "Answer only from the caller's notes and the conversation."
storage.enabled = true

[permissions]
chat = "authenticated"
history = "owner"
```

An agent may also override the app's global `[ai]` provider settings for
itself:

```toml
[ai]
provider = "custom"
endpoint = "http://localhost:8080"
model = "local"
api_key = ""
timeout_secs = 60
reasoning = true
```

`reasoning` decides whether the model's thinking is surfaced for this agent:
the chat stream emits `reasoning` events, the thinking is kept on the stored
message, and the admin dashboard offers a **Show reasoning** toggle under each
answer that expands it in a `<pre>` above the message body. Left off, reasoning
is dropped before it reaches any caller.

Omit the key and the agent inherits the app-wide `[ai] reasoning` (itself off by
default). Setting it explicitly overrides that in either direction:
`reasoning = false` on an agent disables it even when the app enables it.

Any key left out falls back to the app's own `[ai]` section. The agent's
`[agent] system` prompt is the more specific setting and takes precedence over
an `[ai] system` fallback when both are set.

`POST <base>/ai/agents/<name>/chat` takes one new turn:

```json
{
  "message": "What did I buy last week?",
  "thread_id": null,
  "messages": [],
  "title": null,
  "stream": true
}
```

Rules:

* `message` is the next user turn.
* `messages` is only for a non-stored agent; it supplies the earlier turns.
* `thread_id` is only for a stored agent; omit it to start a new conversation.
* the configured `system` prompt always applies; caller-supplied `system`
  messages are refused.

The reply streams with the same `delta` / `reasoning` / `error` / `done` event
types as `/ai/chat`. With `"stream": false` it answers as one JSON object like
`/ai/chat`, plus `thread_id` when the conversation is stored.

### Stored history

`storage.enabled = true` creates two generated resources:

* `ai_<name>_thread`
* `ai_<name>_message`

They are migrated like any other resource and are read-only over CRUD:
`history` decides who may list and read them, while writes stay `private`
because the chat route is the only writer. A thread is always continued by its
owner; `history` governs browsing, not who may append a turn.

A stored thread also carries a hidden rolling summary. Once the unsummarised
tail of a conversation crosses `summary_after_characters`, the oldest turns are
replaced by a summary the model writes for itself, and only the recent tail is
replayed:

```toml
[agent.storage]
enabled = true
summary_after_characters = 12000   # the default
```

The summary itself is budgeted at **at most half** that number of characters
(clamped to 200–2000); a shorter transcript gets a proportionally smaller
budget. This is the only setting involved; there is no token-based threshold. A
summary never splits a tool call from its result, so the retained tail remains a
conversation every provider will accept.

`scope = "global"` (the default) keeps threads per-user across the whole app.
`scope = "organization"` stamps `organization_id` too, so using the agent
requires an active organisation exactly the way an org-scoped resource does.

---

## From a function

```rust
// The whole answer.
let reply = ctx.chat(Chat::ask("Summarise this").system("Be terse"))?;

// Just the words.
let answer = ctx.ask("What is six times seven?")?;

// The whole answer, and every token also sent to this function's caller.
let reply = ctx.chat_streaming(Chat::ask(question))?;
```

```ts
import { ai } from "apiplant";

const { text } = ai.chat("Summarise this");
const answer  = ai.ask("What is six times seven?");
const reply   = ai.chatStreaming({ messages });
```

A function that only forwards a prompt adds nothing, since `/ai/chat` already
does that. A function is worthwhile when it does work that does not belong in a
browser: fetching context from the database, fixing the instructions rather than
letting the caller supply them, checking a quota, or recording the exchange.

`chat_streaming` preserves streaming through that wrapper. Without it, wrapping
a streaming model in a function would turn it into a blocking endpoint. When
that function is exposed as an admin action, the dashboard consumes its
`/stream` endpoint, so every token handed to `emit` shows up live there while
the handler's return value still arrives as the final result.

Errors (no provider configured, an empty conversation, a provider refusal) are
returned as `Err`; whether they should fail the request is the function's
decision.

---

## Streaming a function

Every function has a second endpoint:

```
POST <base>/functions/<name>          → the return value, as JSON
POST <base>/functions/<name>/stream   → what it emits, as it emits it
```

The same handler answers both. It streams by calling `emit`:

```rust
for paragraph in report {
    if !ctx.emit(&paragraph) {
        break;              // no reader left; stop working.
    }
}
```

* On a non-streaming invocation the chunk is discarded, so one handler can serve
  as a streaming endpoint, a plain endpoint and a lifecycle hook without
  distinguishing between them.
* `emit` reports whether to **continue**, not whether the chunk was delivered.
  `false` means a streaming caller closed the connection and the remaining work
  has no recipient. A plain invocation always returns `true`, because its caller
  is still waiting for the return value.

The stream uses the same three events as `/ai/chat`, and the function's return
value arrives as the `done` event's `result`:

```
event: delta
data: {"text":"…"}

event: done
data: {"result":{"lines":3}}
```

Access, method and visibility are the function's own: `/stream` applies the same
rules and grants no additional access. A function nobody may call returns `404`
on both endpoints.

---

## What the framework does not do

Nothing calls the assistant on your behalf. No CRUD operation summarises a row,
no hook writes a description, nothing is embedded, indexed or retrieved
automatically. The assistant is a service a function can call and an endpoint a
client can call, exactly like `[email]` and `[cache]`.

This is intentional: a framework that generated text on its own initiative would
be spending your provider credit on an assumption.

---

## See also

* [`examples/19-ai`](../examples/19-ai): a local model, the chat endpoint, and
  a function in front of it.
* [`examples/20-streaming`](../examples/20-streaming): streaming on its own,
  with no model involved.
* [Functions](functions.md): writing the function that does the wrapping.
* [Configuration](configuration.md): `main.toml` in full.
