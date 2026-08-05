// Types for apiplant functions written in TypeScript.
//
// `apiplant build` copies this file into your app's functions/ directory, so an
// editor types everything with no package to install and no node_modules to
// keep. It declares the `apiplant` module the same way the runtime provides it,
// so `import { db } from "apiplant"` resolves with no tsconfig `paths` entry.
//
// Regenerated on every build; edit your functions, not this file.

declare module "apiplant" {
  // ---- declaring functions -------------------------------------------------

  /** Who may call a function's endpoint -- the same grammar a resource's
   *  `[permissions]` uses. Absent means `"private"`: unreachable over HTTP. */
  export type Permission =
    | "public"
    | "authenticated"
    | "member"
    | "none"
    | "private"
    | `role:${string}`;

  /** How the dashboard presents a function. */
  export interface AdminPresentation {
    visible?: boolean;
    roles?: string[];
    label?: string;
    group?: string;
    confirm?: string;
    run_label?: string;
    order?: number;
  }

  /**
   * Everything about one endpoint except the code that runs it.
   *
   * `InputSchema` is the schema this entry declared, which is where the
   * handler's `input` type comes from; there is nothing to spell out by hand.
   */
  export type FunctionDefinition<InputSchema = unknown> = {
    /** Semver of the function itself. Defaults to "0.0.0". */
    version?: string;
    /** Shown in the generated API docs. */
    description?: string;
    /** Who may call it. Defaults to "private". */
    permission?: Permission;
    /** HTTP verb the endpoint answers. Defaults to "POST". */
    method?: "GET" | "POST" | "PUT" | "DELETE";
    /** The request body. A schema built with `s` is checked before the handler
     *  runs *and* published to the docs; a plain JSON Schema object is only
     *  published. */
    input?: InputSchema;
    /** The response body, for the docs. */
    output?: Schema<unknown> | object;
    /** The shape of this function's `functions/<name>.toml`, for the docs. */
    config?: Schema<unknown> | object;
    /** How the dashboard presents it. */
    admin?: AdminPresentation;
    /** What runs. `input` is the request body -- typed from `input` above, and
     *  already validated against it when it was a schema. */
    handler(input: InferInput<InputSchema>, ctx: Ctx): unknown;
  };

  /** What `defineFunctions` returns: the module's default export. */
  export interface FunctionModule {
    readonly __apiplant: number;
    readonly manifest: ReadonlyArray<Record<string, unknown>>;
    readonly handlers: Record<string, (input: never, ctx: Ctx) => unknown>;
  }

  /**
   * Declare this module's functions: one entry per endpoint, each pairing what
   * the endpoint *is* with what it *does*.
   *
   * ```ts
   * export default defineFunctions({
   *   greet: {
   *     permission: "public",
   *     input: s.object({ name: s.string({ minLength: 1 }) }),
   *     handler(input, ctx) {
   *       return { message: `Hello, ${input.name}!` };
   *     },
   *   },
   * });
   * ```
   *
   * The handler's `input` is typed from the schema beside it, so the shape is
   * written once and honoured in three places: your editor, the request (a bad
   * body is a 400 naming the field) and the generated OpenAPI document.
   */
  export function defineFunctions<Schemas extends Record<string, unknown>>(
    // A mapped type rather than a plain `Record<string, …>`: it is what lets
    // TypeScript infer each entry's schema *and* feed the type it describes
    // back in as that entry's handler parameter.
    definitions: { [K in keyof Schemas]: FunctionDefinition<Schemas[K]> },
  ): FunctionModule;

  /** The body type a definition's `input` implies. */
  type InferInput<S> = S extends Schema<infer T> ? T : unknown;

  // ---- postgres ------------------------------------------------------------

  /** A JSON value, which is what any column becomes on the way out. */
  export type Value =
    | string
    | number
    | boolean
    | null
    | Value[]
    | { [key: string]: Value };

  /** One row: column name to value. */
  export type Row = Record<string, Value>;

  /** A statement with its bound parameters, as `sql` builds one. */
  export interface Query {
    sql: string;
    params: unknown[];
  }

  /**
   * The app's database.
   *
   * Synchronous by design: the isolate waits while the host runs the statement
   * on the thread that owns the connection pool. Values are bound, never
   * interpolated -- write `$1`, `$2`, ... and pass `params`, or use `sql`.
   *
   * Every method throws on a database error, so a failure cannot be mistaken
   * for an empty result.
   */
  export const db: {
    /** Every row a SELECT returned. */
    query<T = Row>(sql: string, params?: unknown[]): T[];
    query<T = Row>(query: Query): T[];

    /** The first row, or `null` when nothing matched. */
    first<T = Row>(sql: string, params?: unknown[]): T | null;
    first<T = Row>(query: Query): T | null;

    /** Exactly one row; throws when there is none. */
    one<T = Row>(sql: string, params?: unknown[]): T;
    one<T = Row>(query: Query): T;

    /** The single column of the single row, e.g. a `count(*)`. */
    value<T = Value>(sql: string, params?: unknown[]): T;
    value<T = Value>(query: Query): T;

    /** An INSERT, UPDATE or DELETE. Returns how many rows it touched. */
    execute(sql: string, params?: unknown[]): number;
    execute(query: Query): number;
  };

  /**
   * Build a statement with its values bound rather than interpolated.
   *
   * ```ts
   * const rows = db.query(sql`SELECT * FROM apiplant_note WHERE owner = ${id}`);
   * ```
   */
  export function sql(strings: TemplateStringsArray, ...values: unknown[]): Query;

  // ---- cache ---------------------------------------------------------------

  /**
   * The app's Redis, when `[cache]` is configured in main.toml. Throws
   * "no cache configured" when it is not.
   *
   * Values round-trip as JSON: an object goes in and an object comes back.
   */
  export const cache: {
    /** The value, or `null` when the key is absent or expired. */
    get<T = Value>(key: string): T | null;
    /** Whether the key is there, without fetching it. */
    has(key: string): boolean;
    /** Write a value. `ttlSeconds` defaults to `[cache] default_ttl_secs`;
     *  `0` never expires. */
    set(key: string, value: unknown, ttlSeconds?: number): void;
    /** Remove a key. `true` when it was there. */
    delete(key: string): boolean;
    /** Add to a counter (default 1), returning the new value. */
    increment(key: string, by?: number, ttlSeconds?: number): number;
    /** Seconds until expiry: `null` when absent, `0` when it never expires. */
    ttl(key: string): number | null;
    /** Return what's cached, or compute it, cache it and return that. */
    remember<T>(key: string, ttlSeconds: number, compute: () => T): T;
  };

  // ---- queues --------------------------------------------------------------

  /** What `queue.publish` reports back. */
  export interface Publication {
    /** Id of the queued message -- the same id its handler sees as
     *  `delivery().messageId`. */
    id: string;
    topic: string;
    /**
     * How many subscribers it was queued for.
     *
     * `0` means nothing subscribes to that topic. Not an error -- the message
     * is still recorded in `queue_message` -- but almost always a typo.
     */
    delivered: number;
  }

  /**
   * Work that happens after the response.
   *
   * `publish` records a message and returns; whichever functions
   * `[queues.subscribe]` points at that topic run afterwards, on their own,
   * with retries. Nothing the handler does can fail the request that published
   * it -- which is the reason to reach for this rather than just calling it.
   */
  export const queue: {
    publish(topic: string, message?: unknown): Publication;
  };

  /** The delivery a subscriber is running for. */
  export interface DeliveryContext {
    topic: string;
    /** Stable across retries, so it works as an idempotency key. */
    messageId: string;
    /** This function's name, as the subscription named it. */
    subscriber: string;
    /**
     * Which attempt this is, from 1.
     *
     * Delivery is at-least-once: anything above 1 is a message whose side
     * effects may already have partly happened.
     */
    attempts: number;
    /** Who published it, or `""` when the server did. */
    principalId: string;
  }

  /**
   * When running as a queue subscriber, the delivery this call is; `null` for
   * an HTTP call or a lifecycle hook.
   *
   * The message body arrives as the function's ordinary input, so a handler is
   * just a function and can be called by hand to test it.
   */
  export function delivery(): DeliveryContext | null;

  // ---- email ---------------------------------------------------------------

  /** A recipient: `"ann@example.com"`, `"Ann <ann@example.com>"`, or split. */
  export type EmailAddress = string | { email: string; name?: string };

  /** A message handed to `email.send`. Send at least one of `text` and `html`. */
  export interface EmailMessage {
    to: EmailAddress | EmailAddress[];
    subject: string;
    text?: string;
    html?: string;
    cc?: EmailAddress | EmailAddress[];
    bcc?: EmailAddress | EmailAddress[];
    /** Overrides `[email] from` for this message. */
    from?: EmailAddress;
    /** Overrides `[email] reply_to` for this message. */
    reply_to?: EmailAddress;
  }

  /** What the provider reported about a message it accepted. */
  export interface SentEmail {
    /** Which provider took it. */
    provider: string;
    /** The provider's own id for the message; empty when it returns none. */
    id: string;
    /** How many recipients it went to (`to` + `cc` + `bcc`). */
    recipients: number;
  }

  /** The app's mail provider, when `[email]` is configured. */
  export const email: {
    send(message: EmailMessage): SentEmail;
  };

  // ---- payments ------------------------------------------------------------

  /** One request to the app's payment provider. */
  export type PaymentsRequest = { op: string } & Record<string, unknown>;

  /** What a checkout or billing-portal request answers with. */
  export interface PaymentsUrlReply extends Record<string, Value> {
    url: string;
  }

  /** The app's payment provider, when `[payments]` is configured. */
  export const payments: {
    /** Start a checkout and get the provider URL to send the buyer to. */
    checkout(stripePriceId: string, recurring: boolean, organizationId: string): PaymentsUrlReply;
    /** Open the provider's self-service billing screens for one customer. */
    billingPortal(stripeCustomerId: string): PaymentsUrlReply;
    /** Ask the provider what a subscription's state actually is. */
    subscription<T = Value>(id: string): T;
    /** Cancel a subscription, usually at the end of the paid period. */
    cancelSubscription<T = Value>(id: string, atPeriodEnd?: boolean): T;
    /** One raw request to the provider, for anything the helpers do not cover yet. */
    request<T = Value>(request: PaymentsRequest): T;
  };

  // ---- the assistant -------------------------------------------------------

  /** One model tool call. */
  export interface ToolCall {
    id: string;
    name: string;
    input: Value;
  }

  /** One tool the model may call. */
  export interface ToolDefinition {
    name: string;
    description?: string;
    input_schema?: Schema<unknown> | object;
  }

  /** One turn of a conversation. */
  export interface ChatMessage {
    role: "system" | "user" | "assistant" | "tool";
    content: string;
    tool_call_id?: string;
    tool_calls?: ToolCall[];
  }

  /** A question, plus anything about it that should differ from `[ai]`. */
  export interface ChatRequest {
    messages: ChatMessage[];
    /** Overrides `[ai] model`. */
    model?: string;
    /** Overrides `[ai] system`. A `system` message wins over both. */
    system?: string;
    temperature?: number;
    max_tokens?: number;
    /** One or more tools the model may call. */
    tools?: ToolDefinition[];
    /** Forward the answer to this function's caller as it arrives. */
    stream?: boolean;
  }

  /** A complete answer. */
  export interface ChatReply {
    /** The whole message. */
    text: string;
    /** Which provider answered: `"openai"`, `"anthropic"`, `"custom"`. */
    provider: string;
    /** The model that was asked for. */
    model: string;
    /** The provider's word for why it stopped, when it said. */
    finish_reason?: string;
    input_tokens?: number;
    output_tokens?: number;
    /** One or more tool calls the model asked the caller to run. */
    tool_calls: ToolCall[];
  }

  /**
   * The app's AI assistant, when `[ai]` is configured in main.toml. Throws
   * "no ai provider configured" when it is not.
   */
  export const ai: {
    /** Ask, and wait for the whole answer. A string is a one-question chat. */
    chat(request: ChatRequest | string): ChatReply;
    /** Just the text of the answer. */
    ask(prompt: string): string;
    /** Ask, emitting each token to your own caller as it arrives. */
    chatStreaming(request: ChatRequest | string): ChatReply;
  };

  /**
   * Push a chunk of the response to the caller before this function returns.
   *
   * Only reaches anybody when the call came through
   * `<base>/functions/<name>/stream`; a no-op otherwise, so one handler works
   * streamed, plain and as a hook. Answers "keep going?": `false` once the
   * caller has hung up, `true` otherwise.
   */
  export function emit(chunk: string): boolean;

  // ---- the request ---------------------------------------------------------

  /** This function's `functions/<name>.toml`, as an object. */
  export function config<T = Record<string, unknown>>(): T;

  /** The authenticated caller's id, or `""` when the endpoint is public. */
  export function principalId(): string;

  /** What a lifecycle hook is running for; `null` for a plain HTTP call. */
  export function hook<T extends Row = Row>(): HookContext<T> | null;

  /** Write to the server's log. `console.log` and friends land here too. */
  export const log: Logger;

  export interface Logger {
    trace(message: unknown): void;
    debug(message: unknown): void;
    info(message: unknown): void;
    warn(message: unknown): void;
    error(message: unknown): void;
  }

  /** Everything a lifecycle hook is told about the request it is running in. */
  export interface HookContext<T extends Row = Row> {
    /** e.g. `"before_create"`, or `"before_login"` for an auth hook. */
    event: string;
    /**
     * `"create"`, `"read"`, `"update"`, `"delete"` or `"list"` — or, for an
     * auth hook, `"register"`, `"login"` or `"api_key"`.
     */
    action: string;
    /** `"before"` or `"after"`. */
    phase: string;
    /** The resource the hook is attached to. */
    resource: string;
    url: string;
    method: string;
    query: Record<string, string>;
    authenticated: boolean;
    principal_id: string;
    organization_id: string | null;
    /** The caller's primary role, and every role they hold. */
    role: string;
    roles: string[];
    record_id: string | null;
    /** The submitted body, on `before_create` / `before_update`. */
    data: T | null;
    /** The record, on every other single-record event. */
    row: T | null;
    /** The page, on `after_list`. */
    rows: T[] | null;
  }

  /**
   * Reject the caller's request with a 400 and this message.
   *
   * Everything else thrown is the function's own fault: a 500, with the message
   * in the server log rather than in the response.
   */
  export class BadRequest extends Error {
    constructor(message: string);
  }

  /** Reject with a status of your choosing; 4xx blames the caller, 5xx you. */
  export class HttpError extends Error {
    constructor(status: number, message: string);
    readonly status: number;
  }

  // ---- schemas -------------------------------------------------------------

  /** A declared shape: JSON Schema for the docs, a check for the request, and
   *  a TypeScript type for the handler. */
  export interface Schema<T> {
    readonly __schema: number;
    readonly json: object;
    /** Present only so TypeScript can carry `T`; never read at runtime. */
    readonly __type?: T;
  }

  /** A schema whose field may be absent. */
  export interface OptionalSchema<T> extends Schema<T> {
    readonly __optional: number;
  }

  /** The TypeScript type a schema describes: `Infer<typeof Input>`. */
  export type Infer<S> = S extends OptionalSchema<infer T>
    ? T | undefined
    : S extends Schema<infer T>
      ? T
      : never;

  type Fields = Record<string, Schema<unknown>>;

  type ObjectOf<F extends Fields> = {
    [K in keyof F as F[K] extends OptionalSchema<unknown> ? never : K]: Infer<F[K]>;
  } & {
    [K in keyof F as F[K] extends OptionalSchema<unknown> ? K : never]?: Infer<F[K]>;
  };

  export interface StringOptions {
    minLength?: number;
    maxLength?: number;
    /** A regular expression, as source text. */
    pattern?: string;
    /** An OpenAPI format hint, e.g. `"email"`, `"uuid"`, `"date-time"`. */
    format?: string;
    description?: string;
  }

  export interface NumberOptions {
    minimum?: number;
    maximum?: number;
    description?: string;
  }

  export interface ArrayOptions {
    minItems?: number;
    maxItems?: number;
    description?: string;
  }

  export interface Described {
    description?: string;
  }

  /**
   * A small schema builder: declare a shape once and get its JSON Schema (for
   * the OpenAPI docs), its validation (a 400 naming the field that was wrong)
   * and its TypeScript type (the handler's `input`).
   *
   * It covers objects, arrays, strings, numbers, booleans and enums. For
   * anything richer, hand `input` a plain JSON Schema object: it reaches the
   * docs untouched, and the handler does its own checking.
   */
  export const s: {
    string(options?: StringOptions): Schema<string>;
    number(options?: NumberOptions): Schema<number>;
    integer(options?: NumberOptions): Schema<number>;
    boolean(options?: Described): Schema<boolean>;
    enum<const V extends readonly string[]>(values: V, options?: Described): Schema<V[number]>;
    array<S extends Schema<unknown>>(items: S, options?: ArrayOptions): Schema<Infer<S>[]>;
    /** Fields are required unless wrapped in `s.optional`. */
    object<F extends Fields>(fields: F, options?: Described): Schema<ObjectOf<F>>;
    optional<S extends Schema<unknown>>(field: S): OptionalSchema<Infer<S>>;
    /** Anything at all: documented, unchecked. */
    any(options?: Described): Schema<Value>;
  };

  /** Check a body against a schema, or throw the `BadRequest` that says why. */
  export function parse<S extends Schema<unknown>>(schema: S, body: unknown): Infer<S>;

  // ---- the older, import-free style ----------------------------------------

  /**
   * The host, handed to every handler as its second argument.
   *
   * Everything on it is also importable from this module (`db.query` for
   * `ctx.query`, `cache` for `ctx.cache`, and so on), which is usually the
   * nicer way round -- but a function that would rather take the host as an
   * argument than import it can.
   */
  export interface Ctx {
    /** Run SQL. A SELECT returns rows; anything else returns
     *  `{ rows_affected }`. Prefer `db`, which splits the two. */
    query(sql: string, params?: unknown[]): Row[] | { rows_affected: number };
    config<T = Record<string, unknown>>(): T;
    principalId(): string;
    hook<T extends Row = Row>(): HookContext<T> | null;
    sendEmail(message: EmailMessage): SentEmail;
    payments<T = Value>(request: PaymentsRequest): T;
    cache(request: Record<string, unknown>): Value;
    chat(request: ChatRequest | string): ChatReply;
    /** Queue a message for its topic's subscribers. Prefer `queue.publish`. */
    publish(topic: string, message?: unknown): Publication;
    emit(chunk: string): boolean;
    log: Logger;
    BadRequest: typeof BadRequest;
  }

  /** One entry of a hand-written `manifest`, for a module that does not use
   *  `defineFunctions`. */
  export interface FunctionManifest {
    name: string;
    version?: string;
    description?: string;
    permission?: Permission;
    method?: "GET" | "POST" | "PUT" | "DELETE";
    input_schema?: object;
    output_schema?: object;
    config_schema?: object;
    admin?: AdminPresentation;
  }
}

// ---- globals ---------------------------------------------------------------
//
// A module that declares its functions the long way -- `export const manifest`
// plus one export per entry -- imports nothing, so the types it needs are here
// as well. They are the same declarations, reached through the module.

type Ctx = import("apiplant").Ctx;
type Row = import("apiplant").Row;
type Value = import("apiplant").Value;
type Permission = import("apiplant").Permission;
type FunctionManifest = import("apiplant").FunctionManifest;
type HookContext<T extends Row = Row> = import("apiplant").HookContext<T>;
type EmailMessage = import("apiplant").EmailMessage;
type SentEmail = import("apiplant").SentEmail;

/** Also a global, so `throw new BadRequest("…")` needs no import. */
declare const BadRequest: typeof import("apiplant").BadRequest;

/** `console.log` and friends go to the server's log, not to stdout. */
declare const console: {
  log(message: unknown): void;
  info(message: unknown): void;
  debug(message: unknown): void;
  warn(message: unknown): void;
  error(message: unknown): void;
  trace(message: unknown): void;
};

/** The standard timers, available as usual. An invocation is not finished until
 *  its pending timers have run. */
declare function setTimeout(
  callback: (...args: never[]) => void,
  delay?: number,
  ...args: never[]
): number;
declare function setInterval(
  callback: (...args: never[]) => void,
  delay?: number,
  ...args: never[]
): number;
declare function clearTimeout(id?: number): void;
declare function clearInterval(id?: number): void;
