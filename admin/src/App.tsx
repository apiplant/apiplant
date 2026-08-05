/**
 * The shell: a top bar, a sidebar, and whichever screen the route names.
 *
 * The sidebar lists only what the operator can reach: resources their role may
 * see and actions they may run, so the navigation never offers something that
 * would return a permission error.
 */

import { For, Match, Show, Switch, createMemo, createSignal, onCleanup, onMount } from "solid-js";
import {
  Avatar,
  Button,
  EmptyState,
  HeadMark,
  Menu,
  MenuItem,
  MenuSeparator,
  Spinner,
  ThemeToggle,
  ToastStack,
} from "./ui";
import { DashboardPage } from "./pages/dashboard";
import { AuthPage } from "./pages/auth";
import { AcceptInvitePage, ResetPasswordPage, VerifyEmailPage } from "./pages/link";
import { RecordPage, ResourceListPage } from "./pages/resource";
import { ActionPage } from "./pages/action";
import { AgentPage } from "./pages/agent";
import { CliPage } from "./pages/cli";
import { AccountPage, ApiKeysPage, OrganizationPage, TeamPage } from "./pages/settings";
import { BillingPage, noticeCheckoutOutcome } from "./pages/billing";
import { AdminAiAssist } from "./ai-assist";
import {
  adoptOAuthToken,
  avatarOf,
  currentOrganization,
  currentUserAvatar,
  currentUserLabel,
  agentByName,
  dismissToast,
  functionByName,
  isSignedIn,
  manifest,
  navigate,
  navigationGroups,
  notify,
  organizationLabel,
  refreshSession,
  reportError,
  resourceByName,
  restoreSession,
  route,
  session,
  setActiveOrganization,
  setManifest,
  signOut,
  syncRouteFromHash,
  toasts,
  verifySession,
  visibleAgents,
  visibleFunctions,
} from "./store";
import type { AdminManifest, Route } from "./types";

const MANIFEST_URL = "./apiplant-admin.json";

export function App() {
  const [loading, setLoading] = createSignal(true);
  const [fatal, setFatal] = createSignal<string | null>(null);

  onMount(() => {
    restoreSession();
    // A sign-in that went out to GitHub and came back leaves the session in the
    // URL fragment. Take it before the router looks at the hash, since
    // `#token=…` is a credential and not a route.
    const arrived = adoptOAuthToken();
    syncRouteFromHash();
    // A buyer returning from the provider lands on `#/billing?checkout=…`.
    // Report the outcome and clean the address before anything reads the hash
    // again and finds a query string on a route that has none.
    noticeCheckoutOutcome();
    const onPopState = () => syncRouteFromHash();
    window.addEventListener("popstate", onPopState);
    window.addEventListener("hashchange", onPopState);
    onCleanup(() => {
      window.removeEventListener("popstate", onPopState);
      window.removeEventListener("hashchange", onPopState);
    });

    void (async () => {
      try {
        const response = await fetch(MANIFEST_URL);
        if (!response.ok) throw new Error("This dashboard's configuration could not be loaded.");
        setManifest((await response.json()) as AdminManifest);
        document.title = manifest()?.title ?? "apiplant admin";
        if (arrived) notify("success", "Welcome back.");
        if (isSignedIn()) {
          try {
            // Verify the stored credential before loading anything with it;
            // an invalid token is dropped here and the sign-in screen takes
            // over.
            if (await verifySession()) await refreshSession();
          } catch (error) {
            // A stale token is the usual cause, so the sign-in screen is more
            // useful than an error the operator cannot act on.
            signOut();
            reportError(error);
          }
        }
      } catch (error) {
        setFatal(error instanceof Error ? error.message : String(error));
      } finally {
        setLoading(false);
      }
    })();
  });

  return (
    <>
      <Switch>
        <Match when={loading()}>
          <div class="relative z-10 flex min-h-screen items-center justify-center gap-2 text-sm text-faint">
            <Spinner class="h-4 w-4" /> Loading…
          </div>
        </Match>
        <Match when={fatal()}>
          <div class="relative z-10 flex min-h-screen items-center justify-center p-6">
            <EmptyState title="This dashboard could not start" description={fatal()!} />
          </div>
        </Match>
        {/*
          The emailed links come before the sign-in screen, because they are
          shown to people who cannot sign in yet, which is the purpose of the
          link. They also take precedence over a live session: someone signed in
          as one account who opens an invitation addressed to another should get
          the invitation, not their own dashboard.
        */}
        <Match when={route().kind === "accept-invite"}>
          <AcceptInvitePage token={(route() as { token: string }).token} />
        </Match>
        <Match when={route().kind === "verify-email"}>
          <VerifyEmailPage token={(route() as { token: string }).token} />
        </Match>
        <Match when={route().kind === "reset-password"}>
          <ResetPasswordPage token={(route() as { token: string }).token} />
        </Match>
        <Match when={!isSignedIn()}>
          <AuthPage />
        </Match>
        {/*
          Nothing stands between signing in and the dashboard: every account is
          given a personal organization when it is created, so there is always
          somewhere to work. Someone who has left all of theirs still gets the
          shell, which says so and offers to start another.
        */}
        <Match when={true}>
          <Shell />
        </Match>
      </Switch>
      <AdminAiAssist />
      <ToastStack toasts={toasts()} onDismiss={dismissToast} />
    </>
  );
}

function Shell() {
  const [navOpen, setNavOpen] = createSignal(false);

  return (
    <div class="relative z-10 flex min-h-screen flex-col">
      <TopBar onToggleNav={() => setNavOpen((open) => !open)} />
      <div class="flex min-h-0 flex-1">
        {/* One sidebar, shown as a drawer on small screens. */}
        <div
          class={`fixed inset-0 z-30 bg-canvas/60 backdrop-blur-sm lg:hidden ${navOpen() ? "" : "hidden"}`}
          onClick={() => setNavOpen(false)}
          aria-hidden="true"
        />
        {/*
          On a wide screen the sidebar sticks below the 4rem top bar and is
          capped to the remaining viewport height and scrolls independently.
          Otherwise an app with enough resources to overflow the screen would
          make the page taller and scroll its own navigation out of view.
        */}
        <aside
          class={`fixed inset-y-0 left-0 z-40 w-64 shrink-0 overflow-y-auto overscroll-contain border-r border-line bg-surface pt-16 transition-transform lg:sticky lg:top-16 lg:z-auto lg:h-[calc(100dvh-4rem)] lg:translate-x-0 lg:self-start lg:bg-surface/40 lg:pt-0 ${
            navOpen() ? "translate-x-0" : "-translate-x-full"
          }`}
        >
          <Navigation onNavigate={() => setNavOpen(false)} />
        </aside>

        <main class={`min-w-0 flex-1 ${route().kind === "agent" ? "overflow-y-auto lg:overflow-hidden" : "overflow-y-auto"}`}>
          <div
            class={`mx-auto ${
              route().kind === "agent"
                ? "max-w-[96rem] px-0 py-0 sm:px-0 lg:px-8 lg:py-4 lg:h-[calc(100dvh-4rem)] lg:overflow-hidden"
                : "max-w-6xl px-4 py-6 sm:px-6 lg:px-8"
            }`}
          >
            <CurrentPage />
          </div>
        </main>
      </div>
    </div>
  );
}

function TopBar(props: { onToggleNav: () => void }) {
  const organizations = () => session.organizations;

  return (
    <header class="sticky top-0 z-30 flex h-16 items-center gap-3 border-b border-line bg-surface/80 px-4 backdrop-blur-md sm:px-6">
      <button
        type="button"
        class="rounded-lg p-2 text-muted transition-colors hover:bg-surface-2 hover:text-ink lg:hidden"
        aria-label="Toggle navigation"
        onClick={props.onToggleNav}
      >
        <svg width="18" height="18" viewBox="0 0 18 18" fill="none" stroke="currentColor" stroke-width="1.6">
          <path d="M2.5 4.5h13M2.5 9h13M2.5 13.5h13" stroke-linecap="round" />
        </svg>
      </button>

      <button
        type="button"
        class="flex items-center gap-2 transition-opacity hover:opacity-80"
        onClick={() => navigate({ kind: "dashboard" })}
      >
        <HeadMark class="h-7" src={manifest()?.logo} />
        <span class="hidden text-sm font-semibold tracking-tight text-ink sm:block">
          {manifest()?.app_name} <span class="text-accent">admin</span>
        </span>
      </button>

      <div class="flex-1" />

      <Show when={organizations().length > 1}>
        <Menu
          trigger={(open) => (
            <button
              type="button"
              onClick={open}
              class="flex max-w-48 items-center gap-2 rounded-lg border border-line bg-surface px-2.5 py-1.5 text-[0.8125rem] text-ink transition-colors hover:border-line-strong"
            >
              <Avatar
                name={organizationLabel(currentOrganization())}
                src={avatarOf(currentOrganization())}
                size="sm"
              />
              <span class="truncate">{organizationLabel(currentOrganization())}</span>
              <svg class="h-3 w-3 shrink-0 text-faint" viewBox="0 0 12 12" fill="none" stroke="currentColor" stroke-width="1.4">
                <path d="M2.5 4.5 6 8l3.5-3.5" stroke-linecap="round" stroke-linejoin="round" />
              </svg>
            </button>
          )}
        >
          <p class="px-3 pb-1 pt-1.5 text-[0.6875rem] font-semibold uppercase tracking-wide text-faint">
            Switch organization
          </p>
          <For each={organizations()}>
            {(organization) => (
              <MenuItem onClick={() => void setActiveOrganization(String(organization.id ?? ""))}>
                <Avatar name={organizationLabel(organization)} src={avatarOf(organization)} size="sm" />
                <span class="truncate">{organizationLabel(organization)}</span>
              </MenuItem>
            )}
          </For>
        </Menu>
      </Show>

      <Show when={organizations().length === 1}>
        <span class="hidden items-center gap-2 rounded-lg border border-line px-2.5 py-1.5 text-[0.8125rem] text-muted sm:flex">
          <Avatar
            name={organizationLabel(currentOrganization())}
            src={avatarOf(currentOrganization())}
            size="sm"
          />
          <span class="max-w-40 truncate">{organizationLabel(currentOrganization())}</span>
        </span>
      </Show>

      <ThemeToggle />

      <Menu
        trigger={(open) => (
          <button
            type="button"
            onClick={open}
            class="rounded-full transition-opacity hover:opacity-80"
            aria-label="Account menu"
          >
            <Avatar name={currentUserLabel()} src={currentUserAvatar()} />
          </button>
        )}
      >
        <div class="border-b border-line px-3 pb-2 pt-1">
          <p class="truncate text-[0.8125rem] font-medium text-ink">{currentUserLabel()}</p>
          <Show when={session.roles.length}>
            <p class="mt-0.5 text-[0.6875rem] text-faint">{session.roles.join(", ")}</p>
          </Show>
        </div>
        <MenuItem onClick={() => navigate({ kind: "account" })}>Your account</MenuItem>
        <MenuItem onClick={() => navigate({ kind: "keys" })}>API keys</MenuItem>
        <Show when={manifest()?.docs_url}>
          {(docs) => <MenuItem href={docs()}>API documentation</MenuItem>}
        </Show>
        <MenuSeparator />
        <MenuItem danger onClick={signOut}>
          Sign out
        </MenuItem>
      </Menu>
    </header>
  );
}

function Navigation(props: { onNavigate: () => void }) {
  const groups = createMemo(navigationGroups);
  const agents = createMemo(visibleAgents);
  // Actions live under one heading rather than one per `group`. A function's
  // group usually names the same area as a resource group ("Support"), and two
  // identical headings listing different things is confusing, so the group only
  // orders them here and captions each item.
  const actions = createMemo(() =>
    [...visibleFunctions()].sort(
      (left, right) =>
        (left.group ?? "￿").localeCompare(right.group ?? "￿") ||
        left.order - right.order ||
        left.label.localeCompare(right.label),
    ),
  );

  const isActive = (test: (route: Route) => boolean) => test(route());

  const item = (active: boolean) =>
    [
      "flex w-full items-center gap-2.5 rounded-lg px-3 py-2 text-left text-[0.8125rem] transition-colors",
      active ? "bg-accent-soft font-medium text-ink" : "text-muted hover:bg-surface-2 hover:text-ink",
    ].join(" ");

  const go = (next: Route) => {
    navigate(next);
    props.onNavigate();
  };

  return (
    <nav class="space-y-6 p-3">
      <div class="space-y-0.5">
        <button
          type="button"
          class={item(isActive((r) => r.kind === "dashboard"))}
          onClick={() => go({ kind: "dashboard" })}
        >
          Home
        </button>
      </div>

      <For each={groups()}>
        {(group) => (
          <div>
            <p class="px-3 pb-1.5 text-[0.6875rem] font-semibold uppercase tracking-[0.08em] text-faint">
              {group.group ?? "Manage"}
            </p>
            <div class="space-y-0.5">
              <For each={group.resources}>
                {(resource) => (
                  <button
                    type="button"
                    class={item(
                      isActive(
                        (r) =>
                          (r.kind === "resource" || r.kind === "record" || r.kind === "new") &&
                          r.name === resource.name,
                      ),
                    )}
                    onClick={() => go({ kind: "resource", name: resource.name })}
                  >
                    {resource.plural}
                  </button>
                )}
              </For>
            </div>
          </div>
        )}
      </For>

      <Show when={agents().length}>
        <div>
          <p class="px-3 pb-1.5 text-[0.6875rem] font-semibold uppercase tracking-[0.08em] text-faint">
            Agents
          </p>
          <div class="space-y-0.5">
            <For each={agents()}>
              {(agent) => (
                <button
                  type="button"
                  class={item(isActive((r) => r.kind === "agent" && r.name === agent.name))}
                  onClick={() => go({ kind: "agent", name: agent.name })}
                >
                  <span class="block min-w-0 truncate">{agent.label}</span>
                </button>
              )}
            </For>
          </div>
        </div>
      </Show>

      <Show when={actions().length}>
        <div>
          <p class="px-3 pb-1.5 text-[0.6875rem] font-semibold uppercase tracking-[0.08em] text-faint">
            Actions
          </p>
          <div class="space-y-0.5">
            <For each={actions()}>
              {(fn) => (
                <button
                  type="button"
                  class={item(isActive((r) => r.kind === "action" && r.name === fn.name))}
                  onClick={() => go({ kind: "action", name: fn.name })}
                >
                  <span class="min-w-0">
                    <span class="block truncate">{fn.label}</span>
                    <Show when={fn.group}>
                      <span class="block truncate text-[0.6875rem] text-faint">{fn.group}</span>
                    </Show>
                  </span>
                </button>
              )}
            </For>
          </div>
        </div>
      </Show>

      <div>
        <p class="px-3 pb-1.5 text-[0.6875rem] font-semibold uppercase tracking-[0.08em] text-faint">
          Settings
        </p>
        <div class="space-y-0.5">
          <button
            type="button"
            class={item(isActive((r) => r.kind === "team"))}
            onClick={() => go({ kind: "team" })}
          >
            Team
          </button>
          <button
            type="button"
            class={item(isActive((r) => r.kind === "organization"))}
            onClick={() => go({ kind: "organization" })}
          >
            Organization
          </button>
          {/* Only where the app processes payments; the routes and tables
              behind this screen do not exist otherwise. */}
          <Show when={manifest()?.billing}>
            <button
              type="button"
              class={item(isActive((r) => r.kind === "billing"))}
              onClick={() => go({ kind: "billing" })}
            >
              Billing
            </button>
          </Show>
          <button
            type="button"
            class={item(isActive((r) => r.kind === "account"))}
            onClick={() => go({ kind: "account" })}
          >
            Your account
          </button>
        </div>
      </div>
    </nav>
  );
}

function CurrentPage() {
  return (
    <Switch fallback={<DashboardPage />}>
      <Match when={route().kind === "resource" && resourceByName((route() as { name: string }).name)}>
        {(resource) => <ResourceListPage resource={resource()} />}
      </Match>
      <Match when={route().kind === "new" && resourceByName((route() as { name: string }).name)}>
        {(resource) => <RecordPage resource={resource()} id={null} />}
      </Match>
      <Match when={route().kind === "record" && resourceByName((route() as { name: string }).name)}>
        {(resource) => <RecordPage resource={resource()} id={(route() as { id: string }).id} />}
      </Match>
      <Match when={route().kind === "action" && functionByName((route() as { name: string }).name)}>
        {(fn) => <ActionPage fn={fn()} />}
      </Match>
      <Match when={route().kind === "agent" && agentByName((route() as { name: string }).name)}>
        {(agent) => <AgentPage agent={agent()} threadId={(route() as { threadId?: string }).threadId ?? null} />}
      </Match>
      <Match when={route().kind === "account"}>
        <AccountPage />
      </Match>
      <Match when={route().kind === "team"}>
        <TeamPage />
      </Match>
      <Match when={route().kind === "organization"}>
        <OrganizationPage />
      </Match>
      <Match when={route().kind === "keys"}>
        <ApiKeysPage />
      </Match>
      <Match when={route().kind === "billing"}>
        <BillingPage />
      </Match>
      <Match when={route().kind === "cli"}>
        <CliPage />
      </Match>
      <Match
        when={
          route().kind === "resource" ||
          route().kind === "record" ||
          route().kind === "new" ||
          route().kind === "action" ||
          route().kind === "agent"
        }
      >
        <EmptyState
          title="That is no longer here"
          description="It may have been renamed or removed since the link was created."
        >
          <Button variant="primary" onClick={() => navigate({ kind: "dashboard" })}>
            Go home
          </Button>
        </EmptyState>
      </Match>
    </Switch>
  );
}
