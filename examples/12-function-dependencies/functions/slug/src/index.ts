/**
 * `slug` — an apiplant function with npm dependencies.
 *
 * A directory rather than a single file, which for TypeScript means an ordinary
 * npm project: your `package.json`, your dependencies, your bundler. apiplant
 * runs `pnpm install` (once) and `pnpm run build`, then copies the bundle to
 * `functions/slug.js` — the same deal a Rust directory gets from its Cargo.toml
 * and a Go one from its go.mod.
 *
 * Two things this file could not do as a single `.ts`:
 *
 *   - `import slugify from "slugify"` — a package from npm
 *   - `import { isReserved } from "./reserved.ts"` — a second source file
 *
 * `apiplant` itself is *not* bundled: it is marked external in the build script,
 * because the host provides it to the isolate. Everything else is inlined, so
 * what the server loads is still one self-contained file.
 */

import slugify from "slugify";

import { defineFunctions, db, log, s, sql } from "apiplant";

import { isReserved } from "./reserved.ts";

export default defineFunctions({
  slug: {
    version: "1.0.0",
    description: "Turns a title into a URL slug, using the `slugify` package.",
    permission: "public",
    method: "POST",
    input: s.object({
      title: s.string({ minLength: 1, description: "The text to slugify." }),
    }),
    output: s.object({
      slug: s.string(),
      reserved: s.boolean(),
      taken: s.boolean(),
    }),

    handler(input) {
      // The dependency: strips accents, collapses punctuation, lower-cases.
      const slug = slugify(input.title, { lower: true, strict: true });

      // The sibling module.
      const reserved = isReserved(slug);

      // The host, reached through the module the bundler left external. Nothing
      // about this changes because the function is a directory.
      const taken =
        db.value<number>(
          sql`SELECT count(*)::int AS n FROM apiplant_article WHERE slug = ${slug}`,
        ) > 0;

      log.info(`slugified "${input.title}" to "${slug}"`);

      return { slug: reserved ? `${slug}-page` : slug, reserved, taken };
    },
  },
});
