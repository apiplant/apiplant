import { For, Show, createMemo, createSignal } from "solid-js";
import {
  addFunctionConfig,
  addFunctionFile,
  configValue,
  deleteFunction,
  fileState,
  fileText,
  functionExports,
  setFileText,
  studio,
} from "../lib/store";
import { formatBytes } from "../lib/fs";
import { libraryName } from "../lib/functions";
import { LANGUAGE_LABEL, type FunctionEntry } from "../lib/types";
import { Badge, Button, Card, CardHeader, CodeEditor, EmptyState, Modal, Mono, TextInput } from "./ui";

const EDITOR_LANGUAGE: Record<string, string> = {
  rs: "rust",
  ts: "typescript",
  js: "javascript",
  mjs: "javascript",
  cjs: "javascript",
  json: "json",
  c: "c",
  h: "c",
  zig: "zig",
  go: "go",
  toml: "toml",
  mod: "go.mod",
  sum: "go.sum",
};

function extensionOf(path: string): string {
  const base = path.slice(path.lastIndexOf("/") + 1);
  const dot = base.lastIndexOf(".");
  return dot > 0 ? base.slice(dot + 1) : "";
}

export function FunctionPage(props: { entry: FunctionEntry }) {
  const [selected, setSelected] = createSignal<string | null>(null);
  const [confirming, setConfirming] = createSignal(false);
  const [addingFile, setAddingFile] = createSignal(false);
  const [newFileName, setNewFileName] = createSignal("");

  /** Sources plus configs, since both are just files you edit here. */
  const files = createMemo(() => {
    const list = props.entry.files.map((file) => file.path);
    for (const config of props.entry.configs) list.push(config.path);
    return list;
  });

  const active = createMemo(() => {
    const current = selected();
    if (current && files().includes(current)) return current;
    return files()[0] ?? null;
  });

  const exports = createMemo(() => functionExports(props.entry));

  const basePath = () => {
    const raw = configValue("server", "base_path");
    return typeof raw === "string" && raw ? raw.replace(/\/$/, "") : "";
  };

  const staleLibrary = createMemo(() => {
    if (!props.entry.libPath) return false;
    // A source edited in this session is, by definition, newer than the library.
    return props.entry.files.some((file) => {
      const state = fileState(file.path);
      return !!state && state.current !== state.original;
    });
  });

  const addFile = () => {
    const name = newFileName().trim();
    if (!name) return;
    addFunctionFile(props.entry.name, name, "");
    setSelected(`functions/${props.entry.name}/${name}`);
    setNewFileName("");
    setAddingFile(false);
  };

  return (
    <div class="animate-rise mx-auto w-full max-w-5xl px-6 py-6">
      <header class="mb-5">
        <div class="flex flex-wrap items-center gap-2.5">
          <h1 class="font-mono text-xl font-semibold tracking-tight">{props.entry.name}</h1>
          <Badge tone="info">{LANGUAGE_LABEL[props.entry.language]}</Badge>
          <Badge>{props.entry.layout === "directory" ? "directory" : "single file"}</Badge>
          <Show
            when={props.entry.libPath}
            fallback={<Badge tone="warn">not built</Badge>}
          >
            <Badge tone={staleLibrary() ? "warn" : "accent"}>
              {staleLibrary() ? "build out of date" : `built · ${formatBytes(props.entry.libSize)}`}
            </Badge>
          </Show>
        </div>

        <p class="mt-2 max-w-3xl text-xs leading-relaxed text-muted">
          Builds to <Mono>functions/{libraryName(props.entry.name, props.entry.language)}</Mono> with{" "}
          <Mono>apiplant build &lt;app&gt;</Mono>
          <Show when={props.entry.language === "typescript"}>
            {" "}
            — JavaScript the server runs in a V8 isolate, not a shared library
          </Show>
          . Every function it exports is mounted at{" "}
          <Mono>{basePath()}/functions/&lt;name&gt;</Mono> with the method and visibility its manifest
          declares — unless it is Private, which keeps it available to lifecycle hooks only.
        </p>

        <div class="mt-3 flex flex-wrap items-center gap-2">
          <span class="text-[0.6875rem] uppercase tracking-wide text-faint">exports</span>
          <For each={exports()}>
            {(name) => (
              <Mono>
                {name}
                <Show when={usedAsHook(name)}>
                  <span class="ml-1.5 text-accent">hook</span>
                </Show>
              </Mono>
            )}
          </For>
        </div>
      </header>

      <div class="grid gap-4 lg:grid-cols-[15rem_1fr]">
        <div class="space-y-3">
          <Card>
            <CardHeader title="Files">
              <Show when={props.entry.layout === "directory"}>
                <Button size="sm" variant="ghost" onClick={() => setAddingFile(true)}>
                  Add
                </Button>
              </Show>
            </CardHeader>
            <div class="p-1.5">
              <For each={files()}>
                {(path) => {
                  const state = () => fileState(path);
                  const dirty = () => {
                    const current = state();
                    return !!current && current.current !== current.original;
                  };
                  return (
                    <button
                      type="button"
                      onClick={() => setSelected(path)}
                      class={[
                        "flex w-full items-center gap-2 rounded-md px-2 py-1.5 text-left font-mono text-[0.75rem] transition-colors",
                        active() === path ? "bg-surface-3 text-ink" : "text-muted hover:bg-surface-2 hover:text-ink",
                      ].join(" ")}
                    >
                      <span class="flex-1 truncate" title={path}>
                        {path.slice("functions/".length)}
                      </span>
                      <Show when={dirty()}>
                        <span class="h-1.5 w-1.5 rounded-full bg-accent" />
                      </Show>
                    </button>
                  );
                }}
              </For>
              <Show when={props.entry.libPath}>
                <div class="mt-1 flex items-center gap-2 rounded-md px-2 py-1.5 font-mono text-[0.75rem] text-faint">
                  <span class="flex-1 truncate">{props.entry.libPath!.slice("functions/".length)}</span>
                  <span>{formatBytes(props.entry.libSize)}</span>
                </div>
              </Show>
            </div>
          </Card>

          <Card>
            <CardHeader title="Configuration" hint="functions/<function>.toml, keyed by function name." />
            <div class="space-y-2 px-3 py-3">
              <For each={exports()}>
                {(name) => {
                  const config = () => props.entry.configs.find((entry) => entry.name === name);
                  return (
                    <div class="flex items-center justify-between gap-2">
                      <Mono class="truncate">{name}.toml</Mono>
                      <Show
                        when={config()}
                        fallback={
                          <Button size="sm" variant="ghost" onClick={() => addFunctionConfig(props.entry.name, name)}>
                            create
                          </Button>
                        }
                      >
                        <Button size="sm" variant="ghost" onClick={() => setSelected(config()!.path)}>
                          edit
                        </Button>
                      </Show>
                    </div>
                  );
                }}
              </For>
            </div>
          </Card>

          <Card class="border-danger-line">
            <div class="flex items-center justify-between gap-2 px-3 py-3">
              <span class="text-xs text-muted">Delete function</span>
              <Show
                when={confirming()}
                fallback={
                  <Button size="sm" variant="danger" onClick={() => setConfirming(true)}>
                    Delete
                  </Button>
                }
              >
                <div class="flex gap-1">
                  <Button size="sm" variant="ghost" onClick={() => setConfirming(false)}>
                    Cancel
                  </Button>
                  <Button size="sm" variant="danger" onClick={() => deleteFunction(props.entry.name)}>
                    Confirm
                  </Button>
                </div>
              </Show>
            </div>
          </Card>
        </div>

        <div>
          <Show
            when={active()}
            fallback={
              <EmptyState title="No source files" description="This entry has nothing the studio can edit." />
            }
          >
            {(path) => (
              <>
                <div class="mb-2 flex items-center justify-between">
                  <Mono>{path()}</Mono>
                  <Show when={fileState(path())?.original === null}>
                    <Badge tone="accent">new file</Badge>
                  </Show>
                </div>
                <CodeEditor
                  language={EDITOR_LANGUAGE[extensionOf(path())] ?? extensionOf(path())}
                  value={fileText(path()) ?? ""}
                  onInput={(text) => setFileText(path(), text)}
                  minHeight="34rem"
                />
              </>
            )}
          </Show>
        </div>
      </div>

      <Show when={addingFile()}>
        <Modal
          title="Add a file"
          subtitle={`Into functions/${props.entry.name}/ — a second source file, a header, a helper module.`}
          onClose={() => setAddingFile(false)}
          width="26rem"
        >
          <TextInput
            mono
            value={newFileName()}
            placeholder={props.entry.language === "rust" ? "src/util.rs" : "util.c"}
            onInput={setNewFileName}
          />
          <div class="mt-4 flex justify-end gap-2">
            <Button variant="ghost" onClick={() => setAddingFile(false)}>
              Cancel
            </Button>
            <Button variant="primary" onClick={addFile} disabled={!newFileName().trim()}>
              Create
            </Button>
          </div>
        </Modal>
      </Show>
    </div>
  );
}

/** Whether any resource wires this function name into its lifecycle. */
function usedAsHook(name: string): boolean {
  return (studio.project?.resources ?? []).some((entry) =>
    Object.values(entry.resource.hooks).includes(name),
  );
}
