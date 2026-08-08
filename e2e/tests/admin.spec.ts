/**
 * The framework, proved end to end through the interface a person actually uses.
 *
 * One database created empty, one example app (`examples/07-functions`) served
 * by the real binary, and one browser walking the whole arc the docs describe:
 * an account, an organisation, a resource created / edited / searched / deleted,
 * two functions with different visibilities run as actions, and an API key that
 * works against the same API afterwards.
 *
 * Every step is asserted twice — on screen, and against the REST API — so a
 * green run means the server did the thing, not merely that the dashboard said
 * so. The steps share one page and one account on purpose: this is a story,
 * and `serial` is what keeps it one.
 */

import { expect, test, type APIRequestContext, type Page } from "@playwright/test";
import {
  PASSWORD,
  api,
  anonymousRequest,
  expectResult,
  expectToast,
  manifest,
  row,
  settle,
  storedSession,
  uniqueEmail,
} from "./helpers";

test.describe.configure({ mode: "serial" });

const EMAIL = uniqueEmail();
const NOTE_TITLE = "First delivery run";
const NOTE_TITLE_EDITED = "First delivery run (revised)";
const SECOND_NOTE = "Depot inventory";

let page: Page;
let anonymous: APIRequestContext;
/** Filled in as the story goes, and used by the later steps. */
const state = { token: "", organizationId: "", noteId: "", apiKey: "" };

test.beforeAll(async ({ browser }) => {
  page = await browser.newPage();
  anonymous = await anonymousRequest();
});

test.afterAll(async () => {
  await page.close();
  await anonymous.dispose();
});

test("the app boots on an empty database and serves its dashboard", async () => {
  const health = await api(anonymous, "/_health");
  expect(health.status).toBe(200);
  expect(health.body.framework).toBe("apiplant");

  // Migrations ran from the resources alone: the resource's table answers before
  // anything has ever written to it.
  const notes = await api(anonymous, "/note");
  expect(notes.status, "note is a public resource").toBe(200);
  expect(Array.isArray(notes.body) ? notes.body : notes.body.data).toEqual([]);

  // The dashboard is described by the app itself, not by anything checked in
  // beside it: the manifest names the app, its resource and its two functions.
  const derived = await manifest(anonymous);
  // `[app] name` in main.toml, not the directory it lives in.
  expect(derived.app_name).toBe("07 · Functions");
  expect(derived.resources.map((entry: any) => entry.name)).toContain("note");
  expect(derived.functions.map((entry: any) => entry.name).sort()).toEqual(["greet", "stats"]);

  await page.goto("./");
  await expect(page.getByRole("heading", { name: "Sign in" })).toBeVisible();
  await expect(page.getByText(derived.app_name, { exact: false }).first()).toBeVisible();
});

test("registration creates an account and signs it in", async () => {
  await page.getByRole("button", { name: "Create account" }).first().click();
  await expect(page.getByRole("heading", { name: "Create your account" })).toBeVisible();

  await page.getByLabel("Email").fill(EMAIL);
  // Two boxes, because this form *sets* a password rather than checking one:
  // a typo in something nobody can read would otherwise surface at the next
  // sign-in. Anchored, so "Password" does not also match "Confirm password" —
  // and a regex rather than `exact`, since a required field's accessible name
  // carries the marker after it.
  await page.getByLabel(/^Password/).fill(PASSWORD);
  await page.getByLabel("Confirm password").fill(PASSWORD);
  await page.getByRole("button", { name: "Create account" }).last().click();

  // Nothing stands in the way: the account was given a personal organisation
  // as it was created, so registration lands straight in the dashboard.
  await expect(page.getByRole("button", { name: "Home" })).toBeVisible();

  const session = await storedSession(page);
  expect(session.token, "a session token was issued").toBeTruthy();
  state.token = session.token;

  // The account is real: the same credentials log in over the API.
  const login = await api(anonymous, "/auth/login", {
    method: "POST",
    body: { email: EMAIL, password: PASSWORD },
  });
  expect(login.status).toBe(200);
  expect(login.body.token).toBeTruthy();
});

test("every new account is given a personal organization it administers", async () => {
  await settle(page);

  const session = await storedSession(page);
  expect(session.organizationId, "the personal organization became the active one").toBeTruthy();
  state.organizationId = session.organizationId;

  const organizations = await api(anonymous, "/organization", { token: state.token });
  expect(organizations.status).toBe(200);
  const rows = Array.isArray(organizations.body) ? organizations.body : organizations.body.data;
  // Named after the address they signed up with, and theirs to rename.
  expect(rows).toHaveLength(1);
  expect(rows[0].name).toBe(EMAIL.split("@")[0]);

  // They are its admin — the membership the server stamped says so.
  const memberships = await api(anonymous, "/membership", {
    token: state.token,
    organizationId: state.organizationId,
  });
  const members = Array.isArray(memberships.body) ? memberships.body : memberships.body.data;
  expect(members.some((entry: any) => entry.role === "admin")).toBe(true);
});

test("the navigation offers exactly what the app declares", async () => {
  const nav = page.locator("nav");
  // A resource from resources/, and both functions as actions.
  await expect(nav.getByRole("button", { name: /Notes/i })).toBeVisible();
  await expect(nav.getByRole("button", { name: /^Greet$/i })).toBeVisible();
  await expect(nav.getByRole("button", { name: /^Stats$/i })).toBeVisible();

  // Auth resources get purpose-built screens instead of generic tables, so they
  // are not in the resource navigation.
  await expect(nav.getByRole("button", { name: /^Memberships$/i })).toHaveCount(0);
  await expect(nav.getByRole("button", { name: /^Api keys$/i })).toHaveCount(0);
});

test("a record can be created from the dashboard", async () => {
  await page.locator("nav").getByRole("button", { name: /Notes/i }).click();
  await expect(page.getByRole("heading", { name: "Notes" }).first()).toBeVisible();
  // An admin's list starts filtered to their own records, so the empty list
  // says so; without that filter it would be the plain "No notes yet".
  await expect(page.getByText(/No notes yet|Nothing matched those filters/i)).toBeVisible();

  await page.getByRole("button", { name: /New note/i }).first().click();
  await page.getByLabel("Title").fill(NOTE_TITLE);
  await page.getByLabel("Body").fill("Left the depot at 06:00.");
  await page.getByRole("button", { name: /Create note/i }).click();
  await expectToast(page, /Note created/i);
  await settle(page);

  const notes = await api(anonymous, "/note");
  const rows = Array.isArray(notes.body) ? notes.body : notes.body.data;
  expect(rows).toHaveLength(1);
  expect(rows[0].title).toBe(NOTE_TITLE);
  expect(rows[0].body).toBe("Left the depot at 06:00.");
  state.noteId = rows[0].id;
});

test("an edit made in the form reaches the database", async () => {
  await page.getByLabel("Title").fill(NOTE_TITLE_EDITED);
  await page.getByRole("button", { name: "Save changes" }).click();
  await expectToast(page, /Changes saved/i);

  const note = await api(anonymous, `/note/${state.noteId}`);
  expect(note.status).toBe(200);
  expect(note.body.title).toBe(NOTE_TITLE_EDITED);
});

test("the list shows records and its search filters them", async () => {
  await page.getByRole("button", { name: /All notes/i }).click();
  await expect(row(page, NOTE_TITLE_EDITED)).toBeVisible();

  // A second record, so a filter has something to exclude.
  await page.getByRole("button", { name: /New note/i }).first().click();
  await page.getByLabel("Title").fill(SECOND_NOTE);
  await page.getByRole("button", { name: /Create note/i }).click();
  await expectToast(page, /Note created/i);
  await page.getByRole("button", { name: /All notes/i }).click();
  await settle(page);

  await expect(page.locator("tbody tr")).toHaveCount(2);

  // The box filters on the resource's search fields through the API's
  // `?search=`, so part of a title finds it — in any case, and from the middle of a word.
  await page.getByPlaceholder(/Search by/i).fill("depot");
  await page.getByPlaceholder(/Search by/i).press("Enter");
  await settle(page);
  await expect(row(page, SECOND_NOTE)).toBeVisible();
  await expect(row(page, NOTE_TITLE_EDITED)).toHaveCount(0);

  // A term in neither title still matches nothing, rather than everything.
  await page.getByPlaceholder(/Search by/i).fill("warehouse");
  await page.getByPlaceholder(/Search by/i).press("Enter");
  await settle(page);
  await expect(page.getByText(/Nothing matched those filters/i)).toBeVisible();

  await page.getByPlaceholder(/Search by/i).fill("");
  await page.getByPlaceholder(/Search by/i).press("Enter");
  await settle(page);
  await expect(page.locator("tbody tr")).toHaveCount(2);
});

test("a public function runs as an action, with a form derived from its input", async () => {
  await page.locator("nav").getByRole("button", { name: /^Greet$/i }).click();
  await expect(page.getByRole("heading", { name: "Greet" })).toBeVisible();
  // The doc comment on the handler's Input field is the help text.
  await expect(page.getByText(/Who to greet/i)).toBeVisible();

  await page.getByLabel("Name").fill("Ada");
  await page.getByRole("button", { name: /^Greet$/ }).last().click();

  // `functions/greet.toml` sets the greeting, so config reached the function.
  await expect(page.getByText("Buongiorno, Ada!")).toBeVisible();
  // And it queried the database for real: one account exists.
  await expectResult(page, "Message", "Buongiorno, Ada!");
  await expectResult(page, "Registered users", "1");

  // Public visibility means anonymous callers may run it too.
  const anonymousRun = await api(anonymous, "/functions/greet", {
    method: "POST",
    body: { name: "Grace" },
  });
  expect(anonymousRun.status).toBe(200);
  expect(anonymousRun.body.message).toBe("Buongiorno, Grace!");
});

test("an authenticated function counts what the dashboard just created", async () => {
  await page.locator("nav").getByRole("button", { name: /^Stats$/i }).click();
  await expect(page.getByText(/Nothing to fill in/i)).toBeVisible();
  await page.getByRole("button", { name: /^Stats$/ }).last().click();

  // Two notes were made above; the function reads the same rows.
  await expectResult(page, "Notes", "2");
  await expectResult(page, "Users", "1");
  // `Authenticated` visibility means the caller is known to the handler.
  await expectResult(page, "Asked by", /^[0-9a-f-]{36}$/);

  // Its declared visibility is enforced by the server, not the dashboard.
  const denied = await api(anonymous, "/functions/stats");
  expect(denied.status).toBe(401);
});

test("an API key issued in the dashboard authenticates against the API", async () => {
  await page.getByRole("button", { name: "Account menu" }).click();
  await page.getByRole("button", { name: "API keys", exact: true }).click();
  await expect(page.getByRole("heading", { name: "API keys" })).toBeVisible();

  await page.getByRole("button", { name: "New key" }).click();
  await page.getByLabel("Name").fill("End-to-end run");
  await page.getByRole("button", { name: "Create key" }).click();

  await expect(page.getByRole("heading", { name: /Copy your key now/i })).toBeVisible();
  const secret = (await page.locator("code").first().innerText()).trim();
  expect(secret.length).toBeGreaterThan(16);
  state.apiKey = secret;
  await page.getByRole("button", { name: "Done" }).click();

  await expect(page.getByText("End-to-end run")).toBeVisible();

  // The key acts as its owner: it may call the function anonymous callers cannot.
  const asKey = await api(anonymous, "/functions/stats", { apiKey: state.apiKey });
  expect(asKey.status).toBe(200);
  expect(asKey.body.notes).toBe(2);

  // And a key that was never issued does not.
  const forged = await api(anonymous, "/functions/stats", { apiKey: "not-a-real-key" });
  expect(forged.status).toBe(401);
});

test("a record deleted in the dashboard is gone from the API", async () => {
  await page.locator("nav").getByRole("button", { name: /Notes/i }).click();
  await row(page, NOTE_TITLE_EDITED).click();
  await expect(page.getByRole("button", { name: "Save changes" })).toBeVisible();

  await page.getByRole("button", { name: "Delete" }).click();
  await page.getByRole("button", { name: /^Delete$/ }).last().click();
  await expectToast(page, /Note deleted/i);
  await settle(page);

  await expect(row(page, NOTE_TITLE_EDITED)).toHaveCount(0);
  await expect(row(page, SECOND_NOTE)).toBeVisible();

  const gone = await api(anonymous, `/note/${state.noteId}`);
  expect(gone.status).toBe(404);
});

test("signing out ends the session and the way back in still works", async () => {
  await page.getByRole("button", { name: "Account menu" }).click();
  await page.getByRole("button", { name: "Sign out", exact: true }).click();
  await expect(page.getByRole("heading", { name: "Sign in" })).toBeVisible();

  const stored = await page.evaluate(() => localStorage.getItem("apiplant-admin-session"));
  expect(stored === null || !JSON.parse(stored).token).toBeTruthy();

  await page.getByLabel("Email").fill(EMAIL);
  await page.getByLabel("Password").fill(PASSWORD);
  await page.getByRole("button", { name: "Sign in" }).last().click();

  // Straight back to the work, with the organisation still there.
  await expect(page.getByRole("button", { name: "Home" })).toBeVisible();
  await page.locator("nav").getByRole("button", { name: /Notes/i }).click();
  await expect(row(page, SECOND_NOTE)).toBeVisible();
});
