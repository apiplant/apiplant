/**
 * The landing screen: what you can work on, and what you can do.
 *
 * Intentionally not a metrics dashboard. The purpose is navigation, so this is
 * a short greeting and two lists: the resources the operator manages and the
 * actions they can run.
 */

import { For, Show, createMemo } from "solid-js";
import { Button, Card, CardHeader, EmptyState, PageTitle } from "../ui";
import {
  visibleAgents,
  currentOrganization,
  currentUserLabel,
  manifest,
  navigate,
  navigationGroups,
  organizationLabel,
  session,
  visibleFunctions,
} from "../store";

function greeting(): string {
  const hour = new Date().getHours();
  if (hour < 12) return "Good morning";
  if (hour < 18) return "Good afternoon";
  return "Good evening";
}

export function DashboardPage() {
  const resources = createMemo(() => navigationGroups().flatMap((group) => group.resources));
  const actions = createMemo(() => visibleFunctions());
  const agents = createMemo(() => visibleAgents());
  const name = createMemo(() => {
    const label = currentUserLabel();
    // A full email address reads poorly in a greeting; use the local part.
    return label.includes("@") ? label.split("@")[0] : label;
  });

  return (
    <>
      <PageTitle
        title={`${greeting()}, ${name()}`}
        subtitle={
          session.organizationId
            ? `You are working in ${organizationLabel(currentOrganization())}.`
            : "Choose an organization from the top bar to get started."
        }
      />

      <Show when={!session.organizations.length && !session.loading}>
        <Card class="mb-4 border-accent-line bg-accent-soft/30">
          <div class="flex flex-wrap items-center justify-between gap-3 px-5 py-4">
            <div>
              <p class="text-sm font-medium text-ink">You are not in an organization yet</p>
              <p class="mt-0.5 text-xs text-muted">
                Create one to start, or wait for a teammate to add you to theirs.
              </p>
            </div>
            <Button variant="primary" onClick={() => navigate({ kind: "organization" })}>
              Create an organization
            </Button>
          </div>
        </Card>
      </Show>

      <div class="grid gap-4 xl:grid-cols-3">
        <Card>
          <CardHeader title="Manage" hint="Everything you have access to." />
          <Show
            when={resources().length}
            fallback={
              <div class="px-5 py-4">
                <EmptyState
                  title="Nothing to manage yet"
                  description="Resources you can see appear here once the application defines them."
                />
              </div>
            }
          >
            <div class="grid gap-px bg-line sm:grid-cols-2">
              <For each={resources()}>
                {(resource) => (
                  <button
                    type="button"
                    class="flex flex-col items-start bg-surface px-5 py-4 text-left transition-colors hover:bg-surface-2"
                    onClick={() => navigate({ kind: "resource", name: resource.name })}
                  >
                    <span class="text-sm font-medium text-ink">{resource.plural}</span>
                    <span class="mt-0.5 text-[0.6875rem] text-faint">
                      {resource.scope === "global" ? "Shared across organizations" : "In this organization"}
                    </span>
                  </button>
                )}
              </For>
            </div>
          </Show>
        </Card>

        <Card>
          <CardHeader title="Agents" hint="Configured assistants you can chat with." />
          <Show
            when={agents().length}
            fallback={
              <div class="px-5 py-4">
                <EmptyState
                  title="No agents available"
                  description="Configured agents appear here once this application defines them."
                />
              </div>
            }
          >
            <ul class="divide-y divide-line">
              <For each={agents()}>
                {(agent) => (
                  <li>
                    <button
                      type="button"
                      class="flex w-full items-center gap-3 px-5 py-3 text-left transition-colors hover:bg-surface-2/60"
                      onClick={() => navigate({ kind: "agent", name: agent.name })}
                    >
                      <span class="min-w-0 flex-1">
                        <span class="block truncate text-sm font-medium text-ink">{agent.label}</span>
                        <span class="mt-0.5 block truncate text-[0.6875rem] text-faint">
                          {agent.description || (agent.storage ? "Stored conversation history." : "Transient chat.")}
                        </span>
                      </span>
                      <svg
                        class="h-3.5 w-3.5 shrink-0 text-faint"
                        viewBox="0 0 16 16"
                        fill="none"
                        stroke="currentColor"
                        stroke-width="1.5"
                      >
                        <path d="m6 3.5 4.5 4.5L6 12.5" stroke-linecap="round" stroke-linejoin="round" />
                      </svg>
                    </button>
                  </li>
                )}
              </For>
            </ul>
          </Show>
        </Card>

        <Card>
          <CardHeader title="Actions" hint="One-off operations this application offers." />
          <Show
            when={actions().length}
            fallback={
              <div class="px-5 py-4">
                <EmptyState
                  title="No actions available"
                  description="Actions are functions the application exposes to operators. This one has none."
                />
              </div>
            }
          >
            <ul class="divide-y divide-line">
              <For each={actions()}>
                {(fn) => (
                  <li>
                    <button
                      type="button"
                      class="flex w-full items-center gap-3 px-5 py-3 text-left transition-colors hover:bg-surface-2/60"
                      onClick={() => navigate({ kind: "action", name: fn.name })}
                    >
                      <span class="min-w-0 flex-1">
                        <span class="block truncate text-sm font-medium text-ink">{fn.label}</span>
                        <Show when={fn.description}>
                          <span class="mt-0.5 block truncate text-[0.6875rem] text-faint">{fn.description}</span>
                        </Show>
                      </span>
                      <svg
                        class="h-3.5 w-3.5 shrink-0 text-faint"
                        viewBox="0 0 16 16"
                        fill="none"
                        stroke="currentColor"
                        stroke-width="1.5"
                      >
                        <path d="m6 3.5 4.5 4.5L6 12.5" stroke-linecap="round" stroke-linejoin="round" />
                      </svg>
                    </button>
                  </li>
                )}
              </For>
            </ul>
          </Show>
        </Card>
      </div>

      <Show when={manifest()?.docs_url}>
        {(docs) => (
          <p class="mt-6 text-center text-xs text-faint">
            Building against this application?{" "}
            <a class="text-accent hover:underline" href={docs()} target="_blank" rel="noreferrer">
              Read the API documentation
            </a>
            .
          </p>
        )}
      </Show>
    </>
  );
}
