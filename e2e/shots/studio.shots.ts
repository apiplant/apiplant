/**
 * The pictures in `docs/studio.md`, taken from the real editor.
 *
 * The studio has no server and no database: it holds a File System Access
 * handle to the directory you picked and edits it in the browser. That picker
 * is a native dialog, so the run installs an in-memory implementation of the
 * same API (`fs-access-shim.ts`) seeded with an example read off disk. The
 * studio is not modified and does not know the difference — and because writes
 * land in memory, photographing a checked-in example cannot change it.
 *
 *   cd e2e && pnpm shots:studio
 */

import { expect, test, type Browser, type BrowserContext, type Page } from "@playwright/test";
import { basename } from "node:path";
import { dismissToasts, shoot, SHOTS_THEME } from "./helpers";
import { installFileSystemAccess, readTree } from "./fs-access-shim";
import { STUDIO_APP } from "../studio-screenshots.config";

test.describe.configure({ mode: "serial" });

let context: BrowserContext;
let page: Page;

/** A page with the picker replaced, holding the named app directory. */
async function studioContext(browser: Browser, appDir: string): Promise<BrowserContext> {
  const ctx = await browser.newContext({
    viewport: { width: 1440, height: 900 },
    deviceScaleFactor: 2,
    colorScheme: SHOTS_THEME,
  });
  await ctx.addInitScript(installFileSystemAccess, {
    name: basename(appDir),
    tree: readTree(appDir),
  });
  return ctx;
}

/** Click through the landing screen into the editor. */
async function openApp(target: Page) {
  await target.getByRole("button", { name: /Open app directory/i }).click();
  await expect(target.getByRole("button", { name: /^Overview$/i })).toBeVisible();
  await target.waitForLoadState("networkidle");
  await dismissToasts(target);
}

test.beforeAll(async ({ browser }) => {
  context = await studioContext(browser, STUDIO_APP);
  page = await context.newPage();
});

test.afterAll(async () => {
  await context.close();
});

test("the landing screen", async () => {
  await page.goto("./");
  await expect(page.getByRole("button", { name: /Open app directory/i })).toBeVisible();
  await shoot(page, "studio-landing");
});

test("the overview", async () => {
  await openApp(page);
  await shoot(page, "studio-overview");
});

test("main.toml as a form", async () => {
  await page.getByRole("button", { name: /^Configuration$/i }).click();
  await expect(page.getByRole("heading", { name: /Configuration/i }).first()).toBeVisible();
  await shoot(page, "studio-configuration");
});

test("a resource", async () => {
  await page.locator("aside, nav").getByRole("button", { name: /^Products?$/i }).first().click();
  await expect(page.getByRole("heading", { name: /Product/i }).first()).toBeVisible();
  await shoot(page, "studio-resource");
});

test("the permissions of a resource", async () => {
  await page.getByRole("button", { name: /^Permissions$/i }).click();
  await expect(page.getByText(/create|read|update|delete/i).first()).toBeVisible();
  await shoot(page, "studio-permissions");
});

test("starting a resource", async () => {
  await page.getByRole("button", { name: /^New resource$/i }).click();
  const dialog = page.getByRole("dialog");
  await expect(dialog.getByText(/One resources\/\*\.toml/i)).toBeVisible();
  await dialog.getByRole("textbox").first().fill("invoice");
  await shoot(page, "studio-new-resource");
  await dialog.getByRole("button", { name: /^Cancel$/i }).click();
});

test("starting a function", async () => {
  await page.getByRole("button", { name: /^New function$/i }).click();
  const dialog = page.getByRole("dialog");
  await expect(dialog.getByText(/A library in functions\//i)).toBeVisible();
  await dialog.getByRole("textbox").first().fill("reprice");
  await shoot(page, "studio-new-function");
  await dialog.getByRole("button", { name: /^Cancel$/i }).click();
});

test("a function", async () => {
  await page.locator("aside, nav").getByRole("button", { name: /back.?office/i }).first().click();
  await expect(page.getByRole("heading", { name: /back.?office/i }).first()).toBeVisible();
  await shoot(page, "studio-function");
});

test("pending changes", async () => {
  // Nothing is written until Save, and *Pending changes* is where an edit is
  // reviewed first — which is the studio's whole safety model, so the picture
  // needs a real edit behind it rather than an empty list.
  await page.getByRole("button", { name: /^Configuration$/i }).click();
  // The form's captions are not `<label for>`, so the field is reached by its
  // placeholder — which the studio sets to the framework's own default.
  const appName = page.getByRole("textbox", { name: "13-real-world" });
  await appName.fill("Acme Logistics");
  await appName.blur();

  // The header's save-state button is the way in, and it is also what changes:
  // "saved" becomes a count the moment an edit is staged.
  await page.locator("header").getByRole("button", { name: /change|unsaved|saved/i }).first().click();
  await expect(page.getByText(/main\.toml/i).first()).toBeVisible();
  await shoot(page, "studio-changes");
});
