/**
 * The pictures in `docs/admin.md`, taken from a real dashboard.
 *
 * Nothing here is mocked or drawn: the browser signs in to the running example
 * app the same way an operator does, walks to each screen the guide describes,
 * and photographs it. That is the point — a screenshot that can go stale
 * without anything failing is worse than no screenshot, so re-running this is
 * how the guide is kept honest after an interface change.
 *
 *   cd e2e && pnpm shots
 *
 * The story is serial and shares one page, like the test suite: each shot is
 * taken from wherever the previous one left off.
 */

import { expect, test, type Page } from "@playwright/test";
import { dismissToasts, shoot, showEverybody, signIn, switchOrganization, SEED_PASSWORD } from "./helpers";

test.describe.configure({ mode: "serial" });

let page: Page;

test.beforeAll(async ({ browser }) => {
  page = await browser.newPage();
});

test.afterAll(async () => {
  await page.close();
});

test("sign in", async () => {
  await page.goto("./");
  await expect(page.getByRole("heading", { name: "Sign in" })).toBeVisible();
  await shoot(page, "admin-sign-in");

  await signIn(page, "admin@example.com", SEED_PASSWORD);
});

test("home", async () => {
  // The seeded trade all belongs to Acme; a fresh session may open in the
  // other tenant, where every list would be empty.
  await switchOrganization(page, /Acme/i);
  await expect(page.getByRole("button", { name: "Home" })).toBeVisible();
  await shoot(page, "admin-home");
});

test("a resource list", async () => {
  await page.locator("nav").getByRole("button", { name: /^Orders$/i }).click();
  await expect(page.getByRole("heading", { name: /^Orders$/i }).first()).toBeVisible();
  await showEverybody(page);
  await expect(page.locator("tbody tr").first()).toBeVisible();
  await shoot(page, "admin-resource-list");
});

test("one record", async () => {
  // A product rather than the order above: orders are read-only here, and the
  // guide is describing the form an operator actually edits.
  await page.locator("nav").getByRole("button", { name: /^Products$/i }).click();
  await expect(page.getByRole("heading", { name: /^Products$/i }).first()).toBeVisible();
  await showEverybody(page);
  await page.locator("tbody tr").first().click();
  await expect(page.getByRole("heading", { name: /^Details$/i })).toBeVisible();
  await shoot(page, "admin-record");
});

test("an action", async () => {
  await page.locator("nav").getByRole("button", { name: /Sales summary/i }).click();
  await expect(page.getByRole("heading", { name: /Sales summary/i }).first()).toBeVisible();
  // Run it: an action's whole point is the output beside the form, and an
  // empty result pane shows only half of the screen the guide describes.
  await page.getByRole("button", { name: /Show summary/i }).click();
  await expect(page.getByText(/No result yet/i)).toHaveCount(0);
  await dismissToasts(page);
  await shoot(page, "admin-action");
});

test("the team", async () => {
  await page.getByRole("button", { name: /^Team$/i }).first().click();
  await expect(page.getByRole("heading", { name: /^Team$/i }).first()).toBeVisible();
  // The heading paints before the members arrive; wait for a seeded one, or
  // the picture is of an empty screen mid-fetch.
  await expect(page.getByRole("listitem").filter({ hasText: "@example.com" }).first()).toBeVisible();
  await shoot(page, "admin-team");
});

test("api keys", async () => {
  // Not in the resource navigation — issuing a key is something an operator
  // does to their own account, so it lives behind the account menu.
  await page.getByRole("button", { name: /Account menu/i }).click();
  await page.getByRole("menuitem", { name: /API keys/i }).or(page.getByRole("button", { name: /API keys/i })).first().click();
  await expect(page.getByRole("heading", { name: /API keys/i }).first()).toBeVisible();

  // Issue one, so the picture shows the screen doing its job rather than its
  // empty state. The database is thrown away after the run, so the secret in
  // the shot is a secret to nothing.
  await page.getByRole("button", { name: /New key/i }).first().click();
  await page.getByLabel(/Name|Label/i).first().fill("Nightly stock sync");
  await page.getByRole("button", { name: /^(Create|Create key)$/i }).last().click();
  // The secret is shown once, in a dialog, and that is the part worth a
  // picture — the list afterwards only ever holds the prefix.
  await expect(page.getByText(/Copy your key now/i)).toBeVisible();
  await shoot(page, "admin-api-key-created");

  await page.getByRole("button", { name: /^Done$/i }).click();
  await expect(page.getByText(/No keys yet/i)).toHaveCount(0);
  await dismissToasts(page);
  await shoot(page, "admin-api-keys");
});
