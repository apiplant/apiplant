/**
 * The screenshots the guides embed, published at `/docs-images/<file>`.
 *
 * They live in `../docs/images/` because that is where a guide read on GitHub
 * has to find them: `![…](images/admin-home.png)` resolves against the file,
 * and only a path relative to `docs/` works there. This site is a single-page
 * app whose URLs are `/docs/admin`, so the same relative path would resolve
 * differently on every route — hence one absolute prefix, rewritten into the
 * `src` by the image rule in `src/lib/docs.ts`.
 *
 * The directory is outside `public/`, which Vite copies verbatim, so it is
 * served here in dev and emitted as assets in the build.
 */

import { readdirSync, readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { extname, join } from "node:path";
import type { Plugin } from "vite";

/** The URL prefix. Must match `DOCS_IMAGE_BASE` in `src/lib/docs.ts`. */
export const DOCS_IMAGE_BASE = "/docs-images/";

const IMAGES_DIR = fileURLToPath(new URL("../../docs/images", import.meta.url));

const MIME: Record<string, string> = {
  ".png": "image/png",
  ".jpg": "image/jpeg",
  ".jpeg": "image/jpeg",
  ".gif": "image/gif",
  ".svg": "image/svg+xml",
  ".webp": "image/webp",
};

function imageNames(): string[] {
  try {
    return readdirSync(IMAGES_DIR).filter((name) => extname(name).toLowerCase() in MIME);
  } catch {
    // A checkout without screenshots still builds; the guides simply show
    // broken images rather than failing the build.
    return [];
  }
}

export function docsImagesPlugin(): Plugin {
  return {
    name: "apiplant-docs-images",

    configureServer(server) {
      server.middlewares.use((request, response, next) => {
        const url = (request.url ?? "").split("?")[0];
        if (!url.startsWith(DOCS_IMAGE_BASE)) return next();

        const name = decodeURIComponent(url.slice(DOCS_IMAGE_BASE.length));
        // No traversal out of the directory, whatever the request asked for.
        if (!imageNames().includes(name)) return next();

        response.setHeader("content-type", MIME[extname(name).toLowerCase()]);
        response.end(readFileSync(join(IMAGES_DIR, name)));
      });
    },

    generateBundle() {
      for (const name of imageNames()) {
        this.emitFile({
          type: "asset",
          // `fileName`, not `name`: the markdown says `images/admin-home.png`
          // and the rewrite only swaps the prefix, so the hashed name Rollup
          // would otherwise invent has nothing to rewrite it to.
          fileName: `${DOCS_IMAGE_BASE.replace(/^\//, "")}${name}`,
          source: readFileSync(join(IMAGES_DIR, name)),
        });
      }
    },
  };
}
