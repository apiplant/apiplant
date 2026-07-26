/**
 * The first thing after signing in, when there is nowhere to work yet.
 *
 * Almost every resource is scoped to an organisation. Someone who belongs to
 * none sees empty tables and gets a 400 from every write, which reads as a
 * broken dashboard rather than as an account that is not finished — so this
 * stands in front of the whole interface until it is resolved.
 *
 * There are two ways it resolves, and which one is offered is the app's
 * decision, not ours: if the `organization` resource lets an authenticated
 * caller create one, this is a form (and whoever creates it becomes its admin).
 * If the app provisions tenants itself, there is nothing useful to offer, so it
 * says who to ask and waits.
 */

import { For, Show, createMemo, createSignal } from "solid-js";
import { createMutable } from "solid-js/store";
import { Button, Card, CardHeader, Field, HeadMark, ThemeToggle } from "../ui";
import { FieldEditor, buildPayload, createDraft } from "../fields";
import type { Draft, DraftError } from "../fields";
import {
  api,
  asRecord,
  currentUserLabel,
  manifest,
  mayCreateOrganization,
  notify,
  refreshSession,
  resourceByName,
  session,
  setActiveOrganization,
  signOut,
} from "../store";
import type { FieldManifest } from "../types";

/** The organisation's own fields, minus the two this form spells out itself. */
function extraFields(): FieldManifest[] {
  const resource = resourceByName("organization");
  if (!resource) return [];
  return resource.fields.filter(
    (field) =>
      field.name !== "name" &&
      field.name !== "slug" &&
      field.writable &&
      field.admin_visible &&
      !field.hidden &&
      !field.readonly,
  );
}

/** `Acme Logistics` → `acme-logistics`, as a starting point someone can edit. */
function slugify(name: string): string {
  return name
    .toLowerCase()
    .replace(/[^a-z0-9]+/g, "-")
    .replace(/^-+|-+$/g, "")
    .slice(0, 48);
}

export function OnboardingPage() {
  return (
    <div class="relative z-10 flex min-h-screen flex-col">
      <header class="flex items-center justify-between px-5 py-4">
        <div class="flex items-center gap-2.5">
          <HeadMark class="h-7 w-7" src={manifest()?.logo} />
          <span class="text-sm font-semibold text-ink">{manifest()?.app_name}</span>
        </div>
        <div class="flex items-center gap-2">
          <span class="hidden text-xs text-faint sm:inline">{currentUserLabel()}</span>
          <ThemeToggle />
          <Button variant="ghost" onClick={signOut}>
            Sign out
          </Button>
        </div>
      </header>

      <main class="flex flex-1 items-start justify-center px-5 py-8 sm:items-center sm:py-12">
        <div class="w-full max-w-xl">
          <Show when={mayCreateOrganization()} fallback={<AskAnAdmin />}>
            <CreateOrganization />
          </Show>
        </div>
      </main>
    </div>
  );
}

function CreateOrganization() {
  const resource = createMemo(() => resourceByName("organization"));
  const extras = createMemo(extraFields);
  const draft = createMutable<Draft>(
    resource() ? createDraft(resource()!, null) : ({} as Draft),
  );

  const [name, setName] = createSignal("");
  // The slug follows the name until someone types their own, at which point it
  // stops moving under them.
  const [slug, setSlug] = createSignal("");
  const [slugTouched, setSlugTouched] = createSignal(false);
  const [busy, setBusy] = createSignal(false);
  const [error, setError] = createSignal<string | null>(null);
  const [fieldErrors, setFieldErrors] = createSignal<DraftError[]>([]);

  const effectiveSlug = () => (slugTouched() ? slug() : slugify(name()));
  const errorFor = (field: string) =>
    fieldErrors().find((entry) => entry.field === field)?.message ?? null;

  const submit = async (event: Event) => {
    event.preventDefault();
    if (busy()) return;
    setError(null);
    setFieldErrors([]);

    const current = resource();
    if (!current) return setError("This app has no organization resource.");
    if (!name().trim()) return setError("Give it a name.");

    // The two fields the form spells out live in the same draft as the app's
    // own, so validation and typing happen in one place.
    draft.name = name().trim();
    if ("slug" in draft) draft.slug = effectiveSlug();

    const { payload, errors } = buildPayload(current, draft);
    if (errors.length) {
      setFieldErrors(errors);
      return;
    }

    setBusy(true);
    try {
      const created = asRecord(await api("/organization", { method: "POST", body: payload }));
      // The server makes the creator an admin member, so the session has to be
      // re-read before anything will look right.
      await refreshSession();
      if (created?.id) await setActiveOrganization(String(created.id));
      notify("success", `${payload.name} is ready.`);
    } catch (failure) {
      setError(failure instanceof Error ? failure.message : String(failure));
    } finally {
      setBusy(false);
    }
  };

  return (
    <form onSubmit={(event) => void submit(event)}>
      <Card>
        <CardHeader
          title="Create your organization"
          hint="You will be its admin. Everything you create afterwards belongs to it."
        />
        <div class="space-y-4 px-5 py-5">
          <p class="text-[0.8125rem] text-faint">
            An organization is the workspace this app's records live in. You need one before
            there is anywhere to put anything.
          </p>

          <Field label="Name" required error={errorFor("name")}>
            <input
              class="input"
              autofocus
              placeholder="Acme Logistics"
              value={name()}
              onInput={(event) => setName(event.currentTarget.value)}
            />
          </Field>

          <Show when={"slug" in draft}>
            <Field
              label="Short name"
              help="A short, unique handle used in links."
              error={errorFor("slug")}
            >
              <input
                class="input"
                placeholder={slugify(name()) || "acme-logistics"}
                value={effectiveSlug()}
                onInput={(event) => {
                  setSlugTouched(true);
                  setSlug(event.currentTarget.value);
                }}
              />
            </Field>
          </Show>

          <For each={extras()}>
            {(field) => (
              <FieldEditor field={field} draft={draft} error={errorFor(field.name)} />
            )}
          </For>

          <Show when={error()}>
            <p class="rounded-lg border border-danger-line bg-danger-soft px-3 py-2 text-[0.8125rem] text-ink">
              {error()}
            </p>
          </Show>

          <Button type="submit" variant="primary" loading={busy()} class="w-full">
            Create organization
          </Button>
        </div>
      </Card>
    </form>
  );
}

function AskAnAdmin() {
  const [checking, setChecking] = createSignal(false);

  const recheck = async () => {
    setChecking(true);
    try {
      await refreshSession();
      // Landing back here means nothing changed; saying so beats a screen that
      // flickers and looks like it ignored the press.
      if (session.organizations.length === 0) {
        notify("info", "Still no organization on your account.");
      }
    } finally {
      setChecking(false);
    }
  };

  return (
    <Card>
      <CardHeader title="You are not in an organization yet" />
      <div class="space-y-4 px-5 py-5 text-[0.8125rem] text-faint">
        <p>
          Your account is set up, but it does not belong to a workspace — and almost everything
          here lives inside one. This app does not let members start their own, so an
          administrator needs to add you.
        </p>
        <p>
          Ask someone who already administers an organization to invite{" "}
          <span class="font-medium text-ink">{currentUserLabel()}</span> from their Team screen.
          Once they have, sign in again or check now.
        </p>
        <div class="flex gap-2">
          <Button variant="primary" loading={checking()} onClick={() => void recheck()}>
            Check again
          </Button>
          <Button variant="ghost" onClick={signOut}>
            Sign out
          </Button>
        </div>
      </div>
    </Card>
  );
}
