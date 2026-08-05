// The `apiplant` module, as a TypeScript function imports it.
//
//     import { defineFunctions, db, cache, email, BadRequest } from "apiplant";
//
// This file is the module's implementation. It is embedded in the `apiplant-js`
// crate and served to the isolate under the specifier `apiplant`, so there is
// nothing to install and nothing to resolve on disk -- an app's functions/
// directory stays a directory of source files.
//
// Everything here is built on one primitive: `host(kind, payload)`, the op that
// hands a request to the Rust side and blocks until it answers. The bootstrap
// owns that op; this module owns the shape of what goes through it.
//
// Written as plain JavaScript with a hand-written `apiplant.d.ts` beside it,
// because a package that needs compiling would put a toolchain back in front of
// an app whose whole point is not needing one.

const { host, hostJson, ctx, BadRequest } = globalThis.__apiplantInternals;

// ---- functions -------------------------------------------------------------

/**
 * Declare this module's functions: what each endpoint is, and what runs.
 *
 * The alternative is exporting a `manifest` array and a matching function per
 * entry, which means writing every name twice and keeping two lists in step.
 * Here a name is a key, its endpoint description sits on the handler, and the
 * result is the module's default export:
 *
 *     export default defineFunctions({
 *       greet: {
 *         permission: "public",
 *         input: s.object({ name: s.string() }),
 *         handler(input, ctx) { return { hi: input.name }; },
 *       },
 *     });
 *
 * When `input` is a schema, the request is validated against it before the
 * handler runs and a bad body is a 400 naming the field -- which is the check
 * every handler would otherwise open with.
 */
export function defineFunctions(definitions) {
  const manifest = [];
  const handlers = {};

  for (const [name, definition] of Object.entries(definitions)) {
    const handler =
      typeof definition === "function" ? definition : definition.handler;
    if (typeof handler !== "function") {
      throw new TypeError(`function \`${name}\` has no handler`);
    }

    const { input, output, config, ...entry } = typeof definition === "function"
      ? {}
      : definition;
    delete entry.handler;

    manifest.push({
      name,
      ...entry,
      // A schema built with `s` carries its JSON Schema; a plain object already
      // is one. Either way the docs get the same thing.
      ...(input ? { input_schema: jsonSchema(input) } : {}),
      ...(output ? { output_schema: jsonSchema(output) } : {}),
      ...(config ? { config_schema: jsonSchema(config) } : {}),
    });

    handlers[name] = input && input.__schema
      ? (body, context) => handler(parse(input, body), context)
      : handler;
  }

  return { __apiplant: 1, manifest, handlers };
}

/** The JSON Schema of `s`-built schema, or the object as given. */
function jsonSchema(schema) {
  return schema.__schema ? schema.json : schema;
}

// ---- postgres --------------------------------------------------------------

/**
 * The app's database.
 *
 * Every method is synchronous: the isolate waits while the host runs the query
 * on the thread that owns the connection pool. Values are always bound, never
 * interpolated -- pass them in `params` and write `$1`, `$2`, ... in the SQL, or
 * use the `sql` template, which numbers them for you.
 */
export const db = {
  /** Every row a SELECT returned. */
  query(sql, params = []) {
    const request = typeof sql === "string" ? { sql, params } : sql;
    const rows = hostJson("query", request);
    if (!Array.isArray(rows)) {
      // A non-SELECT went through `query`: `{rows_affected}` is not rows, and
      // silently returning `[]` would hide the mistake until it mattered.
      throw new Error(
        "this statement returned no rows; use `db.execute` for INSERT, UPDATE and DELETE",
      );
    }
    return rows;
  },

  /** The first row, or `null` when the query matched nothing. */
  first(sql, params = []) {
    return db.query(sql, params)[0] ?? null;
  },

  /**
   * Exactly one row.
   *
   * Throws when there is none -- the common case where "no such id" should be a
   * 404-shaped failure rather than a `Cannot read properties of null`.
   */
  one(sql, params = []) {
    const row = db.first(sql, params);
    if (row === null) throw new Error("expected one row, found none");
    return row;
  },

  /** The single column of the single row, e.g. a `count(*)`. */
  value(sql, params = []) {
    const row = db.one(sql, params);
    const columns = Object.values(row);
    if (columns.length !== 1) {
      throw new Error(
        `expected one column, found ${columns.length}: ${Object.keys(row).join(", ")}`,
      );
    }
    return columns[0];
  },

  /** An INSERT, UPDATE or DELETE. Returns how many rows it touched. */
  execute(sql, params = []) {
    const request = typeof sql === "string" ? { sql, params } : sql;
    const result = hostJson("query", request);
    if (Array.isArray(result)) return result.length;
    return result?.rows_affected ?? 0;
  },
};

/**
 * Build a query with its values bound, not interpolated.
 *
 *     db.query(sql`SELECT * FROM apiplant_note WHERE owner = ${id} LIMIT ${10}`)
 *
 * Each `${...}` becomes a `$n` placeholder and its value goes into `params`, so
 * an apostrophe in a name is data and nothing a caller sends can become SQL.
 */
export function sql(strings, ...values) {
  let text = strings[0];
  for (let i = 0; i < values.length; i++) {
    text += `$${i + 1}${strings[i + 1]}`;
  }
  return { sql: text, params: values };
}

// ---- cache -----------------------------------------------------------------

/**
 * The app's Redis, when `[cache]` is configured. Every method throws with
 * "no cache configured" when it isn't, rather than pretending to work.
 *
 * Values are JSON: what you set is what you get back, not a string of it.
 */
export const cache = {
  /** The value, or `null` when the key is absent or expired. */
  get(key) {
    const reply = hostJson("cache", { op: "get", key });
    return reply.hit ? reply.value : null;
  },

  /** Whether the key is there, without fetching it. */
  has(key) {
    return hostJson("cache", { op: "exists", key }).exists === true;
  },

  /** Write a value. `ttlSeconds` defaults to `[cache] default_ttl_secs`; 0 never expires. */
  set(key, value, ttlSeconds) {
    hostJson("cache", {
      op: "set",
      key,
      value: value === undefined ? null : value,
      ...(ttlSeconds === undefined ? {} : { ttl: ttlSeconds }),
    });
  },

  /** Remove a key. `true` when it was there. */
  delete(key) {
    return hostJson("cache", { op: "delete", key }).deleted === true;
  },

  /** Add to a counter (default 1), returning the new value. */
  increment(key, by = 1, ttlSeconds) {
    return hostJson("cache", {
      op: "incr",
      key,
      by,
      ...(ttlSeconds === undefined ? {} : { ttl: ttlSeconds }),
    }).value;
  },

  /** Seconds left before the key expires: `null` when absent, `0` when it never expires. */
  ttl(key) {
    return hostJson("cache", { op: "ttl", key }).ttl ?? null;
  },

  /**
   * Read through the cache: return what's stored, or compute it, store it and
   * return that. The pattern most `cache.get`/`cache.set` pairs are written to
   * be, with the race the pair usually forgets about being harmless here (two
   * callers may both compute; both write the same value).
   */
  remember(key, ttlSeconds, compute) {
    const hit = cache.get(key);
    if (hit !== null) return hit;
    const value = compute();
    cache.set(key, value, ttlSeconds);
    return value;
  },
};

// ---- email -----------------------------------------------------------------

/** The app's mail provider, when `[email]` is configured. */
export const email = {
  /**
   * Send one message. `from` and `reply_to` fall back to `[email]` in
   * main.toml, so most calls are `to`, `subject` and a body.
   *
   * Returns what the provider reported: which one took it, its id, and how many
   * recipients it went to.
   */
  send(message) {
    return hostJson("send_email", message);
  },
};

// ---- payments --------------------------------------------------------------

/** The app's payment provider, when `[payments]` is configured. */
export const payments = {
  /** Start a checkout, and get the provider URL to send the buyer to. */
  checkout(stripePriceId, recurring, organizationId) {
    return ctx.payments({
      op: "checkout",
      stripe_price_id: stripePriceId,
      recurring,
      organization_id: organizationId,
    });
  },

  /** Open the provider's self-service billing screens for one customer. */
  billingPortal(stripeCustomerId) {
    return ctx.payments({ op: "portal", stripe_customer_id: stripeCustomerId });
  },

  /** Ask the provider what a subscription's state actually is. */
  subscription(id) {
    return ctx.payments({ op: "subscription", id });
  },

  /** Cancel a subscription, usually at the end of the paid period. */
  cancelSubscription(id, atPeriodEnd = true) {
    return ctx.payments({ op: "cancel", id, at_period_end: atPeriodEnd });
  },

  /** One raw request to the provider, for anything the helpers do not cover yet. */
  request(request) {
    return ctx.payments(request);
  },
};

// ---- the assistant ---------------------------------------------------------

function normaliseChatReply(reply) {
  return {
    ...reply,
    tool_calls: Array.isArray(reply?.tool_calls) ? reply.tool_calls : [],
  };
}

/** The app's AI assistant, when `[ai]` is configured. */
export const ai = {
  /**
   * Ask, and wait for the whole answer.
   *
   * A string is the short form of a one-question conversation; pass an object
   * to continue one, or to override the model, system prompt or temperature
   * `[ai]` already sets.
   *
   * ```js
   * const { text } = ai.chat("Summarise this in one line: " + body);
   * ```
   */
  chat(request) {
    return normaliseChatReply(ctx.chat(request));
  },

  /** Just the text of the answer, for the common case that wants nothing else. */
  ask(prompt) {
    return ai.chat(prompt).text;
  },

  /**
   * Ask, and pass the answer to your own caller as it is written -- every
   * token is `emit`ted on its way through, and the complete reply still comes
   * back. On a call nobody is streaming this is exactly `chat`.
   */
  chatStreaming(request) {
    const body = typeof request === "string"
      ? { messages: [{ role: "user", content: request }] }
      : request;
    return normaliseChatReply(ctx.chat({ ...body, stream: true }));
  },
};

/**
 * Push a chunk of the response to the caller now, rather than at the end.
 *
 * Only goes anywhere when the function was called through
 * `<base>/functions/<name>/stream`; everywhere else it is a no-op, so the same
 * handler works streamed, plain, or as a hook. The answer is "keep going?":
 * `false` once the caller has hung up, `true` otherwise -- including on a call
 * nobody is streaming, whose caller is still waiting for the return value.
 *
 * ```js
 * for (const line of answer) {
 *   if (!emit(line)) return { stopped: true };
 * }
 * ```
 */
export function emit(chunk) {
  return ctx.emit(chunk);
}

// ---- queues ----------------------------------------------------------------

/**
 * Work that happens after the response.
 *
 * `queue.publish` records a message and returns; whichever functions the app's
 * `[queues.subscribe]` points at that topic run afterwards, on their own, with
 * retries. Nothing the handler does can fail the request that published it --
 * which is the reason to reach for this rather than just calling the function.
 *
 * ```js
 * queue.publish("order.paid", { orderId: order.id });
 * ```
 */
export const queue = {
  /**
   * Queue a message. Returns `{ id, topic, delivered }`, where `delivered` is
   * how many subscribers it was queued for.
   *
   * `delivered: 0` is not an error -- the message is still recorded -- but it
   * means nothing is listening to that topic, which is nearly always a typo.
   */
  publish(topic, message) {
    return ctx.publish(topic, message);
  },
};

/**
 * When running as a queue subscriber, the delivery this call is:
 * `{ topic, messageId, subscriber, attempts, principalId }`. `null` for an HTTP
 * call or a lifecycle hook.
 *
 * The message body itself arrives as the function's ordinary input, so a
 * handler is just a function and can be called by hand to test it.
 *
 * `attempts` is the one to branch on. Delivery is at-least-once, so anything
 * above 1 is a message whose side effects may already have partly happened.
 */
export function delivery() {
  const raw = ctx.hook();
  if (!raw || raw.event !== "message") return null;
  return {
    topic: raw.topic,
    messageId: raw.message_id,
    subscriber: raw.subscriber,
    attempts: raw.attempts,
    principalId: raw.principal_id ?? "",
  };
}

// ---- the request -----------------------------------------------------------

/** This function's `functions/<name>.toml`, as an object. */
export function config() {
  return ctx.config();
}

/** The authenticated caller's id, or `""` when the endpoint is public. */
export function principalId() {
  return ctx.principalId();
}

/**
 * When running as a lifecycle hook, what it is running for: the event, the
 * resource, the caller, and the row or body in `data` / `row` / `rows`.
 * `null` for a plain HTTP invocation.
 */
export function hook() {
  return ctx.hook();
}

/** Write to the server's log. `console.log` and friends land here too. */
export const log = ctx.log;

/**
 * Reject the caller's request with a 400 and this message.
 *
 * Everything else thrown is the function's own fault and becomes a 500 whose
 * message goes to the log instead of to the caller.
 */
export { BadRequest };

/** Reject a hook's request with a status of your choosing. */
export class HttpError extends Error {
  constructor(status, message) {
    super(message);
    this.name = "HttpError";
    this.status = status;
    this.request = status >= 400 && status < 500;
  }
}

// ---- schemas ---------------------------------------------------------------

/**
 * A very small schema builder.
 *
 * It exists because two things want the same description and neither can derive
 * it from the other: the OpenAPI document needs JSON Schema, and the handler
 * needs the request checked before it runs. Declaring the shape once produces
 * both, and (through `Infer`) types the handler's argument as well.
 *
 * Deliberately a subset -- objects, arrays, strings, numbers, booleans, enums,
 * and the constraints that produce good error messages. Anything richer is
 * still JSON Schema: pass a plain object as `input` and it goes to the docs
 * untouched, with validation left to the handler.
 */
function schema(json, check) {
  return { __schema: 1, json, check };
}

export const s = {
  string(options = {}) {
    const { minLength, maxLength, pattern, format, description } = options;
    return schema(
      { type: "string", ...clean({ minLength, maxLength, pattern, format, description }) },
      (value, path) => {
        if (typeof value !== "string") return `${path} must be a string`;
        if (minLength !== undefined && value.length < minLength) {
          return `${path} must be at least ${minLength} characters`;
        }
        if (maxLength !== undefined && value.length > maxLength) {
          return `${path} must be at most ${maxLength} characters`;
        }
        if (pattern !== undefined && !new RegExp(pattern).test(value)) {
          return `${path} must match ${pattern}`;
        }
        return null;
      },
    );
  },

  number(options = {}) {
    return numeric("number", options);
  },

  integer(options = {}) {
    return numeric("integer", options);
  },

  boolean(options = {}) {
    return schema({ type: "boolean", ...clean(options) }, (value, path) =>
      typeof value === "boolean" ? null : `${path} must be true or false`,
    );
  },

  /** One of a fixed set of strings. */
  enum(values, options = {}) {
    return schema({ type: "string", enum: values, ...clean(options) }, (value, path) =>
      values.includes(value)
        ? null
        : `${path} must be one of ${values.map((v) => `"${v}"`).join(", ")}`,
    );
  },

  array(items, options = {}) {
    const { minItems, maxItems, description } = options;
    return schema(
      { type: "array", items: jsonSchema(items), ...clean({ minItems, maxItems, description }) },
      (value, path) => {
        if (!Array.isArray(value)) return `${path} must be an array`;
        if (minItems !== undefined && value.length < minItems) {
          return `${path} must have at least ${minItems} items`;
        }
        if (maxItems !== undefined && value.length > maxItems) {
          return `${path} must have at most ${maxItems} items`;
        }
        for (let i = 0; i < value.length; i++) {
          const failure = validate(items, value[i], `${path}[${i}]`);
          if (failure) return failure;
        }
        return null;
      },
    );
  },

  /**
   * An object with known fields. Every field is required unless wrapped in
   * `s.optional`, which is the way round that catches typos: a body missing a
   * field it needs fails, rather than arriving as `undefined` three lines later.
   */
  object(fields, options = {}) {
    const required = Object.entries(fields)
      .filter(([, field]) => !field.__optional)
      .map(([name]) => name);

    const properties = {};
    for (const [name, field] of Object.entries(fields)) {
      properties[name] = jsonSchema(field);
    }

    return schema(
      {
        type: "object",
        properties,
        ...(required.length ? { required } : {}),
        ...clean(options),
      },
      (value, path) => {
        if (value === null || typeof value !== "object" || Array.isArray(value)) {
          return `${path} must be an object`;
        }
        for (const [name, field] of Object.entries(fields)) {
          const present = value[name] !== undefined && value[name] !== null;
          if (!present) {
            if (field.__optional) continue;
            return `${path === "body" ? "" : `${path}.`}${name} is required`;
          }
          const failure = validate(
            field,
            value[name],
            path === "body" ? name : `${path}.${name}`,
          );
          if (failure) return failure;
        }
        return null;
      },
    );
  },

  /** A field that may be absent. */
  optional(field) {
    return { ...field, __optional: 1 };
  },

  /** Anything at all: present in the docs, unchecked at the door. */
  any(options = {}) {
    return schema({ ...clean(options) }, () => null);
  },
};

function numeric(type, options) {
  const { minimum, maximum, description } = options;
  return schema(
    { type, ...clean({ minimum, maximum, description }) },
    (value, path) => {
      if (typeof value !== "number" || Number.isNaN(value)) {
        return `${path} must be a number`;
      }
      if (type === "integer" && !Number.isInteger(value)) {
        return `${path} must be a whole number`;
      }
      if (minimum !== undefined && value < minimum) return `${path} must be at least ${minimum}`;
      if (maximum !== undefined && value > maximum) return `${path} must be at most ${maximum}`;
      return null;
    },
  );
}

/** Drop the keys that weren't set, so the JSON Schema stays readable. */
function clean(options) {
  const out = {};
  for (const [key, value] of Object.entries(options)) {
    if (value !== undefined) out[key] = value;
  }
  return out;
}

function validate(field, value, path) {
  return field.check ? field.check(value, path) : null;
}

/**
 * Check a body against a schema, returning it, or throw the 400 that says which
 * field was wrong.
 */
export function parse(schemaOrJson, body) {
  if (!schemaOrJson.__schema) return body;
  const failure = schemaOrJson.check(body, "body");
  if (failure) throw new BadRequest(failure);
  return body;
}
