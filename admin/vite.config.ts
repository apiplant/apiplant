import { defineConfig } from "vite";
import solid from "@solidjs/vite-plugin";
import tailwindcss from "@tailwindcss/vite";

export default defineConfig({
  plugins: [solid(), tailwindcss()],
  base: "./",
  publicDir: "../studio/public",
  server: { port: 5274 },
  build: {
    target: "es2022",
    // Straight into the crate that embeds it. There is no admin/dist: one
    // tracked copy, inside the package cargo publishes, is the whole story.
    outDir: "../crates/apiplant-assets/assets/admin",
    emptyOutDir: true,
    cssCodeSplit: false,
    chunkSizeWarningLimit: 900,
    rollupOptions: {
      output: {
        entryFileNames: "app.js",
        assetFileNames: (assetInfo) => {
          if (assetInfo.names.includes("style.css")) return "app.css";
          return "assets/[name]-[hash][extname]";
        },
      },
    },
  },
});
