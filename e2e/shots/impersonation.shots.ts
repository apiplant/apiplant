/**
 * The impersonation picture in `docs/authentication.md`, taken from a real
 * dashboard.
 *
 * It needs `examples/27-back-office`: the app whose
 * `[organization] global_admin_role` names somebody, so Rae Root may borrow an
 * account she shares no organisation with. The picture is of the borrowed
 * session itself — the strip across the top that says whose account is in use
 * and holds the way out — because that is what an operator must always be able
 * to see.
 *
 *   cd e2e && pnpm shots:impersonation
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

test("a borrowed account", async () => {
  await page.goto("./");
  await signIn(page, "root@example.com");

  // The Users screen is where a borrowed account is picked: every account in
  // the deployment, with Act as on the row.
  await page.locator("nav").getByRole("button", { name: /^Users$/i }).click();
  await expect(page.getByRole("heading", { name: /^Users$/i }).first()).toBeVisible();
  await expect(page.getByRole("button", { name: /Act as/i }).first()).toBeVisible();
  // Let the route change finish crossing over: a picture taken mid-fade shows
  // both screens at half opacity.
  await page.waitForTimeout(700);

  await page.getByRole("button", { name: /Act as/i }).first().click();

  // The strip says whose account is in use; the header chip says it again for
  // the screens the strip does not reach. Waiting on both is waiting on the
  // session having actually changed.
  await expect(page.getByText(/You are working as /i)).toBeVisible();
  await expect(page.getByRole("button", { name: /Back to your account/i })).toBeVisible();
  await page.waitForLoadState("networkidle");
  await page.waitForTimeout(700);
  await page.mouse.move(10, 10);
  await shoot(page, "admin-impersonation");
});
