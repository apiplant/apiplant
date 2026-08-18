/**
 * The glue the screenshot runs share: where a picture is written, and how to
 * get past the sign-in form.
 */

import { expect, type Page } from "@playwright/test";
import { join } from "node:path";
import { fileURLToPath } from "node:url";

/** Where the pictures land: the directory `docs/*.md` links them from. */
export const SHOTS_DIR = fileURLToPath(new URL("../../docs/images", import.meta.url));

/** Every seeded account in the examples uses this. */
export const SEED_PASSWORD = "password";

/**
 * Write one picture into `docs/images/`.
 *
 * Animations are frozen and the network is allowed to settle first, so a rerun
 * that changed nothing produces a byte-identical file instead of a diff made of
 * half-finished transitions.
 */
export async function shoot(page: Page, name: string, fullPage = false): Promise<void> {
  await page.waitForLoadState("networkidle");
  // Clicking deep in the sidebar scrolls it, and a picture that opens halfway
  // down the navigation reads as a different application on every page.
  await page.evaluate(() => {
    window.scrollTo(0, 0);
    for (const element of document.querySelectorAll("nav, aside, main")) element.scrollTop = 0;
  });
  await page.screenshot({
    path: join(SHOTS_DIR, `${name}.png`),
    fullPage,
    animations: "disabled",
    caret: "hide",
  });
}

/** Sign in through the form, and wait until the dashboard proper is up. */
export async function signIn(page: Page, email: string, password = SEED_PASSWORD): Promise<void> {
  await page.getByLabel("Email").fill(email);
  await page.getByLabel(/^Password/).fill(password);
  // "Sign in" is both the tab above the form and the form's own submit button.
  await page.locator("form").getByRole("button", { name: /^Sign in$/i }).click();
  await expect(page.getByRole("button", { name: "Home" })).toBeVisible();
  await page.waitForLoadState("networkidle");
  await dismissToasts(page);
}

/**
 * Wait out the toasts. Both apps report every success with one, and "Welcome
 * back." would otherwise sit in the corner of half the pictures.
 *
 * Waiting rather than clicking a close control: "Close" is also the studio's
 * header button for putting the app directory down, and a dismissal that
 * matched by name would end the session instead of the notification.
 */
export async function dismissToasts(page: Page): Promise<void> {
  await page.waitForTimeout(6_000);
}

/**
 * Make the named organisation the active one. The examples seed their rows into
 * one tenant while a fresh sign-in may land in another, and a list filtered to
 * the wrong workspace is an empty list.
 */
export async function switchOrganization(page: Page, name: string | RegExp): Promise<void> {
  const current = page.locator("header").getByRole("button", { name: /organization|Inc\.|Corporation/i }).first();
  await current.click();
  await page.getByRole("menuitem", { name }).or(page.getByRole("button", { name })).first().click();
  await page.waitForLoadState("networkidle");
  await dismissToasts(page);
}

/**
 * A list starts filtered to the signed-in operator's own records, which for a
 * seeded fixture means nothing at all. Widen it to the whole organisation.
 */
export async function showEverybody(page: Page): Promise<void> {
  const filter = page.getByRole("combobox", { name: /Ownership filter/i });
  if ((await filter.count()) === 0) return;
  await filter.selectOption({ label: "Everybody" });
  await page.waitForLoadState("networkidle");
}
