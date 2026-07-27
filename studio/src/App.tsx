import { For, Show, createEffect, createMemo, createSignal, onCleanup, onMount } from "solid-js";
import { Landing } from "./components/Landing";
import { Sidebar } from "./components/Sidebar";
import { OverviewPage } from "./components/OverviewPage";
import { ConfigPage } from "./components/ConfigPage";
import { ResourcePage } from "./components/ResourcePage";
import { FunctionPage } from "./components/FunctionPage";
import { ChangesPage } from "./components/ChangesPage";
import { NewFunctionDialog, NewResourceDialog } from "./components/Dialogs";
import { Badge, Button, EmptyState, HeadMark, ThemeToggle } from "./components/ui";
import { setView, view } from "./lib/nav";
import {
  closeProject,
  dismissToast,
  functionEntry,
  pendingChanges,
  reloadProject,
  resourceEntry,
  saveAll,
  studio,
} from "./lib/store";

export function App() {
  const [newResource, setNewResource] = createSignal(false);
  const [newFunction, setNewFunction] = createSignal(false);

  const changes = createMemo(() => pendingChanges());
  const resourceName = createMemo(() => {
    const current = view();
    return current.kind === "resource" ? current.name : null;
  });
  const functionName = createMemo(() => {
    const current = view();
    return current.kind === "function" ? current.name : null;
  });

  // Cmd/Ctrl+S saves; the browser's own save dialog is never what you want here.
  const onKeyDown = (event: KeyboardEvent) => {
    if ((event.metaKey || event.ctrlKey) && event.key.toLowerCase() === "s") {
      event.preventDefault();
      if (studio.project && changes().length) void saveAll();
    }
  };

  onMount(() => window.addEventListener("keydown", onKeyDown));
  onCleanup(() => window.removeEventListener("keydown", onKeyDown));

  createEffect(() => {
    const name = studio.project?.name;
    document.title = name ? `${name} - apiplant studio` : "apiplant studio";
  });

  // Losing unsaved edits to a stray tab close would be unrecoverable.
  createEffect(() => {
    const dirty = changes().length > 0;
    const warn = (event: BeforeUnloadEvent) => {
      if (!dirty) return;
      event.preventDefault();
      event.returnValue = "";
    };
    window.addEventListener("beforeunload", warn);
    onCleanup(() => window.removeEventListener("beforeunload", warn));
  });

  return (
    <Show when={studio.project} fallback={<Landing />}>
      {(project) => (
        <div class="relative z-10 flex h-screen flex-col">
          <header class="flex h-14 shrink-0 items-center gap-3 border-b border-line bg-surface/70 px-4 backdrop-blur-md">
            <button
              type="button"
              class="group flex items-center gap-2"
              onClick={() => setView({ kind: "overview" })}
              title="Overview"
            >
              <HeadMark class="h-7 transition-opacity group-hover:opacity-80" />
              <span class="text-sm font-semibold tracking-tight">
                apiplant <span class="text-accent">studio</span>
              </span>
            </button>

            <span class="text-faint">/</span>
            <span class="font-mono text-[0.8125rem] text-muted">{project().name}</span>

            <div class="flex-1" />

            <Show when={studio.loading}>
              <span class="text-xs text-faint">Reading directory…</span>
            </Show>

            <button
              type="button"
              onClick={() => setView({ kind: "changes" })}
              class="rounded-lg px-2 py-1 transition-colors hover:bg-surface-2"
              title="Review pending changes"
            >
              <Show when={changes().length} fallback={<Badge>saved</Badge>}>
                <Badge tone="accent">
                  {changes().length} unsaved file{changes().length === 1 ? "" : "s"}
                </Badge>
              </Show>
            </button>

            <ThemeToggle />

            <Button size="sm" variant="ghost" onClick={() => void reloadProject()} title="Re-read from disk">
              Reload
            </Button>
            <Button
              size="sm"
              variant="ghost"
              onClick={() => {
                if (!changes().length || confirm("Close this app? Unsaved changes are lost.")) closeProject();
              }}
            >
              Close
            </Button>
            <Button
              size="sm"
              variant="primary"
              disabled={!changes().length || studio.saving}
              onClick={() => void saveAll()}
            >
              {studio.saving ? "Saving…" : "Save"}
              <Show when={changes().length}>
                <span class="ml-0.5 opacity-70">⌘S</span>
              </Show>
            </Button>
          </header>

          <div class="flex min-h-0 flex-1">
            <Sidebar
              project={project()}
              onNewResource={() => setNewResource(true)}
              onNewFunction={() => setNewFunction(true)}
            />

            <main class="min-w-0 flex-1 overflow-y-auto">
              <Show when={view().kind === "overview"}>
                <OverviewPage
                  project={project()}
                  onNewResource={() => setNewResource(true)}
                  onNewFunction={() => setNewFunction(true)}
                />
              </Show>

              <Show when={view().kind === "config"}>
                <ConfigPage />
              </Show>

              <Show when={view().kind === "changes"}>
                <ChangesPage />
              </Show>

              {/* Keyed so each resource and function gets its own page state. */}
              <Show when={resourceName()} keyed>
                {(name) => (
                  <Show
                    when={resourceEntry(name)}
                    fallback={
                      <div class="mx-auto max-w-3xl px-6 py-16">
                        <EmptyState
                          title="That resource is gone"
                          description="It was deleted or renamed. Pick another from the sidebar."
                        />
                      </div>
                    }
                  >
                    {(found) => <ResourcePage entry={found()} />}
                  </Show>
                )}
              </Show>

              <Show when={functionName()} keyed>
                {(name) => (
                  <Show
                    when={functionEntry(name)}
                    fallback={
                      <div class="mx-auto max-w-3xl px-6 py-16">
                        <EmptyState
                          title="That function is gone"
                          description="It was deleted. Pick another from the sidebar."
                        />
                      </div>
                    }
                  >
                    {(found) => <FunctionPage entry={found()} />}
                  </Show>
                )}
              </Show>
            </main>
          </div>

          <Show when={newResource()}>
            <NewResourceDialog onClose={() => setNewResource(false)} />
          </Show>
          <Show when={newFunction()}>
            <NewFunctionDialog onClose={() => setNewFunction(false)} />
          </Show>

          <div class="pointer-events-none fixed bottom-4 right-4 z-[60] flex w-80 flex-col gap-2">
            <For each={studio.toasts}>
              {(item) => (
                <button
                  type="button"
                  onClick={() => dismissToast(item.id)}
                  class={[
                    "animate-rise pointer-events-auto rounded-lg border px-3 py-2 text-left text-xs shadow-lg shadow-black/40 backdrop-blur",
                    item.kind === "success"
                      ? "border-accent-line bg-accent-soft/95 text-accent"
                      : item.kind === "error"
                        ? "border-danger-line bg-danger-soft text-danger"
                        : "border-line bg-surface/95 text-muted",
                  ].join(" ")}
                >
                  {item.message}
                </button>
              )}
            </For>
          </div>
        </div>
      )}
    </Show>
  );
}
