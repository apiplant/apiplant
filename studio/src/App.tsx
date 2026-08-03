import { For, Show, createEffect, createMemo, createSignal, onCleanup, onMount } from "solid-js";
import { Landing } from "./components/Landing";
import { Sidebar } from "./components/Sidebar";
import { OverviewPage } from "./components/OverviewPage";
import { ConfigPage } from "./components/ConfigPage";
import { ResourcePage } from "./components/ResourcePage";
import { FunctionPage } from "./components/FunctionPage";
import { AgentPage } from "./components/AgentPage";
import { ChangesPage } from "./components/ChangesPage";
import { NewAgentDialog, NewFunctionDialog, NewResourceDialog } from "./components/Dialogs";
import { Badge, Button, EmptyState, HeadMark, Modal, ThemeToggle } from "./components/ui";
import { droppedDirectoryHandle, permissionState } from "./lib/fs";
import { setView, syncViewFromLocation, view } from "./lib/nav";
import {
  type AppCandidate,
  agentEntry,
  closeProject,
  dismissToast,
  functionEntry,
  inspectDirectory,
  openProject,
  pendingChanges,
  reloadProject,
  resourceEntry,
  saveAll,
  studio,
  toast,
  type DirectorySelection,
} from "./lib/store";
import { loadRememberedProject } from "./lib/persistence";

export function App() {
  const [newResource, setNewResource] = createSignal(false);
  const [newFunction, setNewFunction] = createSignal(false);
  const [newAgent, setNewAgent] = createSignal(false);
  const [navOpen, setNavOpen] = createSignal(false);
  const [dropActive, setDropActive] = createSignal(false);
  const [switchRequest, setSwitchRequest] = createSignal<DirectorySelection | null>(null);
  const [droppedChoices, setDroppedChoices] = createSignal<{ parentName: string; candidates: AppCandidate[] } | null>(
    null,
  );
  const [rememberedProject, setRememberedProject] = createSignal<{
    name: string;
    handle: FileSystemDirectoryHandle;
  } | null>(null);

  const changes = createMemo(() => pendingChanges());
  const dirty = createMemo(() => changes().length > 0);
  const resourceName = createMemo(() => {
    const current = view();
    return current.kind === "resource" ? current.name : null;
  });
  const functionName = createMemo(() => {
    const current = view();
    return current.kind === "function" ? current.name : null;
  });
  const agentName = createMemo(() => {
    const current = view();
    return current.kind === "agent" ? current.name : null;
  });

  // Cmd/Ctrl+S saves; the browser's own save dialog is never what you want here.
  const onKeyDown = (event: KeyboardEvent) => {
    if ((event.metaKey || event.ctrlKey) && event.key.toLowerCase() === "s") {
      event.preventDefault();
      if (studio.project && changes().length) void saveAll();
    }
  };

  const describeSelection = (selection: DirectorySelection) =>
    selection.kind === "app" ? selection.handle.name : selection.parent.name;

  const openHandle = async (
    handle: FileSystemDirectoryHandle,
    options: { preserveView?: boolean } = {},
  ): Promise<boolean> => {
    try {
      await openProject(handle, { preserveView: options.preserveView });
      setNewResource(false);
      setNewFunction(false);
      setNewAgent(false);
      setRememberedProject(null);
      return true;
    } catch (error) {
      toast(error instanceof Error ? error.message : String(error), "error");
      return false;
    }
  };

  const openSelection = async (selection: DirectorySelection) => {
    setDroppedChoices(null);
    if (selection.kind === "app") {
      await openHandle(selection.handle);
      return;
    }
    if (selection.candidates.length === 0) {
      toast(
        `No apiplant app found in ${selection.parent.name}. An app directory holds a main.toml, a models/ folder, an agents/ folder, or a functions/ folder.`,
        "error",
      );
      return;
    }
    if (selection.candidates.length === 1) {
      await openHandle(selection.candidates[0].handle);
      return;
    }
    setDroppedChoices({ parentName: selection.parent.name, candidates: selection.candidates });
  };

  const requestOpen = async (selection: DirectorySelection) => {
    if (selection.kind === "candidates" && selection.candidates.length === 0) {
      await openSelection(selection);
      return;
    }
    if (studio.project && dirty()) {
      setSwitchRequest(selection);
      return;
    }
    await openSelection(selection);
  };

  onMount(() => window.addEventListener("keydown", onKeyDown));
  onCleanup(() => window.removeEventListener("keydown", onKeyDown));

  onMount(() => {
    let cancelled = false;
    void (async () => {
      try {
        const remembered = await loadRememberedProject();
        if (!remembered || cancelled || studio.project) return;
        const permission = await permissionState(remembered.handle);
        if (cancelled) return;
        if (permission === "granted") {
          if (!(await openHandle(remembered.handle, { preserveView: true })) && !cancelled) {
            setRememberedProject(remembered);
          }
          return;
        }
        setRememberedProject(remembered);
      } catch {
        // Remembered project restore is best effort only.
      }
    })();
    onCleanup(() => {
      cancelled = true;
    });

    onMount(() => {
      syncViewFromLocation();
      const onPopState = () => syncViewFromLocation();
      window.addEventListener("popstate", onPopState);
      onCleanup(() => window.removeEventListener("popstate", onPopState));
    });
  });

  onMount(() => {
    let dragDepth = 0;
    const hasFiles = (dataTransfer: DataTransfer | null) =>
      !!dataTransfer && Array.from(dataTransfer.types).includes("Files");

    const onDragEnter = (event: DragEvent) => {
      if (!hasFiles(event.dataTransfer)) return;
      event.preventDefault();
      dragDepth += 1;
      setDropActive(true);
    };

    const onDragOver = (event: DragEvent) => {
      if (!hasFiles(event.dataTransfer)) return;
      event.preventDefault();
      if (event.dataTransfer) event.dataTransfer.dropEffect = "copy";
      if (!dropActive()) setDropActive(true);
    };

    const onDragLeave = (event: DragEvent) => {
      if (!hasFiles(event.dataTransfer)) return;
      event.preventDefault();
      dragDepth = Math.max(0, dragDepth - 1);
      if (!dragDepth) setDropActive(false);
    };

    const onDrop = async (event: DragEvent) => {
      if (!hasFiles(event.dataTransfer)) return;
      event.preventDefault();
      dragDepth = 0;
      setDropActive(false);
      if (studio.loading || studio.saving) return;

      const handle = await droppedDirectoryHandle(event.dataTransfer?.items);
      if (!handle) {
        toast("Drop an app directory, not individual files", "error");
        return;
      }

      try {
        await requestOpen(await inspectDirectory(handle));
      } catch (error) {
        toast(error instanceof Error ? error.message : String(error), "error");
      }
    };

    window.addEventListener("dragenter", onDragEnter);
    window.addEventListener("dragover", onDragOver);
    window.addEventListener("dragleave", onDragLeave);
    window.addEventListener("drop", onDrop);
    onCleanup(() => {
      window.removeEventListener("dragenter", onDragEnter);
      window.removeEventListener("dragover", onDragOver);
      window.removeEventListener("dragleave", onDragLeave);
      window.removeEventListener("drop", onDrop);
    });
  });

  const saveAndOpen = async () => {
    const selection = switchRequest();
    if (!selection) return;
    try {
      await saveAll();
    } catch {
      return;
    }
    setSwitchRequest(null);
    await openSelection(selection);
  };

  const discardAndOpen = async () => {
    const selection = switchRequest();
    if (!selection) return;
    setSwitchRequest(null);
    await openSelection(selection);
  };

  const openCandidate = async (candidate: AppCandidate) => {
    if (await openHandle(candidate.handle)) {
      setDroppedChoices(null);
    }
  };

  createEffect(() => {
    const name = studio.project?.name;
    document.title = name ? `${name} - apiplant studio` : "apiplant studio";
  });

  createEffect(() => {
    void view();
    setNavOpen(false);
  });

  // Losing unsaved edits to a stray tab close would be unrecoverable.
  createEffect(() => {
    const warn = (event: BeforeUnloadEvent) => {
      if (!dirty()) return;
      event.preventDefault();
      event.returnValue = "";
    };
    window.addEventListener("beforeunload", warn);
    onCleanup(() => window.removeEventListener("beforeunload", warn));
  });

  return (
    <>
      <Show
        when={studio.project}
        fallback={
          <Landing
            rememberedProjectName={rememberedProject()?.name ?? null}
            onReopenRemembered={
              rememberedProject() ? () => void openHandle(rememberedProject()!.handle, { preserveView: true }) : undefined
            }
          />
        }
      >
        {(project) => (
          <div class="relative z-10 flex h-screen flex-col">
            <header class="flex min-h-14 shrink-0 flex-wrap items-center gap-x-3 gap-y-2 border-b border-line bg-surface/70 px-4 py-2 backdrop-blur-md">
              <div class="flex min-w-0 flex-1 items-center gap-3">
                <button
                  type="button"
                  class="rounded-lg p-2 text-muted transition-colors hover:bg-surface-2 hover:text-ink lg:hidden"
                  aria-label="Toggle navigation"
                  onClick={() => setNavOpen((open) => !open)}
                >
                  <svg width="18" height="18" viewBox="0 0 18 18" fill="none" stroke="currentColor" stroke-width="1.6">
                    <path d="M2.5 4.5h13M2.5 9h13M2.5 13.5h13" stroke-linecap="round" />
                  </svg>
                </button>

                <button
                  type="button"
                  class="group flex shrink-0 items-center gap-2"
                  onClick={() => setView({ kind: "overview" })}
                  title="Overview"
                >
                  <HeadMark class="h-7 transition-opacity group-hover:opacity-80" />
                  <span class="text-sm font-semibold tracking-tight whitespace-nowrap">
                    apiplant <span class="text-accent">studio</span>
                  </span>
                </button>

                <span class="text-faint">/</span>
                <span class="min-w-0 truncate font-mono text-[0.8125rem] text-muted">{project().name}</span>
              </div>

              <div class="flex w-full flex-wrap items-center justify-end gap-2 sm:w-auto">
                <Show when={studio.loading}>
                  <span class="mr-auto text-xs text-faint sm:mr-0">Reading directory…</span>
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
                    if (!changes().length || confirm("Close this app? Unsaved changes are lost.")) {
                      setRememberedProject({ name: project().name, handle: project().handle });
                      closeProject();
                    }
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
                    <span class="ml-0.5 hidden opacity-70 sm:inline">⌘S</span>
                  </Show>
                </Button>
              </div>
            </header>

            <div class="flex min-h-0 flex-1">
              <div
                class={`fixed inset-0 z-30 bg-black/50 backdrop-blur-sm lg:hidden ${navOpen() ? "" : "hidden"}`}
                onClick={() => setNavOpen(false)}
                aria-hidden="true"
              />
              <aside
                class={`fixed left-0 top-14 z-40 h-[calc(100dvh-3.5rem)] transition-transform lg:static lg:z-auto lg:h-auto lg:translate-x-0 ${
                  navOpen() ? "translate-x-0" : "-translate-x-full"
                }`}
              >
                <Sidebar
                  project={project()}
                  onNewResource={() => {
                    setNavOpen(false);
                    setNewResource(true);
                  }}
                  onNewFunction={() => {
                    setNavOpen(false);
                    setNewFunction(true);
                  }}
                  onNewAgent={() => {
                    setNavOpen(false);
                    setNewAgent(true);
                  }}
                  onNavigate={() => setNavOpen(false)}
                />
              </aside>

              <main class="min-w-0 flex-1 overflow-y-auto">
                <Show when={view().kind === "overview"}>
                  <OverviewPage
                    project={project()}
                    onNewResource={() => setNewResource(true)}
                    onNewFunction={() => setNewFunction(true)}
                    onNewAgent={() => setNewAgent(true)}
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

                <Show when={agentName()} keyed>
                  {(name) => (
                    <Show
                      when={agentEntry(name)}
                      fallback={
                        <div class="mx-auto max-w-3xl px-6 py-16">
                          <EmptyState
                            title="That agent is gone"
                            description="It was deleted. Pick another from the sidebar."
                          />
                        </div>
                      }
                    >
                      {(found) => <AgentPage entry={found()} />}
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
            <Show when={newAgent()}>
              <NewAgentDialog onClose={() => setNewAgent(false)} />
            </Show>
          </div>
        )}
      </Show>

      <Show when={switchRequest()}>
        {(selection) => (
          <Modal
            title="Save current app before opening another?"
            subtitle={`Open ${describeSelection(selection())} after this.`}
            onClose={() => setSwitchRequest(null)}
            width="30rem"
          >
            <div class="grid gap-4">
              <p class="text-sm leading-relaxed text-muted">
                {studio.project?.name} has {changes().length} unsaved file{changes().length === 1 ? "" : "s"}.
              </p>
              <div class="flex justify-end gap-2">
                <Button onClick={() => setSwitchRequest(null)}>Cancel</Button>
                <Button variant="danger" onClick={() => void discardAndOpen()}>
                  Open without saving
                </Button>
                <Button variant="primary" disabled={studio.saving} onClick={() => void saveAndOpen()}>
                  {studio.saving ? "Saving…" : "Save and open"}
                </Button>
              </div>
            </div>
          </Modal>
        )}
      </Show>

      <Show when={droppedChoices()}>
        {(selection) => (
          <Modal
            title={`Apps found in ${selection().parentName}`}
            subtitle="Pick the app directory to open."
            onClose={() => setDroppedChoices(null)}
            width="28rem"
          >
            <div class="grid gap-2">
              <For each={selection().candidates}>
                {(candidate) => (
                  <button
                    type="button"
                    onClick={() => void openCandidate(candidate)}
                    class="group flex items-center justify-between rounded-lg border border-line bg-surface px-4 py-3 text-left transition-colors hover:border-line-strong hover:bg-surface-2"
                  >
                    <span class="font-mono text-[0.8125rem] text-ink">{candidate.name}</span>
                    <span class="text-xs text-faint transition-colors group-hover:text-accent">Open →</span>
                  </button>
                )}
              </For>
            </div>
          </Modal>
        )}
      </Show>

      <Show when={dropActive()}>
        <div class="pointer-events-none fixed inset-0 z-[55] flex items-center justify-center bg-black/55 backdrop-blur-sm">
          <div class="rounded-2xl border border-accent-line bg-surface/95 px-8 py-6 text-center shadow-2xl shadow-black/40">
            <p class="text-xs font-semibold uppercase tracking-[0.14em] text-accent">Drop to open</p>
            <p class="mt-2 text-sm text-ink">Drop an app directory or a parent folder anywhere on the page.</p>
          </div>
        </div>
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
    </>
  );
}
