import { defineConfig } from "vite";
import solid from "vite-plugin-solid";
import tailwindcss from "@tailwindcss/vite";

export default defineConfig({
  plugins: [solid(), tailwindcss()],
  base: "./",
  publicDir: "../studio/public",
  server: { port: 5274 },
  build: {
    target: "es2022",
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
