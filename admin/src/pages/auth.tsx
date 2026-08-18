/**
 * The way in: sign in, or create an account.
 *
 * One card with two tabs. Extra profile fields the `user` resource marks required
 * are collected as ordinary inputs, so creating an account never requires
 * entering JSON.
 */

import { For, Show, createEffect, createMemo, createSignal } from "solid-js";
import { Button, Card, Dialog, Field, HeadMark, ThemeToggle } from "../ui";
import { FieldEditor, buildPayload, createDraft, createDraftStore } from "../fields";
import { PasswordFields, createPasswordPair } from "../password";
import {
  api,
  asRecord,
  manifest,
  notify,
  oauthAvailable,
  persistSession,
  refreshSession,
  startOAuth,
  syncRouteFromHash,
  updateSession,
} from "../store";
import { ProviderMark } from "../brand-icons";
import type { FieldManifest, OAuthProviderManifest, ResourceManifest } from "../types";

/**
 * Sign-up shares the resource form machinery, which wants a resource.
 *
 * Exported because accepting an invitation is also a sign-up, collecting the
 * same extra fields from the same manifest, and two separate copies of this
 * shape would drift the first time somebody added a field to it.
 */
export function signupResource(fields: FieldManifest[]): ResourceManifest {
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
    search_fields: [],
    columns: [],
    fields,
    relations: [],
    children: [],
    permissions: {
      list: { value: "public", role: null, org_class: null, note: "", requires_org: false, rules: [] },
      read: { value: "public", role: null, org_class: null, note: "", requires_org: false, rules: [] },
      create: { value: "public", role: null, org_class: null, note: "", requires_org: false, rules: [] },
      update: { value: "public", role: null, org_class: null, note: "", requires_org: false, rules: [] },
      delete: { value: "public", role: null, org_class: null, note: "", requires_org: false, rules: [] },
    },
  };
}

/**
 * The provider buttons, above the form.
 *
 * Above, because for anybody who has an account through one of them it is the
 * whole screen — a password field they will never fill in should not be the
 * first thing they read. The order is the order `[oauth]` names them in, so an
 * app decides which one goes first by writing it first.
 *
 * Each button is a `<button>` and not an `<a>` even though the endpoint answers
 * with a redirect, because the URL needs `return_to` and `token_delivery`
 * appended for *this* page — see `startOAuth`.
 */
function ProviderButtons(props: { providers: OAuthProviderManifest[]; verb: string }) {
  return (
    <div class="space-y-2.5 px-5 pt-5">
      <For each={props.providers}>
        {(provider) => (
          <button
            type="button"
            class="flex w-full items-center gap-2.5 rounded-lg border border-line-strong bg-surface px-3.5 py-2.5 text-[0.8125rem] font-medium text-ink transition-colors hover:bg-surface-2"
            onClick={() => startOAuth(provider)}
          >
            <ProviderMark provider={provider.provider} label={provider.label} icon={provider.icon} />
            <span>
              {props.verb} with {provider.label}
            </span>
            {/* Said before the button is pressed rather than explained after
                an account turns up with an address nobody can write to. */}
            <Show when={!provider.provides_email}>
              <span class="ml-auto text-[0.6875rem] font-normal text-faint">no email</span>
            </Show>
          </button>
        )}
      </For>

      <div class="flex items-center gap-3 pt-1.5 text-[0.6875rem] uppercase tracking-wide text-faint">
        <span class="h-px flex-1 bg-line" />
        or
        <span class="h-px flex-1 bg-line" />
      </div>
    </div>
  );
}

export function AuthPage() {
  const current = () => manifest()!;
  const [mode, setMode] = createSignal<"signin" | "signup">("signin");
  const [identity, setIdentity] = createSignal("");
  const [password, setPassword] = createSignal("");
  const [busy, setBusy] = createSignal(false);
  const [error, setError] = createSignal<string | null>(null);
  /** A one-off note above the form: "check your email", and the like. */
  const [notice, setNotice] = createSignal<string | null>(null);
  /**
   * Shown after a login was refused for an unconfirmed address. It is the only
   * moment we know an address needs confirming *and* who it belongs to, so the
   * resend button belongs here rather than on a screen nobody would find.
   */
  const [unconfirmed, setUnconfirmed] = createSignal(false);
  const [forgotting, setForgotting] = createSignal(false);

  // Two boxes when choosing a password, one when typing a known one.
  const passwords = createPasswordPair();

  /**
   * Offered only where they can work: a console built by `apiplant admin` and
   * hosted on another origin has nowhere for the callback to land, so it shows
   * the password form alone rather than a button that would strand somebody on
   * an API URL. See `oauthAvailable`.
   */
  const providers = createMemo(() =>
    oauthAvailable() ? (current().auth.oauth_providers ?? []) : [],
  );

  const extras = createMemo(() => signupResource(current().auth.signup_fields));
  const extraDraft = createDraftStore(createDraft(extras(), null));

  const switchMode = (next: "signin" | "signup") => {
    setMode(next);
    setError(null);
    setNotice(null);
    setUnconfirmed(false);
  };

  const submit = async (event: Event) => {
    event.preventDefault();
    if (busy()) return;
    setError(null);
    setNotice(null);
    setUnconfirmed(false);

    const identityValue = identity().trim();
    const registering = mode() === "signup";
    const secret = registering ? passwords.value() : password();

    if (!identityValue) {
      setError(`Enter your ${current().auth.identity_label.toLowerCase()}.`);
      return;
    }
    if (!secret) {
      setError(
        registering
          ? (passwords.error() ?? "Choose a password, and type it twice.")
          : "Enter your password.",
      );
      return;
    }

    let body: Record<string, unknown> = {
      [current().auth.identity_field]: identityValue,
      password: secret,
    };

    if (registering && current().auth.signup_fields.length) {
      const { payload, errors } = buildPayload(extras(), extraDraft.values);
      if (errors.length) {
        setError(errors[0].message);
        return;
      }
      body = { ...payload, ...body };
    }

    setBusy(true);
    try {
      const response = asRecord(
        await api(registering ? "/auth/register" : "/auth/login", { method: "POST", body }),
      );
      const token = typeof response?.token === "string" ? response.token : "";

      // Registering into an app that confirms addresses does not produce a
      // session, since the account is not yet usable. Reporting that is better
      // than issuing a token the next request would reject.
      if (!token && registering && response?.verification_required) {
        passwords.reset();
        switchMode("signin");
        setNotice(
          `Open the link sent to ${identityValue} to confirm your address, then sign in.`,
        );
        return;
      }
      if (!token) throw new Error("The server did not return a session.");

      updateSession({ token, apiKey: "", userId: decodeSubject(token) });
      persistSession();
      setPassword("");
      passwords.reset();
      await refreshSession();
      // Adopt the address bar rather than jumping to the dashboard: someone who
      // followed a link here, such as a console handoff or a shared record,
      // had a destination, and signing in should not discard it.
      syncRouteFromHash();
      notify("success", registering ? "Your account is ready." : "Welcome back.");
    } catch (failure) {
      const status = (failure as { status?: number }).status;
      const message = failure instanceof Error ? failure.message : String(failure);
      // A 403 on a login with the correct password means the address is not
      // confirmed, which is the one failure the user can act on.
      if (!registering && status === 403 && /confirm/i.test(message)) {
        setUnconfirmed(true);
      }
      setError(message);
    } finally {
      setBusy(false);
    }
  };

  /** Request the confirmation email again. Always reports success, since the
   *  endpoint returns 202 whether or not there was anything to send. */
  const resendConfirmation = async () => {
    setBusy(true);
    try {
      await api("/auth/verify-email/resend", {
        method: "POST",
        body: { email: identity().trim() },
      });
      setUnconfirmed(false);
      setError(null);
      setNotice("If that address still needs confirming, a new link is on its way.");
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
                <button type="button" class={tabClass(mode() === "signin")} onClick={() => switchMode("signin")}>
                  Sign in
                </button>
                <button type="button" class={tabClass(mode() === "signup")} onClick={() => switchMode("signup")}>
                  Create account
                </button>
              </div>
            </Show>

            <Show when={providers().length}>
              <ProviderButtons
                providers={providers()}
                verb={mode() === "signin" ? "Sign in" : "Sign up"}
              />
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

              <Show
                when={mode() === "signup"}
                fallback={
                  <Field label="Password" required>
                    <input
                      class="input"
                      type="password"
                      autocomplete="current-password"
                      value={password()}
                      onInput={(event) => setPassword(event.currentTarget.value)}
                    />
                  </Field>
                }
              >
                <PasswordFields
                  pair={passwords}
                  help={
                    current().auth.require_email_verification
                      ? "You'll confirm your address by email before signing in."
                      : undefined
                  }
                />
                <For each={current().auth.signup_fields}>
                  {(field) => <FieldEditor field={field} draft={extraDraft} />}
                </For>
              </Show>

              <Show when={notice()}>
                <p class="rounded-lg border border-line bg-surface-2/60 px-3 py-2 text-[0.8125rem] text-ink">
                  {notice()}
                </p>
              </Show>

              <Show when={error()}>
                <p class="rounded-lg border border-danger-line bg-danger-soft px-3 py-2 text-[0.8125rem] text-ink">
                  {error()}
                </p>
              </Show>

              {/* Only offered where it can actually help: an unconfirmed
                  address, and a server that can send the message again. */}
              <Show when={unconfirmed() && current().auth.require_email_verification}>
                <Button
                  type="button"
                  variant="ghost"
                  class="w-full"
                  loading={busy()}
                  onClick={() => void resendConfirmation()}
                >
                  Send the confirmation link again
                </Button>
              </Show>

              <Button type="submit" variant="primary" size="lg" class="w-full" loading={busy()}>
                {mode() === "signin" ? "Sign in" : "Create account"}
              </Button>

              <Show when={mode() === "signin" && current().auth.password_reset_enabled}>
                <button
                  type="button"
                  class="w-full text-center text-xs text-muted transition-colors hover:text-ink"
                  onClick={() => setForgotting(true)}
                >
                  Forgot your password?
                </button>
              </Show>
            </form>
          </Card>

          <ForgotPasswordDialog
            open={forgotting()}
            identity={identity()}
            onClose={() => setForgotting(false)}
          />

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

/**
 * Ask for a reset link.
 *
 * The response is identical in all cases, since the endpoint always accepts so
 * that it cannot be used to discover which addresses have accounts. This screen
 * therefore closes with wording that is accurate either way rather than
 * implying a confirmation it did not receive.
 */
function ForgotPasswordDialog(props: { open: boolean; identity: string; onClose: () => void }) {
  const [address, setAddress] = createSignal("");
  const [busy, setBusy] = createSignal(false);
  const [error, setError] = createSignal<string | null>(null);
  const label = () => manifest()?.auth.identity_label ?? "Email";

  // Whatever they had already typed into the sign-in box is almost certainly
  // the address they want, so start there.
  createEffect(
    () => [props.open, props.identity] as const,
    ([open, identity]) => {
      if (open) {
        setAddress(identity);
        setError(null);
      }
    },
  );

  const send = async () => {
    const value = address().trim();
    if (!value) {
      setError(`Enter your ${label().toLowerCase()}.`);
      return;
    }
    setBusy(true);
    setError(null);
    try {
      await api("/auth/password/forgot", { method: "POST", body: { email: value } });
      props.onClose();
      notify("info", "If that address has an account, a reset link is on its way.");
    } catch (failure) {
      setError(failure instanceof Error ? failure.message : String(failure));
    } finally {
      setBusy(false);
    }
  };

  return (
    <Dialog
      open={props.open}
      title="Reset your password"
      description="We'll email you a link. Your current password keeps working until you use it."
      onClose={props.onClose}
      footer={
        <>
          <Button variant="ghost" onClick={props.onClose}>
            Cancel
          </Button>
          <Button variant="primary" loading={busy()} onClick={() => void send()}>
            Send reset link
          </Button>
        </>
      }
    >
      <div class="space-y-4">
        <Field label={label()} required>
          <input
            class="input"
            type="email"
            autocomplete="username"
            value={address()}
            onInput={(event) => setAddress(event.currentTarget.value)}
          />
        </Field>
        <Show when={error()}>
          <p class="rounded-lg border border-danger-line bg-danger-soft px-3 py-2 text-[0.8125rem] text-ink">
            {error()}
          </p>
        </Show>
      </div>
    </Dialog>
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
