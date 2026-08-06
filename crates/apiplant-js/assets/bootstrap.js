// The half of the JavaScript function runtime that is itself JavaScript.
//
// Loaded into every isolate before the app's module, this defines the `ctx` a
// function receives and the entry point the host calls. Everything here talks to
// Rust through one op, `op_apiplant_host(kind, payload) -> string`, which blocks
// the isolate until the host thread answers -- the same synchronous shape a C
// function's `host.query` has, so a function author never deals with two kinds
// of asynchrony at once.

const ops = Deno.core.ops;

/// Ask the host for something. Payload and reply are both JSON text.
function host(kind, payload) {
  return ops.op_apiplant_host(kind, JSON.stringify(payload ?? null));
}

/// Host replies use the in-band convention the C ABI uses: an object with an
/// `error` key is a failure, anything else is the answer.
function hostJson(kind, payload) {
  const reply = JSON.parse(host(kind, payload));
  if (reply && typeof reply === "object" && !Array.isArray(reply) && "error" in reply) {
    throw new Error(reply.error);
  }
  return reply;
}

/// Thrown to fail a request with a 400 rather than a 500 -- the JavaScript
/// spelling of the C ABI's `APIPLANT_ERR_REQUEST`.
class BadRequest extends Error {
  constructor(message) {
    super(message);
    this.name = "BadRequest";
    this.request = true;
  }
}

/// What every function gets as its second argument.
const ctx = Object.freeze({
  /// Run SQL against the app's database. A SELECT returns an array of row
  /// objects; anything else returns the affected-row count the host reports.
  query(sql, params = []) {
    return hostJson("query", { sql, params });
  },

  /// This function's `functions/<name>.toml`, as an object.
  config() {
    return JSON.parse(host("config", null) || "{}");
  },

  /// The authenticated caller's id, or "" when the endpoint is public.
  principalId() {
    return host("principal_id", null);
  },

  /// For a lifecycle hook, the record and event being handled; `null` for a
  /// plain HTTP invocation.
  hook() {
    const raw = host("hook", null);
    return raw ? JSON.parse(raw) : null;
  },

  /// Send mail through the app's configured provider.
  sendEmail(message) {
    return hostJson("send_email", message);
  },

  /// Talk to the app's configured cache.
  cache(request) {
    return hostJson("cache", request);
  },

  /// Talk to the app's configured payment provider.
  payments(request) {
    return hostJson("payments", request);
  },

  /// Ask the app's configured AI assistant, and wait for the whole answer.
  /// A string is shorthand for a one-question conversation. Tool definitions
  /// and tool-call messages pass straight through in the object form.
  chat(request) {
    const body = typeof request === "string"
      ? { messages: [{ role: "user", content: request }] }
      : request;
    return hostJson("ai", body);
  },

  /// Queue a message for whatever subscribes to this topic in
  /// [queues.subscribe]. Returns once the message is recorded, not once it has
  /// been handled -- the handler runs after this function does.
  publish(topic, message) {
    return hostJson("publish", { op: "publish", topic, message: message ?? {} });
  },

  /// Push a chunk of the answer to the caller now, before this function
  /// returns. Only goes anywhere when the call came through
  /// `<base>/functions/<name>/stream`; answers whether it is still worth
  /// producing more (false once the caller has hung up).
  emit(chunk) {
    return hostJson("emit", String(chunk)).delivered === true;
  },

  log: Object.freeze({
    trace: (m) => host("log", { level: "trace", message: String(m) }),
    debug: (m) => host("log", { level: "debug", message: String(m) }),
    info: (m) => host("log", { level: "info", message: String(m) }),
    warn: (m) => host("log", { level: "warn", message: String(m) }),
    error: (m) => host("log", { level: "error", message: String(m) }),
  }),

  BadRequest,
});

globalThis.BadRequest = BadRequest;

// What the `apiplant` module is built on. Not API: a function reaches all of
// this through `import { db, cache, ... } from "apiplant"`, or through the `ctx`
// its handler is handed. Living here rather than in the module keeps one
// implementation of the op protocol.
globalThis.__apiplantInternals = Object.freeze({ host, hostJson, ctx, BadRequest });

// `console` is not part of deno_core, and a function that logs to a black hole
// is worse than one that cannot log at all -- so it goes to the host's tracing.
globalThis.console = Object.freeze({
  log: ctx.log.info,
  info: ctx.log.info,
  debug: ctx.log.debug,
  warn: ctx.log.warn,
  error: ctx.log.error,
  trace: ctx.log.trace,
});

// The Web platform globals, from `deno_web`.
//
// `deno_core` on its own is just V8 plus ops: it has no `TextEncoder`, no `URL`,
// not even `setTimeout`. `deno_web` implements those to spec, but it does not
// install them -- each of its files is a module that *returns* its interfaces,
// leaving the choice of what a given runtime exposes to the embedder. That
// choice is the list below, and it is deliberately a list rather than a loop:
// every global a function can see is a global we have agreed to keep working.
//
// `deno_web`'s files are not ES modules and cannot be `import`ed: each is an
// IIFE that returns its interfaces, fetched with `core.loadExtScript`. Their
// sources live in the startup snapshot (see `build.rs`), so none of this
// touches the disk, and a script already loaded is returned rather than re-run.
const ext = (specifier) => Deno.core.loadExtScript(specifier);

// `fetch` is ours rather than `deno_web`'s: it is one op over the same reqwest
// client the rest of the server uses, so a function's outbound request goes
// through the same TLS configuration and the same egress rules.
import { fetch, Headers, Request, Response } from "ext:apiplant_js/fetch.js";

const encoding = ext("ext:deno_web/08_text_encoding.js");
const base64 = ext("ext:deno_web/05_base64.js");
const url = ext("ext:deno_web/00_url.js");
const urlPattern = ext("ext:deno_web/01_urlpattern.js");
const timers = ext("ext:deno_web/02_timers.js");
const perf = ext("ext:deno_web/15_performance.js");
const clone = ext("ext:deno_web/02_structured_clone.js");
const file = ext("ext:deno_web/09_file.js");
const fileReader = ext("ext:deno_web/10_filereader.js");
const streams = ext("ext:deno_web/06_streams.js");
const compression = ext("ext:deno_web/14_compression.js");
const domException = ext("ext:deno_web/01_dom_exception.js");
const abort = ext("ext:deno_web/03_abort_signal.js");
const event = ext("ext:deno_web/02_event.js");

const { TextEncoder, TextDecoder, TextEncoderStream, TextDecoderStream } = encoding;
const { atob, btoa } = base64;
const { URL, URLSearchParams } = url;
const { URLPattern } = urlPattern;
const { setTimeout, clearTimeout, setInterval, clearInterval } = timers;
const { performance } = perf;
const { structuredClone } = clone;
const { Blob, File } = file;
const { FileReader } = fileReader;
const { ReadableStream, WritableStream, TransformStream, ReadableStreamDefaultReader,
  ByteLengthQueuingStrategy, CountQueuingStrategy } = streams;
const { CompressionStream, DecompressionStream } = compression;
const { DOMException } = domException;
const { AbortController, AbortSignal } = abort;
const { Event, EventTarget, CustomEvent } = event;

Object.defineProperties(globalThis, Object.getOwnPropertyDescriptors({
  // Encoding and base64.
  TextEncoder, TextDecoder, TextEncoderStream, TextDecoderStream, atob, btoa,
  // URLs. Note there is no `location`: a function is not a document, so a
  // relative URL has nothing to resolve against and `new URL(path)` throws.
  URL, URLSearchParams, URLPattern,
  // Timers. `deno_web`'s are refed, so an invocation is not finished until its
  // timers have run -- which is what makes `await new Promise(r =>
  // setTimeout(r, 10))` behave, exactly as the hand-rolled pair before it did.
  setTimeout, clearTimeout, setInterval, clearInterval, performance,
  // Structured data.
  structuredClone, Blob, File, FileReader,
  // Streams.
  ReadableStream, WritableStream, TransformStream, ReadableStreamDefaultReader,
  ByteLengthQueuingStrategy, CountQueuingStrategy,
  CompressionStream, DecompressionStream,
  // Events, and the error type the above throw.
  Event, EventTarget, CustomEvent, AbortController, AbortSignal, DOMException,
  // HTTP.
  fetch, Headers, Request, Response,
}));

/// The module namespace, put here by the host once the app's module has been
/// evaluated. Kept off `ctx` because it is host plumbing, not function API.
globalThis.__apiplantModule = null;

/// The module's functions, however it declared them.
///
/// Two spellings, and the difference is only where the names live:
///
///   * `export default defineFunctions({...})` -- the `apiplant` module's form,
///     where each entry carries its own handler, so a name is written once.
///   * `export const manifest = [...]` plus one export per entry -- the form
///     that needs no import, and the one a hand-written module or another
///     language's port uses.
function declared() {
  const module = globalThis.__apiplantModule;
  const bundle = module?.default;
  if (bundle && typeof bundle === "object" && bundle.__apiplant) {
    return { manifest: bundle.manifest, handlers: bundle.handlers };
  }
  return { manifest: module?.manifest, handlers: module };
}

/// The manifest the host reads at boot, normalised to an array.
///
/// A module may export one manifest object or several; both spellings exist
/// because a single-function file reads better without the brackets.
globalThis.__apiplantManifest = () => {
  const { manifest } = declared();
  if (manifest === undefined || manifest === null) return null;
  return JSON.stringify(Array.isArray(manifest) ? manifest : [manifest]);
};

/// The host's one entry point into user code.
///
/// Always resolves -- never rejects -- with `{ok}` or `{error, request}`, so the
/// Rust side has one shape to read and the caller-fault/server-fault split is
/// decided here, where the thrown value is still intact.
globalThis.__apiplantInvoke = async (name, inputJson) => {
  const fn = declared().handlers?.[name];
  if (typeof fn !== "function") {
    return JSON.stringify({
      error: `this module exports no function named \`${name}\``,
      request: false,
    });
  }

  let input;
  try {
    input = inputJson === "" ? null : JSON.parse(inputJson);
  } catch (e) {
    return JSON.stringify({ error: `request body is not valid JSON: ${e}`, request: true });
  }

  try {
    const output = await fn(input, ctx);
    return JSON.stringify({ ok: output === undefined ? null : output });
  } catch (e) {
    // A thrown `BadRequest`, anything else marked `request`, or a 4xx `status`
    // is the caller's fault; every other throw is the function's.
    const request =
      e instanceof BadRequest ||
      e?.request === true ||
      (typeof e?.status === "number" && e.status >= 400 && e.status < 500);
    const error = e instanceof Error ? `${e.message}` : String(e);
    return JSON.stringify({ error: error || "function threw", request });
  }
};
