import { For, Show, createSignal } from "solid-js";
import { Badge, Button, Leaf, Mono } from "./ui";
import { isSupported } from "../lib/fs";
import { chooseDirectory, openProject, studio, toast, type AppCandidate } from "../lib/store";

/** What the studio does with the directory, said plainly before it asks for it. */
const PROMISES = [
  {
    title: "Reads your app directory",
    body: "main.toml, every models/*.toml, and everything under functions/ — sources, per-function config and the built libraries.",
  },
  {
    title: "Edits resources and functions",
    body: "Fields, permissions, multitenancy, lifecycle hooks, and new function scaffolds in Rust, C, Zig or Go.",
  },
  {
    title: "Writes back only what changed",
    body: "Edits stay in the browser until you save. Nothing is uploaded — there is no server behind this page.",
  },
];

export function Landing() {
  const [candidates, setCandidates] = createSignal<AppCandidate[] | null>(null);
  const [parentName, setParentName] = createSignal("");
  const [error, setError] = createSignal<string | null>(null);
  const supported = isSupported();

  const browse = async () => {
    setError(null);
    try {
      const result = await chooseDirectory();
      if (!result) return;
      if (result.kind === "app") {
        await openProject(result.handle);
        return;
      }
      setParentName(result.parent.name);
      if (result.candidates.length === 0) {
        setCandidates([]);
        setError(
          `No apiplant app found in ${result.parent.name}. An app directory holds a main.toml, a models/ folder, or a functions/ folder.`,
        );
        return;
      }
      if (result.candidates.length === 1) {
        await openProject(result.candidates[0].handle);
        return;
      }
      setCandidates(result.candidates);
    } catch (failure) {
      const message = failure instanceof Error ? failure.message : String(failure);
      setError(message);
      toast(message, "error");
    }
  };

  return (
    <div class="relative z-10 mx-auto flex min-h-screen w-full max-w-5xl flex-col justify-center px-6 py-16">
      <div class="animate-rise">
        <div class="flex items-center gap-3">
          <Leaf class="h-10 w-10 text-accent" />
          <div>
            <h1 class="text-2xl font-semibold tracking-tight">apiplant studio</h1>
            <p class="text-sm text-muted">A local editor for an apiplant app directory.</p>
          </div>
        </div>

        <p class="mt-8 max-w-2xl text-[0.9375rem] leading-relaxed text-muted">
          Point it at the folder you would hand to the <Mono>apiplant</Mono> binary. It loads the resources,
          permissions, hooks and functions that folder declares, lets you edit them as forms or as TOML, and
          writes the result straight back to disk.
        </p>

        <div class="mt-8 flex flex-wrap items-center gap-3">
          <Button variant="primary" onClick={browse} disabled={!supported || studio.loading}>
            <svg width="15" height="15" viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.5">
              <path
                d="M1.75 12.25V4.5c0-.55.45-1 1-1h3.1c.32 0 .62.15.81.4l.78 1.05c.19.25.49.4.81.4h4c.55 0 1 .45 1 1v5.9c0 .55-.45 1-1 1H2.75c-.55 0-1-.45-1-1z"
                stroke-linejoin="round"
              />
            </svg>
            {studio.loading ? "Opening…" : "Open app directory"}
          </Button>
          <span class="text-xs text-faint">
            or pick a parent folder — <Mono>examples/</Mono> works — and choose the app inside it.
          </span>
        </div>

        <Show when={!supported}>
          <div class="mt-6 max-w-2xl rounded-lg border border-[#4a3c1a] bg-[#2c2413]/60 px-4 py-3 text-xs leading-relaxed text-warn">
            This browser has no File System Access API, so the studio cannot read or write a local directory.
            Chrome, Edge, Opera or Arc (desktop) support it; Firefox and Safari do not yet.
          </div>
        </Show>

        <Show when={error()}>
          {(message) => (
            <div class="mt-6 max-w-2xl rounded-lg border border-[#4a2626] bg-[#2c1818]/60 px-4 py-3 text-xs leading-relaxed text-danger">
              {message()}
            </div>
          )}
        </Show>

        <Show when={candidates()?.length}>
          <div class="mt-8 max-w-2xl">
            <h2 class="text-xs font-semibold uppercase tracking-[0.08em] text-faint">
              Apps found in {parentName()}
            </h2>
            <div class="mt-3 grid gap-2">
              <For each={candidates()!}>
                {(candidate) => (
                  <button
                    type="button"
                    onClick={() => openProject(candidate.handle)}
                    class="group flex items-center justify-between rounded-lg border border-line bg-surface px-4 py-3 text-left transition-colors hover:border-line-strong hover:bg-surface-2"
                  >
                    <span class="font-mono text-[0.8125rem] text-ink">{candidate.name}</span>
                    <span class="text-xs text-faint transition-colors group-hover:text-accent">Open →</span>
                  </button>
                )}
              </For>
            </div>
          </div>
        </Show>

        <div class="mt-14 grid gap-px overflow-hidden rounded-xl border border-line bg-line sm:grid-cols-3">
          <For each={PROMISES}>
            {(promise) => (
              <div class="bg-surface p-4">
                <h3 class="text-[0.8125rem] font-semibold text-ink">{promise.title}</h3>
                <p class="mt-1.5 text-xs leading-relaxed text-muted">{promise.body}</p>
              </div>
            )}
          </For>
        </div>

        <div class="mt-6 flex items-center gap-2 text-xs text-faint">
          <Badge tone="accent">local only</Badge>
          <span>
            The page holds a handle to the folder you choose and nothing else. Closing the tab drops it.
          </span>
        </div>
      </div>
    </div>
  );
}
