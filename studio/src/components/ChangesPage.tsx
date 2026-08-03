import { For, Show, createMemo, createSignal } from "solid-js";
import { pendingChanges, reloadProject, saveAll, studio, type ChangeKind } from "../lib/store";
import { Badge, Button, Card, CardHeader, CodeEditor, EmptyState, Mono } from "./ui";

const MARK: Record<ChangeKind, { glyph: string; class: string; label: string }> = {
  added: { glyph: "+", class: "text-accent", label: "new file" },
  modified: { glyph: "~", class: "text-warn", label: "modified" },
  deleted: { glyph: "−", class: "text-danger", label: "deleted" },
};

export function ChangesPage() {
  const [selected, setSelected] = createSignal<string | null>(null);
  const [confirmingDiscard, setConfirmingDiscard] = createSignal(false);

  const changes = createMemo(() => pendingChanges());
  const active = createMemo(() => {
    const current = selected();
    const list = changes();
    if (current && list.some((change) => change.path === current)) return current;
    return list[0]?.path ?? null;
  });
  const activeChange = createMemo(() => changes().find((change) => change.path === active()));
  const activeFile = createMemo(() => (active() ? studio.project?.files[active()!] : undefined));
  const dirDeletes = createMemo(() => studio.project?.pendingDirDeletes ?? []);

  return (
    <div class="animate-rise mx-auto w-full max-w-5xl px-6 py-6">
      <header class="mb-5 flex flex-wrap items-end justify-between gap-3">
        <div>
          <h1 class="text-xl font-semibold tracking-tight">Pending changes</h1>
          <p class="mt-2 max-w-2xl text-xs leading-relaxed text-muted">
            Nothing has touched the disk yet. Saving writes exactly these files into{" "}
            <Mono>{studio.project?.name}</Mono> and leaves everything else alone.
          </p>
        </div>
        <div class="flex items-center gap-2">
          <Show
            when={confirmingDiscard()}
            fallback={
              <Button
                variant="ghost"
                disabled={!changes().length}
                onClick={() => setConfirmingDiscard(true)}
              >
                Discard all
              </Button>
            }
          >
            <Button variant="ghost" onClick={() => setConfirmingDiscard(false)}>
              Cancel
            </Button>
            <Button
              variant="danger"
              onClick={() => {
                setConfirmingDiscard(false);
                void reloadProject();
              }}
            >
              Discard and re-read from disk
            </Button>
          </Show>
          <Button variant="primary" disabled={!changes().length || studio.saving} onClick={() => void saveAll()}>
            {studio.saving ? "Saving…" : `Save ${changes().length} file${changes().length === 1 ? "" : "s"}`}
          </Button>
        </div>
      </header>

      <Show
        when={changes().length}
        fallback={
          <EmptyState
            title="Everything is saved"
            description="The directory on disk matches what the studio is holding."
          />
        }
      >
        <div class="grid min-w-0 gap-4 lg:grid-cols-[20rem_minmax(0,1fr)]">
          <Card class="self-start">
            <CardHeader title="Files" />
            <div class="p-1.5">
              <For each={changes()}>
                {(change) => (
                  <button
                    type="button"
                    onClick={() => setSelected(change.path)}
                    class={[
                      "flex w-full items-center gap-2 rounded-md px-2 py-1.5 text-left font-mono text-[0.75rem] transition-colors",
                      active() === change.path
                        ? "bg-surface-3 text-ink"
                        : "text-muted hover:bg-surface-2 hover:text-ink",
                    ].join(" ")}
                  >
                    <span class={`w-2 shrink-0 text-center font-semibold ${MARK[change.kind].class}`}>
                      {MARK[change.kind].glyph}
                    </span>
                    <span class="flex-1 truncate" title={change.path}>
                      {change.path}
                    </span>
                  </button>
                )}
              </For>
            </div>
            <Show when={dirDeletes().length}>
              <div class="border-t border-line px-3 py-2">
                <p class="text-[0.6875rem] uppercase tracking-wide text-faint">directories removed</p>
                <For each={dirDeletes()}>
                  {(dir) => <p class="mt-1 font-mono text-[0.75rem] text-danger">{dir}</p>}
                </For>
              </div>
            </Show>
          </Card>

          <div class="min-w-0">
            <Show when={activeChange()}>
              {(change) => (
                <>
                  <div class="mb-2 flex items-center gap-2">
                    <Mono>{change().path}</Mono>
                    <Badge
                      tone={
                        change().kind === "added" ? "accent" : change().kind === "deleted" ? "danger" : "warn"
                      }
                    >
                      {MARK[change().kind].label}
                    </Badge>
                  </div>
                  <Show
                    when={change().kind !== "deleted"}
                    fallback={
                      <Card class="px-4 py-10 text-center text-xs text-muted">
                        This file will be removed from the directory.
                      </Card>
                    }
                  >
                    <CodeEditor
                      readOnly
                      value={activeFile()?.current ?? ""}
                      minHeight="30rem"
                      language={change().path.split(".").pop()}
                    />
                  </Show>
                </>
              )}
            </Show>
          </div>
        </div>
      </Show>
    </div>
  );
}
