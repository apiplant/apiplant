/**
 * The Web platform globals, used the way a real handler would use them.
 *
 *   POST /api/functions/exchange   authenticated - fetch, URL, Headers, Response
 *   POST /api/functions/receipt    authenticated - Intl: money and dates by locale
 *   POST /api/functions/digest     public        - TextEncoder, streams, base64
 *
 * None of this is imported. A function runs in a V8 isolate with a fixed set of
 * globals -- `fetch`, `URL`, `TextEncoder`, `Intl`, the streams -- and the list
 * is in `apiplant.d.ts`. If it typechecks, it is there at runtime.
 *
 * The one thing that is *not* there is `location`, so every URL must be
 * absolute. `new URL("/rates")` throws; `new URL("/rates", base)` is the way.
 */

import { BadRequest, config, db, defineFunctions, log, s } from "apiplant";

export default defineFunctions({
  /**
   * Calls a JSON API and stores what came back.
   *
   * The point of the example is the error handling, which is where a
   * hand-written HTTP client usually goes wrong: `fetch` rejects only when the
   * request never completed. A 404 or a 500 is a *resolved* promise with
   * `ok: false`, so a handler that forgets to check it will happily parse an
   * error page as its payload.
   */
  exchange: {
    description: "Fetches a rate from an upstream API and records it.",
    permission: "authenticated",
    method: "POST",
    input: s.object({
      base: s.string({ minLength: 3, maxLength: 3, description: "e.g. EUR" }),
      quote: s.string({ minLength: 3, maxLength: 3, description: "e.g. USD" }),
    }),
    output: s.object({ rate: s.number(), fetchedFrom: s.string() }),

    async handler(input) {
      // `[functions.web] rates_url` in web.toml, so the upstream is a
      // deployment decision rather than a constant in the source.
      const base = (config() as { rates_url?: string }).rates_url;
      if (!base) throw new Error("`rates_url` is not configured");

      // URLSearchParams handles the escaping; string concatenation does not.
      // Note the leading slash replaces the base's whole path, which is why the
      // API version belongs here rather than in `rates_url`.
      const url = new URL("/v1/latest", base);
      url.searchParams.set("base", input.base.toUpperCase());
      url.searchParams.set("symbols", input.quote.toUpperCase());

      const response = await fetch(url, {
        headers: new Headers({ accept: "application/json" }),
        // Bound this call independently of the 30s default.
        signal: AbortSignal.timeout(5_000),
      });

      // The check that is easy to forget. Without it, `.json()` below would
      // parse the upstream's error page and the failure would surface later,
      // somewhere unrelated.
      if (!response.ok) {
        log.warn(`rates upstream answered ${response.status} ${response.statusText}`);
        throw new BadRequest(`the rates provider answered ${response.status}`);
      }

      const payload = (await response.json()) as { rates?: Record<string, number> };
      const rate = payload.rates?.[input.quote.toUpperCase()];
      if (typeof rate !== "number") {
        throw new BadRequest(`no rate for ${input.base}/${input.quote}`);
      }

      // `execute`, not `query`: an INSERT returns an affected-row count rather
      // than rows, and `query` refuses a statement that produced none.
      db.execute(
        "INSERT INTO apiplant_note (title, body) VALUES ($1, $2)",
        [`${input.base}/${input.quote}`, String(rate)],
      );

      // `response.url` is where the response finally came from, which is not
      // the requested URL if the upstream redirected.
      return { rate, fetchedFrom: response.url };
    },
  },

  /**
   * Formats an amount and a date for a given locale.
   *
   * `Intl` here is the real thing, with the full ICU tables behind it, so
   * `de-DE` genuinely differs from `en-US` rather than falling back to English.
   * Worth knowing when asserting on the output: CLDR separates an amount from
   * its currency symbol with a non-breaking space (U+00A0), not a plain one.
   */
  receipt: {
    description: "Renders a total and a date the way one locale expects them.",
    permission: "authenticated",
    method: "POST",
    input: s.object({
      amount: s.number({ description: "In major units, e.g. 1234.5" }),
      currency: s.string({ minLength: 3, maxLength: 3 }),
      locale: s.string({ description: "A BCP 47 tag, e.g. de-DE" }),
      timeZone: s.optional(s.string({ description: "e.g. Europe/Rome" })),
    }),
    output: s.object({
      total: s.string(),
      issued: s.string(),
      summary: s.string(),
    }),

    handler(input) {
      const now = new Date();
      const zone = input.timeZone ?? "UTC";

      const total = new Intl.NumberFormat(input.locale, {
        style: "currency",
        currency: input.currency.toUpperCase(),
      }).format(input.amount);

      const issued = new Intl.DateTimeFormat(input.locale, {
        dateStyle: "long",
        timeStyle: "short",
        timeZone: zone,
      }).format(now);

      // ListFormat knows each locale's conjunction and comma conventions —
      // "a, b, and c" in English, "a, b und c" in German.
      const summary = new Intl.ListFormat(input.locale, { type: "conjunction" })
        .format([total, issued, zone]);

      return { total, issued, summary };
    },
  },

  /**
   * Hashes a payload and returns it base64-encoded.
   *
   * Shows the byte-level globals working together. Note `TextEncoder` produces
   * UTF-8: "héllo" is six bytes, not five, which is the whole reason to encode
   * explicitly rather than trust `length`.
   */
  digest: {
    description: "Encodes and compresses a string, reporting the sizes.",
    permission: "public",
    method: "POST",
    input: s.object({ text: s.string({ minLength: 1 }) }),
    output: s.object({
      characters: s.number(),
      bytes: s.number(),
      gzipped: s.number(),
      base64: s.string(),
    }),

    async handler(input) {
      const bytes = new TextEncoder().encode(input.text);

      // CompressionStream is a real stream, so it is driven rather than called.
      const gzip = new CompressionStream("gzip");
      const writer = gzip.writable.getWriter();
      void writer.write(bytes);
      void writer.close();

      const chunks: Uint8Array[] = [];
      for await (const chunk of gzip.readable) chunks.push(chunk);
      const gzipped = chunks.reduce((total, chunk) => total + chunk.length, 0);

      // `btoa` is byte-oriented and throws above U+00FF, which is why the text
      // goes through `TextEncoder` first rather than straight in.
      const base64 = btoa(String.fromCharCode(...bytes));

      return {
        characters: input.text.length,
        bytes: bytes.length,
        gzipped,
        base64,
      };
    },
  },
});
