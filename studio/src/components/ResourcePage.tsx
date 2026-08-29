import { For, Show, createMemo, createSignal } from "solid-js";
import {
  ACTIONS,
  AUTH_HOOK_EVENTS,
  DEFAULT_PERMISSIONS,
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
  type Effect,
  type HookEvent,
  type OnDelete,
  type PermissionRule,
  type PermissionSet,
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
import {
  formatPolicy,
  parsePolicy,
  permissionConflicts,
  type Subject,
} from "../lib/permissions";
import { PolicyPhrase } from "./PolicyPhrase";
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

/**
 * Levels a clause of each effect may name.
 *
 * `owner` is deliberately absent: as a level it says the same thing as an
 * ownership clause — "allow only if they own the row" — and offering both
 * spellings of one policy is how a form teaches the model wrong. Files that
 * already use it, several built-ins among them, keep working: the sentence
 * still says what they say, and offers to rewrite itself.
 *
 * Ownership is a comparison against the caller, so `public` cannot be one:
 * an anonymous request owns nothing, and the clause would match nobody however
 * the data looked. `private` — "no-one" — is not a caller at all but the
 * absence of an endpoint, so only an `allow` clause can say it: denying no-one
 * is a clause that does nothing, and the way to shut an action to everybody is
 * to allow no-one, or to deny everybody.
 */
const LEVELS_BY_EFFECT: Record<Effect, string[]> = {
  allow: ["public", "authenticated", "member", "role", "private"],
  own: ["authenticated", "member", "role"],
  deny: ["public", "authenticated", "member", "role"],
};

// `{res}` is substituted with `<base_path>/<resource>`, which already carries a
// leading slash — so these must not add one of their own.
const ACTION_ENDPOINT: Record<Action, string> = {
  list: "GET {res}",
  read: "GET {res}/{id}",
  create: "POST {res}",
  update: "PATCH {res}/{id}",
  delete: "DELETE {res}/{id}",
};

const SCALAR_DEFAULTABLE: FieldType[] = [
  "string",
  "text",
  "integer",
  "big_int",
  "float",
  "boolean",
];

export function ResourcePage(props: { entry: ResourceEntry }) {
  const [tab, setTab] = createSignal<TabId>("fields");
  const [tomlDraft, setTomlDraft] = createSignal<string | null>(null);
  const [tomlError, setTomlError] = createSignal<string | null>(null);
  const [confirmingDelete, setConfirmingDelete] = createSignal(false);

  const resource = () => props.entry.resource;
  const edit = (update: (draft: Resource) => void) =>
    updateResource(props.entry.name, update);

  const basePath = () => {
    const raw = configValue("server", "base_path");
    return typeof raw === "string" && raw ? raw.replace(/\/$/, "") : "";
  };

  const resourceNames = createMemo(() =>
    (studio.project?.resources ?? []).map((entry) => entry.name),
  );
  const issues = createMemo(() =>
    validateResource(resource(), resourceNames()),
  );

  const canonicalToml = createMemo(() =>
    props.entry.path
      ? (fileText(props.entry.path) ?? emitResource(resource()))
      : emitResource(resource()),
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
          <h1 class="font-mono text-xl font-semibold tracking-tight">
            {resource().name}
          </h1>
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
                <Mono>{`${basePath()}/{parent}/{id}/${resource().name}`}</Mono>{" "}
                for every resource that references it.
              </>
            }
          >
            {props.entry.builtinSummary ??
              BUILTIN_SUMMARY[props.entry.name as BuiltinName]}
          </Show>
        </p>

        <Show when={props.entry.builtin && !props.entry.path}>
          <div class="mt-3 flex items-center gap-3 rounded-lg border border-line bg-surface px-3 py-2">
            <p class="flex-1 text-xs leading-relaxed text-muted">
              This resource exists without a file. Editing anything writes{" "}
              <Mono>resources/…</Mono> to replace the framework default — the
              framework keeps using it for auth, ownership and org resolution.
            </p>
            <Button
              size="sm"
              onClick={() => customizeBuiltin(props.entry.name)}
            >
              Create the file
            </Button>
          </div>
        </Show>

        <Show when={issues().length}>
          <ul class="mt-3 space-y-1 rounded-lg border border-danger-line bg-danger-soft px-3 py-2">
            <For each={issues()}>
              {(issue) => (
                <li class="text-xs leading-relaxed text-danger">{issue}</li>
              )}
            </For>
          </ul>
        </Show>

        <Show when={originalHadComments()}>
          <p class="mt-3 rounded-lg border border-warn-line bg-warn-soft px-3 py-2 text-xs leading-relaxed text-warn">
            The file on disk has comments. Saving rewrites it from the form,
            which drops them — edit on the TOML tab instead to keep them.
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
            {
              id: "hooks",
              label: "Hooks",
              badge: Object.keys(resource().hooks).length,
            },
            { id: "settings", label: "Settings" },
            { id: "toml", label: "TOML" },
          ]}
        />
      </div>

      <Show when={tab() === "fields"}>
        <FieldsTab
          entry={props.entry}
          onEdit={edit}
          resourceNames={resourceNames()}
        />
      </Show>

      <Show when={tab() === "permissions"}>
        <PermissionsTab
          resource={resource()}
          onEdit={edit}
          basePath={basePath()}
        />
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
              The file as it will be written. Edit here to keep comments and
              anything the forms do not model.
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

  /**
   * Only one field is open at a time. A schema is read far more often than it is
   * edited, so the list defaults to one scannable line per column and the full
   * set of controls belongs to whichever field is actually being worked on.
   * Tracked by position, which is what moving a field changes.
   */
  const [openIndex, setOpenIndex] = createSignal<number | null>(null);
  const toggleOpen = (index: number) =>
    setOpenIndex((current) => (current === index ? null : index));

  const addField = () => {
    const base = "field";
    let name = base;
    let index = 2;
    while (resource().fields.some((field) => field.name === name))
      name = `${base}_${index++}`;
    const at = resource().fields.length;
    props.onEdit((draft) => {
      draft.fields.push({ name, type: "string" });
    });
    // A new field is always a placeholder name, so open it ready to be typed over.
    setOpenIndex(at);
  };

  const automatic = createMemo(() => {
    const columns = ["id — uuid primary key"];
    if (resource().timestamps)
      columns.push("created_at, updated_at — timestamptz");
    if (resource().scope === "organization")
      columns.push("organization_id — reference to organization");
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
              No fields yet. The resource would still work — it just carries
              only its automatic columns.
            </p>
          }
        >
          <div class="divide-y divide-line">
            <For each={resource().fields} keyed={false}>
              {(field, index) => (
                <FieldRow
                  field={field()}
                  position={index}
                  count={resource().fields.length}
                  open={openIndex() === index}
                  onToggle={() => toggleOpen(index)}
                  resourceNames={props.resourceNames}
                  onChange={(update) =>
                    props.onEdit((draft) => update(draft.fields[index]))
                  }
                  onRemove={() => {
                    const at = index;
                    props.onEdit((draft) => {
                      draft.fields.splice(at, 1);
                    });
                    // Positions below the hole shift up; anything at or after it
                    // would otherwise open the wrong field.
                    setOpenIndex((current) =>
                      current === null || current === at
                        ? null
                        : current > at
                          ? current - 1
                          : current,
                    );
                  }}
                  onMove={(direction) => {
                    const at = index;
                    const target = at + direction;
                    if (target < 0 || target >= resource().fields.length)
                      return;
                    props.onEdit((draft) => {
                      const [moved] = draft.fields.splice(at, 1);
                      draft.fields.splice(target, 0, moved);
                    });
                    // Follow the field that moved rather than the slot it left.
                    setOpenIndex((current) =>
                      current === at
                        ? target
                        : current === target
                          ? at
                          : current,
                    );
                  }}
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

/** The flags that read as constraints, shown as pills on a collapsed row. */
const FIELD_FLAGS = [
  { key: "required", label: "required", hint: "NOT NULL" },
  {
    key: "unique",
    label: "unique",
    hint: "UNIQUE constraint; a conflict returns 409",
  },
  {
    key: "hidden",
    label: "hidden",
    hint: "Stripped from every API response, still writable",
  },
] as const;

function FieldRow(props: {
  field: Field;
  position: number;
  count: number;
  open: boolean;
  onToggle: () => void;
  resourceNames: string[];
  onChange: (update: (field: Field) => void) => void;
  onRemove: () => void;
  onMove: (direction: 1 | -1) => void;
}) {
  const isReference = () => props.field.type === "reference";

  /**
   * Everything the expanded editor would show, folded into one line. A reader
   * scanning the schema wants the shape — name, type, what it points at, what it
   * defaults to — not eleven controls per column.
   */
  const detail = createMemo(() => {
    const parts: string[] = [];
    const field = props.field;
    if (isReference() && field.references) parts.push(`→ ${field.references}`);
    if (field.max_length) parts.push(`≤ ${field.max_length}`);
    if (field.format && field.format !== "plain") parts.push(field.format);
    if (field.default !== undefined) parts.push(`= ${field.default}`);
    return parts;
  });

  const flags = createMemo(() =>
    FIELD_FLAGS.filter((flag) => Boolean(props.field[flag.key])),
  );

  return (
    <div
      class={`group transition-colors ${props.open ? "bg-surface-2/50" : "hover:bg-surface-2/30"}`}
    >
      <div class="flex items-center gap-2 px-3 py-2">
        <button
          type="button"
          onClick={props.onToggle}
          aria-expanded={props.open ? "true" : "false"}
          title={props.open ? "Collapse" : "Edit this field"}
          class="flex min-w-0 flex-1 items-center gap-2 rounded text-left"
        >
          <svg
            width="12"
            height="12"
            viewBox="0 0 16 16"
            fill="none"
            stroke="currentColor"
            stroke-width="1.7"
            class={`shrink-0 text-faint transition-transform ${props.open ? "rotate-90" : ""}`}
          >
            <path
              d="M6 3.5 10.5 8 6 12.5"
              stroke-linecap="round"
              stroke-linejoin="round"
            />
          </svg>
          <span
            class={`truncate font-mono text-[0.78125rem] ${props.field.name ? "text-ink" : "text-faint italic"}`}
          >
            {props.field.name || "unnamed"}
          </span>
          <Badge
            tone={isReference() ? "accent" : "neutral"}
            class="shrink-0 font-mono"
          >
            {props.field.type}
          </Badge>
          <Show when={!props.open}>
            <span class="hidden min-w-0 items-center gap-2 sm:flex">
              <For each={detail()}>
                {(part) => (
                  <span class="truncate font-mono text-[0.6875rem] text-faint">
                    {part}
                  </span>
                )}
              </For>
              <For each={flags()}>
                {(flag) => (
                  <span
                    class="shrink-0 text-[0.6875rem] text-muted"
                    title={flag.hint}
                  >
                    {flag.label}
                  </span>
                )}
              </For>
            </span>
          </Show>
        </button>

        <div
          class={`flex shrink-0 items-center gap-0.5 transition-opacity ${
            props.open
              ? ""
              : "sm:opacity-0 sm:group-hover:opacity-100 sm:focus-within:opacity-100"
          }`}
        >
          <button
            type="button"
            title="Move up"
            disabled={props.position === 0}
            onClick={() => props.onMove(-1)}
            class="rounded p-1 text-faint hover:bg-surface-3 hover:text-ink disabled:pointer-events-none disabled:opacity-30"
          >
            <svg
              width="13"
              height="13"
              viewBox="0 0 16 16"
              fill="none"
              stroke="currentColor"
              stroke-width="1.5"
            >
              <path
                d="M8 12.5v-9M4.5 7L8 3.5 11.5 7"
                stroke-linecap="round"
                stroke-linejoin="round"
              />
            </svg>
          </button>
          <button
            type="button"
            title="Move down"
            disabled={props.position === props.count - 1}
            onClick={() => props.onMove(1)}
            class="rounded p-1 text-faint hover:bg-surface-3 hover:text-ink disabled:pointer-events-none disabled:opacity-30"
          >
            <svg
              width="13"
              height="13"
              viewBox="0 0 16 16"
              fill="none"
              stroke="currentColor"
              stroke-width="1.5"
            >
              <path
                d="M8 3.5v9M4.5 9L8 12.5 11.5 9"
                stroke-linecap="round"
                stroke-linejoin="round"
              />
            </svg>
          </button>
          <button
            type="button"
            title="Remove field"
            onClick={props.onRemove}
            class="rounded p-1 text-faint hover:bg-danger-soft hover:text-danger"
          >
            <svg
              width="13"
              height="13"
              viewBox="0 0 16 16"
              fill="none"
              stroke="currentColor"
              stroke-width="1.5"
            >
              <path
                d="M3 4.5h10M6.5 4.5V3h3v1.5M5 4.5l.5 8h5l.5-8"
                stroke-linecap="round"
                stroke-linejoin="round"
              />
            </svg>
          </button>
        </div>
      </div>

      <Show when={props.open}>
        <div class="space-y-3 border-t border-line/60 px-4 py-3 pl-8">
          <div class="flex flex-wrap items-center gap-2">
            <TextInput
              mono
              lowercase
              autofocus
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
                  if (!["string", "text"].includes(field.type))
                    delete field.format;
                })
              }
            />

            <div class="flex w-full flex-wrap items-center gap-1.5 sm:w-auto sm:flex-1">
              <For each={FIELD_FLAGS}>
                {(flag) => (
                  <Toggle
                    label={flag.label}
                    hint={flag.hint}
                    checked={!!props.field[flag.key]}
                    onChange={(value) =>
                      props.onChange((field) => {
                        if (value) field[flag.key] = true;
                        else delete field[flag.key];
                      })
                    }
                  />
                )}
              </For>
            </div>
          </div>

          <div class="grid grid-cols-2 gap-3 sm:grid-cols-4">
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
                  value={
                    props.field.max_length ? String(props.field.max_length) : ""
                  }
                  placeholder="unbounded"
                  onInput={(value) =>
                    props.onChange((field) => {
                      const parsed = Number.parseInt(value, 10);
                      if (Number.isFinite(parsed) && parsed > 0)
                        field.max_length = parsed;
                      else delete field.max_length;
                    })
                  }
                />
              </Labelled>
            </Show>

            <Show
              when={
                props.field.type === "string" || props.field.type === "text"
              }
            >
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
                      value={
                        props.field.default === true
                          ? "true"
                          : props.field.default === false
                            ? "false"
                            : ""
                      }
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
                    value={
                      props.field.default === undefined
                        ? ""
                        : String(props.field.default)
                    }
                    placeholder="none"
                    onInput={(value) =>
                      props.onChange((field) => {
                        if (value === "") {
                          delete field.default;
                          return;
                        }
                        const numeric = [
                          "integer",
                          "big_int",
                          "float",
                        ].includes(field.type);
                        const parsed = Number(value);
                        field.default =
                          numeric && Number.isFinite(parsed) ? parsed : value;
                      })
                    }
                  />
                </Show>
              </Labelled>
            </Show>

            <Show when={isReference() && props.field.references}>
              <div class="col-span-2 self-end pb-1 text-[0.6875rem] leading-relaxed text-faint sm:col-span-2">
                Expands as{" "}
                <Mono>?expand={props.field.name.replace(/_id$/, "")}</Mono> and
                publishes{" "}
                <Mono>
                  GET /{props.field.references}/{"{id}"}/…
                </Mono>
              </div>
            </Show>
          </div>
        </div>
      </Show>
    </div>
  );
}

// ---- permissions ------------------------------------------------------------

/** A new clause when there is nothing above it to follow. */
const DEFAULT_CLAUSE: PermissionRule = { policy: "member", effect: "allow" };

/**
 * The access policy, one action per block and one sentence per clause.
 *
 * The clauses are the whole model: a caller no sentence names is refused, so
 * reading the list top to bottom *is* reading the rule. Effect used to be the
 * column a clause sat in; it is now the first word of the sentence, which says
 * the same thing in the place somebody looking for it would read.
 */
function PermissionsTab(props: {
  resource: Resource;
  onEdit: (update: (draft: Resource) => void) => void;
  basePath: string;
}) {
  const rulesOf = (action: Action): PermissionSet =>
    props.resource.permissions[action] ?? DEFAULT_PERMISSIONS[action];

  /**
   * Rewrite one action's clauses.
   *
   * Emptying the list cannot leave the action out of the file: an omitted
   * action is the `member` default, which is the opposite of what clearing it
   * means, so it is written as `private` instead.
   */
  const setRules = (action: Action, rules: PermissionRule[]) =>
    props.onEdit((draft) => {
      draft.permissions[action] = rules.length
        ? rules
        : [{ policy: "private", effect: "allow" }];
    });

  const editRule = (
    action: Action,
    index: number,
    update: (rule: PermissionRule) => PermissionRule,
  ) =>
    setRules(
      action,
      rulesOf(action).map((rule, i) => (i === index ? update(rule) : rule)),
    );

  const editSubject = (
    action: Action,
    index: number,
    update: (subject: Subject) => Subject,
  ) =>
    editRule(action, index, (rule) => ({
      ...rule,
      policy: formatPolicy(update(parsePolicy(rule.policy))),
    }));

  /**
   * Change what a clause does, keeping who it names where that still parses.
   *
   * The levels differ by effect — nobody owns a row anonymously, and a denial
   * naming everybody leaves nothing behind — so a level the new effect cannot
   * take falls back to the signed-in caller rather than being written out as
   * something the server would reject.
   */
  const setEffect = (action: Action, index: number, effect: Effect) =>
    editRule(action, index, (rule) => {
      const subject = parsePolicy(rule.policy);
      const level = LEVELS_BY_EFFECT[effect].includes(subject.level)
        ? subject.level
        : "authenticated";
      return {
        effect,
        policy: formatPolicy({
          ...subject,
          level,
          role: level === "role" ? subject.role || "admin" : "",
        }),
      };
    });

  /** A new clause repeats the one above it — most lists are a variation. */
  const addRule = (action: Action) => {
    const rules = rulesOf(action);
    const previous = rules[rules.length - 1];
    setRules(action, [...rules, previous ? { ...previous } : DEFAULT_CLAUSE]);
  };

  const allRules = () => ACTIONS.flatMap((action) => rulesOf(action));
  const usesEffect = (effect: Effect) =>
    allRules().some((rule) => rule.effect === effect);
  const isPrivate = (action: Action) =>
    rulesOf(action).every(
      (rule) => parsePolicy(rule.policy).level === "private",
    );

  return (
    <Card>
      <CardHeader
        title="Access policy"
        hint="Evaluated after authentication. A caller no clause names is refused, and deny is consulted before the rest."
      />

      <div class="divide-y divide-line">
        <For each={ACTIONS}>
          {(action) => (
            <div class="px-4 py-3">
              <div class="flex flex-wrap items-baseline gap-x-2 gap-y-0.5">
                <p class="text-[0.8125rem] font-medium capitalize text-ink">
                  {action}
                </p>
                <p class="font-mono text-[0.6875rem] text-faint">
                  {ACTION_ENDPOINT[action].replace(
                    "{res}",
                    `${props.basePath}/${props.resource.name}`,
                  )}
                </p>
                <Show when={isPrivate(action)}>
                  <p class="text-[0.6875rem] text-faint">
                    — 404, and left out of the docs.
                  </p>
                </Show>
              </div>

              {/* Keyed by position, not by value: a clause *is* its policy
                  string, so `For` would tear the line down on every edit and
                  take the open control with it. */}
              <For each={rulesOf(action)} keyed={false}>
                {(rule, index) => (
                  <ClauseLine
                    rule={rule()}
                    action={action}
                    onSubject={(update) => editSubject(action, index, update)}
                    onEffect={(effect) => setEffect(action, index, effect)}
                    onRemove={() =>
                      setRules(
                        action,
                        rulesOf(action).filter((_, i) => i !== index),
                      )
                    }
                  />
                )}
              </For>

              {/* A warning, not an error: every one of these loads and runs.
                  It sits under the action it is about and nowhere else — a
                  second copy at the top of the page would move the clause you
                  are editing out from under the pointer. */}
              <For each={permissionConflicts(rulesOf(action), action)}>
                {(issue) => (
                  <p class="mt-1 text-[0.6875rem] leading-relaxed text-warn">
                    {issue}
                  </p>
                )}
              </For>

              <button
                type="button"
                class="mt-1 text-[0.6875rem] text-faint transition-colors hover:text-accent"
                onClick={() => addRule(action)}
              >
                + add a clause
              </button>
            </div>
          )}
        </For>
      </div>

      <div class="border-t border-line px-4 py-3 text-[0.6875rem] leading-relaxed text-faint">
        <Show when={props.resource.scope === "organization"}>
          Rows are isolated per organisation before any of this runs, and{" "}
          <Mono>public</Mono> behaves as <Mono>member</Mono> here.{" "}
        </Show>
        <Show
          when={allRules().some((rule) =>
            rule.policy.includes(ORG_CLASS_SUFFIX),
          )}
        >
          A class narrows a clause to organisations whose <Mono>org_class</Mono>{" "}
          matches — never widens it. Leave it empty for every organisation.{" "}
        </Show>
        <Show
          when={
            usesEffect("own") ||
            allRules().some(
              (rule) => parsePolicy(rule.policy).level === "owner",
            )
          }
        >
          Ownership compares <Mono>{props.resource.owner_field}</Mono> to the
          caller and stamps it on create.{" "}
        </Show>
        <Show when={usesEffect("deny")}>
          A <Mono>deny</Mono> outranks every <Mono>allow</Mono> the same caller
          matches, and matches only a role somebody actually holds — never the
          blanket one an <Mono>admin</Mono> gets, which would lock the
          organisation's own administrators out.
        </Show>
      </div>
    </Card>
  );
}

/** One clause, read as a sentence, with the remove control on the same line. */
function ClauseLine(props: {
  rule: PermissionRule;
  action: Action;
  onSubject: (update: (subject: Subject) => Subject) => void;
  onEffect: (effect: Effect) => void;
  onRemove: () => void;
}) {
  const subject = () => parsePolicy(props.rule.policy);

  /** A legacy `owner` level allowed outright is this clause's own effect. */
  const legacyOwner = () =>
    props.rule.effect === "allow" && subject().level === "owner";

  // Ownership on create is a contradiction the server rejects, so the editor
  // does not offer it — `own` as an effect, or `owner` as a level. A file that
  // sets one by hand still shows it (and a warning under the action), but the
  // form will not put it there.
  const ownershipOffered = () => props.action !== "create";

  return (
    <div class="group mt-0.5 flex items-start gap-1.5">
      <span class="select-none pt-[0.3rem] text-[0.6875rem] leading-none text-faint">
        —
      </span>
      <div class="min-w-0 flex-1">
        <PolicyPhrase
          subject={subject()}
          onChange={props.onSubject}
          effect={props.rule.effect}
          onEffectChange={props.onEffect}
          effects={ownershipOffered() ? undefined : ["allow", "deny"]}
          levels={LEVELS_BY_EFFECT[props.rule.effect]}
        />
        <Show when={legacyOwner() && ownershipOffered()}>
          <p class="text-[0.6875rem] leading-relaxed text-faint">
            Written the old way —{" "}
            <button
              type="button"
              class="text-accent underline decoration-dotted underline-offset-2 hover:text-ink"
              onClick={() => {
                props.onSubject((current) => ({
                  ...current,
                  level: "authenticated",
                }));
                props.onEffect("own");
              }}
            >
              say it as an ownership clause
            </button>
          </p>
        </Show>
      </div>
      <button
        type="button"
        class="shrink-0 rounded px-1.5 text-faint transition-colors hover:text-danger"
        title="Remove this clause"
        onClick={props.onRemove}
      >
        ×
      </button>
    </div>
  );
}


// ---- hooks ------------------------------------------------------------------

type HookGroup = {
  action: string;
  note: string;
  events: (HookEvent | AuthHookEvent)[];
};

const HOOK_GROUPS: HookGroup[] = [
  {
    action: "list",
    note: "before sees nothing, but returning data answers the request without a query; after receives the rows and can replace the body",
    events: ["before_list", "after_list"],
  },
  {
    action: "read",
    note: "before can answer from a cache by returning data, skipping the query; after receives the fetched row",
    events: ["before_read", "after_read"],
  },
  {
    action: "create",
    note: "before can validate, rewrite or abort; after sees the created row",
    events: ["before_create", "after_create"],
  },
  {
    action: "update",
    note: "before receives the submitted body, after the updated row",
    events: ["before_update", "after_update"],
  },
  {
    action: "delete",
    note: "both receive the row; declaring either costs one extra read",
    events: ["before_delete", "after_delete"],
  },
];

// The auth endpoints are the user table's other door, so their hooks live in
// the same section — but only `user` has them, and the server says so too.
const AUTH_HOOK_GROUPS: HookGroup[] = [
  {
    action: "register",
    note: "runs around POST /auth/register, outside the create hooks above",
    events: ["before_register", "after_register"],
  },
  {
    action: "login",
    note: "before sees the claimed identity, never the password; after sees every attempt, successful or not",
    events: ["before_login", "after_login"],
  },
  {
    action: "api key",
    note: "before writes the key row; after can widen the response, but never reaches the plaintext key",
    events: ["before_api_key", "after_api_key"],
  },
];

function HooksTab(props: {
  resource: Resource;
  onEdit: (update: (draft: Resource) => void) => void;
}) {
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
        <For
          each={[
            ...HOOK_GROUPS,
            ...(props.resource.name === "user" ? AUTH_HOOK_GROUPS : []),
          ]}
        >
          {(group) => (
            <div class="px-4 py-3">
              <div class="mb-2 flex items-baseline gap-2">
                <h4 class="text-[0.8125rem] font-medium capitalize text-ink">
                  {group.action}
                </h4>
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
          (event) =>
            props.resource.hooks[event] &&
            !names().includes(props.resource.hooks[event]!),
        )}
      >
        <div class="border-t border-line px-4 py-3 text-xs leading-relaxed text-warn">
          A hook names a function no library in <Mono>functions/</Mono> exports.
          Requests on that operation fail closed with a 500 until the library is
          built and dropped in.
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
    resource().fields.filter(
      (field) =>
        !field.hidden && (field.type === "string" || field.type === "text"),
    ),
  );
  // A field that was chosen and then deleted (or hidden) would fail the app at
  // load, so the list shown — and the list written — is the surviving one.
  const chosen = createMemo(() =>
    (resource().search_fields ?? []).filter((name) =>
      searchable().some((field) => field.name === name),
    ),
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
          <Labelled
            label="table"
            hint={`Physical table; defaults to apiplant_${resource().name}.`}
          >
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
          <Labelled
            label="scope"
            hint="Organisation-scoped resources are isolated per tenant automatically."
          >
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
          <Labelled
            label="owner field"
            hint="Column compared to the caller for `owner` permissions."
          >
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
                Nothing to search yet. Searching means matching part of a value,
                so it needs a visible <Mono>string</Mono> or <Mono>text</Mono>{" "}
                field.
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
                    None chosen — a term is matched against whichever single
                    field names a record
                  </>
                }
              >
                A term matches a row when any of {chosen().join(", ")} contains
                it. Callers that know the resource can still narrow one request
                with <Mono>?search_fields=</Mono>.
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
            <Labelled
              label="identity field"
              hint="What POST /auth/login expects — email, username, …"
            >
              <TextInput
                mono
                value={resource().auth?.identity_field ?? "email"}
                onInput={(value) =>
                  props.onEdit((draft) => {
                    draft.auth = {
                      identity_field: value,
                      password_field:
                        draft.auth?.password_field ?? "password_hash",
                      oauth_providers: draft.auth?.oauth_providers ?? [],
                    };
                  })
                }
              />
            </Labelled>
            <Labelled
              label="password field"
              hint="Where the argon2id hash is stored. Mark it hidden."
            >
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
                      password_field:
                        draft.auth?.password_field ?? "password_hash",
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
          title={
            props.entry.builtin
              ? "Revert to the built-in"
              : "Delete this resource"
          }
          hint={
            props.entry.builtin
              ? "Removes the file; the framework goes back to shipping its own definition."
              : "Removes the resource file. The table and its rows stay in Postgres — apiplant never drops anything."
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
            <Button
              size="sm"
              variant="ghost"
              onClick={() => props.setConfirming(false)}
            >
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
