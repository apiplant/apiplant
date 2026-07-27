import { defineConfig, devices } from "@playwright/test";

/**
 * The app under test. `07-functions` is the smallest example that has all of
 * the pieces the dashboard is built out of at once: registration, an
 * organisation, a resource with real CRUD, and two callable functions with
 * different visibilities.
 *
 * Point APP_DIR at another example to run the same shell against it — the start
 * script reads that app's own `main.toml` for the database and the port.
 */
const APP_DIR = process.env.APP_DIR ?? "examples/07-functions";
const ORIGIN = process.env.APP_ORIGIN ?? "http://127.0.0.1:8099";
const BASE_PATH = process.env.APP_BASE_PATH ?? "/api";

/**
 * `--headed` is Playwright's own flag; HEADED=1 is here so the same choice can
 * be made from an environment variable in a script or a CI job that wants to
 * watch. Headed runs are also slowed down, because a run nobody can follow is
 * not worth showing.
 */
const headed = process.env.HEADED === "1" || process.env.HEADED === "true";
const slowMo = Number(process.env.SLOW_MO ?? (headed ? 250 : 0));

export default defineConfig({
  testDir: "./tests",
  // The suite tells one story against one database, so it must not be sharded
  // across workers or retried halfway through.
  fullyParallel: false,
  workers: 1,
  retries: 0,
  timeout: 60_000,
  expect: { timeout: 10_000 },
  reporter: process.env.CI ? [["list"], ["html", { open: "never" }]] : [["list"]],

  use: {
    baseURL: `${ORIGIN}/admin/`,
    trace: "retain-on-failure",
    video: process.env.CI ? "retain-on-failure" : "off",
    screenshot: "only-on-failure",
    // `--headed` on the command line wins on its own; this is the env-variable
    // way in, for a script or a CI job that wants to watch.
    headless: !headed,
    launchOptions: { slowMo },
  },

  projects: [{ name: "chromium", use: { ...devices["Desktop Chrome"] } }],

  webServer: {
    command: "bash ./scripts/start-app.sh",
    env: { APP_DIR },
    // The database reset plus a cargo build of the binary and the example's
    // functions is the slow part, and it happens before the port opens.
    url: `${ORIGIN}${BASE_PATH}/_health`,
    reuseExistingServer: false,
    timeout: 300_000,
    stdout: "pipe",
    stderr: "pipe",
  },

  metadata: { appDir: APP_DIR, origin: ORIGIN, basePath: BASE_PATH },
});

export { APP_DIR, BASE_PATH, ORIGIN };
