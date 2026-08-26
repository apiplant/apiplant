/**
 * The impersonation pictures in `docs/authentication.md`, taken from a real
 * dashboard: both doors into a borrowed session, the session itself, and the
 * way out of it.
 *
 * It needs `examples/27-back-office`: the app whose
 * `[organization] global_admin_role` names somebody, so Rae Root may borrow an
 * account she shares no organisation with — while Nadia Nolan, an ordinary
 * admin of Northwind Traders, may borrow only her own members. The flow is
 * photographed from both sides because the two doors are not the same door:
 * one is a row on the Team screen, the other a button on a `user` record.
 *
 *   cd e2e && pnpm shots:impersonation
 */

import { expect, test, type Page } from "@playwright/test";
import { dismissToasts, shoot, signIn } from "./helpers";

test.describe.configure({ mode: "serial" });

let page: Page;

test.beforeAll(async ({ browser }) => {
  page = await browser.newPage();
});

test.afterAll(async () => {
  await page.close();
});

/**
 * Settle a route change before photographing it: a picture taken mid-fade
 * shows both screens at half opacity, and a hovered row lights up a control
 * the reader has no reason to think is special.
 */
async function settle(target: Page): Promise<void> {
  await target.waitForLoadState("networkidle");
  await target.waitForTimeout(700);
  await target.mouse.move(10, 10);
}

/**
 * Make the named organisation the active one.
 *
 * Not `switchOrganization()` from the helpers: that one finds the header's
 * switcher by the shapes company names take in the other examples, and none of
 * this app's four is an "Inc." — the trigger simply wears whichever
 * organisation is current, so it is found by that instead.
 */
async function standIn(target: Page, name: RegExp): Promise<void> {
  await target
    .locator("header")
    .getByRole("button")
    .filter({ hasText: /Northwind Traders|Umbra Logistics|Lumen Health|Vantage Support/ })
    .first()
    .click();
  await target.locator("header").getByRole("button", { name }).last().click();
  await target.waitForLoadState("networkidle");
  await dismissToasts(target);
}

test("an organisation admin's door", async () => {
  await page.goto("./");
  // Nadia administers Northwind and nothing else. Her Team screen offers
  // "Act as" beside each member she may borrow — the door every app has, since
  // `allow_impersonation` is on by default.
  await signIn(page, "nadia@northwind.example");
  // She belongs to two organisations and may land in either; only in Northwind
  // is she an admin, and the door is a permission of the *membership*.
  await standIn(page, /Northwind/);

  await page.getByRole("button", { name: /^Team$/i }).first().click();
  await expect(page.getByRole("heading", { name: /^Team$/i }).first()).toBeVisible();
  await expect(page.getByRole("listitem").filter({ hasText: "@northwind.example" }).first()).toBeVisible();
  await expect(page.getByRole("button", { name: /^Act as$/i }).first()).toBeVisible();
  await settle(page);
  await shoot(page, "admin-impersonation-team");

  // Sign out: the rest of the flow is the back office's, and a session left
  // signed in as Nadia would land the next test on her dashboard.
  await page.getByRole("button", { name: /Account menu/i }).click();
  await page.getByRole("menuitem", { name: /Sign out/i }).or(page.getByRole("button", { name: /Sign out/i })).first().click();
  await expect(page.getByLabel("Email")).toBeVisible();
});

test("the back office's door", async () => {
  await signIn(page, "root@example.com");

  // The other door: the Users screen lists every account in the deployment,
  // whichever organisation it belongs to, with Act as on each row Rae may
  // borrow — and none on her own. `docs/admin.md` photographs this screen for
  // what it lists; here it is the step before the session changes hands.
  await page.locator("nav").getByRole("button", { name: /^Users$/i }).click();
  await expect(page.getByRole("heading", { name: /^Users$/i }).first()).toBeVisible();
  await expect(page.getByRole("button", { name: /^Act as$/i }).first()).toBeVisible();
  await settle(page);
  await shoot(page, "admin-impersonation-users");

  // The same door from the other side: opening the account shows Act as this
  // user on the record, which is where a global admin lands after searching for
  // somebody rather than scrolling to them.
  await page.getByRole("button", { name: /nina@northwind\.example/ }).first().click();
  await expect(page.getByRole("button", { name: /Act as this user/i })).toBeVisible();
  await settle(page);
  await shoot(page, "admin-impersonation-user");
});

test("a borrowed account", async () => {
  // Nina belongs to Northwind, which Rae belongs to not at all — the reach
  // only a global admin has.
  await page.getByRole("button", { name: /Act as this user/i }).click();

  // The strip says whose account is in use; the header chip says it again for
  // the screens the strip does not reach. Waiting on both is waiting on the
  // session having actually changed.
  await expect(page.getByText(/You are working as /i)).toBeVisible();
  await expect(page.getByRole("button", { name: /Back to your account/i })).toBeVisible();
  await dismissToasts(page);
  await settle(page);
  await shoot(page, "admin-impersonation");
});

test("the way out", async () => {
  await page.getByRole("button", { name: /Back to your account/i }).click();

  // Back on Rae's own dashboard: the strip is gone, and the confirmation is
  // still up. Photographed before `dismissToasts`, because the toast saying
  // the session is her own again is the point of the picture.
  await expect(page.getByText(/You are working as /i)).toHaveCount(0);
  await expect(page.getByText(/You are yourself again/i)).toBeVisible();
  await page.waitForLoadState("networkidle");
  await page.mouse.move(10, 10);
  await shoot(page, "admin-impersonation-ended");
});
