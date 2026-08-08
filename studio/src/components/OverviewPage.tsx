import { For, Show, createMemo } from "solid-js";
import { setView } from "../lib/nav";
import { configValue, functionExports, pendingChanges, studio, toast, type Project } from "../lib/store";
import { LANGUAGE_LABEL } from "../lib/types";
import { Badge, Button, Card, CardHeader, Mono } from "./ui";

function Stat(props: { label: string; value: string | number; hint?: string }) {
  return (
    <div class="px-4 py-3">
      <p class="text-[0.6875rem] font-semibold uppercase tracking-[0.08em] text-faint">{props.label}</p>
      <p class="mt-1 text-xl font-semibold tracking-tight text-ink">{props.value}</p>
      <Show when={props.hint}>
        <p class="mt-0.5 text-[0.6875rem] text-muted">{props.hint}</p>
      </Show>
    </div>
  );
}

function copy(text: string) {
  navigator.clipboard
    ?.writeText(text)
    .then(() => toast("Copied to clipboard", "success"))
    .catch(() => toast("Clipboard unavailable", "error"));
}

export function OverviewPage(props: {
  project: Project;
  onNewResource: () => void;
  onNewFunction: () => void;
  onNewAgent: () => void;
}) {
  const custom = createMemo(() => props.project.resources.filter((entry) => !entry.builtin));
  const overridden = createMemo(() => props.project.resources.filter((entry) => entry.builtin && entry.path));
  const hooked = createMemo(() =>
    props.project.resources.filter((entry) => Object.keys(entry.resource.hooks).length > 0),
  );
  const unbuilt = createMemo(() => props.project.functions.filter((entry) => !entry.libPath));
  const storedAgents = createMemo(() => props.project.agents.filter((entry) => entry.storageEnabled));
  const changes = createMemo(() => pendingChanges());

  const url = createMemo(() => {
    const host = configValue("server", "host") ?? "127.0.0.1";
    const port = configValue("server", "port") ?? 8080;
    const base = String(configValue("server", "base_path") ?? "").replace(/\/$/, "");
    const scheme = props.project.hasTls ? "https" : "http";
    return `${scheme}://${host === "0.0.0.0" ? "127.0.0.1" : host}:${port}${base}`;
  });

  const docsUrl = createMemo(() => {
    if (configValue("docs", "enabled") === false) return null;
    const path = String(configValue("docs", "path") ?? "/docs");
    return `${url()}${path.startsWith("/") ? path : `/${path}`}`;
  });

  return (
    <div class="animate-rise mx-auto w-full max-w-5xl px-6 py-6">
      <header class="mb-5">
        <div class="flex flex-wrap items-center gap-2.5">
          <h1 class="text-xl font-semibold tracking-tight">{props.project.name}</h1>
          <Badge tone="accent">app directory</Badge>
          <Show when={props.project.hasTls}>
            <Badge tone="info">TLS</Badge>
          </Show>
        </div>
        <p class="mt-2 max-w-3xl text-xs leading-relaxed text-muted">
          Everything below is read from this folder. Run it with{" "}
          <Mono>apiplant run {props.project.name}</Mono> and the API answers at <Mono>{url()}</Mono>
          <Show when={docsUrl()}>
            {" "}
            with Swagger UI at <Mono>{docsUrl()}</Mono>
          </Show>
          .
        </p>
      </header>

      <div class="grid gap-3 lg:grid-cols-3">
        <Card class="lg:col-span-2">
          <div class="grid grid-cols-2 divide-x divide-line sm:grid-cols-5">
            <Stat
              label="Resources"
              value={custom().length}
              hint={`+ ${props.project.resources.length - custom().length} built-in`}
            />
            <Stat label="Functions" value={props.project.functions.length} hint={`${unbuilt().length} not built`} />
            <Stat label="Agents" value={props.project.agents.length} hint={`${storedAgents().length} stored`} />
            <Stat label="With hooks" value={hooked().length} hint="resources running custom logic" />
            <Stat label="Unsaved" value={changes().length} hint="files pending write" />
          </div>
        </Card>

        <Card>
          <CardHeader title="Next steps" />
          <div class="space-y-2 px-4 py-3">
            <For
              each={[
                { label: "Add a resource", action: props.onNewResource },
                { label: "Add an agent", action: props.onNewAgent },
                { label: "Add a function", action: props.onNewFunction },
                { label: "Review configuration", action: () => setView({ kind: "config" }) },
                { label: "Review pending changes", action: () => setView({ kind: "changes" }) },
              ]}
            >
              {(item) => (
                <button
                  type="button"
                  onClick={item.action}
                  class="block w-full rounded-md px-2 py-1.5 text-left text-xs text-muted transition-colors hover:bg-surface-2 hover:text-ink"
                >
                  {item.label} →
                </button>
              )}
            </For>
          </div>
        </Card>
      </div>

      <div class="mt-3 grid gap-3 lg:grid-cols-2">
        <Card>
          <CardHeader title="Resources" hint="Your resources, plus any built-in you have overridden." />
          <Show
            when={custom().length || overridden().length}
            fallback={
              <p class="px-4 py-8 text-center text-xs text-muted">
                No resources on disk — the app serves only the built-in auth and tenancy resources.
              </p>
            }
          >
            <div class="divide-y divide-line">
              <For each={[...custom(), ...overridden()]}>
                {(entry) => (
                  <button
                    type="button"
                    onClick={() => setView({ kind: "resource", name: entry.name })}
                    class="flex w-full items-center gap-3 px-4 py-2.5 text-left transition-colors hover:bg-surface-2/50"
                  >
                    <span class="flex-1 truncate font-mono text-[0.8125rem] text-ink">{entry.name}</span>
                    <Show when={Object.keys(entry.resource.hooks).length}>
                      <span class="text-[0.6875rem] text-accent">
                        {Object.keys(entry.resource.hooks).length} hooks
                      </span>
                    </Show>
                    <span class="text-[0.6875rem] text-faint">{entry.resource.fields.length} fields</span>
                    <span class="w-16 text-right text-[0.6875rem] text-faint">
                      {entry.resource.scope === "global" ? "global" : "org"}
                    </span>
                  </button>
                )}
              </For>
            </div>
          </Show>
        </Card>

        <Card>
          <CardHeader title="Functions" hint="Every library in functions/, and what it exports." />
          <Show
            when={props.project.functions.length}
            fallback={
              <p class="px-4 py-8 text-center text-xs text-muted">
                No functions yet. They mount their own endpoints and back the lifecycle hooks.
              </p>
            }
          >
            <div class="divide-y divide-line">
              <For each={props.project.functions}>
                {(entry) => (
                  <button
                    type="button"
                    onClick={() => setView({ kind: "function", name: entry.name })}
                    class="flex w-full items-center gap-3 px-4 py-2.5 text-left transition-colors hover:bg-surface-2/50"
                  >
                    <span class="flex-1 truncate font-mono text-[0.8125rem] text-ink">{entry.name}</span>
                    <span class="truncate text-[0.6875rem] text-faint">
                      {functionExports(entry).join(", ")}
                    </span>
                    <span class="w-10 text-right text-[0.6875rem] text-muted">
                      {LANGUAGE_LABEL[entry.language]}
                    </span>
                    <span class="w-16 text-right text-[0.6875rem]">
                      <Show when={entry.libPath} fallback={<span class="text-warn">not built</span>}>
                        <span class="text-faint">built</span>
                      </Show>
                    </span>
                  </button>
                )}
              </For>
            </div>
          </Show>
        </Card>
      </div>

      <Card class="mt-3">
        <CardHeader title="Agents" hint="Configured assistants in agents/, with their own prompt and access." />
        <Show
          when={props.project.agents.length}
          fallback={
            <p class="px-4 py-8 text-center text-xs text-muted">
              No agents yet. Add one to expose a named assistant under <Mono>/ai/agents/&lt;name&gt;/chat</Mono>.
            </p>
          }
        >
          <div class="divide-y divide-line">
            <For each={props.project.agents}>
              {(entry) => (
                <button
                  type="button"
                  onClick={() => setView({ kind: "agent", name: entry.name })}
                  class="flex w-full items-center gap-3 px-4 py-2.5 text-left transition-colors hover:bg-surface-2/50"
                >
                  <span class="flex-1 truncate font-mono text-[0.8125rem] text-ink">{entry.name}</span>
                  <span class="truncate text-[0.6875rem] text-faint">{entry.chat}</span>
                  <span class="w-16 text-right text-[0.6875rem] text-muted">
                    {entry.storageEnabled ? "stored" : "live"}
                  </span>
                  <span class="w-24 text-right text-[0.6875rem] text-faint">{entry.scope}</span>
                </button>
              )}
            </For>
          </div>
        </Show>
      </Card>

      <Card class="mt-3">
        <CardHeader
          title="Running this app"
          hint="The studio only edits files; building and serving stay with the CLI."
        />
        <div class="space-y-2 px-4 py-4">
          <For
            each={[
              {
                command: "cargo install apiplant",
                note: "install the CLI (once, from crates.io)",
              },
              {
                command:
                  "docker run -d --name apiplant-postgres -e POSTGRES_HOST_AUTH_METHOD=trust -p 5432:5432 postgres:16",
                note: "Postgres on the port the app expects",
              },
              {
                command: `apiplant check ${props.project.name}`,
                note: "load and validate the app, then exit",
              },
              {
                command: `apiplant build --release ${props.project.name}`,
                note: "build every source in functions/ into what the server loads",
              },
              { command: `apiplant run ${props.project.name}`, note: "migrate, mount and serve" },
            ]}
          >
            {(item) => (
              <div class="flex items-center gap-3 rounded-lg border border-line bg-editor-bg px-3 py-2">
                <code class="flex-1 font-mono text-[0.78125rem] text-ink">
                  <span class="text-faint">$ </span>
                  {item.command}
                </code>
                <span class="hidden text-[0.6875rem] text-faint sm:block">{item.note}</span>
                <Button size="sm" variant="ghost" onClick={() => copy(item.command)}>
                  copy
                </Button>
              </div>
            )}
          </For>
          <Show when={unbuilt().length}>
            <p class="pt-1 text-[0.6875rem] leading-relaxed text-warn">
              {unbuilt().length} function
              {unbuilt().length === 1 ? " has" : "s have"} not been built yet — until{" "}
              <Mono>apiplant build</Mono> runs, those endpoints are not mounted and any hook pointing at them
              fails closed.
            </p>
          </Show>
        </div>
      </Card>

      <Show when={studio.project?.problems.length}>
        <Card class="mt-3 border-danger-line">
          <CardHeader title="Files that did not parse" hint="Left untouched; fix them on disk or in the editor." />
          <div class="divide-y divide-line">
            <For each={studio.project!.problems}>
              {(problem) => (
                <div class="px-4 py-2.5">
                  <p class="font-mono text-[0.75rem] text-danger">{problem.path}</p>
                  <p class="mt-0.5 text-[0.6875rem] text-muted">{problem.message}</p>
                </div>
              )}
            </For>
          </div>
        </Card>
      </Show>
    </div>
  );
}
