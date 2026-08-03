import { defineConfig } from "vite";
import solid from "vite-plugin-solid";
import tailwindcss from "@tailwindcss/vite";
import { fileURLToPath } from "node:url";

export default defineConfig({
  plugins: [solid(), tailwindcss()],
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
