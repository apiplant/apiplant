// Three endpoints that exist to be watched: one that is slow, one that fails,
// and one that calls another service so the trace has more than one hop.
//
// Nothing here mentions OpenTelemetry. A function is already inside the
// request's span, so `log.info` lands on it and a thrown error colours it red —
// that is the whole integration.

import { defineFunctions, log, s, sql, db } from "apiplant";

export default defineFunctions({
  slow: {
    version: "1.0.0",
    description: "Takes as long as you ask it to. Gives the latency histogram something to say.",
    permission: "public",
    method: "POST",
    input: s.object({
      ms: s.integer({ minimum: 0, maximum: 5000, description: "How long to take." }),
    }),
    output: s.object({ slept_ms: s.integer() }),

    handler(input) {
      // Real latency rather than a busy loop: Postgres does the waiting, so the
      // span's duration is time actually spent in a downstream call — which is
      // what a slow request usually is.
      db.query(sql`SELECT pg_sleep(${input.ms / 1000})`);

      // Written inside the request's span, so this line carries the route, the
      // method and the trace id. In a log backend that is a filter, not a grep.
      log.info(`slow: slept ${input.ms}ms`);

      return { slept_ms: input.ms };
    },
  },

  boom: {
    version: "1.0.0",
    description: "Throws. Produces the red span you build an alert on.",
    permission: "public",
    method: "POST",
    output: s.object({ never: s.string() }),

    handler() {
      // An uncaught throw becomes a 500, and the middleware records it onto the
      // span as `error.type` + an `exception` event. A trace backend renders
      // that as a failed span; a metrics backend counts it under
      // `http.response.status_code="500"`. No reporting call to remember.
      throw new Error("the thing that was supposed to work did not");
    },
  },

  count: {
    version: "1.0.0",
    description: "A plain query, for a span that succeeds.",
    permission: "public",
    method: "GET",
    output: s.object({ notes: s.integer() }),

    handler() {
      const notes = db.value<number>("SELECT count(*)::int AS n FROM apiplant_note");
      log.info(`count: ${notes} notes`);
      return { notes };
    },
  },
});
