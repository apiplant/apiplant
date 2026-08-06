# 17 · Functions in TypeScript

The same two endpoints as [example 09](../09-c-functions), written in TypeScript,
now with one lifecycle hook and one AI example in the same file. Everything
around them is identical — same manifest, same config file, same generated docs
— but this is the one language that does not compile to a shared library, so it
is worth knowing what actually happens.

```
17-typescript-functions/
├── main.toml
├── models/
│   └── note.toml       # something for the function to count, plus one hook
└── functions/
    ├── hello.ts        # endpoints, one hook and one AI example
    ├── hello.toml      #   …config for `hello`
    ├── apiplant.d.ts   # ← written by `apiplant build`: the types for `ctx`
    └── hello.js        # ← written by `apiplant build`: what the server runs
```

| Endpoint | Visibility | What it shows |
|---|---|---|
| `POST /api/functions/hello` | public | config, logging, JSON in and out, `BadRequest` |
| `GET /api/functions/notes` | authenticated | querying Postgres, reading the caller id |
| hook `note.before_create` | private | `hook()`, typed hook data, rewriting the body before CRUD runs |
| `POST /api/functions/draftReply` | authenticated | AI request typing, one tool definition, reading `reply.tool_calls` |
| `POST /api/functions/exchange` | authenticated | `fetch`, `URL`, `Headers`, checking `response.ok`, the egress allowlist |
| `POST /api/functions/receipt` | authenticated | `Intl` — currency, dates and lists that actually differ by locale |
| `POST /api/functions/digest` | public | `TextEncoder`, `CompressionStream`, `btoa` |

The last three live in `web.ts` and are about the **Web platform globals**: a
function runs in a V8 isolate with a fixed set of them, listed in
`apiplant.d.ts`. Nothing is imported — if it typechecks, it is there at runtime.

## Running it

No toolchain. `apiplant build` transpiles TypeScript itself, so unlike the C, Zig
and Go examples there is nothing to install first — no node, no deno, no bun.

```bash
createdb -h 127.0.0.1 -p 5432 -U postgres apiplant_typescript_functions

cargo run -p apiplant -- build examples/17-typescript-functions   # *.ts → *.js
cargo run -p apiplant -- run examples/17-typescript-functions
```

`exchange` calls a live currency API, so to run *that* one, allow the host it
talks to. Without `APIPLANT_FETCH_ALLOW` set, `fetch` can reach anything; with
it set, everything else fails:

```bash
APIPLANT_FETCH_ALLOW=api.frankfurter.dev \
  cargo run -p apiplant -- run examples/17-typescript-functions
```

```bash
curl -X POST localhost:8099/api/functions/hello \
  -H 'content-type: application/json' -d '{"name":"Federico"}'
# {"message":"Ciao, Federico!","runtime":"v8"}

curl -X POST localhost:8099/api/functions/hello \
  -H 'content-type: application/json' -d '{}'
# 400 {"error":"`name` is required and must be a string"}

TOKEN=$(curl -sX POST localhost:8099/api/auth/register \
  -H 'content-type: application/json' \
  -d '{"email":"ts@example.com","password":"hunter2hunter2"}' | jq -r .token)

curl -X POST localhost:8099/api/note \
  -H 'content-type: application/json' -d '{"title":"alpha"}'

curl localhost:8099/api/functions/notes -H "Authorization: ******"
# {"notes":1,"caller":"51fa5c2d-…"}

curl -X POST localhost:8099/api/note \
  -H 'content-type: application/json' -d '{"title":"   trimmed by the hook   "}'
# {"title":"trimmed by the hook",...}
```

The Web platform ones, from `web.ts`:

```bash
# fetch: calls the rates API, stores the result, reports where it came from.
curl -X POST localhost:8099/api/functions/exchange \
  -H "Authorization: Bearer $TOKEN" \
  -H 'content-type: application/json' -d '{"base":"EUR","quote":"USD"}'
# {"rate":1.1554,"fetchedFrom":"https://api.frankfurter.dev/v1/latest?base=EUR&symbols=USD"}

# Intl: the same amount and instant, in two locales.
curl -X POST localhost:8099/api/functions/receipt \
  -H "Authorization: Bearer $TOKEN" -H 'content-type: application/json' \
  -d '{"amount":1234.5,"currency":"EUR","locale":"de-DE","timeZone":"Europe/Rome"}'
# {"total":"1.234,50 €","issued":"6. August 2026 um 15:48",
#  "summary":"1.234,50 €, 6. August 2026 um 15:48 und Europe/Rome"}

curl -X POST localhost:8099/api/functions/receipt \
  -H "Authorization: Bearer $TOKEN" -H 'content-type: application/json' \
  -d '{"amount":1234,"currency":"JPY","locale":"ja-JP","timeZone":"Asia/Tokyo"}'
# {"total":"￥1,234","issued":"2026年8月6日 22:48",
#  "summary":"￥1,234、2026年8月6日 22:48、Asia/Tokyo"}

# Encoding: "héllo world" is 11 characters but 12 bytes.
curl -X POST localhost:8099/api/functions/digest \
  -H 'content-type: application/json' -d '{"text":"héllo world"}'
# {"characters":11,"bytes":12,"gzipped":38,"base64":"aMOpbGxvIHdvcmxk"}
```

Note the German `und` and the Japanese `、` in `summary`: `Intl.ListFormat`
knows each locale's conventions, which is the sort of thing a hand-rolled
`join(", ")` gets wrong everywhere but English.

A `fetch` to a host outside `APIPLANT_FETCH_ALLOW` fails with a `TypeError`, and
so does a **redirect** to one — the older `frankfurter.app` host redirects to
`api.frankfurter.dev`, and an allowlist that only checked the first URL would
have followed it anywhere the upstream chose.

To try the AI function as well, uncomment the `[ai]` block in `main.toml`, point
it at a model, and then:

```bash
curl -X POST localhost:8099/api/functions/draftReply \
  -H "Authorization: ******" \
  -H 'content-type: application/json' \
  -d '{"question":"Draft a short reply to a customer asking for an update."}'
```

## What runs, and where

Two stages, split at build time:

| | |
|---|---|
| `apiplant build` | parses `hello.ts`, drops the type annotations, writes `hello.js` |
| `apiplant run` | loads `hello.js` into a pool of V8 isolates and calls into it |

The server never sees TypeScript, so a syntax error is a build failure rather
than something you discover at boot. What it *doesn't* do is type-check — the
same choice `deno run --no-check` and `bun` make. Checking happens in your
editor, against the `apiplant.d.ts` that `apiplant build` writes into
`functions/`, and in CI if you want it:

```bash
npx tsc --noEmit --strict examples/17-typescript-functions/functions/hello.ts
```

## Hooks and AI tool definitions

`models/note.toml` points `before_create` at `noteBeforeCreate`, so the same
TypeScript file is both HTTP endpoints and a lifecycle hook. The hook reads
`hook()` rather than a request body and returns `{ data: ... }`, which becomes
the row CRUD writes.

`draftReply` is optional rather than core to the example: it compiles, it
demonstrates the `ai.chat({ tools: [...] })` shape, and it reads
`reply.tool_calls`, but it only works once `[ai]` is configured. `main.toml`
ships a commented local-model block to make that easy to turn on.

## The parts that differ from C

**`ctx` instead of callbacks.** `ctx.query`, `ctx.config()`, `ctx.principalId()`,
`ctx.payments()`, `ctx.chat()` and `ctx.log` are the same host calls
`apiplant.h` declares, and they are **synchronous** for the same reason they are
in C: the host does the work on another thread while the isolate waits. A
function only writes `async` when it wants to for its own reasons.

**Errors instead of status codes.** `throw new BadRequest("…")` is the C ABI's
`APIPLANT_ERR_REQUEST` — a 400 with your message. Every other throw is a 500, and
the message goes to the log rather than to the caller. A failed host call
(`ctx.query` against a bad table) arrives as an ordinary thrown `Error`, so
ignoring it can't turn a failure into data.

**One file, no imports.** Nothing bundles, so a function is a single
self-contained module and `import` is refused at build time. Reach the outside
world through `ctx`, not through packages.

## What to know before you ship one

Isolates share nothing. Module-level state — a cache, a counter, a connection —
exists once per worker in the pool, not once per app, and disappears whenever the
process restarts. Use the database or the `[cache]` from
[example 16](../16-caching) for anything that has to be shared or survive.

Two knobs, both environment variables:

| | |
|---|---|
| `APIPLANT_JS_WORKERS` | isolates per module (default 2) — how many calls run at once |
| `APIPLANT_JS_TIMEOUT_MS` | how long one call may run (default 30000) before its isolate is terminated |

That timeout is not optional politeness: JavaScript is not preemptible, so
`while (true) {}` would hold a worker forever. When it fires, that one request
fails with a 500 and the worker carries on.

## Next

[Example 08](../08-hooks) runs functions around CRUD operations; a TypeScript
function works there too, reading `ctx.hook()` for the record and event. See
[example 12](../12-function-dependencies) for the directory form of a function —
which, for now, is exactly what TypeScript does *not* support.
