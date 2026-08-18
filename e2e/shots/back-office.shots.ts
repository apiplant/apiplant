/**
 * The two back-office pictures in `docs/admin.md`.
 *
 * They need a different app: the screens appear only for whoever
 * `[organization] global_admin_role` names, and `examples/27-back-office` is
 * the example that sets it — three seeded tenants, nine accounts, and one
 * membership that makes Rae Root the deployment's administrator.
 *
 *   cd e2e && pnpm shots:back-office
 */

import { expect, test, type Page } from "@playwright/test";
import { shoot, signIn } from "./helpers";

test.describe.configure({ mode: "serial" });

let page: Page;

test.beforeAll(async ({ browser }) => {
  page = await browser.newPage();
});

test.afterAll(async () => {
  await page.close();
});

test("every organisation in the deployment", async () => {
  await page.goto("./");
  await signIn(page, "root@example.com");

  await page.locator("nav").getByRole("button", { name: /^Organizations$/i }).click();
  await expect(page.getByRole("heading", { name: /^Organizations$/i }).first()).toBeVisible();
  await expect(page.getByRole("listitem").first()).toBeVisible();
  await shoot(page, "admin-back-office-organizations");
});

test("every account in it", async () => {
  await page.locator("nav").getByRole("button", { name: /^Users$/i }).click();
  await expect(page.getByRole("heading", { name: /^Users$/i }).first()).toBeVisible();
  await expect(page.getByRole("listitem").first()).toBeVisible();
  await shoot(page, "admin-back-office-users");
});
