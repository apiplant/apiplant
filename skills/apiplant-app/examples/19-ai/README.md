# 19 · AI (one provider, streamed)

An app names an assistant, and gets a streaming chat endpoint plus a `chat`
call every function can make. Swapping a local model for OpenAI or Anthropic is
three lines of `main.toml`; nothing in `resources/` or `functions/` changes.

```
19-ai/
├── main.toml                  # [ai] provider, endpoint, model, system prompt
├── agents/
│   └── coach.toml             # one configured agent, with stored history
├── resources/
│   └── note.toml              # owner-scoped rows the assistant answers about
└── functions/
    ├── ask.rs                 # a function in front of the model — and still streaming
    ├── ask.toml               # …its persona and how much context to include
    ├── coach.rs               # a persisted conversation over the generated agent tables
    └── coach.toml             # …its persona and how much note context to include
```

## Run it

It ships pointed at a model on `localhost:8080` — llama.cpp, vLLM, LM Studio
and most gateways all serve the OpenAI chat-completions shape, and Ollama does
too on `:11434`. No key, no account.

```bash
createdb -h 127.0.0.1 -p 5432 -U postgres apiplant_ai

cargo run -p apiplant -- build examples/19-ai
cargo run -p apiplant -- run examples/19-ai
```

```
INFO apiplant_server:   ai -> custom (local at http://localhost:8080/v1/chat/completions)
INFO apiplant_server:   fn ask -> /api/functions/ask
```

Pointing it somewhere else is `AI_ENDPOINT=http://localhost:11434 …`, or the
commented `[ai]` block for OpenAI or Anthropic at the bottom of `main.toml`.

## 1. What the app will admit to

```bash
API=http://localhost:8102/api
curl -s $API/ai/config
# { "provider": "custom", "model": "local", "access": "authenticated", "streaming": true, "agents": [...] }
```

Public, and deliberately thin: enough for a front end to decide between
rendering a chat box and rendering a sign-in prompt. The API key is not in it
and never will be — unlike a payment provider's publishable key, an AI key is a
spending credential with no browser-safe half.

## 2. Ask it something

`POST /api/ai/chat` takes a conversation and answers as server-sent events.

```bash
TOKEN=$(curl -s -X POST $API/auth/register \
  -d '{"email":"ann@example.test","password":"hunter2"}' | jq -r .token)

curl -N -X POST $API/ai/chat \
  -H "Authorization: Bearer $TOKEN" \
  -d '{"messages":[{"role":"user","content":"Name three colours, comma separated."}]}'
```

```
event: delta
data: {"text":"Red"}

event: delta
data: {"text":", blue"}

event: done
data: {"finish_reason":"stop","input_tokens":42,"output_tokens":9}
```

Three event types, and every payload is JSON so a client parses each frame the
same way:

| event | means |
|-------|-------|
| `delta` | more of the answer — append it |
| `reasoning` | more of the model's *thinking*. Never part of the answer; ignoring it is correct |
| `error` | it stopped early, and why. Always followed by `done` |
| `done` | nothing more is coming |

The split between `delta` and `reasoning` is made once, on the server, whatever
shape the provider used. A hosted model separates the two itself. A local
Qwen/DeepSeek on llama.cpp with no reasoning parser does not: its chat template
opens `<think>` in the *prompt*, so the reply arrives as
`…the thinking…</think>the answer` with no opening tag in sight, and anything
reading it as a matched pair shows the thinking as the message. `[ai]
reasoning_format` names which of the three shapes your server uses —
`reasoning_format = "implicit"` for that one. See `docs/ai.md`.

In a browser that is four lines and no library:

```js
const events = new EventSource(url);           // or fetch() + a ReadableStream for POST
events.addEventListener("delta", (e) => output.append(JSON.parse(e.data).text));
events.addEventListener("done", () => events.close());
```

A caller with nothing to do with half an answer asks for the whole thing:

```bash
curl -X POST $API/ai/chat -H "Authorization: Bearer $TOKEN" \
  -d '{"messages":[{"role":"user","content":"Name three colours."}],"stream":false}'
# { "text": "Red, blue, green.", "provider": "custom", "model": "local", "finish_reason": "stop" }
```

## 3. Who may ask

`[ai] access` takes the same words a resource's `[permissions]` does —
`public`, `authenticated` (the default), `member`, `role:analyst`. It is
`authenticated` here for the reason it is the default: the endpoint spends a
GPU, or money, on behalf of whoever calls it, and a public one is an open proxy
to your provider account.

```bash
curl -X POST $API/ai/chat -d '{"messages":[{"role":"user","content":"hi"}]}'
# {"error":"authentication required"}
```

## 4. A function in front of the model

`/api/ai/chat` is the model, unmediated. Most apps want something in front of
it: context the caller could not supply, instructions the caller does not get
to write, and a record of what was asked. That is `functions/ask.rs` — and it
still streams, because `ctx.chat_streaming` hands every token to the caller on
its way through.

```bash
curl -X POST $API/note -H "Authorization: Bearer $TOKEN" \
  -d '{"body":"Bought a red bicycle on Tuesday for 300 euro."}'

curl -N -X POST $API/functions/ask/stream \
  -H "Authorization: Bearer $TOKEN" \
  -d '{"question":"What did I buy and when?"}'
```

```
event: delta
data: {"text":"You bought"}
…
event: done
data: {"result":{"answer":"You bought a red bicycle on Tuesday.","model":"local","context_notes":1}}
```

The notes go into the prompt filtered by the caller's own id — the resource is
`owner`-scoped, so there is no way to ask about somebody else's rows — and the
persona lives in `functions/ask.toml`, which makes changing the assistant's
instructions a config edit rather than a rebuild.

The same function, called at `/api/functions/ask` without the `/stream`, waits
and answers with that `result` object as ordinary JSON. One handler, two
endpoints, no flag in the code: see [example 20](../20-streaming), which is the
streaming half on its own, with no model involved.

## 5. A configured agent with persisted history

`agents/coach.toml` declares a named agent under the app's `[ai]` provider:

```bash
curl -N -X POST $API/ai/agents/coach/chat \
  -H "Authorization: ******" \
  -d '{"message":"What did I buy last week?"}'
```

The reply streams exactly like `/ai/chat`, and a stored agent also returns a
`thread_id` so the next turn can continue it:

```bash
curl -X POST $API/functions/coach \
  -H "Authorization: ******" \
  -d '{"message":"What did I buy last week?"}'
# { "thread_id": "...", "answer": "You bought a red bicycle on Tuesday.", ... }

curl -N -X POST $API/functions/coach/stream \
  -H "Authorization: ******" \
  -d '{"thread_id":"...","message":"And how much was it?"}'
```

That function stores both sides of the conversation in the generated
`ai_coach_thread` and `ai_coach_message` resources, then streams the assistant
reply on the way back out. So the example shows both layers:

* the generic configured-agent route at `/ai/agents/coach/chat`
* a function using the generated storage tables for a custom workflow

## 6. Swapping the provider

```toml
[ai]
provider = "anthropic"
model    = "claude-sonnet-4-5"
api_key  = "${ANTHROPIC_API_KEY}"
```

That is the whole change. Behind it: a different URL, a different auth header,
a system prompt that moves out of the message list onto its own field, and a
completely different event format on the wire. In front of it: the same
`/ai/chat`, the same `ctx.chat`, the same three event types.

## What to read next

* [Sending email](../../docs/email.md) — the same shape, one section and eight
  providers, for the other thing every app eventually needs.
* [`examples/20-streaming`](../20-streaming) — streaming without a model.
