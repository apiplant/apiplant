import { For, Show, createMemo, createSignal } from "solid-js";
import {
  ACTIONS,
  AUTH_HOOK_EVENTS,
  CONTENT_FORMATS,
  FIELD_TYPES,
  HOOK_EVENTS,
  ON_DELETE,
  ORG_CLASS_SUFFIX,
  SCOPES,
  type Action,
  type ContentFormat,
  type AuthHookEvent,
  type Field,
  type FieldType,
  type HookEvent,
  type OnDelete,
  type Resource,
  type ResourceEntry,
  type Scope,
} from "../lib/types";
import {
  allFunctionNames,
  configValue,
  customizeBuiltin,
  deleteResource,
  fileText,
  setResourceFromToml,
  studio,
  toast,
  updateResource,
  validateResource,
} from "../lib/store";
import { emitResource, hasComments } from "../lib/toml";
import { BUILTIN_SUMMARY, type BuiltinName } from "../lib/builtins";
import { setView } from "../lib/nav";
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
  TextInput,
  Toggle,
} from "./ui";

type TabId = "fields" | "permissions" | "hooks" | "settings" | "toml";

const ACCESS_OPTIONS = [
  { value: "public", label: "public — anyone" },
  { value: "authenticated", label: "authenticated — any signed-in caller" },
  { value: "member", label: "member — of the active organisation" },
  { value: "owner", label: "owner — only rows they own" },
  { value: "role", label: "role:… — a named org role" },
  { value: "private", label: "private — not exposed" },
] as const;

// `{res}` is substituted with `<base_path>/<resource>`, which already carries a
// leading slash — so these must not add one of their own.
const ACTION_ENDPOINT: Record<Action, string> = {
  list: "GET {res}",
  read: "GET {res}/{id}",
  create: "POST {res}",
  update: "PATCH {res}/{id}",
  delete: "DELETE {res}/{id}",
};

const SCALAR_DEFAULTABLE: FieldType[] = ["string", "text", "integer", "big_int", "float", "boolean"];

export function ResourcePage(props: { entry: ResourceEntry }) {
  const [tab, setTab] = createSignal<TabId>("fields");
  const [tomlDraft, setTomlDraft] = createSignal<string | null>(null);
  const [tomlError, setTomlError] = createSignal<string | null>(null);
  const [confirmingDelete, setConfirmingDelete] = createSignal(false);

  const resource = () => props.entry.resource;
  const edit = (update: (draft: Resource) => void) => updateResource(props.entry.name, update);

  const basePath = () => {
    const raw = configValue("server", "base_path");
    return typeof raw === "string" && raw ? raw.replace(/\/$/, "") : "";
  };

  const resourceNames = createMemo(() => (studio.project?.resources ?? []).map((entry) => entry.name));
  const issues = createMemo(() => validateResource(resource(), resourceNames()));

  const canonicalToml = createMemo(() =>
    props.entry.path ? (fileText(props.entry.path) ?? emitResource(resource())) : emitResource(resource()),
  );
  const originalHadComments = createMemo(() => {
    const path = props.entry.path;
    const original = path ? studio.project?.files[path]?.original : null;
    return !!original && hasComments(original) && original !== canonicalToml();
  });

  const commitToml = (text: string) => {
    setTomlDraft(text);
    try {
      setResourceFromToml(props.entry.name, text);
      setTomlError(null);
    } catch (error) {
      setTomlError(error instanceof Error ? error.message : String(error));
    }
  };

  return (
    <div class="animate-rise mx-auto w-full max-w-5xl px-6 py-6">
      <header class="mb-5">
        <div class="flex flex-wrap items-center gap-2.5">
          <h1 class="font-mono text-xl font-semibold tracking-tight">{resource().name}</h1>
          <Show when={props.entry.builtin}>
            <Badge tone={props.entry.path ? "accent" : "neutral"}>
              {props.entry.path ? "built-in, overridden" : "built-in default"}
            </Badge>
          </Show>
          <Badge tone={resource().scope === "global" ? "info" : "neutral"}>
            {resource().scope === "global" ? "global" : "org-scoped"}
          </Badge>
          <Show when={props.entry.path}>
            <Mono>{props.entry.path}</Mono>
          </Show>
        </div>

        <p class="mt-2 max-w-3xl text-xs leading-relaxed text-muted">
          <Show
            when={props.entry.builtin}
            fallback={
              <>
                Published at{" "}
                <Mono>
                  {basePath()}/{resource().name}
                </Mono>{" "}
                — list, read, create, update and delete, plus a nested{" "}
                <Mono>{`${basePath()}/{parent}/{id}/${resource().name}`}</Mono> for every resource that
                references it.
              </>
            }
          >
            {props.entry.builtinSummary ?? BUILTIN_SUMMARY[props.entry.name as BuiltinName]}
          </Show>
        </p>

        <Show when={props.entry.builtin && !props.entry.path}>
          <div class="mt-3 flex items-center gap-3 rounded-lg border border-line bg-surface px-3 py-2">
            <p class="flex-1 text-xs leading-relaxed text-muted">
              This resource exists without a file. Editing anything writes{" "}
              <Mono>models/…</Mono> to replace the framework default — the framework keeps using it for auth,
              ownership and org resolution.
            </p>
            <Button size="sm" onClick={() => customizeBuiltin(props.entry.name)}>
              Create the file
            </Button>
          </div>
        </Show>

        <Show when={issues().length}>
          <ul class="mt-3 space-y-1 rounded-lg border border-danger-line bg-danger-soft px-3 py-2">
            <For each={issues()}>
              {(issue) => <li class="text-xs leading-relaxed text-danger">{issue}</li>}
            </For>
          </ul>
        </Show>

        <Show when={originalHadComments()}>
          <p class="mt-3 rounded-lg border border-warn-line bg-warn-soft px-3 py-2 text-xs leading-relaxed text-warn">
            The file on disk has comments. Saving rewrites it from the form, which drops them — edit on the
            TOML tab instead to keep them.
          </p>
        </Show>
      </header>

      <div class="mb-4 flex items-center justify-between gap-3">
        <Tabs
          active={tab()}
          onChange={setTab}
          tabs={[
            { id: "fields", label: "Fields", badge: resource().fields.length },
            { id: "permissions", label: "Permissions" },
            { id: "hooks", label: "Hooks", badge: Object.keys(resource().hooks).length },
            { id: "settings", label: "Settings" },
            { id: "toml", label: "TOML" },
          ]}
        />
      </div>

      <Show when={tab() === "fields"}>
        <FieldsTab entry={props.entry} onEdit={edit} resourceNames={resourceNames()} />
      </Show>

      <Show when={tab() === "permissions"}>
        <PermissionsTab resource={resource()} onEdit={edit} basePath={basePath()} />
      </Show>

      <Show when={tab() === "hooks"}>
        <HooksTab resource={resource()} onEdit={edit} />
      </Show>

      <Show when={tab() === "settings"}>
        <SettingsTab
          entry={props.entry}
          onEdit={edit}
          confirming={confirmingDelete()}
          setConfirming={setConfirmingDelete}
        />
      </Show>

      <Show when={tab() === "toml"}>
        <div>
          <div class="mb-2 flex items-center justify-between">
            <p class="text-xs text-muted">
              The file as it will be written. Edit here to keep comments and anything the forms do not model.
            </p>
            <Show when={tomlError()}>
              <span class="text-xs text-danger">{tomlError()}</span>
            </Show>
          </div>
          <CodeEditor
            language="toml"
            value={tomlDraft() ?? canonicalToml()}
            onInput={commitToml}
            minHeight="28rem"
          />
        </div>
      </Show>
    </div>
  );
}

// ---- fields -----------------------------------------------------------------

function FieldsTab(props: {
  entry: ResourceEntry;
  onEdit: (update: (draft: Resource) => void) => void;
  resourceNames: string[];
}) {
  const resource = () => props.entry.resource;

  const addField = () => {
    const base = "field";
    let name = base;
    let index = 2;
    while (resource().fields.some((field) => field.name === name)) name = `${base}_${index++}`;
    props.onEdit((draft) => {
      draft.fields.push({ name, type: "string" });
    });
  };

  const automatic = createMemo(() => {
    const columns = ["id — uuid primary key"];
    if (resource().timestamps) columns.push("created_at, updated_at — timestamptz");
    if (resource().scope === "organization") columns.push("organization_id — reference to organization");
    return columns;
  });

  return (
    <div class="space-y-3">
      <Card>
        <CardHeader
          title="Columns"
          hint="Each one becomes a column and a documented property on every response."
        >
          <Button size="sm" variant="primary" onClick={addField}>
            Add field
          </Button>
        </CardHeader>

        <Show
          when={resource().fields.length}
          fallback={
            <p class="px-4 py-8 text-center text-xs text-muted">
              No fields yet. The resource would still work — it just carries only its automatic columns.
            </p>
          }
        >
          <div class="divide-y divide-line">
            <For each={resource().fields}>
              {(field, index) => (
                <FieldRow
                  field={field}
                  resourceNames={props.resourceNames}
                  onChange={(update) => props.onEdit((draft) => update(draft.fields[index()]))}
                  onRemove={() =>
                    props.onEdit((draft) => {
                      draft.fields.splice(index(), 1);
                    })
                  }
                  onMove={(direction) =>
                    props.onEdit((draft) => {
                      const target = index() + direction;
                      if (target < 0 || target >= draft.fields.length) return;
                      const [moved] = draft.fields.splice(index(), 1);
                      draft.fields.splice(target, 0, moved);
                    })
                  }
                />
              )}
            </For>
          </div>
        </Show>
      </Card>

      <div class="flex flex-wrap items-center gap-2 px-1 text-xs text-faint">
        <span class="font-medium text-muted">Added automatically:</span>
        <For each={automatic()}>{(column) => <Mono>{column}</Mono>}</For>
      </div>
    </div>
  );
}

function FieldRow(props: {
  field: Field;
  resourceNames: string[];
  onChange: (update: (field: Field) => void) => void;
  onRemove: () => void;
  onMove: (direction: 1 | -1) => void;
}) {
  const isReference = () => props.field.type === "reference";

  return (
    <div class="group px-4 py-3 transition-colors hover:bg-surface-2/40">
      <div class="flex flex-wrap items-center gap-2">
        <TextInput
          mono
          lowercase
          class="min-w-0 flex-1 sm:max-w-[16rem]"
          value={props.field.name}
          placeholder="column_name"
          onInput={(value) =>
            props.onChange((field) => {
              field.name = value;
            })
          }
        />
        <Select
          class="w-28 shrink-0 sm:w-40"
          value={props.field.type}
          options={FIELD_TYPES}
          onChange={(value) =>
            props.onChange((field) => {
              field.type = value as FieldType;
              if (field.type !== "reference") {
                delete field.references;
                delete field.on_delete;
              }
              if (field.type !== "string") delete field.max_length;
              if (!["string", "text"].includes(field.type)) delete field.format;
            })
          }
        />

        <div class="order-last flex w-full flex-wrap items-center gap-1.5 sm:order-none sm:w-auto sm:flex-1">
          <Toggle
            label="required"
            hint="NOT NULL"
            checked={!!props.field.required}
            onChange={(value) =>
              props.onChange((field) => {
                if (value) field.required = true;
                else delete field.required;
              })
            }
          />
          <Toggle
            label="unique"
            hint="UNIQUE constraint; a conflict returns 409"
            checked={!!props.field.unique}
            onChange={(value) =>
              props.onChange((field) => {
                if (value) field.unique = true;
                else delete field.unique;
              })
            }
          />
          <Toggle
            label="hidden"
            hint="Stripped from every API response, still writable"
            checked={!!props.field.hidden}
            onChange={(value) =>
              props.onChange((field) => {
                if (value) field.hidden = true;
                else delete field.hidden;
              })
            }
          />
        </div>

        <div class="ml-auto flex shrink-0 items-center gap-0.5 transition-opacity sm:opacity-0 sm:group-hover:opacity-100 sm:focus-within:opacity-100">
          <button
            type="button"
            title="Move up"
            onClick={() => props.onMove(-1)}
            class="rounded p-1 text-faint hover:bg-surface-3 hover:text-ink"
          >
            <svg width="13" height="13" viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.5">
              <path d="M8 12.5v-9M4.5 7L8 3.5 11.5 7" stroke-linecap="round" stroke-linejoin="round" />
            </svg>
          </button>
          <button
            type="button"
            title="Move down"
            onClick={() => props.onMove(1)}
            class="rounded p-1 text-faint hover:bg-surface-3 hover:text-ink"
          >
            <svg width="13" height="13" viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.5">
              <path d="M8 3.5v9M4.5 9L8 12.5 11.5 9" stroke-linecap="round" stroke-linejoin="round" />
            </svg>
          </button>
          <button
            type="button"
            title="Remove field"
            onClick={props.onRemove}
            class="rounded p-1 text-faint hover:bg-danger-soft hover:text-danger"
          >
            <svg width="13" height="13" viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.5">
              <path d="M3 4.5h10M6.5 4.5V3h3v1.5M5 4.5l.5 8h5l.5-8" stroke-linecap="round" stroke-linejoin="round" />
            </svg>
          </button>
        </div>
      </div>

      <div class="mt-2.5 grid grid-cols-2 gap-3 sm:grid-cols-4">
        <Show when={isReference()}>
          <Labelled label="references">
            <Select
              value={props.field.references ?? ""}
              options={["", ...props.resourceNames]}
              onChange={(value) =>
                props.onChange((field) => {
                  if (value) field.references = value;
                  else delete field.references;
                })
              }
            />
          </Labelled>
          <Labelled label="on delete">
            <Select
              value={props.field.on_delete ?? "restrict"}
              options={ON_DELETE}
              onChange={(value) =>
                props.onChange((field) => {
                  field.on_delete = value as OnDelete;
                })
              }
            />
          </Labelled>
        </Show>

        <Show when={props.field.type === "string"}>
          <Labelled label="max length">
            <TextInput
              type="number"
              value={props.field.max_length ? String(props.field.max_length) : ""}
              placeholder="unbounded"
              onInput={(value) =>
                props.onChange((field) => {
                  const parsed = Number.parseInt(value, 10);
                  if (Number.isFinite(parsed) && parsed > 0) field.max_length = parsed;
                  else delete field.max_length;
                })
              }
            />
          </Labelled>
        </Show>

        <Show when={props.field.type === "string" || props.field.type === "text"}>
          <Labelled label="content">
            <Select
              value={props.field.format ?? "plain"}
              options={CONTENT_FORMATS}
              onChange={(value) =>
                props.onChange((field) => {
                  field.format = value as ContentFormat;
                })
              }
            />
          </Labelled>
        </Show>

        <Show when={SCALAR_DEFAULTABLE.includes(props.field.type)}>
          <Labelled label="default">
            <Show
              when={props.field.type !== "boolean"}
              fallback={
                <Select
                  value={props.field.default === true ? "true" : props.field.default === false ? "false" : ""}
                  options={[
                    { value: "", label: "none" },
                    { value: "true", label: "true" },
                    { value: "false", label: "false" },
                  ]}
                  onChange={(value) =>
                    props.onChange((field) => {
                      if (value === "") delete field.default;
                      else field.default = value === "true";
                    })
                  }
                />
              }
            >
              <TextInput
                mono
                value={props.field.default === undefined ? "" : String(props.field.default)}
                placeholder="none"
                onInput={(value) =>
                  props.onChange((field) => {
                    if (value === "") {
                      delete field.default;
                      return;
                    }
                    const numeric = ["integer", "big_int", "float"].includes(field.type);
                    const parsed = Number(value);
                    field.default = numeric && Number.isFinite(parsed) ? parsed : value;
                  })
                }
              />
            </Show>
          </Labelled>
        </Show>

        <Show when={isReference() && props.field.references}>
          <div class="col-span-2 self-end pb-1 text-[0.6875rem] leading-relaxed text-faint sm:col-span-2">
            Expands as <Mono>?expand={props.field.name.replace(/_id$/, "")}</Mono> and publishes{" "}
            <Mono>GET /{props.field.references}/{"{id}"}/…</Mono>
          </div>
        </Show>
      </div>
    </div>
  );
}

// ---- permissions ------------------------------------------------------------

function PermissionsTab(props: {
  resource: Resource;
  onEdit: (update: (draft: Resource) => void) => void;
  basePath: string;
}) {
  // A policy is `<level>` or `<level>@org_class=<name>`; the UI edits the two
  // halves separately and writes them back as one string.
  const bareOf = (value: string | undefined) => (value ?? "member").split(ORG_CLASS_SUFFIX)[0];
  const levelOf = (value: string | undefined) => (bareOf(value).startsWith("role:") ? "role" : bareOf(value));
  const roleOf = (value: string | undefined) =>
    bareOf(value).startsWith("role:") ? bareOf(value).slice(5) : "";
  const classOf = (value: string | undefined) => {
    const at = (value ?? "").indexOf(ORG_CLASS_SUFFIX);
    return at === -1 ? "" : (value ?? "").slice(at + ORG_CLASS_SUFFIX.length);
  };
  const compose = (bare: string, orgClass: string) => (orgClass ? `${bare}${ORG_CLASS_SUFFIX}${orgClass}` : bare);

  return (
    <Card>
      <CardHeader
        title="Access policy"
        hint="Evaluated after authentication; omitted actions default to member of the active organisation."
      />
      <div class="divide-y divide-line">
        <For each={ACTIONS}>
          {(action) => {
            const current = () => props.resource.permissions[action];
            return (
              <div class="flex flex-wrap items-center gap-3 px-4 py-3">
                <div class="w-40 shrink-0">
                  <p class="text-[0.8125rem] font-medium capitalize text-ink">{action}</p>
                  <p class="font-mono text-[0.6875rem] text-faint">
                    {ACTION_ENDPOINT[action].replace("{res}", `${props.basePath}/${props.resource.name}`)}
                  </p>
                </div>
                <Select
                  class="max-w-sm flex-1"
                  value={levelOf(current())}
                  options={ACCESS_OPTIONS}
                  onChange={(value) =>
                    props.onEdit((draft) => {
                      const bare = value === "role" ? `role:${roleOf(current()) || "admin"}` : value;
                      draft.permissions[action] = compose(bare, classOf(current()));
                    })
                  }
                />
                <Show when={levelOf(current()) === "role"}>
                  <TextInput
                    mono
                    class="max-w-[10rem]"
                    value={roleOf(current())}
                    placeholder="admin"
                    onInput={(value) =>
                      props.onEdit((draft) => {
                        draft.permissions[action] = compose(`role:${value}`, classOf(current()));
                      })
                    }
                  />
                </Show>
                <label class="flex items-center gap-2 text-[0.6875rem] text-faint">
                  <span class="whitespace-nowrap">in org class</span>
                  <TextInput
                    mono
                    class="max-w-[9rem]"
                    value={classOf(current())}
                    placeholder="any"
                    onInput={(value) =>
                      props.onEdit((draft) => {
                        draft.permissions[action] = compose(bareOf(current()), value.trim());
                      })
                    }
                  />
                </label>
              </div>
            );
          }}
        </For>
      </div>
      <div class="border-t border-line px-4 py-3 text-[0.6875rem] leading-relaxed text-faint">
        <Show when={props.resource.scope === "organization"}>
          Rows are isolated per organisation before any of this runs, and <Mono>public</Mono> behaves as{" "}
          <Mono>member</Mono> here.{" "}
        </Show>
        <Show when={Object.values(props.resource.permissions).some((value) => value?.includes(ORG_CLASS_SUFFIX))}>
          A class narrows an action to organisations whose <Mono>org_class</Mono> matches — never widens it.
          Leave it empty for every organisation.{" "}
        </Show>
        <Show when={Object.values(props.resource.permissions).some((value) => bareOf(value) === "owner")}>
          <Mono>owner</Mono> compares <Mono>{props.resource.owner_field}</Mono> to the caller and stamps it on
          create.
        </Show>
      </div>
    </Card>
  );
}

// ---- hooks ------------------------------------------------------------------

type HookGroup = { action: string; note: string; events: (HookEvent | AuthHookEvent)[] };

const HOOK_GROUPS: HookGroup[] = [
  { action: "list", note: "before sees nothing, but returning data answers the request without a query; after receives the rows and can replace the body", events: ["before_list", "after_list"] },
  { action: "read", note: "before can answer from a cache by returning data, skipping the query; after receives the fetched row", events: ["before_read", "after_read"] },
  { action: "create", note: "before can validate, rewrite or abort; after sees the created row", events: ["before_create", "after_create"] },
  { action: "update", note: "before receives the submitted body, after the updated row", events: ["before_update", "after_update"] },
  { action: "delete", note: "both receive the row; declaring either costs one extra read", events: ["before_delete", "after_delete"] },
];

// The auth endpoints are the user table's other door, so their hooks live in
// the same section — but only `user` has them, and the server says so too.
const AUTH_HOOK_GROUPS: HookGroup[] = [
  { action: "register", note: "runs around POST /auth/register, outside the create hooks above", events: ["before_register", "after_register"] },
  { action: "login", note: "before sees the claimed identity, never the password; after sees every attempt, successful or not", events: ["before_login", "after_login"] },
  { action: "api key", note: "before writes the key row; after can widen the response, but never reaches the plaintext key", events: ["before_api_key", "after_api_key"] },
];

function HooksTab(props: { resource: Resource; onEdit: (update: (draft: Resource) => void) => void }) {
  const names = createMemo(() => allFunctionNames());

  return (
    <Card>
      <CardHeader
        title="Lifecycle hooks"
        hint="One function per event. Hooks ignore visibility, so hook functions are usually Private."
      />
      <datalist id="function-names">
        <For each={names()}>{(name) => <option value={name} />}</For>
      </datalist>
      <div class="divide-y divide-line">
        <For each={[...HOOK_GROUPS, ...(props.resource.name === "user" ? AUTH_HOOK_GROUPS : [])]}>
          {(group) => (
            <div class="px-4 py-3">
              <div class="mb-2 flex items-baseline gap-2">
                <h4 class="text-[0.8125rem] font-medium capitalize text-ink">{group.action}</h4>
                <p class="text-[0.6875rem] text-faint">{group.note}</p>
              </div>
              <div class="grid gap-3 sm:grid-cols-2">
                <For each={group.events}>
                  {(event) => (
                    <Labelled label={event.replaceAll("_", " ")}>
                      <TextInput
                        mono
                        lowercase
                        list="function-names"
                        placeholder="function name"
                        value={props.resource.hooks[event] ?? ""}
                        onInput={(value) =>
                          props.onEdit((draft) => {
                            if (value) draft.hooks[event] = value;
                            else delete draft.hooks[event];
                          })
                        }
                      />
                    </Labelled>
                  )}
                </For>
              </div>
            </div>
          )}
        </For>
      </div>
      <Show
        when={[...HOOK_EVENTS, ...AUTH_HOOK_EVENTS].some(
          (event) => props.resource.hooks[event] && !names().includes(props.resource.hooks[event]!),
        )}
      >
        <div class="border-t border-line px-4 py-3 text-xs leading-relaxed text-warn">
          A hook names a function no library in <Mono>functions/</Mono> exports. Requests on that operation
          fail closed with a 500 until the library is built and dropped in.
        </div>
      </Show>
    </Card>
  );
}

// ---- settings ---------------------------------------------------------------

function SettingsTab(props: {
  entry: ResourceEntry;
  onEdit: (update: (draft: Resource) => void) => void;
  confirming: boolean;
  setConfirming: (value: boolean) => void;
}) {
  const resource = () => props.entry.resource;

  // Only a visible text column can be searched: `?search=` matches substrings,
  // and a hidden field would answer questions its own responses refuse.
  const searchable = createMemo(() =>
    resource().fields.filter((field) => !field.hidden && (field.type === "string" || field.type === "text")),
  );
  // A field that was chosen and then deleted (or hidden) would fail the app at
  // load, so the list shown — and the list written — is the surviving one.
  const chosen = createMemo(() =>
    (resource().search_fields ?? []).filter((name) => searchable().some((field) => field.name === name)),
  );

  const toggleSearchField = (name: string) => {
    props.onEdit((draft) => {
      const current = (draft.search_fields ?? []).filter((entry) =>
        draft.fields.some((field) => field.name === entry && !field.hidden),
      );
      const next = current.includes(name)
        ? current.filter((entry) => entry !== name)
        : [...current, name];
      if (next.length) draft.search_fields = next;
      else delete draft.search_fields;
    });
  };

  return (
    <div class="space-y-3">
      <Card>
        <CardHeader title="Identity" />
        <div class="grid gap-4 px-4 py-4 sm:grid-cols-2">
          <Labelled
            label="name"
            hint={
              props.entry.builtin
                ? "Fixed — the framework looks this resource up by name."
                : "The URL segment and the logical name. snake_case; renaming moves the file."
            }
          >
            <CommitInput
              mono
              lowercase
              value={resource().name}
              disabled={props.entry.builtin}
              onCommit={(value) => {
                props.onEdit((draft) => {
                  draft.name = value;
                });
                setView({ kind: "resource", name: value });
              }}
            />
          </Labelled>
          <Labelled label="table" hint={`Physical table; defaults to apiplant_${resource().name}.`}>
            <TextInput
              mono
              lowercase
              value={resource().table ?? ""}
              placeholder={`apiplant_${resource().name}`}
              onInput={(value) =>
                props.onEdit((draft) => {
                  if (value) draft.table = value;
                  else delete draft.table;
                })
              }
            />
          </Labelled>
          <Labelled label="scope" hint="Organisation-scoped resources are isolated per tenant automatically.">
            <Select
              value={resource().scope}
              options={SCOPES}
              onChange={(value) =>
                props.onEdit((draft) => {
                  draft.scope = value as Scope;
                })
              }
            />
          </Labelled>
          <Labelled label="owner field" hint="Column compared to the caller for `owner` permissions.">
            <TextInput
              mono
              lowercase
              value={resource().owner_field}
              placeholder="owner_id"
              onInput={(value) =>
                props.onEdit((draft) => {
                  draft.owner_field = value || "owner_id";
                })
              }
            />
          </Labelled>
          <div class="sm:col-span-2">
            <Switch
              checked={resource().timestamps}
              label="Add created_at and updated_at"
              onChange={(value) =>
                props.onEdit((draft) => {
                  draft.timestamps = value;
                })
              }
            />
          </div>
        </div>
      </Card>

      <Card>
        <CardHeader
          title="Search"
          hint="What one ?search= term is matched against — in the API, and in the dashboard's search box."
        />
        <div class="px-4 py-4">
          <Show
            when={searchable().length}
            fallback={
              <p class="text-xs leading-relaxed text-muted">
                Nothing to search yet. Searching means matching part of a value, so it needs a visible{" "}
                <Mono>string</Mono> or <Mono>text</Mono> field.
              </p>
            }
          >
            <div class="flex flex-wrap gap-1.5">
              <For each={searchable()}>
                {(field) => (
                  <Toggle
                    checked={chosen().includes(field.name)}
                    label={field.name}
                    hint={`Search ${field.name} as well`}
                    onChange={() => toggleSearchField(field.name)}
                  />
                )}
              </For>
            </div>
            <p class="mt-3 text-xs leading-relaxed text-muted">
              <Show
                when={chosen().length}
                fallback={
                  <>
                    None chosen — a term is matched against whichever single field names a record
                  </>
                }
              >
                A term matches a row when any of {chosen().join(", ")} contains it. Callers that know the
                model can still narrow one request with <Mono>?search_fields=</Mono>.
              </Show>
            </p>
          </Show>
        </div>
      </Card>

      <Show when={resource().name === "user"}>
        <Card>
          <CardHeader
            title="Authentication"
            hint="Only meaningful on the user resource — what /auth/login and /auth/register work against."
          />
          <div class="grid gap-4 px-4 py-4 sm:grid-cols-2">
            <Labelled label="identity field" hint="What POST /auth/login expects — email, username, …">
              <TextInput
                mono
                value={resource().auth?.identity_field ?? "email"}
                onInput={(value) =>
                  props.onEdit((draft) => {
                    draft.auth = {
                      identity_field: value,
                      password_field: draft.auth?.password_field ?? "password_hash",
                      oauth_providers: draft.auth?.oauth_providers ?? [],
                    };
                  })
                }
              />
            </Labelled>
            <Labelled label="password field" hint="Where the argon2id hash is stored. Mark it hidden.">
              <TextInput
                mono
                value={resource().auth?.password_field ?? "password_hash"}
                onInput={(value) =>
                  props.onEdit((draft) => {
                    draft.auth = {
                      identity_field: draft.auth?.identity_field ?? "email",
                      password_field: value,
                      oauth_providers: draft.auth?.oauth_providers ?? [],
                    };
                  })
                }
              />
            </Labelled>
            <Labelled
              class="sm:col-span-2"
              label="oauth providers"
              hint="Comma separated. Declares linked-identity scaffolding."
            >
              <TextInput
                mono
                lowercase
                value={(resource().auth?.oauth_providers ?? []).join(", ")}
                placeholder="google, github"
                onInput={(value) =>
                  props.onEdit((draft) => {
                    draft.auth = {
                      identity_field: draft.auth?.identity_field ?? "email",
                      password_field: draft.auth?.password_field ?? "password_hash",
                      oauth_providers: value
                        .split(",")
                        .map((provider) => provider.trim())
                        .filter(Boolean),
                    };
                  })
                }
              />
            </Labelled>
          </div>
        </Card>
      </Show>

      <Card class="border-danger-line">
        <CardHeader
          title={props.entry.builtin ? "Revert to the built-in" : "Delete this resource"}
          hint={
            props.entry.builtin
              ? "Removes the file; the framework goes back to shipping its own definition."
              : "Removes the model file. The table and its rows stay in Postgres — apiplant never drops anything."
          }
        >
          <Show
            when={props.confirming}
            fallback={
              <Button
                size="sm"
                variant="danger"
                disabled={props.entry.builtin && !props.entry.path}
                onClick={() => props.setConfirming(true)}
              >
                {props.entry.builtin ? "Revert" : "Delete"}
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
                deleteResource(props.entry.name);
              }}
            >
              Yes, {props.entry.builtin ? "revert" : "delete"}
            </Button>
          </Show>
        </CardHeader>
      </Card>
    </div>
  );
}

export function copyToClipboard(text: string) {
  navigator.clipboard
    ?.writeText(text)
    .then(() => toast("Copied", "success"))
    .catch(() => toast("Could not copy", "error"));
}
