import { defineConfig } from "vite";
import solid from "vite-plugin-solid";
import tailwindcss from "@tailwindcss/vite";
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { searchIndexPlugin } from "./build/search-index.ts";

/* The download links name a release asset, and every asset name carries the
   version — so the site has to know it. Read from the workspace manifest, the
   same string `cargo build` stamps into the binary, rather than a copy here
   that would quietly go stale one release later. */
function workspaceVersion(): string {
  const manifest = readFileSync(fileURLToPath(new URL("../Cargo.toml", import.meta.url)), "utf8");
  const match = /^\s*version\s*=\s*"([^"]+)"/m.exec(
    manifest.slice(manifest.indexOf("[workspace.package]")),
  );
  if (!match) throw new Error("no [workspace.package] version in ../Cargo.toml");
  return match[1];
}

export default defineConfig({
  plugins: [solid(), tailwindcss(), searchIndexPlugin()],
  define: { __APIPLANT_VERSION__: JSON.stringify(workspaceVersion()) },
  // The guides are the repository's own docs/, one directory up. The alias is
  // what `import.meta.glob("@docs/*.md")` resolves against.
  resolve: {
    alias: { "@docs": fileURLToPath(new URL("../docs", import.meta.url)) },
  },
  server: {
    port: 5274,
    // The docs live in ../docs and are imported raw; the dev server has to be
    // allowed to read above its own root to serve them.
    fs: { allow: [".."] },
  },
  build: {
    target: "es2022",
    outDir: "dist",
    emptyOutDir: true,
    rollupOptions: {
      output: {
        // Each guide's markdown is a small module, and Rollup would otherwise
        // inline all nineteen into the entry — every reader downloading the
        // whole manual to see the landing page. One chunk per guide instead,
        // fetched when that guide is opened.
        manualChunks(id) {
          // The prebuilt search index, and the engine that reads it: neither is
          // touched until the reader types in the search box, so both stay out
          // of the entry chunk.
          if (id.includes("virtual:search-index")) return "search-index";
          if (id.includes("/node_modules/zbsearch/")) return "search-engine";
          const match = /\/docs\/([\w.-]+)\.md(\?|$)/.exec(id);
          return match ? `doc-${match[1]}` : undefined;
        },
      },
    },
    // Shiki's grammars are the largest chunks, and each is loaded only when a
    // page actually contains that language.
    chunkSizeWarningLimit: 700,
  },
});
