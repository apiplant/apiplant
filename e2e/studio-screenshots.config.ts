/**
 * The configuration behind `pnpm shots:studio`.
 *
 * Separate from `screenshots.config.ts` because the studio needs nothing the
 * admin shots need: no database, no binary, no served app. It is a static
 * front end that edits a directory in the browser, so the only server here is
 * its own Vite dev server.
 */

import { defineConfig, devices } from "@playwright/test";
import { SHOTS_THEME } from "./shots/helpers";
import { fileURLToPath } from "node:url";

const ORIGIN = process.env.STUDIO_ORIGIN ?? "http://localhost:5273";

/** The app the studio is photographed editing — see `shots/studio.shots.ts`. */
export const STUDIO_APP = fileURLToPath(
  new URL(`../${process.env.STUDIO_APP_DIR ?? "examples/13-real-world"}`, import.meta.url),
);

export default defineConfig({
  testDir: "./shots",
  testMatch: "**/studio.shots.ts",
  fullyParallel: false,
  workers: 1,
  retries: 0,
  timeout: 120_000,
  expect: { timeout: 15_000 },
  reporter: [["list"]],

  use: {
    baseURL: `${ORIGIN}/`,
    headless: true,
    viewport: { width: 1440, height: 900 },
    deviceScaleFactor: 2,
    colorScheme: SHOTS_THEME,
  },

  projects: [{ name: "chromium", use: { ...devices["Desktop Chrome"] } }],

  webServer: {
    command: "pnpm --dir ../studio dev",
    url: ORIGIN,
    reuseExistingServer: true,
    timeout: 120_000,
    stdout: "pipe",
    stderr: "pipe",
  },
});

export { ORIGIN };
