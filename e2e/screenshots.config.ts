/**
 * The configuration behind `pnpm shots`: the same real server the test suite
 * drives, pointed at a richer example, with the browser told to render as a
 * retina display so the pictures in `docs/` stay sharp when a reader zooms.
 *
 * This is deliberately a separate config from `playwright.config.ts`. The suite
 * proves the framework and must fail loudly; this one produces documentation
 * assets and is run by hand when the interface changes.
 */

import { defineConfig, devices } from "@playwright/test";

/**
 * `13-real-world` is the example with enough furniture to photograph: nineteen
 * resources grouped by `[admin]`, seeded rows in all of them, and a function
 * mounted as an action. An empty dashboard documents nothing.
 */
const APP_DIR = process.env.APP_DIR ?? "examples/13-real-world";
const ORIGIN = process.env.APP_ORIGIN ?? "http://127.0.0.1:8099";
const BASE_PATH = process.env.APP_BASE_PATH ?? "/api";

export default defineConfig({
  testDir: "./shots",
  testMatch: "**/*.shots.ts",
  fullyParallel: false,
  workers: 1,
  retries: 0,
  timeout: 120_000,
  expect: { timeout: 15_000 },
  reporter: [["list"]],

  use: {
    baseURL: `${ORIGIN}/admin/`,
    headless: true,
    // A window wide enough for the sidebar and a table side by side, and a
    // 2× device pixel ratio so the PNG survives a retina screen.
    viewport: { width: 1440, height: 900 },
    deviceScaleFactor: 2,
    colorScheme: "light",
  },

  projects: [{ name: "chromium", use: { ...devices["Desktop Chrome"] } }],

  webServer: {
    command: "bash ./scripts/start-app.sh",
    env: { APP_DIR, APP_SEED: "1" },
    url: `${ORIGIN}${BASE_PATH}/_health`,
    // Reuse a server already up: capturing is iterative, and rebuilding the
    // binary between attempts is the slow part.
    reuseExistingServer: true,
    timeout: 300_000,
    stdout: "pipe",
    stderr: "pipe",
  },

  metadata: { appDir: APP_DIR, origin: ORIGIN, basePath: BASE_PATH },
});

export { APP_DIR, BASE_PATH, ORIGIN };
