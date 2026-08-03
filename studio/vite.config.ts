import { defineConfig } from "vite";
import solid from "vite-plugin-solid";
import tailwindcss from "@tailwindcss/vite";

export default defineConfig({
  plugins: [solid(), tailwindcss()],
  server: { port: 5273 },
  build: {
    target: "es2022",
    // Straight into the crate that embeds it. There is no studio/dist: one
    // tracked copy, inside the package cargo publishes, is the whole story.
    outDir: "../crates/apiplant-assets/assets/studio",
    emptyOutDir: true,
    // CodeMirror plus four language grammars is most of the bundle. This is a
    // tool you run on localhost against your own directory, so one chunk is the
    // right trade; the warning would only be noise.
    chunkSizeWarningLimit: 900,
  },
});
