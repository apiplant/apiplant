import { For, Show, createMemo, createSignal } from "solid-js";
import { emitAgent, validateAgent } from "../lib/agents";
import { deleteAgent, fileText, setAgentFromToml, studio, updateAgent } from "../lib/store";
import { setView } from "../lib/nav";
import type { AgentAiOverride, AgentEntry, TomlTable } from "../lib/types";
import {
  Badge,
  Button,
  Card,
  CardHeader,
  CodeEditor,
  CommitInput,
  Labelled,
  Mono,
  Select,
  Switch,
  Tabs,
  TextArea,
  TextInput,
} from "./ui";
import { hasComments } from "../lib/toml";
import { formatPolicy, parsePolicy } from "../lib/permissions";
import { PolicyPhrase } from "./PolicyPhrase";

type TabId = "settings" | "permissions" | "prompt" | "tools" | "ai" | "toml";

const CHAT_LEVELS = ["public", "authenticated", "member", "role", "private"];

const HISTORY_LEVELS = [
  "public",
  "authenticated",
  "member",
  "owner",
  "role",
  "private",
];

const AI_PROVIDER_OPTIONS = [
  { value: "", label: "inherit global provider" },
  { value: "none", label: "none" },
  { value: "openai", label: "openai" },
  { value: "anthropic", label: "anthropic" },
  { value: "custom", label: "custom" },
] as const;

function parsePositiveInteger(value: string): number | undefined {
  const parsed = Number.parseInt(value, 10);
  return value !== "" && Number.isFinite(parsed) && parsed > 0 ? parsed : undefined;
}

export function AgentPage(props: { entry: AgentEntry }) {
  const [tab, setTab] = createSignal<TabId>("settings");
  const [draft, setDraft] = createSignal<string | null>(null);
  const [error, setError] = createSignal<string | null>(null);
  const [confirmingDelete, setConfirmingDelete] = createSignal(false);

  const agent = () => props.entry;
  const edit = (update: (draft: AgentEntry) => void) => updateAgent(props.entry.name, update);
  const canonical = createMemo(() => fileText(agent().path) ?? emitAgent(agent()));
  const issues = createMemo(() =>
    validateAgent(agent(), (studio.project?.agents ?? []).map((entry) => entry.name)),
  );
  const originalHadComments = createMemo(() => {
    const original = studio.project?.files[agent().path]?.original;
    return !!original && hasComments(original) && original !== canonical();
  });

  const commitToml = (text: string) => {
    setDraft(text);
    try {
      const nextName = setAgentFromToml(agent().name, text);
      setError(null);
      if (nextName && nextName !== agent().name) setView({ kind: "agent", name: nextName });
    } catch (failure) {
      setError(failure instanceof Error ? failure.message : String(failure));
    }
  };

  return (
    <div class="animate-rise mx-auto w-full max-w-5xl px-6 py-6">
      <header class="mb-5">
        <div class="flex flex-wrap items-center gap-2.5">
          <h1 class="font-mono text-xl font-semibold tracking-tight">{agent().name}</h1>
          <Badge tone="accent">agent</Badge>
          <Badge>{agent().storageEnabled ? "stored history" : "live only"}</Badge>
          <Badge tone="info">{agent().scope}</Badge>
          <Show when={agent().aiOverride?.model}>
            <Badge>{agent().aiOverride!.model}</Badge>
          </Show>
          <Mono>{agent().path}</Mono>
        </div>
        <p class="mt-2 max-w-3xl text-xs leading-relaxed text-muted">
          Chat route: <Mono>/ai/agents/{agent().name}/chat</Mono>
          <Show when={agent().storageEnabled}>
            {" "}
            with generated <Mono>ai_{agent().name}_thread</Mono> and <Mono>ai_{agent().name}_message</Mono>{" "}
            resources for persisted history.
          </Show>
        </p>

        <Show when={issues().length}>
          <ul class="mt-3 space-y-1 rounded-lg border border-danger-line bg-danger-soft px-3 py-2">
            <For each={issues()}>{(issue) => <li class="text-xs leading-relaxed text-danger">{issue}</li>}</For>
          </ul>
        </Show>

        <Show when={originalHadComments()}>
          <p class="mt-3 rounded-lg border border-warn-line bg-warn-soft px-3 py-2 text-xs leading-relaxed text-warn">
            The file on disk has comments. Saving from the form rewrites it, which drops them — edit on the TOML
            tab instead to keep them.
          </p>
        </Show>
      </header>

      <div class="mb-4 flex items-center justify-between gap-3">
        <Tabs
          active={tab()}
          onChange={setTab}
          tabs={[
            { id: "settings", label: "Settings" },
            { id: "permissions", label: "Permissions" },
            { id: "prompt", label: "Prompt" },
            { id: "tools", label: "Tools", badge: agent().tools.length ? String(agent().tools.length) : undefined },
            { id: "ai", label: "AI override", badge: agent().aiOverride ? "on" : undefined },
            { id: "toml", label: "TOML" },
          ]}
        />
      </div>

      <Show when={tab() === "settings"}>
        <SettingsTab entry={agent()} onEdit={edit} confirming={confirmingDelete()} setConfirming={setConfirmingDelete} />
      </Show>

      <Show when={tab() === "permissions"}>
        <PermissionsTab entry={agent()} onEdit={edit} />
      </Show>

      <Show when={tab() === "prompt"}>
        <PromptTab entry={agent()} onEdit={edit} />
      </Show>

      <Show when={tab() === "tools"}>
        <ToolsTab entry={agent()} onEdit={edit} />
      </Show>

      <Show when={tab() === "ai"}>
        <AiOverrideTab entry={agent()} onEdit={edit} />
      </Show>

      <Show when={tab() === "toml"}>
        <div>
          <div class="mb-2 flex items-center justify-between">
            <p class="text-xs text-muted">The file as it will be written.</p>
            <Show when={error()}>
              <span class="text-xs text-danger">{error()}</span>
            </Show>
          </div>
          <CodeEditor language="toml" value={draft() ?? canonical()} onInput={commitToml} minHeight="30rem" />
        </div>
      </Show>
    </div>
  );
}

function SettingsTab(props: {
  entry: AgentEntry;
  onEdit: (update: (draft: AgentEntry) => void) => void;
  confirming: boolean;
  setConfirming: (value: boolean) => void;
}) {
  return (
    <div class="space-y-3">
      <Card>
        <CardHeader title="Identity" />
        <div class="grid gap-4 px-4 py-4 sm:grid-cols-2">
          <Labelled label="name" hint="Route name and, with storage on, the generated history resources.">
            <CommitInput
              mono
              lowercase
              value={props.entry.name}
              placeholder="coach"
              onCommit={(value) => {
                props.onEdit((draft) => {
                  draft.name = value;
                });
                setView({ kind: "agent", name: value });
              }}
            />
          </Labelled>

          <Labelled class="sm:col-span-2" label="description" hint="Shown in discovery surfaces and operator UIs.">
            <TextInput
              value={props.entry.description}
              placeholder="What this agent helps with."
              onInput={(value) =>
                props.onEdit((draft) => {
                  draft.description = value;
                })
              }
            />
          </Labelled>

          <Labelled label="scope" hint="Global threads follow the caller; organization scope requires an active org.">
            <Select
              value={props.entry.scope}
              options={[
                { value: "global", label: "global — shared across the deployment" },
                { value: "organization", label: "organization — stamped with organization_id" },
              ]}
              onChange={(value) =>
                props.onEdit((draft) => {
                  draft.scope = value as "global" | "organization";
                })
              }
            />
          </Labelled>

          <Labelled label="storage" hint="Stored agents keep thread/message history in generated resources.">
            <Select
              value={props.entry.storageEnabled ? "stored" : "transient"}
              options={[
                { value: "stored", label: "stored — keep threads and messages" },
                { value: "transient", label: "transient — one live conversation only" },
              ]}
              onChange={(value) =>
                props.onEdit((draft) => {
                  draft.storageEnabled = value === "stored";
                })
              }
            />
          </Labelled>

          <Show when={props.entry.storageEnabled}>
            <Labelled
              label="summary after chars"
              hint="Character count at which the rolling summary refreshes; the summary is budgeted at half of it. Empty uses the default."
            >
              <TextInput
                type="number"
                value={props.entry.summaryAfterCharacters === undefined ? "" : String(props.entry.summaryAfterCharacters)}
                placeholder="12000"
                onInput={(value) =>
                  props.onEdit((draft) => {
                    draft.summaryAfterCharacters = parsePositiveInteger(value);
                  })
                }
              />
            </Labelled>

          </Show>
        </div>
      </Card>

      <Card class="border-danger-line">
        <CardHeader
          title="Delete this agent"
          hint="Removes the agent file. Stored history resources remain until removed separately."
        >
          <Show
            when={props.confirming}
            fallback={
              <Button size="sm" variant="danger" onClick={() => props.setConfirming(true)}>
                Delete
              </Button>
            }
          >
            <Button size="sm" variant="ghost" onClick={() => props.setConfirming(false)}>
              Cancel
            </Button>
            <Button
              size="sm"
              variant="danger"
              onClick={() => {
                props.setConfirming(false);
                deleteAgent(props.entry.name);
              }}
            >
              Yes, delete
            </Button>
          </Show>
        </CardHeader>
      </Card>
    </div>
  );
}

function PermissionsTab(props: {
  entry: AgentEntry;
  onEdit: (update: (draft: AgentEntry) => void) => void;
}) {
  return (
    <Card>
      <CardHeader
        title="Access policy"
        hint="Chat governs the endpoint; history governs the stored thread and message resources."
      />
      <div class="divide-y divide-line">
        <PermissionRow
          label="chat"
          value={props.entry.chat}
          levels={CHAT_LEVELS}
          hint="The route itself: POST /ai/agents/<name>/chat."
          onChange={(value) =>
            props.onEdit((draft) => {
              draft.chat = value;
            })
          }
        />
        <PermissionRow
          label="history"
          value={props.entry.history}
          levels={HISTORY_LEVELS}
          hint={
            props.entry.storageEnabled
              ? "Read access to generated ai_<name>_thread and ai_<name>_message resources."
              : "Only meaningful once storage is enabled."
          }
          onChange={(value) =>
            props.onEdit((draft) => {
              draft.history = value;
            })
          }
        />
      </div>
    </Card>
  );
}

function PermissionRow(props: {
  label: string;
  value: string;
  levels: readonly string[];
  hint: string;
  onChange: (value: string) => void;
}) {
  return (
    <div class="flex flex-wrap items-baseline gap-x-3 gap-y-1 px-4 py-3">
      <div class="w-40 shrink-0">
        <p class="text-[0.8125rem] font-medium capitalize text-ink">{props.label}</p>
        <p class="text-[0.6875rem] text-faint">{props.hint}</p>
      </div>
      <PolicyPhrase
        class="min-w-0 flex-1"
        subject={parsePolicy(props.value || "private")}
        levels={props.levels}
        onChange={(update) =>
          props.onChange(formatPolicy(update(parsePolicy(props.value || "private"))))
        }
      />
    </div>
  );
}

function PromptTab(props: {
  entry: AgentEntry;
  onEdit: (update: (draft: AgentEntry) => void) => void;
}) {
  return (
    <Card>
      <CardHeader
        title="System prompt"
        hint="Sent before every user turn; caller-supplied system messages do not override it."
      />
      <div class="px-4 py-4">
        <TextArea
          class="min-h-[20rem] leading-relaxed"
          mono
          value={props.entry.system}
          placeholder="Set the standing instructions this agent should always follow."
          onInput={(value) =>
            props.onEdit((draft) => {
              draft.system = value;
            })
          }
        />
      </div>
    </Card>
  );
}

function parseSchema(text: string, fallback: TomlTable): TomlTable {
  try {
    const parsed = JSON.parse(text);
    if (parsed && typeof parsed === "object" && !Array.isArray(parsed)) return parsed as TomlTable;
  } catch {
    // Keep the previous valid schema while the user is mid-edit.
  }
  return fallback;
}

function schemaText(schema: TomlTable): string {
  return JSON.stringify(schema, null, 2);
}

function availableFunctionNames(): string[] {
  const names = new Set<string>();
  for (const entry of studio.project?.functions ?? []) {
    for (const name of entry.exports) names.add(name);
    names.add(entry.name);
  }
  return [...names].sort((a, b) => a.localeCompare(b));
}

function ToolsTab(props: {
  entry: AgentEntry;
  onEdit: (update: (draft: AgentEntry) => void) => void;
}) {
  const functions = createMemo(availableFunctionNames);
  const addTool = () =>
    props.onEdit((draft) => {
      const name = `tool_${draft.tools.length + 1}`;
      draft.tools.push({
        name,
        description: "What this tool does.",
        inputSchema: { type: "object", properties: {} },
        outputSchema: { type: "object" },
        function: functions()[0] ?? "",
      });
    });

  return (
    <div class="space-y-3">
      <Card>
        <CardHeader
          title="Function-backed tools"
          hint="Each tool advertises a loaded function to the model. Functions with permission = 'none' stay hidden from direct calls."
        >
          <Button
            size="sm"
            variant="ghost"
            class="text-accent hover:bg-accent-soft hover:text-accent"
            onClick={addTool}
          >
            Add tool
          </Button>
        </CardHeader>
        <Show
          when={props.entry.tools.length}
          fallback={<p class="px-4 py-8 text-center text-xs text-muted">No tools configured for this agent.</p>}
        >
          <div class="divide-y divide-line">
            <For each={props.entry.tools} keyed={false}>
              {(tool, index) => (
                <div class="space-y-4 px-4 py-4">
                  <div class="grid gap-4 sm:grid-cols-2">
                    <Labelled label="tool name" hint="Name exposed to the model.">
                      <TextInput
                        mono
                        lowercase
                        value={tool().name}
                        placeholder="lookup_order"
                        onInput={(value) =>
                          props.onEdit((draft) => {
                            draft.tools[index].name = value.trim();
                          })
                        }
                      />
                    </Labelled>
                    <Labelled label="function" hint="Loaded function invoked for this tool.">
                      <Show
                        when={functions().length}
                        fallback={
                          <TextInput
                            mono
                            lowercase
                            value={tool().function}
                            placeholder="function_name"
                            onInput={(value) =>
                              props.onEdit((draft) => {
                                draft.tools[index].function = value.trim();
                              })
                            }
                          />
                        }
                      >
                        <Select
                          value={tool().function || functions()[0] || ""}
                          options={functions().map((name) => ({ value: name, label: name }))}
                          onChange={(value) =>
                            props.onEdit((draft) => {
                              draft.tools[index].function = value;
                            })
                          }
                        />
                      </Show>
                    </Labelled>
                    <Labelled class="sm:col-span-2" label="description" hint="Passed to the model for tool selection.">
                      <TextInput
                        value={tool().description}
                        placeholder="Look up an order by id."
                        onInput={(value) =>
                          props.onEdit((draft) => {
                            draft.tools[index].description = value;
                          })
                        }
                      />
                    </Labelled>
                    <Labelled label="input schema" hint="JSON Schema object for tool arguments.">
                      <TextArea
                        mono
                        class="min-h-40 text-xs"
                        value={schemaText(tool().inputSchema)}
                        onInput={(value) =>
                          props.onEdit((draft) => {
                            draft.tools[index].inputSchema = parseSchema(value, draft.tools[index].inputSchema);
                          })
                        }
                      />
                    </Labelled>
                    <Labelled label="output schema" hint="JSON Schema object the function returns.">
                      <TextArea
                        mono
                        class="min-h-40 text-xs"
                        value={schemaText(tool().outputSchema)}
                        onInput={(value) =>
                          props.onEdit((draft) => {
                            draft.tools[index].outputSchema = parseSchema(value, draft.tools[index].outputSchema);
                          })
                        }
                      />
                    </Labelled>
                  </div>
                  <div class="flex justify-end">
                    <Button
                      size="sm"
                      variant="danger"
                      onClick={() =>
                        props.onEdit((draft) => {
                          draft.tools.splice(index, 1);
                        })
                      }
                    >
                      Remove
                    </Button>
                  </div>
                </div>
              )}
            </For>
          </div>
        </Show>
      </Card>
    </div>
  );
}

function emptyOverride(): AgentAiOverride {
  return {};
}

function ensureOverride(entry: AgentEntry): AgentAiOverride {
  return entry.aiOverride ?? emptyOverride();
}

function AiOverrideTab(props: {
  entry: AgentEntry;
  onEdit: (update: (draft: AgentEntry) => void) => void;
}) {
  const enabled = () => !!props.entry.aiOverride;
  const override = () => ensureOverride(props.entry);

  return (
    <Card>
      <CardHeader
        title="Per-agent AI configuration"
        hint="Overrides the global [ai] settings for this agent. Empty fields inherit."
      >
        <Switch
          checked={enabled()}
          label="override global ai configuration"
          onChange={(value) =>
            props.onEdit((draft) => {
              draft.aiOverride = value ? emptyOverride() : null;
            })
          }
        />
      </CardHeader>

      <Show
        when={enabled()}
        fallback={
          <p class="px-4 py-8 text-center text-xs text-muted">
            This agent inherits the app-wide AI provider, endpoint, key, model, timeout and token defaults.
          </p>
        }
      >
        <div class="grid gap-4 px-4 py-4 sm:grid-cols-2">
          <Labelled
            label="provider"
            hint="Provider for this agent. `custom` requires an endpoint."
          >
            <Select
              value={override().provider ?? ""}
              options={AI_PROVIDER_OPTIONS}
              onChange={(value) =>
                props.onEdit((draft) => {
                  const next = ensureOverride(draft);
                  next.provider = value || undefined;
                  draft.aiOverride = next;
                })
              }
            />
          </Labelled>

          <Labelled label="endpoint" hint="Origin, /v1 base, or full path. Empty inherits the global endpoint.">
            <TextInput
              mono
              value={override().endpoint ?? ""}
              placeholder="http://localhost:8080 or https://api.openai.com/v1/chat/completions"
              onInput={(value) =>
                props.onEdit((draft) => {
                  const next = ensureOverride(draft);
                  next.endpoint = value === "" ? undefined : value;
                  draft.aiOverride = next;
                })
              }
            />
          </Labelled>

          <Labelled label="model" hint="Empty inherits the global model.">
            <TextInput
              mono
              value={override().model ?? ""}
              placeholder="gpt-4o-mini"
              onInput={(value) =>
                props.onEdit((draft) => {
                  const next = ensureOverride(draft);
                  next.model = value === "" ? undefined : value.trim();
                  draft.aiOverride = next;
                })
              }
            />
          </Labelled>

          <Labelled label="api key" hint="Empty inherits the global key; for custom, blank sends no auth header.">
            <TextInput
              mono
              value={override().apiKey ?? ""}
              placeholder="$OPENAI_API_KEY"
              onInput={(value) =>
                props.onEdit((draft) => {
                  const next = ensureOverride(draft);
                  next.apiKey = value === "" ? undefined : value;
                  draft.aiOverride = next;
                })
              }
            />
          </Labelled>

          <Labelled label="temperature" hint="Empty inherits the global default.">
            <TextInput
              type="number"
              value={override().temperature === undefined ? "" : String(override().temperature)}
              placeholder="inherit"
              onInput={(value) =>
                props.onEdit((draft) => {
                  const next = ensureOverride(draft);
                  const parsed = Number(value);
                  if (value !== "" && Number.isFinite(parsed)) next.temperature = parsed;
                  else delete next.temperature;
                  draft.aiOverride = next;
                })
              }
            />
          </Labelled>

          <Labelled label="max tokens" hint="Empty inherits the global token budget.">
            <TextInput
              type="number"
              value={override().maxTokens === undefined ? "" : String(override().maxTokens)}
              placeholder="inherit"
              onInput={(value) =>
                props.onEdit((draft) => {
                  const next = ensureOverride(draft);
                  const parsed = Number.parseInt(value, 10);
                  if (value !== "" && Number.isFinite(parsed)) next.maxTokens = parsed;
                  else delete next.maxTokens;
                  draft.aiOverride = next;
                })
              }
            />
          </Labelled>

          <Labelled label="timeout (s)" hint="Empty inherits the global timeout.">
            <TextInput
              type="number"
              value={override().timeoutSecs === undefined ? "" : String(override().timeoutSecs)}
              placeholder="inherit"
              onInput={(value) =>
                props.onEdit((draft) => {
                  const next = ensureOverride(draft);
                  const parsed = Number.parseInt(value, 10);
                  if (value !== "" && Number.isFinite(parsed)) next.timeoutSecs = parsed;
                  else delete next.timeoutSecs;
                  draft.aiOverride = next;
                })
              }
            />
          </Labelled>

          <Labelled
            label="thinking"
            hint="Provider thinking switch. Returned reasoning is stored on the message, revealed by the Show reasoning toggle, and never part of the answer."
          >
            <Select
              value={
                override().thinking === undefined
                  ? "inherit"
                  : override().thinking
                    ? "enabled"
                    : "disabled"
              }
              options={[
                { value: "inherit", label: "inherit — follow the global [ai] setting" },
                { value: "enabled", label: "enabled — ask the model to think" },
                { value: "disabled", label: "disabled — ask it not to" },
              ]}
              onChange={(value) =>
                props.onEdit((draft) => {
                  const next = ensureOverride(draft);
                  // `undefined` drops the key so the agent inherits; an
                  // explicit `false` has to survive, or an agent could never
                  // turn thinking off once it is on globally.
                  next.thinking = value === "inherit" ? undefined : value === "enabled";
                  draft.aiOverride = next;
                })
              }
            />
          </Labelled>

          <Labelled
            class="sm:col-span-2"
            label="fallback system prompt"
            hint="Optional [ai] system override for this agent; the Prompt tab wins when set."
          >
            <TextArea
              mono
              class="min-h-28"
              value={override().system ?? ""}
              placeholder="Leave empty to inherit the global [ai] system prompt."
              onInput={(value) =>
                props.onEdit((draft) => {
                  const next = ensureOverride(draft);
                  next.system = value === "" ? undefined : value;
                  draft.aiOverride = next;
                })
              }
            />
          </Labelled>
        </div>
      </Show>
    </Card>
  );
}
