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
  hook,
  HttpError,
  Infer,
  log,
  principalId,
  Row,
  s,
  sql,
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
