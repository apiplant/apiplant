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

// Timers are not globals in deno_core, they are `Deno.core` calls. A function
// that debounces or polls expects the standard four, and the event loop already
// knows how to wait for them, so they are exposed under their usual names.
//
// Refed on purpose: an invocation is not finished until its timers have run,
// which is what makes `await new Promise(r => setTimeout(r, 10))` behave.
globalThis.setTimeout = (callback, delay = 0, ...args) =>
  Deno.core.createSystemTimer(() => callback(...args), delay, true);
globalThis.setInterval = (callback, delay = 0, ...args) =>
  Deno.core.createSystemInterval(() => callback(...args), delay, true);
globalThis.clearTimeout = (id) => {
  if (id !== undefined) Deno.core.cancelTimer(id);
};
globalThis.clearInterval = globalThis.clearTimeout;

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
