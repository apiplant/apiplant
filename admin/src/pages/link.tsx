/**
 * The three screens somebody arrives at from a link in an email.
 *
 * They are not reachable from the interface, are shown before sign-in, and each
 * consumes a single-use token taken from the URL. Their job is to perform one
 * action, report clearly whether it succeeded, and leave the user signed in.
 *
 * An invalid link (expired, already used, or never issued) is reported
 * identically in all three cases. The server does not distinguish them either,
 * intentionally: revealing why a token is invalid would disclose which tokens
 * once existed.
 */

import { Errored, For, Loading, Show, createMemo, createSignal, onSettled } from "solid-js";
import { Button, Card, HeadMark, Spinner, ThemeToggle } from "../ui";
import { FieldEditor, buildPayload, createDraft, createDraftStore } from "../fields";
import { PasswordFields, createPasswordPair } from "../password";
import {
  api,
  asRecord,
  manifest,
  navigate,
  notify,
  persistSession,
  refreshSession,
  updateSession,
} from "../store";
import { signupResource } from "./auth";

/** One centred card with the app's logo above it, matching the sign-in screen. */
function LinkLayout(props: {
  title: string;
  subtitle?: string;
  children: unknown;
}) {
  const current = () => manifest()!;
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
            <h1 class="text-2xl font-semibold tracking-tight text-ink">{props.title}</h1>
            <Show when={props.subtitle}>
              <p class="mt-1.5 text-sm text-muted">{props.subtitle}</p>
            </Show>
          </div>
          <Card class="border-line-strong/70">{props.children as never}</Card>
        </section>
      </main>
    </div>
  );
}

function Problem(props: { message: string }) {
  return (
    <div class="space-y-4 px-5 py-5">
      <p class="rounded-lg border border-danger-line bg-danger-soft px-3 py-2 text-[0.8125rem] text-ink">
        {props.message}
      </p>
      <Button variant="ghost" class="w-full" onClick={() => navigate({ kind: "dashboard" })}>
        Go to sign in
      </Button>
    </div>
  );
}

/** Adopt a session the server just handed us and land on the dashboard. */
async function signInWith(token: string, message: string) {
  updateSession({ token, apiKey: "", userId: decodeSubject(token) });
  persistSession();
  await refreshSession();
  navigate({ kind: "dashboard" });
  notify("success", message);
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

const DEAD_LINK = "This link is no longer valid. It may have expired, or already been used.";

// --- accepting an invitation ------------------------------------------------

/**
 * The invitation's details are fetched before anything is asked of the user,
 * since an "accept" button with no organisation name gives nothing to agree to.
 *
 * Whether the address already has an account determines the entire form. With
 * one, there is nothing to fill in: the token proves control of the registered
 * address, so joining is a single button. Without one, this is also a sign-up,
 * and it collects a password along with whatever else the `user` resource
 * requires.
 */
export function AcceptInvitePage(props: { token: string }) {
  const [busy, setBusy] = createSignal(false);
  const [error, setError] = createSignal<string | null>(null);
  const passwords = createPasswordPair();

  const invitation = createMemo(async () => {
    const token = props.token;
    if (!token) throw new Error(DEAD_LINK);
    return asRecord(await api(`/auth/invitations/${encodeURIComponent(token)}`));
  });

  const extras = () => signupResource(manifest()?.auth.signup_fields ?? []);
  const draft = createDraftStore();
  onSettled(() => draft.reset(createDraft(extras(), null)));

  const needsAccount = () => invitation()?.has_account === false;

  const accept = async (event: Event) => {
    event.preventDefault();
    if (busy()) return;
    setError(null);

    let body: Record<string, unknown> = {};
    if (needsAccount()) {
      if (!passwords.ready()) {
        setError(passwords.error() ?? "Choose a password, and type it twice.");
        return;
      }
      const { payload, errors } = buildPayload(extras(), draft.values);
      if (errors.length) {
        setError(errors[0].message);
        return;
      }
      body = { ...payload, password: passwords.value() };
    }

    setBusy(true);
    try {
      const response = asRecord(
        await api(`/auth/invitations/${encodeURIComponent(props.token)}/accept`, {
          method: "POST",
          body,
        }),
      );
      const token = typeof response?.token === "string" ? response.token : "";
      if (!token) throw new Error("The server did not return a session.");
      // Select the organisation just joined, rather than whichever sorts
      // first.
      if (typeof response?.organization_id === "string") {
        updateSession({ organizationId: response.organization_id });
      }
      passwords.reset();
      await signInWith(token, `You're in ${String(invitation()?.organization ?? "")}.`);
    } catch (failure) {
      setError(failure instanceof Error ? failure.message : String(failure));
    } finally {
      setBusy(false);
    }
  };

  return (
    <LinkLayout
      title="You've been invited"
      subtitle={
        invitation()
          ? `Join ${String(invitation()!.organization ?? "")} as ${String(invitation()!.email ?? "")}.`
          : undefined
      }
    >
      <Errored fallback={<Problem message={DEAD_LINK} />}>
        <Loading
          fallback={
            <p class="flex items-center gap-2 px-5 py-6 text-xs text-faint">
              <Spinner class="h-4 w-4" /> Checking your invitation…
            </p>
          }
        >
        <Show when={invitation()} fallback={<Problem message={DEAD_LINK} />}>
          <form class="space-y-4 px-5 py-5" onSubmit={accept}>
            <Show when={invitation()!.role}>
              <p class="text-[0.8125rem] text-muted">
                You'll join as <span class="font-medium text-ink">{String(invitation()!.role)}</span>.
              </p>
            </Show>

            <Show
              when={needsAccount()}
              fallback={
                <p class="text-[0.8125rem] text-muted">
                  You already have an account with this address. Accepting adds it to the
                  organization; nothing else changes.
                </p>
              }
            >
              <PasswordFields
                pair={passwords}
                help="This is the password you'll sign in with from now on."
              />
              <For each={manifest()!.auth.signup_fields}>
                {(field) => <FieldEditor field={field} draft={draft} />}
              </For>
            </Show>

            <Show when={error()}>
              <p class="rounded-lg border border-danger-line bg-danger-soft px-3 py-2 text-[0.8125rem] text-ink">
                {error()}
              </p>
            </Show>

            <Button type="submit" variant="primary" size="lg" class="w-full" loading={busy()}>
              {needsAccount() ? "Create account and join" : "Join organization"}
            </Button>
          </form>
        </Show>
        </Loading>
      </Errored>
    </LinkLayout>
  );
}

// --- confirming an address --------------------------------------------------

/**
 * There is nothing to fill in: opening the link *is* the action, so the token
 * is consumed on arrival and the outcome reported. Confirming also signs the
 * user in, since requiring a password immediately after proving mailbox access
 * adds nothing.
 */
export function VerifyEmailPage(props: { token: string }) {
  const [error, setError] = createSignal<string | null>(null);

  onSettled(() => {
    void (async () => {
      if (!props.token) {
        setError(DEAD_LINK);
        return;
      }
      try {
        const response = asRecord(
          await api("/auth/verify-email", { method: "POST", body: { token: props.token } }),
        );
        const token = typeof response?.token === "string" ? response.token : "";
        if (!token) throw new Error(DEAD_LINK);
        await signInWith(token, "Your address is confirmed.");
        // `[auth] verify_email_redirect`, when the deployment named somewhere
        // to go. Signing in first so the session is in place before the browser
        // leaves — the app being landed on is usually the reason to confirm at
        // all. `replace` so Back does not return to a spent token.
        const redirect = typeof response?.redirect_to === "string" ? response.redirect_to.trim() : "";
        if (redirect) window.location.replace(redirect);
      } catch (failure) {
        setError(failure instanceof Error ? failure.message : DEAD_LINK);
      }
    })();
  });

  return (
    <LinkLayout title="Confirming your address">
      <Show
        when={error()}
        fallback={
          <p class="flex items-center gap-2 px-5 py-6 text-xs text-faint">
            <Spinner class="h-4 w-4" /> One moment…
          </p>
        }
      >
        <Problem message={error()!} />
      </Show>
    </LinkLayout>
  );
}

// --- finishing a reset ------------------------------------------------------

/** Choose the new password, twice, and be signed in with it. */
export function ResetPasswordPage(props: { token: string }) {
  const [busy, setBusy] = createSignal(false);
  const [error, setError] = createSignal<string | null>(null);
  const passwords = createPasswordPair();

  const submit = async (event: Event) => {
    event.preventDefault();
    if (busy()) return;
    if (!passwords.ready()) {
      setError(passwords.error() ?? "Choose a password, and type it twice.");
      return;
    }
    setError(null);
    setBusy(true);
    try {
      const response = asRecord(
        await api("/auth/password/reset", {
          method: "POST",
          body: { token: props.token, password: passwords.value() },
        }),
      );
      const token = typeof response?.token === "string" ? response.token : "";
      if (!token) throw new Error(DEAD_LINK);
      passwords.reset();
      await signInWith(token, "Your password has been changed.");
    } catch (failure) {
      setError(failure instanceof Error ? failure.message : String(failure));
    } finally {
      setBusy(false);
    }
  };

  return (
    <LinkLayout title="Choose a new password" subtitle="You'll be signed in straight afterwards.">
      <Show when={props.token} fallback={<Problem message={DEAD_LINK} />}>
        <form class="space-y-4 px-5 py-5" onSubmit={submit}>
          <PasswordFields pair={passwords} label="New password" />

          <Show when={error()}>
            <p class="rounded-lg border border-danger-line bg-danger-soft px-3 py-2 text-[0.8125rem] text-ink">
              {error()}
            </p>
          </Show>

          <Button type="submit" variant="primary" size="lg" class="w-full" loading={busy()}>
            Change password
          </Button>
        </form>
      </Show>
    </LinkLayout>
  );
}
