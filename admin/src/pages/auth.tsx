/**
 * The way in: sign in, or create an account.
 *
 * One card, two tabs, no jargon. Extra profile fields the `user` model marks
 * required are collected as real inputs — nobody should have to type JSON to
 * make an account, which is what the previous version asked for.
 */

import { For, Show, createMemo, createSignal } from "solid-js";
import { createMutable } from "solid-js/store";
import { Button, Card, Field, HeadMark, ThemeToggle } from "../ui";
import { FieldEditor, buildPayload, createDraft } from "../fields";
import type { Draft } from "../fields";
import {
  api,
  asRecord,
  manifest,
  navigate,
  notify,
  persistSession,
  refreshSession,
  session,
} from "../store";
import type { FieldManifest, ResourceManifest } from "../types";

/** Sign-up shares the resource form machinery, which wants a resource. */
function signupPseudoResource(fields: FieldManifest[]): ResourceManifest {
  return {
    name: "user",
    label: "Account",
    plural: "Accounts",
    group: null,
    order: 0,
    builtin: true,
    auth_resource: true,
    visible: false,
    roles: [],
    scope: "global",
    owner_field: "id",
    display_field: null,
    search_field: null,
    columns: [],
    fields,
    relations: [],
    children: [],
    permissions: {
      list: { value: "public", role: null, note: "", requires_org: false },
      read: { value: "public", role: null, note: "", requires_org: false },
      create: { value: "public", role: null, note: "", requires_org: false },
      update: { value: "public", role: null, note: "", requires_org: false },
      delete: { value: "public", role: null, note: "", requires_org: false },
    },
  };
}

export function AuthPage() {
  const current = () => manifest()!;
  const [mode, setMode] = createSignal<"signin" | "signup">("signin");
  const [identity, setIdentity] = createSignal("");
  const [password, setPassword] = createSignal("");
  const [busy, setBusy] = createSignal(false);
  const [error, setError] = createSignal<string | null>(null);

  const extras = createMemo(() => signupPseudoResource(current().auth.signup_fields));
  const extraDraft = createMutable<Draft>(createDraft(extras(), null));

  const submit = async (event: Event) => {
    event.preventDefault();
    if (busy()) return;
    setError(null);

    const identityValue = identity().trim();
    if (!identityValue || !password()) {
      setError(`Enter your ${current().auth.identity_label.toLowerCase()} and password.`);
      return;
    }

    let body: Record<string, unknown> = {
      [current().auth.identity_field]: identityValue,
      password: password(),
    };

    if (mode() === "signup" && current().auth.signup_fields.length) {
      const { payload, errors } = buildPayload(extras(), extraDraft);
      if (errors.length) {
        setError(errors[0].message);
        return;
      }
      body = { ...payload, ...body };
    }

    setBusy(true);
    try {
      const response = asRecord(
        await api(mode() === "signin" ? "/auth/login" : "/auth/register", {
          method: "POST",
          body,
        }),
      );
      const token = typeof response?.token === "string" ? response.token : "";
      if (!token) throw new Error("The server did not return a session.");

      session.token = token;
      session.apiKey = "";
      session.userId = decodeSubject(token);
      persistSession();
      setPassword("");
      await refreshSession();
      navigate({ kind: "dashboard" });
      notify("success", mode() === "signin" ? "Welcome back." : "Your account is ready.");
    } catch (failure) {
      setError(failure instanceof Error ? failure.message : String(failure));
    } finally {
      setBusy(false);
    }
  };

  const tabClass = (active: boolean) =>
    [
      "flex-1 rounded-lg px-3 py-2 text-[0.8125rem] font-medium transition-colors",
      active ? "bg-surface text-ink shadow-sm" : "text-muted hover:text-ink",
    ].join(" ");

  return (
    <div class="relative z-10 flex min-h-screen flex-col">
      <header class="flex items-center justify-between px-5 py-4">
        <div class="flex items-center gap-2">
          <HeadMark class="h-7" src={current().logo} />
          <span class="text-sm font-semibold tracking-tight">
            {current().app_name} <span class="text-accent">admin</span>
          </span>
        </div>
        <ThemeToggle />
      </header>

      <main class="flex flex-1 items-center justify-center px-4 pb-16">
        <section class="w-full max-w-sm">
          <div class="mb-6 text-center">
            <h1 class="text-2xl font-semibold tracking-tight text-ink">
              {mode() === "signin" ? "Sign in" : "Create your account"}
            </h1>
            <p class="mt-1.5 text-sm text-muted">
              {mode() === "signin"
                ? `Manage ${current().app_name}.`
                : `Get access to ${current().app_name}.`}
            </p>
          </div>

          <Card class="border-line-strong/70">
            <Show when={current().auth.allow_registration}>
              <div class="flex gap-1 border-b border-line bg-surface-2/50 p-1.5">
                <button type="button" class={tabClass(mode() === "signin")} onClick={() => setMode("signin")}>
                  Sign in
                </button>
                <button type="button" class={tabClass(mode() === "signup")} onClick={() => setMode("signup")}>
                  Create account
                </button>
              </div>
            </Show>

            <form class="space-y-4 px-5 py-5" onSubmit={submit}>
              <Field label={current().auth.identity_label} required>
                <input
                  class="input"
                  type={current().auth.identity_field === "email" ? "email" : "text"}
                  autocomplete="username"
                  value={identity()}
                  onInput={(event) => setIdentity(event.currentTarget.value)}
                />
              </Field>

              <Field label="Password" required>
                <input
                  class="input"
                  type="password"
                  autocomplete={mode() === "signin" ? "current-password" : "new-password"}
                  value={password()}
                  onInput={(event) => setPassword(event.currentTarget.value)}
                />
              </Field>

              <Show when={mode() === "signup"}>
                <For each={current().auth.signup_fields}>
                  {(field) => <FieldEditor field={field} draft={extraDraft} />}
                </For>
              </Show>

              <Show when={error()}>
                <p class="rounded-lg border border-danger-line bg-danger-soft px-3 py-2 text-[0.8125rem] text-ink">
                  {error()}
                </p>
              </Show>

              <Button type="submit" variant="primary" size="lg" class="w-full" loading={busy()}>
                {mode() === "signin" ? "Sign in" : "Create account"}
              </Button>
            </form>
          </Card>

          <Show when={!current().auth.allow_registration}>
            <p class="mt-4 text-center text-xs leading-relaxed text-faint">
              New accounts are created by an administrator. Ask someone on your team to add you.
            </p>
          </Show>
        </section>
      </main>
    </div>
  );
}

function decodeSubject(token: string): string | null {
  try {
    const payload = token.split(".")[1];
    if (!payload) return null;
    const normalised = payload.replaceAll("-", "+").replaceAll("_", "/");
    const claims = JSON.parse(atob(normalised.padEnd(Math.ceil(normalised.length / 4) * 4, "=")));
    return typeof claims.sub === "string" ? claims.sub : null;
  } catch {
    return null;
  }
}
