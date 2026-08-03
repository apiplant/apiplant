// What `pnpm check` compiles: the declarations, used the way a function would
// use them.
//
// There is nothing to run here. `tsc --noEmit` failing *is* the failure -- if a
// handler's `input` stops being inferred from its schema, or a host call
// forgets a field the Rust side needs, this file stops compiling.

import {
  BadRequest,
  cache,
  config,
  db,
  defineFunctions,
  email,
  ai,
  hook,
  HttpError,
  Infer,
  log,
  payments,
  principalId,
  Row,
  s,
  sql,
  ToolDefinition,
} from "apiplant";

/** Asserts at compile time that `T` is exactly `U`: passing anything but the
 *  literal `true` means the two types have drifted apart. */
type Exact<T, U> = [T] extends [U] ? ([U] extends [T] ? true : false) : false;
const exact = <T, U>(_ok: Exact<T, U>) => {};

/** `Exact` is satisfied by `any` on either side, so inference collapsing to
 *  `any` would pass every assertion below. This is what notices. */
type IsAny<T> = 0 extends 1 & T ? true : false;
const notAny = <T,>(_ok: IsAny<T> extends true ? never : true) => {};

// ---- schemas: one declaration, three uses ----------------------------------

const NewNote = s.object({
  title: s.string({ minLength: 1, maxLength: 200 }),
  body: s.optional(s.string()),
  tags: s.array(s.string()),
  priority: s.enum(["low", "high"] as const),
  pinned: s.optional(s.boolean()),
});

type NewNote = Infer<typeof NewNote>;

// Required fields are required; `s.optional` ones are not; an enum narrows to
// its members rather than widening to `string`.
const note: NewNote = { title: "a", tags: [], priority: "high" };
const _title: string = note.title;
const _body: string | undefined = note.body;
const _tags: string[] = note.tags;
const _priority: "low" | "high" = note.priority;

// @ts-expect-error - `title` is required.
const _missing: NewNote = { tags: [], priority: "low" };

// @ts-expect-error - "urgent" is not one of the declared values.
const _badEnum: NewNote = { title: "a", tags: [], priority: "urgent" };

// ---- functions -------------------------------------------------------------

interface NoteRow extends Row {
  id: string;
  title: string;
}

export default defineFunctions({
  /** The handler's `input` is typed from the schema above. */
  createNote: {
    permission: "authenticated",
    description: "Files a note.",
    input: NewNote,
    handler(input, ctx) {
      // Inferred, not annotated -- and genuinely inferred, not `any`.
      notAny<typeof input>(true);
      exact<typeof input.title, string>(true);
      exact<typeof input.body, string | undefined>(true);

      // @ts-expect-error - `input` has no `nope`.
      input.nope;

      const owner = principalId();
      const row = db.one<NoteRow>(
        sql`INSERT INTO apiplant_note (title, owner) VALUES (${input.title}, ${owner}) RETURNING id, title`,
      );

      cache.delete(`notes:${owner}`);
      log.info(`note ${row.id} filed by ${owner}`);

      // The second argument is still the host, for anyone who wants it.
      ctx.log.debug(ctx.principalId());

      return { id: row.id, title: row.title };
    },
  },

  /** No schema: `input` is `unknown`, and the handler says what it wants. */
  stats: {
    permission: "public",
    method: "GET",
    output: s.object({ notes: s.integer() }),
    handler(input) {
      // No schema means `unknown`: the handler has to say what it expects.
      notAny<typeof input>(true);
      exact<typeof input, unknown>(true);

      const notes = cache.remember("stats:notes", 60, () =>
        db.value<number>("SELECT count(*)::int FROM apiplant_note"),
      );
      return { notes };
    },
  },

  /** A hook, reading the row it is running for and rejecting the request. */
  guard: {
    handler() {
      const context = hook<NoteRow>();
      if (!context) throw new HttpError(500, "not running as a hook");
      if (context.action === "delete" && context.row?.title === "keep") {
        throw new BadRequest("that note cannot be deleted");
      }
      return { data: context.data };
    },
  },

  /** Mail and config. */
  notify: {
    permission: "role:admin",
    handler() {
      const { sender } = config<{ sender: string }>();
      const sent = email.send({
        to: [{ email: "ops@example.com", name: "Ops" }, "second@example.com"],
        subject: "Notes",
        text: "A note was filed.",
        from: sender,
      });
      exact<typeof sent.recipients, number>(true);
      return { provider: sent.provider };
    },
  },

  /** Billing helpers. */
  billing: {
    permission: "member",
    handler() {
      const checkout = payments.checkout("price_123", true, "org_123");
      exact<typeof checkout.url, string>(true);

      const portal = payments.billingPortal("cus_123");
      exact<typeof portal.url, string>(true);

      const subscription = payments.subscription<{ status: string; entitled: boolean }>("sub_123");
      exact<typeof subscription.entitled, boolean>(true);

      const customer = payments.request<{ stripe_customer_id: string }>({
        op: "customer",
        organization_id: "org_123",
        email: "ops@example.com",
      });
      exact<typeof customer.stripe_customer_id, string>(true);

      return { checkout: checkout.url, portal: portal.url, active: subscription.entitled };
    },
  },

  /** AI requests, including tool definitions and tool-call messages. */
  assistant: {
    permission: "authenticated",
    handler() {
      const tools = [
        {
          name: "lookup_note",
          description: "Loads one note by id.",
          input_schema: s.object({ id: s.string({ format: "uuid" }) }),
        },
      ] satisfies ToolDefinition[];

      const reply = ai.chat({
        messages: [
          { role: "user", content: "Find note 11111111-1111-1111-1111-111111111111" },
          {
            role: "assistant",
            content: "",
            tool_calls: [
              {
                id: "call_1",
                name: "lookup_note",
                input: { id: "11111111-1111-1111-1111-111111111111" },
              },
            ],
          },
          {
            role: "tool",
            tool_call_id: "call_1",
            content: "{\"id\":\"11111111-1111-1111-1111-111111111111\"}",
          },
        ],
        tools,
      });

      exact<typeof reply.tool_calls, import("apiplant").ToolCall[]>(true);
      return { text: reply.text, requested: reply.tool_calls.length };
    },
  },
});

// ---- the database ----------------------------------------------------------

const rows = db.query<NoteRow>("SELECT id, title FROM apiplant_note WHERE owner = $1", ["u"]);
exact<typeof rows, NoteRow[]>(true);
exact<ReturnType<typeof db.first<NoteRow>>, NoteRow | null>(true);
exact<ReturnType<typeof db.execute>, number>(true);

// A `sql` template is a query, not a string, and carries its parameters.
const query = sql`SELECT ${1} AS n`;
exact<typeof query.params, unknown[]>(true);

// @ts-expect-error - `db.query` takes SQL and parameters, not a bare value.
db.query(42);

// ---- the cache -------------------------------------------------------------

exact<ReturnType<typeof cache.get<number>>, number | null>(true);
exact<ReturnType<typeof cache.has>, boolean>(true);
exact<ReturnType<typeof cache.increment>, number>(true);
cache.set("k", { any: "json" }, 30);

// @ts-expect-error - a TTL is a number of seconds.
cache.set("k", "v", "30");

// ---- email -----------------------------------------------------------------

// @ts-expect-error - `to` and `subject` are not optional.
email.send({ text: "hi" });

// ---- payments --------------------------------------------------------------

exact<ReturnType<typeof payments.checkout>["url"], string>(true);
notAny<ReturnType<typeof payments.cancelSubscription>>(true);

// @ts-expect-error - a checkout needs the organisation id.
payments.checkout("price_123", true);

// ---- ai --------------------------------------------------------------------

const aiReply = ai.chat("Summarise this.");
exact<typeof aiReply.tool_calls, import("apiplant").ToolCall[]>(true);
exact<ReturnType<typeof ai.ask>, string>(true);

// `hook()` is still typed from the row shape it runs against.
const hooked = hook<NoteRow>();
if (hooked) {
  exact<typeof hooked.row, NoteRow | null>(true);
  exact<typeof hooked.principal_id, string>(true);
}
