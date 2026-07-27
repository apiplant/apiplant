/**
 * The small amount of glue the story in `admin.spec.ts` needs: how to reach the
 * API directly, and how to read the session the dashboard is holding.
 *
 * Every claim the suite makes about the dashboard is checked twice — once as a
 * person sees it, and once against the API the same server publishes — because
 * a screen that says a note was saved proves nothing on its own.
 */

import { expect, request as playwrightRequest, type APIRequestContext, type Page } from "@playwright/test";
import { BASE_PATH, ORIGIN } from "../playwright.config";

export const API = `${ORIGIN}${BASE_PATH}`;

/** A unique account per run, so a suite re-run against a live database still
 *  starts from a clean identity even when the reset is skipped. */
export function uniqueEmail(prefix = "operator"): string {
  return `${prefix}+${Date.now().toString(36)}@example.test`;
}

export const PASSWORD = "correct-horse-battery-staple";

type SessionState = { token: string; apiKey: string; organizationId: string };

/** What the dashboard persisted in `localStorage` — the same token it is
 *  sending on every request. */
export async function storedSession(page: Page): Promise<SessionState> {
  const raw = await page.evaluate(() => localStorage.getItem("apiplant-admin-session"));
  expect(raw, "the dashboard should have persisted a session").not.toBeNull();
  return JSON.parse(raw!) as SessionState;
}

export type ApiOptions = {
  method?: "GET" | "POST" | "PATCH" | "DELETE";
  body?: unknown;
  token?: string;
  apiKey?: string;
  organizationId?: string;
};

/** One call to the app's REST API, with whichever credential the caller names. */
export async function api(
  request: APIRequestContext,
  path: string,
  options: ApiOptions = {},
): Promise<{ status: number; body: any }> {
  const headers: Record<string, string> = { "content-type": "application/json" };
  if (options.token) headers.authorization = `Bearer ${options.token}`;
  if (options.apiKey) headers["x-api-key"] = options.apiKey;
  if (options.organizationId) headers["X-Organization"] = options.organizationId;

  const response = await request.fetch(`${API}${path}`, {
    method: options.method ?? "GET",
    headers,
    data: options.body === undefined ? undefined : JSON.stringify(options.body),
  });

  const text = await response.text();
  let body: unknown = text;
  try {
    body = text ? JSON.parse(text) : null;
  } catch {
    /* a non-JSON body is itself the assertion material */
  }
  return { status: response.status(), body };
}

/** The manifest the server derives from the app on boot — the same document the
 *  dashboard renders itself from. */
export async function manifest(request: APIRequestContext): Promise<any> {
  const response = await request.fetch(`${ORIGIN}/admin/apiplant-admin.json`);
  expect(response.status(), "the app should publish an admin manifest").toBe(200);
  return response.json();
}

/** A request context with no cookies or storage of its own, for the checks that
 *  must be made as an anonymous caller. */
export async function anonymousRequest(): Promise<APIRequestContext> {
  return playwrightRequest.newContext();
}

/** The dashboard fetches as it renders; waiting on the network idling makes the
 *  assertions about what is on screen deterministic. */
export async function settle(page: Page): Promise<void> {
  await page.waitForLoadState("networkidle");
}

/** Records in a list are plain `<tr>`s that navigate on click. */
export function row(page: Page, text: string) {
  return page.locator("tbody tr").filter({ hasText: text });
}

/** One line of an action's result, which the dashboard renders as a definition
 *  list when the function returned a handful of scalars. */
export async function expectResult(page: Page, key: string, value: string | RegExp): Promise<void> {
  const entry = page.locator("dl > div").filter({ has: page.locator("dt", { hasText: new RegExp(`^${key}$`) }) });
  await expect(entry, `the result should have a "${key}" line`).toHaveCount(1);
  await expect(entry.locator("dd")).toHaveText(value);
}

/** A toast, which is how the dashboard reports every success. */
export async function expectToast(page: Page, text: string | RegExp): Promise<void> {
  await expect(page.getByText(text).first()).toBeVisible();
}
