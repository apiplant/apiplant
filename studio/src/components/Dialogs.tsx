import { For, Show, createMemo, createSignal } from "solid-js";
import { addFunction, addResource } from "../lib/store";
import { setView } from "../lib/nav";
import { isValidFunctionName } from "../lib/functions";
import { LANGUAGES, LANGUAGE_EXT, LANGUAGE_LABEL, type Language } from "../lib/types";
import type { TemplateKind } from "../lib/templates";
import { Button, Labelled, Modal, Mono, Select, Switch, TextInput } from "./ui";

const NAME_RULE = /^[a-z_][a-z0-9_]*$/;

export function NewResourceDialog(props: { onClose: () => void }) {
  const [name, setName] = createSignal("");
  const [scope, setScope] = createSignal<"organization" | "global">("organization");

  const valid = createMemo(() => NAME_RULE.test(name()));

  const create = () => {
    if (!valid()) return;
    if (addResource(name(), { scope: scope() })) {
      setView({ kind: "resource", name: name() });
      props.onClose();
    }
  };

  return (
    <Modal
      title="New resource"
      subtitle="One models/*.toml — a table, five CRUD endpoints, and its own permissions."
      onClose={props.onClose}
    >
      <div class="space-y-4">
        <Labelled label="name" hint="snake_case. Becomes the URL segment and the table apiplant_<name>.">
          <TextInput
            mono
            value={name()}
            placeholder="post"
            onInput={(value) => setName(value.trim())}
          />
          <Show when={name() && !valid()}>
            <p class="mt-1 text-[0.6875rem] text-danger">
              Use lowercase letters, digits and underscores, starting with a letter.
            </p>
          </Show>
        </Labelled>

        <Labelled label="tenancy">
          <Select
            value={scope()}
            options={[
              { value: "organization" as const, label: "organisation-scoped — isolated per tenant" },
              { value: "global" as const, label: "global — shared across the deployment" },
            ]}
            onChange={setScope}
          />
        </Labelled>

        <p class="rounded-lg border border-line bg-surface-2 px-3 py-2 text-[0.6875rem] leading-relaxed text-muted">
          It starts with member-only permissions and one <Mono>title</Mono> field, plus the automatic{" "}
          <Mono>id</Mono>, <Mono>created_at</Mono>, <Mono>updated_at</Mono>
          <Show when={scope() === "organization"}>
            {" "}
            and <Mono>organization_id</Mono>
          </Show>
          .
        </p>

        <div class="flex justify-end gap-2">
          <Button variant="ghost" onClick={props.onClose}>
            Cancel
          </Button>
          <Button variant="primary" disabled={!valid()} onClick={create}>
            Create resource
          </Button>
        </div>
      </div>
    </Modal>
  );
}

const LAYOUT_NOTE: Record<Language, string> = {
  rust: "A directory becomes a crate you own — any crates.io dependency, any module layout.",
  go: "A directory brings its own go.mod, so it can require other modules and split across files.",
  c: "A directory compiles every .c it holds, with itself on the include path.",
  zig: "A directory builds from a root <name>.zig that may @import its siblings.",
};

export function NewFunctionDialog(props: { onClose: () => void }) {
  const [name, setName] = createSignal("");
  const [language, setLanguage] = createSignal<Language>("rust");
  const [layout, setLayout] = createSignal<"file" | "directory">("file");
  const [kind, setKind] = createSignal<TemplateKind>("endpoint");
  const [withConfig, setWithConfig] = createSignal(false);

  const valid = createMemo(() => isValidFunctionName(name()));

  const create = () => {
    if (!valid()) return;
    if (addFunction(name(), language(), layout(), language() === "rust" ? kind() : "endpoint", withConfig())) {
      setView({ kind: "function", name: name() });
      props.onClose();
    }
  };

  return (
    <Modal
      title="New function"
      subtitle="A compiled library in functions/, mounted as an endpoint or wired into a resource's lifecycle."
      onClose={props.onClose}
      width="38rem"
    >
      <div class="space-y-4">
        <div class="grid gap-4 sm:grid-cols-2">
          <Labelled label="name" hint="The library name and, by default, the function it exports.">
            <TextInput mono value={name()} placeholder="greet" onInput={(value) => setName(value.trim())} />
            <Show when={name() && !valid()}>
              <p class="mt-1 text-[0.6875rem] text-danger">
                Lowercase letters, digits and underscores — it also has to be a valid identifier.
              </p>
            </Show>
          </Labelled>

          <Labelled label="language" hint="Each needs its own toolchain on PATH when you build.">
            <div class="flex gap-1">
              <For each={LANGUAGES}>
                {(option) => (
                  <button
                    type="button"
                    onClick={() => setLanguage(option)}
                    class={[
                      "flex-1 rounded-md border px-2 py-1.5 text-xs transition-colors",
                      language() === option
                        ? "border-[#1c4c3b] bg-[#0d2b21] text-accent"
                        : "border-line bg-surface-2 text-muted hover:border-line-strong hover:text-ink",
                    ].join(" ")}
                  >
                    {LANGUAGE_LABEL[option]}
                  </button>
                )}
              </For>
            </div>
          </Labelled>
        </div>

        <Labelled label="layout" hint={LAYOUT_NOTE[language()]}>
          <div class="grid grid-cols-2 gap-2">
            <button
              type="button"
              onClick={() => setLayout("file")}
              class={[
                "rounded-lg border px-3 py-2 text-left transition-colors",
                layout() === "file"
                  ? "border-[#1c4c3b] bg-[#0d2b21]"
                  : "border-line bg-surface-2 hover:border-line-strong",
              ].join(" ")}
            >
              <p class="text-[0.8125rem] font-medium text-ink">Single file</p>
              <p class="mt-0.5 font-mono text-[0.6875rem] text-faint">
                functions/{name() || "name"}.{LANGUAGE_EXT[language()]}
              </p>
            </button>
            <button
              type="button"
              onClick={() => setLayout("directory")}
              class={[
                "rounded-lg border px-3 py-2 text-left transition-colors",
                layout() === "directory"
                  ? "border-[#1c4c3b] bg-[#0d2b21]"
                  : "border-line bg-surface-2 hover:border-line-strong",
              ].join(" ")}
            >
              <p class="text-[0.8125rem] font-medium text-ink">Directory</p>
              <p class="mt-0.5 font-mono text-[0.6875rem] text-faint">functions/{name() || "name"}/…</p>
            </button>
          </div>
        </Labelled>

        <Show when={language() === "rust"}>
          <Labelled label="template">
            <Select
              value={kind()}
              options={[
                { value: "endpoint" as const, label: "HTTP endpoint — typed input and output, public" },
                { value: "hook" as const, label: "Lifecycle hook — private, returns a reply::* decision" },
              ]}
              onChange={setKind}
            />
          </Labelled>
        </Show>

        <div class="flex items-center justify-between rounded-lg border border-line bg-surface-2 px-3 py-2.5">
          <div>
            <p class="text-[0.8125rem] text-ink">Add a config file</p>
            <p class="mt-0.5 text-[0.6875rem] text-muted">
              functions/{name() || "name"}.toml, deserialized into the handler's typed <Mono>Config</Mono>.
            </p>
          </div>
          <Switch checked={withConfig()} onChange={setWithConfig} />
        </div>

        <div class="flex justify-end gap-2">
          <Button variant="ghost" onClick={props.onClose}>
            Cancel
          </Button>
          <Button variant="primary" disabled={!valid()} onClick={create}>
            Create function
          </Button>
        </div>
      </div>
    </Modal>
  );
}
