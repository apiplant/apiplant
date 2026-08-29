import { For, Show, createMemo, createSignal } from "solid-js";
import { Badge, Button, HeadMark, Labelled, Modal, Mono, Switch, TextInput, ThemeToggle } from "./ui";
import { isSupported } from "../lib/fs";
import {
  APP_NAME_RULE,
  chooseDirectory,
  chooseParentDirectory,
  createProject,
  openProject,
  studio,
  toast,
  type AppCandidate,
} from "../lib/store";

/** What the studio does with the directory, said plainly before it asks for it. */
const PROMISES = [
  {
    title: "Reads your app directory",
    body: "main.toml, every resources/*.toml, every agents/*.toml, and everything under functions/ — sources, per-function config and the built libraries.",
  },
  {
    title: "Edits resources, agents and functions",
    body: "Fields, permissions, multitenancy, agent prompts and access, plus new function scaffolds in Rust, C, Zig or Go.",
  },
  {
    title: "Writes back only what changed",
    body: "Edits stay in the browser until you save. Nothing is uploaded — there is no server behind this page.",
  },
];

/** The parent folder chosen for a new app, held while its name is typed. */
interface NewAppTarget {
  handle: FileSystemDirectoryHandle;
  entryNames: string[];
}

function NewAppDialog(props: { target: NewAppTarget; onClose: () => void }) {
  const [name, setName] = createSignal("");
  const [withExample, setWithExample] = createSignal(true);
  const [busy, setBusy] = createSignal(false);

  const taken = createMemo(() => props.target.entryNames.includes(name()));
  const valid = createMemo(() => APP_NAME_RULE.test(name()) && !taken());

  const create = async () => {
    if (!valid() || busy()) return;
    setBusy(true);
    try {
      if (await createProject(props.target.handle, name(), { withExampleResource: withExample() })) {
        props.onClose();
      }
    } catch (failure) {
      toast(failure instanceof Error ? failure.message : String(failure), "error");
    } finally {
      setBusy(false);
    }
  };

  return (
    <Modal
      title="New app"
      subtitle={`A new directory inside ${props.target.handle.name}, laid out the way apiplant expects.`}
      onClose={props.onClose}
    >
      <div class="grid gap-4">
        <Labelled
          label="Directory name"
          hint="Also the docs title and the database name in the generated main.toml."
        >
          <TextInput
            value={name()}
            onInput={setName}
            placeholder="my-app"
            mono
            lowercase
            onKeyDown={(event: KeyboardEvent) => {
              if (event.key === "Enter") void create();
            }}
          />
        </Labelled>

        <Show when={name() && !valid()}>
          <p class="text-xs text-danger">
            {taken()
              ? `${props.target.handle.name} already has an entry named ${name()}.`
              : "Start with a letter or digit; then letters, digits, dot, underscore or dash."}
          </p>
        </Show>

        <Switch
          checked={withExample()}
          onChange={setWithExample}
          label="Add an example note resource"
        />

        <p class="text-xs leading-relaxed text-faint">
          The directory is created now; <Mono>main.toml</Mono> and the resources are staged like any other
          edit and written to disk when you press <strong>Save</strong>.
        </p>

        <div class="flex justify-end gap-2">
          <Button onClick={props.onClose}>Cancel</Button>
          <Button variant="primary" onClick={create} disabled={!valid() || busy()}>
            {busy() ? "Creating…" : "Create app"}
          </Button>
        </div>
      </div>
    </Modal>
  );
}

export function Landing(props: { rememberedProjectName?: string | null; onReopenRemembered?: () => void }) {
  const [candidates, setCandidates] = createSignal<AppCandidate[] | null>(null);
  const [parentName, setParentName] = createSignal("");
  const [error, setError] = createSignal<string | null>(null);
  const [newTarget, setNewTarget] = createSignal<NewAppTarget | null>(null);
  /** The last folder browsed into, so a new app can go straight in it. */
  const [parent, setParent] = createSignal<NewAppTarget | null>(null);
  const supported = isSupported();

  const startNew = async () => {
    setError(null);
    try {
      const target = await chooseParentDirectory();
      if (target) setNewTarget(target);
    } catch (failure) {
      const message = failure instanceof Error ? failure.message : String(failure);
      setError(message);
      toast(message, "error");
    }
  };

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
      setParent({ handle: result.parent, entryNames: result.parentEntryNames });
      if (result.candidates.length === 0) {
        setCandidates([]);
        setError(
          `No apiplant app found in ${result.parent.name}. An app directory holds a main.toml, a resources/ folder, an agents/ folder, or a functions/ folder.`,
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
        <div class="flex items-start justify-between gap-6">
          <div class="flex items-center gap-4">
            <HeadMark class="h-16" />
            <div>
              <h1 class="text-2xl font-semibold tracking-tight">
                apiplant <span class="text-accent">studio</span>
              </h1>
              <p class="text-sm text-muted">A local editor for an apiplant app directory.</p>
            </div>
          </div>
          <ThemeToggle />
        </div>

        <p class="mt-8 max-w-2xl text-[0.9375rem] leading-relaxed text-muted">
          Point it at the folder you would hand to the <Mono>apiplant</Mono> binary. It loads the resources,
          permissions, hooks, agents and functions the folder declares, edits them as forms or TOML,
          and writes the result back to disk.
        </p>

        <Show when={props.rememberedProjectName && props.onReopenRemembered}>
          <div class="mt-6 inline-flex max-w-2xl items-center gap-3 rounded-xl border border-line bg-surface px-4 py-3 text-xs text-muted">
            <Badge tone="accent">remembered</Badge>
            <span>
              Last app: <Mono>{props.rememberedProjectName}</Mono>
            </span>
            <Button size="sm" variant="ghost" onClick={props.onReopenRemembered}>
              Reopen
            </Button>
          </div>
        </Show>

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
          <Button onClick={startNew} disabled={!supported || studio.loading}>
            <svg width="15" height="15" viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.5">
              <path
                d="M1.75 12.25V4.5c0-.55.45-1 1-1h3.1c.32 0 .62.15.81.4l.78 1.05c.19.25.49.4.81.4h4c.55 0 1 .45 1 1v5.9c0 .55-.45 1-1 1H2.75c-.55 0-1-.45-1-1z"
                stroke-linejoin="round"
              />
              <path d="M8 7.4v3.2M6.4 9h3.2" stroke-linecap="round" />
            </svg>
            New app directory
          </Button>
          <span class="text-xs text-faint">
            or drag an app directory or parent folder onto the page. A parent folder (e.g. <Mono>examples/</Mono>)
            lets you choose the app inside it.
          </span>
        </div>

        <Show when={!supported}>
          <div class="mt-6 max-w-2xl rounded-lg border border-warn-line bg-warn-soft px-4 py-3 text-xs leading-relaxed text-warn">
            This browser has no File System Access API, so the studio cannot read or write a local directory.
            Chrome, Edge, Opera or Arc (desktop) support it; Firefox and Safari do not yet.
          </div>
        </Show>

        <Show when={error()}>
          {(message) => (
            <div class="mt-6 max-w-2xl rounded-lg border border-danger-line bg-danger-soft px-4 py-3 text-xs leading-relaxed text-danger">
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
                    onClick={() =>
                      void openProject(candidate.handle).catch((failure) =>
                        toast(failure instanceof Error ? failure.message : String(failure), "error"),
                      )
                    }
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

        <Show when={parent()}>
          {(target) => (
            <div class="mt-4 flex items-center gap-3 text-xs text-faint">
              <Button onClick={() => setNewTarget(target())}>New app in {target().handle.name}</Button>
              <span>Creates a fresh app directory inside the folder you just browsed.</span>
            </div>
          )}
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
            The page holds a handle to the folder you choose, remembers the last one in this browser, and keeps
            everything on your machine.
          </span>
        </div>
      </div>

      <Show when={newTarget()}>
        {(target) => <NewAppDialog target={target()} onClose={() => setNewTarget(null)} />}
      </Show>
    </div>
  );
}
