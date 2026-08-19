/**
 * The AI pictures in `docs/ai.md` and `docs/admin.md`, taken from a real
 * dashboard talking to a real model.
 *
 * `examples/19-ai` is the example that configures one: a `[ai]` provider, a
 * stored agent with a tool, an action that wraps the model, and
 * `[admin.ai_assistance]` on the forms. The server the run points at must be
 * reachable on the model's own endpoint, or every reply in the pictures is an
 * error box.
 *
 *   cd e2e && pnpm shots:ai
 */

import { expect, test, type Page } from "@playwright/test";
import { dismissToasts, shoot, shootStable, signIn, SEED_PASSWORD } from "./helpers";

/**
 * Let a route change finish crossing over. The screens fade into each other,
 * and a picture taken mid-fade shows both of them at half opacity.
 */
async function settle(page: Page): Promise<void> {
  await page.waitForTimeout(700);
}

test.describe.configure({ mode: "serial", timeout: 300_000 });

let page: Page;

test.beforeAll(async ({ browser }) => {
  page = await browser.newPage();
});

test.afterAll(async () => {
  await page.close();
});

/**
 * The agent screen with one real exchange in it: the user's turn, the tool
 * call the agent made to answer, and the answer. A fresh thread rather than a
 * seeded one, so the picture shows the screen being used.
 */
test("an agent's conversation", async () => {
  await page.goto("./");
  await signIn(page, "admin@example.com", SEED_PASSWORD);

  await page.locator("nav").getByRole("button", { name: /^Coach$/i }).click();
  await expect(page.getByRole("heading", { name: /^Coach$/i }).first()).toBeVisible();
  await settle(page);

  // Start from an empty history: an earlier run's threads would sit in the
  // sidebar of the picture, and the delete is confirmed in a dialog. Wait for
  // the list to have arrived — either state — before counting it.
  await expect(
    page.getByRole("button", { name: /^Delete /i }).first().or(page.getByText(/No matching threads/i))
  ).toBeVisible();
  let guard = 0;
  while ((await page.getByRole("button", { name: /^Delete /i }).count()) > 0 && guard++ < 10) {
    await page.getByRole("button", { name: /^Delete /i }).first().click();
    const confirm = page.getByRole("button", { name: /^Delete$/i }).last();
    await expect(confirm).toBeVisible();
    await confirm.click();
    await page.waitForTimeout(500);
  }
  await expect(page.getByRole("button", { name: /^Delete /i })).toHaveCount(0);

  await page.getByRole("button", { name: /New/i }).first().click();
  await page.getByPlaceholder(/Message Coach/i).fill("What notes do I have?");
  await page.getByRole("button", { name: /Send message/i }).click();

  // The reply streams; the reasoning toggle appears with it, and waiting on it
  // waits on the whole answer rather than on the first token.
  await expect(page.getByRole("button", { name: /Show reasoning/i })).toBeVisible({ timeout: 240_000 });
  await dismissToasts(page);
  await page.mouse.move(10, 10);
  // The stored copy of the reply repaints a beat after the stream ends, and a
  // frame caught mid-repaint holds the text twice at half opacity.
  await shootStable(page, "admin-ai-agent");
});

test("the reasoning behind a reply", async () => {
  // The toggle sits under the answer; expanding it is the whole point of
  // `thinking` — the trace is there, kept out of the answer.
  await page.getByRole("button", { name: /Show reasoning/i }).click();
  await page.waitForTimeout(500);
  await page.mouse.move(10, 10);
  await shootStable(page, "admin-ai-agent-reasoning");
});

test("assistance on a form field", async () => {
  // The spark sits beside any writable field while the pointer is near it,
  // and the picture is of the prompt box it opens: the part of the feature
  // that does not exist without an `[ai]` provider.
  await page.locator("nav").getByRole("button", { name: /^Notes$/i }).click();
  await expect(page.getByRole("heading", { name: /^Notes$/i }).first()).toBeVisible();
  await settle(page);
  await page.locator("tbody tr").first().click();
  await expect(page.getByRole("button", { name: /Save changes/i })).toBeVisible();
  await settle(page);

  await page.locator("input.input").first().hover();
  await page.getByRole("button", { name: /Fill .* with AI/i }).click({ force: true });
  await expect(page.getByText(/Describe what should be written/i)).toBeVisible();
  await page
    .getByRole("textbox", { name: /Describe what you want AI to write/i })
    .fill("Write a one-line standup update: the importer shipped, the dashboard needs one more day.");
  await page.mouse.move(10, 10);
  await shoot(page, "admin-ai-assist");
});

test("an action that streams its answer", async () => {
  // `ask` wraps the model: it pulls the caller's notes into the prompt, fixes
  // the instructions, and hands every token to the dashboard on the way. The
  // picture is of both panes — the live output and the final result — because
  // either one alone shows only half of what the screen does.
  await page.locator("nav").getByRole("button", { name: /^Ask$/i }).click();
  await expect(page.getByRole("heading", { name: /^Ask$/i }).first()).toBeVisible();
  await settle(page);

  await page.locator("input.input").first().fill("Summarise my notes in one sentence.");
  await page.getByRole("button", { name: /^Ask$/i }).last().click();

  // The result table renders when the stream has closed.
  await expect(page.getByText(/FINAL RESULT/i)).toBeVisible({ timeout: 240_000 });
  await expect(page.getByText(/Context notes/i)).toBeVisible();
  await page.waitForLoadState("networkidle");
  await page.mouse.move(10, 10);
  await shoot(page, "admin-ai-action");
});
