import { For, Show, createMemo, createSignal, type JSX } from "solid-js";
import { isActive, setView, view, type View } from "../lib/nav";
import { pendingChanges, studio, type Project } from "../lib/store";
import { LANGUAGE_LABEL, type FunctionEntry, type ResourceEntry } from "../lib/types";
import { Badge } from "./ui";

function NavItem(props: {
  target: View;
  label: string;
  mono?: boolean;
  dirty?: boolean;
  trailing?: () => JSX.Element;
  icon?: () => JSX.Element;
}) {
  const active = () => isActive(view(), props.target);
  return (
    <button
      type="button"
      onClick={() => setView(props.target)}
      class={[
        "group flex w-full items-center gap-2 rounded-lg px-2.5 py-1.5 text-left text-[0.8125rem] transition-colors duration-100",
        active() ? "bg-surface-3 text-ink" : "text-muted hover:bg-surface-2 hover:text-ink",
      ].join(" ")}
    >
      <Show when={props.icon}>
        <span class={active() ? "text-accent" : "text-faint"}>{props.icon!()}</span>
      </Show>
      <span class={`flex-1 truncate ${props.mono ? "font-mono text-[0.78125rem]" : ""}`}>{props.label}</span>
      <Show when={props.dirty}>
        <span class="h-1.5 w-1.5 shrink-0 rounded-full bg-accent" title="Unsaved changes" />
      </Show>
      <Show when={props.trailing}>{props.trailing!()}</Show>
    </button>
  );
}

function SectionHeader(props: { label: string; count?: number; onAdd?: () => void; addLabel?: string }) {
  return (
    <div class="mb-1 mt-5 flex items-center justify-between px-2.5">
      <h2 class="text-[0.6875rem] font-semibold uppercase tracking-[0.09em] text-faint">
        {props.label}
        <Show when={props.count !== undefined}>
          <span class="ml-1.5 font-normal text-faint/70">{props.count}</span>
        </Show>
      </h2>
      <Show when={props.onAdd}>
        <button
          type="button"
          onClick={props.onAdd}
          title={props.addLabel}
          class="rounded p-0.5 text-faint transition-colors hover:bg-surface-2 hover:text-accent"
        >
          <svg width="14" height="14" viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.6">
            <path d="M8 3.5v9M3.5 8h9" stroke-linecap="round" />
          </svg>
        </button>
      </Show>
    </div>
  );
}

export function Sidebar(props: {
  project: Project;
  onNewResource: () => void;
  onNewFunction: () => void;
}) {
  const [filter, setFilter] = createSignal("");

  const dirtyPaths = createMemo(() => new Set(pendingChanges().map((change) => change.path)));
  const matches = (name: string) => name.toLowerCase().includes(filter().trim().toLowerCase());

  const custom = createMemo(() =>
    props.project.resources.filter((entry) => !entry.builtin && matches(entry.name)),
  );
  const builtins = createMemo(() =>
    props.project.resources.filter((entry) => entry.builtin && matches(entry.name)),
  );
  const functions = createMemo(() => props.project.functions.filter((entry) => matches(entry.name)));

  const resourceDirty = (entry: ResourceEntry) => !!entry.path && dirtyPaths().has(entry.path);
  const functionDirty = (entry: FunctionEntry) =>
    entry.files.some((file) => dirtyPaths().has(file.path)) ||
    entry.configs.some((config) => dirtyPaths().has(config.path));

  return (
    <nav class="flex h-full w-64 shrink-0 flex-col border-r border-line bg-surface/40">
      <div class="px-3 pt-3">
        <NavItem
          target={{ kind: "overview" }}
          label="Overview"
          icon={() => (
            <svg width="14" height="14" viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.4">
              <rect x="2.25" y="2.25" width="5" height="5" rx="1" />
              <rect x="8.75" y="2.25" width="5" height="5" rx="1" />
              <rect x="2.25" y="8.75" width="5" height="5" rx="1" />
              <rect x="8.75" y="8.75" width="5" height="5" rx="1" />
            </svg>
          )}
        />
        <NavItem
          target={{ kind: "config" }}
          label="Configuration"
          dirty={dirtyPaths().has("main.toml")}
          icon={() => (
            <svg width="14" height="14" viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.4">
              <circle cx="8" cy="8" r="2.25" />
              <path d="M8 1.75v1.5M8 12.75v1.5M14.25 8h-1.5M3.25 8h-1.5M12.42 3.58l-1.06 1.06M4.64 11.36l-1.06 1.06M12.42 12.42l-1.06-1.06M4.64 4.64L3.58 3.58" stroke-linecap="round" />
            </svg>
          )}
        />
      </div>

      <div class="px-3 pt-4">
        <div class="relative">
          <svg
            class="pointer-events-none absolute left-2.5 top-1/2 -translate-y-1/2 text-faint"
            width="13"
            height="13"
            viewBox="0 0 16 16"
            fill="none"
            stroke="currentColor"
            stroke-width="1.5"
          >
            <circle cx="7.25" cy="7.25" r="4.25" />
            <path d="M10.5 10.5l3 3" stroke-linecap="round" />
          </svg>
          <input
            class="input pl-8 text-xs"
            placeholder="Filter resources & functions"
            value={filter()}
            spellcheck={false}
            onInput={(event) => setFilter(event.currentTarget.value)}
          />
        </div>
      </div>

      <div class="mt-1 flex-1 overflow-y-auto px-3 pb-6">
        <SectionHeader
          label="Resources"
          count={custom().length}
          onAdd={props.onNewResource}
          addLabel="New resource"
        />
        <Show
          when={custom().length}
          fallback={
            <p class="px-2.5 py-1 text-xs leading-relaxed text-faint">
              No models yet — the app runs on the built-ins below.
            </p>
          }
        >
          <For each={custom()}>
            {(entry) => (
              <NavItem
                target={{ kind: "resource", name: entry.name }}
                label={entry.name}
                mono
                dirty={resourceDirty(entry)}
                trailing={
                  entry.resource.scope === "global"
                    ? () => <span class="text-[0.625rem] text-faint">global</span>
                    : undefined
                }
              />
            )}
          </For>
        </Show>

        <SectionHeader label="Built-ins" count={builtins().length} />
        <For each={builtins()}>
          {(entry) => (
            <NavItem
              target={{ kind: "resource", name: entry.name }}
              label={entry.name}
              mono
              dirty={resourceDirty(entry)}
              trailing={
                entry.path
                  ? () => <span class="text-[0.625rem] text-accent/70">override</span>
                  : () => <span class="text-[0.625rem] text-faint">default</span>
              }
            />
          )}
        </For>

        <SectionHeader
          label="Functions"
          count={props.project.functions.length}
          onAdd={props.onNewFunction}
          addLabel="New function"
        />
        <Show
          when={functions().length}
          fallback={
            <p class="px-2.5 py-1 text-xs leading-relaxed text-faint">
              No functions. Add one to mount custom logic or a lifecycle hook.
            </p>
          }
        >
          <For each={functions()}>
            {(entry) => (
              <NavItem
                target={{ kind: "function", name: entry.name }}
                label={entry.name}
                mono
                dirty={functionDirty(entry)}
                trailing={() => (
                  <span class="text-[0.625rem] text-faint">{LANGUAGE_LABEL[entry.language]}</span>
                )}
              />
            )}
          </For>
        </Show>

        <Show when={studio.project?.problems.length}>
          <div class="mt-6 rounded-lg border border-danger-line bg-danger-soft px-3 py-2">
            <p class="text-[0.6875rem] font-semibold uppercase tracking-wide text-danger">
              {studio.project!.problems.length} file(s) failed to parse
            </p>
            <For each={studio.project!.problems}>
              {(problem) => (
                <p class="mt-1 font-mono text-[0.6875rem] leading-relaxed text-danger/80">{problem.path}</p>
              )}
            </For>
          </div>
        </Show>

        <Show when={props.project.orphanConfigs.length}>
          <div class="mt-4 px-2.5">
            <Badge tone="warn">unmatched config</Badge>
            <For each={props.project.orphanConfigs}>
              {(path) => <p class="mt-1 font-mono text-[0.6875rem] text-faint">{path}</p>}
            </For>
          </div>
        </Show>
      </div>
    </nav>
  );
}
