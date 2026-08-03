# `apiplant` — the module TypeScript functions import

The one module an apiplant function can import. It is what a `.ts` file in an
app's `functions/` directory reaches for:

```ts
import { defineFunctions, db, s, log } from "apiplant";

const NewNote = s.object({
  title: s.string({ minLength: 1 }),
  tags: s.optional(s.array(s.string())),
});

export default defineFunctions({
  createNote: {
    permission: "authenticated",
    description: "Files a note.",
    input: NewNote,
    handler(input) {
      log.info(`filing ${input.title}`);
      const row = db.one(sql`INSERT INTO apiplant_note (title) VALUES (${input.title}) RETURNING id`);
      return { id: row.id };
    },
  },
});
```

**You do not install this.** The module is compiled into the apiplant binary and
served to the V8 isolate that runs your function, and `apiplant build` copies
`apiplant.d.ts` and a `tsconfig.json` into your `functions/` directory so an
editor types all of it. There is no `node_modules`, no `package.json` in your
app, and nothing to keep in step with the server version.

The package here exists so the module has one home: the runtime the isolate
executes, the declarations an editor reads, and the tests that check they agree.

```
typescript/
├── apiplant.js     the runtime, embedded into `apiplant-js` at compile time
├── apiplant.d.ts   the types, copied into functions/ by `apiplant build`
└── test/types.ts   what `pnpm check` compiles: the promises, exercised
```

## What it removes

Without it a function declares a `manifest` array, exports a matching function
per entry, casts every query result, and opens each handler by checking the body
by hand. With it:

| | |
|---|---|
| `defineFunctions` | a name is written once, beside the handler it names |
| `s.object({...})` | one declaration becomes the JSON Schema in the docs, the 400 for a bad body, *and* the type of `input` |
| `db.query` / `one` / `first` / `value` / `execute` | typed rows, and the SELECT/`rows_affected` split handled |
| `` sql`…${value}` `` | placeholders numbered and values bound, never interpolated |
| `cache` | `get`/`set`/`has`/`increment`/`ttl`/`remember` instead of `{op, key}` objects |
| `email.send` | the host's `Message`, typed |
| `payments` | checkout, portal, subscription, cancellation and raw provider requests |
| `ai.chat` / `ask` / `chatStreaming` | whole replies, plain-text shorthand, streaming, and typed `tool_calls` / `tools` |
| `config`, `principalId`, `hook`, `log` | the rest of the host, as functions rather than as a threaded argument |
| `BadRequest`, `HttpError` | the 400/500 split, as exceptions |

Everything is **synchronous**: the isolate blocks while the host does the work on
another thread. `async` appears in a handler only when it wants it.

## Working on it

```bash
pnpm install
pnpm check        # tsc over the declarations and their test
```

`test/types.ts` is a compile-time test: it asserts that a handler's `input` is
inferred from its schema (and is not silently `any`), that required fields are
required, and that each host call takes what the Rust side actually parses. It
runs nothing; `tsc` failing is the failure.

The Rust side has the other half of the tests — `crates/apiplant-js/tests/` loads
this module into a real isolate against a stub host and asserts on the wire
format between them.

## Publishing

The package is publishable but not published: an app never installs it, so npm
would only serve editors that would rather resolve `apiplant` from
`node_modules` than from the generated `apiplant.d.ts`. If that changes, `files`
and `exports` are already set for it.

See [`docs/functions.md`](../docs/functions.md#writing-a-function-in-typescript)
for the whole story, and
[`examples/17-typescript-functions`](../examples/17-typescript-functions) for a
working app.
