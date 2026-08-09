# 20 · Streaming functions

Every function gets a second endpoint. `POST /api/functions/<name>` waits for
the return value; `POST /api/functions/<name>/stream` sends whatever the
function produces along the way, as it is produced.

```
20-streaming/
├── main.toml                  # no [ai] section — streaming is not an AI feature
└── functions/
    └── rehearse.rs            # emits a line, waits two seconds, emits the next
```

## Run it

```bash
createdb -h 127.0.0.1 -p 5432 -U postgres apiplant_streaming

cargo run -p apiplant -- build examples/20-streaming
cargo run -p apiplant -- run examples/20-streaming
```

## 1. The same function, twice

```bash
API=http://localhost:8103/api

curl -N -X POST $API/functions/rehearse/stream -d '{}'
```

```
event: delta
data: {"text":"First, the thing that is quick to know.\n"}

    …two seconds…

event: delta
data: {"text":"Then the part that took a moment to work out.\n"}

    …two seconds…

event: delta
data: {"text":"And finally the bit that needed everything above it.\n"}

event: done
data: {"result":{"lines":3,"abandoned":0,"elapsed_ms":4000}}
```

The pauses are real; watch them arrive. Now the plain endpoint:

```bash
time curl -X POST $API/functions/rehearse -d '{}'
# {"lines":3,"abandoned":0,"elapsed_ms":4000}
# real  0m4.0s
```

Four seconds of nothing, then the same document. Same handler, same
`functions/librehearse.so`, no branch in the code.

## 2. What the handler does

```rust
for line in lines {
    sleep(pause);                   // the work
    if !ctx.emit(&format!("{line}\n")) {
        break;                      // nobody is reading; stop.
    }
}
```

`ctx.emit` sends a chunk to whoever is listening *now*, long before the
function knows what it will eventually return. Two things make it something a
handler can call unconditionally:

* On an invocation nobody is streaming, the chunk is dropped and the call is
  free. The same function is a streaming endpoint, a plain endpoint and a
  lifecycle hook, and it never asks which.
* Its answer is **"keep going?"**, not "did that arrive?". It is `false` only
  once a streaming caller has closed the connection — the one case where
  continuing is work for nobody. On a plain call it is `true`, because that
  caller *is* still waiting, for the return value.

Press `Ctrl-C` a second into the stream and the log says so, one line later:

```
INFO apiplant::function: nobody listening after 1 lines
```

## 3. The event format

Three types, and every payload is JSON — the same three
`/api/ai/chat` uses, so one client handles both:

| event | means |
|-------|-------|
| `delta` | a chunk the function emitted; `{"text": "…"}` |
| `error` | it failed partway. Always followed by `done` |
| `done` | the end. `{"result": …}` is what the function returned |

The status code is decided before the function starts, so a failure halfway
through is an `error` event rather than a `500` — there is no status code left
to spend once three paragraphs have already been sent. A client reads `done`
and stops; it is sent exactly once, whatever happened.

## 4. Why this exists

Because plenty of endpoints are slow for a reason and produce their answer in
pieces: a report being assembled, a batch being processed, a third party being
polled, a model generating tokens. Buffering all of it turns a responsive
interface into a spinner, and the alternatives — polling a job id, opening a
WebSocket — are a lot of machinery for "send it as you get it".

[Example 19](../19-ai) is this same mechanism with a model behind it: a
function that reads the caller's rows, prompts an assistant, and streams the
answer through with `ctx.chat_streaming`.
