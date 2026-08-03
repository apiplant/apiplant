/**
 * A complete apiplant function in TypeScript.
 *
 * Four functions from one file:
 *
 *   POST /api/functions/hello   public         - greets someone, using config
 *   GET  /api/functions/notes   authenticated  - counts rows via the host's DB
 *   note.before_create          private hook   - trims and validates a title
 *   POST /api/functions/draftReply authenticated - asks AI with one tool definition
 *
 * `apiplant build` strips the types and writes `hello.js` next to this file; the
 * server runs that in a V8 isolate. Nothing is installed and nothing is bundled:
 * no node, no deno, no package.json, no node_modules.
 *
 * `apiplant` is the one module a function can import. It is compiled into the
 * server, and `apiplant build` drops the matching `apiplant.d.ts` in this
 * directory, so an editor types all of it with nothing to install either.
 */

import { BadRequest, ai, config, db, defineFunctions, hook, log, principalId, s } from "apiplant";

/**
 * The request body, declared once.
 *
 * This is the boilerplate the package exists to remove: the same declaration
 * becomes the JSON Schema in the generated docs, the check that runs before the
 * handler (a missing `name` is a 400, not a crash), and the type of `input`
 * below -- which is inferred, not annotated.
 */
const Greeting = s.object({
  name: s.string({ minLength: 1, description: "Who to greet." }),
});

export default defineFunctions({
  hello: {
    version: "1.0.0",
    description: "Greets someone from TypeScript.",
    permission: "public",
    method: "POST",
    input: Greeting,
    output: s.object({ message: s.string(), runtime: s.string() }),

    handler(input) {
      // functions/hello.toml, converted to JSON by the host.
      const { greeting = "Hello" } = config<{ greeting?: string }>();

      log.info(`hello invoked from TypeScript for ${input.name}`);

      return { message: `${greeting}, ${input.name}!`, runtime: "v8" };
    },
  },

  notes: {
    version: "1.0.0",
    description: "Counts notes, to show a query from TypeScript.",
    permission: "authenticated",
    method: "GET",
    output: s.object({ notes: s.integer(), caller: s.string() }),

    /**
     * `db` is synchronous: the isolate waits while the host runs the query on
     * the thread that owns the connection pool. There is no pool to open here
     * and no promise to await -- the same contract a C function's `host.query`
     * has, and the reason a handler only says `async` when it wants to.
     *
     * This query has nothing to bind. When one does, write it as
     * `db.query(sql`SELECT … WHERE owner = ${caller}`)`: the `sql` template turns
     * every `${...}` into a `$n` placeholder and passes the value separately, so
     * a title with an apostrophe in it is data rather than SQL.
     */
    handler() {
      const caller = principalId();
      const notes = db.value<number>("SELECT count(*)::int AS n FROM apiplant_note");

      return { notes, caller };
    },
  },

  noteBeforeCreate: {
    version: "1.0.0",
    description: "Trims note titles before they are written.",
    handler() {
      const context = hook<{ title?: string; body?: string }>();
      if (!context) throw new Error("not running as a hook");

      const title = context.data?.title?.trim() ?? "";
      if (!title) throw new BadRequest("`title` is required");

      return {
        data: {
          ...context.data,
          title,
        },
      };
    },
  },

  draftReply: {
    version: "1.0.0",
    description: "Asks the configured assistant, with one tool definition.",
    permission: "authenticated",
    method: "POST",
    input: s.object({
      question: s.string({ minLength: 1, description: "What to ask the assistant." }),
    }),
    output: s.object({
      answer: s.string(),
      tool_calls: s.optional(
        s.array(
          s.object({
            id: s.string(),
            name: s.string(),
            input: s.any(),
          }),
        ),
      ),
    }),

    handler(input) {
      const reply = ai.chat({
        messages: [{ role: "user", content: input.question }],
        tools: [
          {
            name: "lookup_note",
            description: "Loads one note by id for follow-up work.",
            input_schema: s.object({
              id: s.string({ format: "uuid", description: "The note id." }),
            }),
          },
        ],
      });

      return {
        answer: reply.text,
        ...(reply.tool_calls.length ? { tool_calls: reply.tool_calls } : {}),
      };
    },
  },
});
