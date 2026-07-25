/**
 * Turning fields into things people can read and edit.
 *
 * Two jobs live here. `formatValue` renders a stored value as text — a date as
 * a date, a foreign key as the name of the thing it points at. `FieldEditor`
 * renders the input for one field, chosen from the widget the generator
 * resolved, so a `status` column with declared options becomes a dropdown and a
 * `customer_id` becomes a searchable picker rather than a box for a UUID.
 */

import { For, Show, createEffect, createMemo, createSignal, onCleanup } from "solid-js";
import { Field, Spinner, Toggle } from "./ui";
import { api, asRecord, asRecords, resourceByName, session } from "./store";
import type { ApiRecord, FieldManifest, JsonValue, ResourceManifest } from "./types";

/** A form's working copy: every value held as the string (or boolean) the
 *  input produces, converted back on submit. */
export type Draft = Record<string, string | boolean>;

// --- reading ---------------------------------------------------------------

/** The text that names a record: its display field, else a shortened id. */
export function recordLabel(resource: ResourceManifest | null, record: ApiRecord | null): string {
  if (!record) return "—";
  const field = resource?.display_field;
  if (field) {
    const value = record[field];
    if (value !== null && value !== undefined && value !== "") return String(value);
  }
  for (const candidate of ["name", "title", "label", "email", "slug", "number"]) {
    const value = record[candidate];
    if (typeof value === "string" && value) return value;
  }
  const id = record.id;
  return typeof id === "string" ? `${id.slice(0, 8)}…` : "Untitled";
}

function formatTimestamp(value: string, withTime: boolean): string {
  const date = new Date(value);
  if (Number.isNaN(date.getTime())) return value;
  return date.toLocaleString(undefined, {
    dateStyle: "medium",
    ...(withTime ? { timeStyle: "short" } : {}),
  });
}

/**
 * Render one cell or read-only value.
 *
 * A reference resolves through the expanded relation the list request asked
 * for; when the API did not expand it (or the caller cannot read the target),
 * the raw id is the honest fallback rather than a blank.
 */
export function formatValue(field: FieldManifest, record: ApiRecord): string {
  const raw = record[field.name];
  if (raw === null || raw === undefined || raw === "") return "—";

  if (field.type === "reference" && field.relation) {
    const related = asRecord(record[field.relation]);
    if (related) return recordLabel(resourceByName(field.references), related);
    return typeof raw === "string" ? `${raw.slice(0, 8)}…` : String(raw);
  }
  if (field.options.length) {
    const option = field.options.find((entry) => entry.value === String(raw));
    if (option) return option.label;
  }
  if (field.type === "boolean") return raw ? "Yes" : "No";
  if (field.type === "timestamp" && typeof raw === "string") {
    return formatTimestamp(raw, field.widget !== "date");
  }
  if (field.type === "json") {
    const text = JSON.stringify(raw);
    return text.length > 60 ? `${text.slice(0, 57)}…` : text;
  }
  if (typeof raw === "number") return raw.toLocaleString();
  const text = String(raw);
  return text.length > 90 ? `${text.slice(0, 87)}…` : text;
}

// --- drafts ----------------------------------------------------------------

/** `datetime-local` wants `YYYY-MM-DDTHH:mm`; the API speaks RFC 3339. */
function toLocalInput(value: string, withTime: boolean): string {
  const date = new Date(value);
  if (Number.isNaN(date.getTime())) return value;
  const pad = (n: number) => String(n).padStart(2, "0");
  const day = `${date.getFullYear()}-${pad(date.getMonth() + 1)}-${pad(date.getDate())}`;
  return withTime ? `${day}T${pad(date.getHours())}:${pad(date.getMinutes())}` : day;
}

/**
 * Order fields the way a form should read, not the way the schema stores them.
 *
 * Resource fields arrive alphabetically (they come from an ordered map), which
 * puts an `attributes` JSON blob above `name`. So: whatever names the record
 * first, then the columns the author chose to show in the list — that ordering
 * is already a statement about what matters — then everything else, with the
 * large free-text and JSON inputs pushed to the end where they do not separate
 * the short fields from each other.
 */
function formOrder(resource: ResourceManifest, fields: FieldManifest[]): FieldManifest[] {
  const priority = new Map<string, number>();
  if (resource.display_field) priority.set(resource.display_field, 0);
  resource.columns.forEach((name, index) => {
    if (!priority.has(name)) priority.set(name, index + 1);
  });

  const rank = (field: FieldManifest) => {
    const bulky = field.widget === "textarea" || field.widget === "json";
    if (bulky) return 3000;
    return priority.get(field.name) ?? 1000;
  };

  return [...fields].sort((left, right) => rank(left) - rank(right) || 0);
}

export function editableFields(resource: ResourceManifest): FieldManifest[] {
  return formOrder(
    resource,
    resource.fields.filter((field) => field.writable && field.admin_visible),
  );
}

export function readableFields(resource: ResourceManifest): FieldManifest[] {
  return formOrder(
    resource,
    resource.fields.filter((field) => field.admin_visible),
  );
}

export function createDraft(resource: ResourceManifest, record: ApiRecord | null): Draft {
  const draft: Draft = {};
  for (const field of editableFields(resource)) {
    draft[field.name] = draftValue(field, record ? record[field.name] : undefined);
  }
  return draft;
}

function draftValue(field: FieldManifest, value: unknown): string | boolean {
  const resolved = value ?? field.default_value ?? (field.type === "boolean" ? false : null);
  if (field.type === "boolean") return Boolean(resolved);
  if (resolved === null || resolved === undefined) return "";
  if (field.type === "json") return JSON.stringify(resolved, null, 2);
  if (field.type === "timestamp" && typeof resolved === "string") {
    return toLocalInput(resolved, field.widget !== "date");
  }
  return String(resolved);
}

export interface DraftError {
  field: string;
  message: string;
}

/**
 * Convert a draft into a request body, collecting every problem rather than
 * stopping at the first — one round of corrections beats five.
 */
export function buildPayload(
  resource: ResourceManifest,
  draft: Draft,
): { payload: ApiRecord; errors: DraftError[] } {
  const payload: ApiRecord = {};
  const errors: DraftError[] = [];

  for (const field of editableFields(resource)) {
    const raw = draft[field.name];

    if (field.type === "boolean") {
      payload[field.name] = Boolean(raw);
      continue;
    }

    const text = typeof raw === "string" ? raw.trim() : "";
    if (text === "") {
      if (field.required) {
        errors.push({ field: field.name, message: `${field.label} is required.` });
        continue;
      }
      payload[field.name] = null;
      continue;
    }

    switch (field.type) {
      case "integer":
      case "big_int":
      case "float": {
        const parsed = Number(text);
        if (Number.isNaN(parsed)) {
          errors.push({ field: field.name, message: `${field.label} must be a number.` });
          break;
        }
        if (field.type !== "float" && !Number.isInteger(parsed)) {
          errors.push({ field: field.name, message: `${field.label} must be a whole number.` });
          break;
        }
        payload[field.name] = parsed;
        break;
      }
      case "json":
        try {
          payload[field.name] = JSON.parse(text) as JsonValue;
        } catch {
          errors.push({ field: field.name, message: `${field.label} is not valid JSON.` });
        }
        break;
      case "timestamp": {
        const date = new Date(text);
        if (Number.isNaN(date.getTime())) {
          errors.push({ field: field.name, message: `${field.label} is not a valid date.` });
          break;
        }
        payload[field.name] = date.toISOString();
        break;
      }
      default:
        if (field.max_length && text.length > field.max_length) {
          errors.push({
            field: field.name,
            message: `${field.label} must be ${field.max_length} characters or fewer.`,
          });
          break;
        }
        payload[field.name] = text;
        break;
    }
  }

  return { payload, errors };
}

// --- editing ---------------------------------------------------------------

export function FieldEditor(props: {
  field: FieldManifest;
  draft: Draft;
  error?: string | null;
  disabled?: boolean;
}) {
  const value = () => props.draft[props.field.name];
  const text = () => (typeof value() === "string" ? (value() as string) : "");
  const set = (next: string | boolean) => {
    props.draft[props.field.name] = next;
  };

  if (props.field.widget === "switch") {
    return (
      <Toggle
        checked={Boolean(value())}
        onChange={set}
        label={props.field.label}
        help={props.field.help}
        disabled={props.disabled}
      />
    );
  }

  if (props.field.widget === "reference" && props.field.references) {
    return (
      <Field label={props.field.label} help={props.field.help} required={props.field.required} error={props.error}>
        <ReferencePicker
          target={props.field.references}
          value={text()}
          onChange={set}
          disabled={props.disabled}
          placeholder={props.field.placeholder ?? undefined}
        />
      </Field>
    );
  }

  if (props.field.widget === "select") {
    return (
      <Field label={props.field.label} help={props.field.help} required={props.field.required} error={props.error}>
        <select
          class="input"
          disabled={props.disabled}
          value={text()}
          onChange={(event) => set(event.currentTarget.value)}
        >
          <Show when={!props.field.required}>
            <option value="">Not set</option>
          </Show>
          <For each={props.field.options}>{(option) => <option value={option.value}>{option.label}</option>}</For>
        </select>
      </Field>
    );
  }

  if (props.field.widget === "textarea" || props.field.widget === "json") {
    const isJson = props.field.widget === "json";
    return (
      <Field
        label={props.field.label}
        help={props.field.help ?? (isJson ? "Structured data, written as JSON." : null)}
        required={props.field.required}
        error={props.error}
      >
        <textarea
          class={`input ${isJson ? "min-h-32 font-mono text-[0.78125rem]" : "min-h-28"}`}
          disabled={props.disabled}
          placeholder={props.field.placeholder ?? undefined}
          value={text()}
          onInput={(event) => set(event.currentTarget.value)}
        />
      </Field>
    );
  }

  const inputType = () => {
    switch (props.field.widget) {
      case "number":
        return "number";
      case "email":
        return "email";
      case "url":
        return "url";
      case "password":
        return "password";
      case "color":
        return "color";
      case "date":
        return "date";
      case "date_time":
        return "datetime-local";
      default:
        return "text";
    }
  };

  return (
    <Field label={props.field.label} help={props.field.help} required={props.field.required} error={props.error}>
      <input
        class="input"
        type={inputType()}
        step={props.field.type === "float" ? "any" : undefined}
        maxLength={props.field.max_length ?? undefined}
        disabled={props.disabled}
        placeholder={props.field.placeholder ?? undefined}
        value={text()}
        onInput={(event) => set(event.currentTarget.value)}
      />
    </Field>
  );
}

/**
 * Pick a related record by name.
 *
 * Searches the target resource's display field as you type and shows what you
 * chose, so a foreign key never appears as a UUID. When the current value
 * cannot be resolved — the row was deleted, or you cannot read that resource —
 * the raw id is shown rather than pretending the field is empty.
 */
export function ReferencePicker(props: {
  target: string;
  value: string;
  onChange: (id: string) => void;
  disabled?: boolean;
  placeholder?: string;
}) {
  const resource = createMemo(() => resourceByName(props.target));
  const [open, setOpen] = createSignal(false);
  const [query, setQuery] = createSignal("");
  const [results, setResults] = createSignal<ApiRecord[]>([]);
  const [selected, setSelected] = createSignal<ApiRecord | null>(null);
  const [loading, setLoading] = createSignal(false);
  const [failed, setFailed] = createSignal(false);

  let container: HTMLDivElement | undefined;
  let searchTimer: number | undefined;
  let resolvedFor = "";

  onCleanup(() => window.clearTimeout(searchTimer));

  /** Resolve the current id to a record so the chosen thing has a name. */
  const resolveSelection = async () => {
    const current = props.value;
    if (!current) {
      setSelected(null);
      resolvedFor = "";
      return;
    }
    if (resolvedFor === current) return;
    resolvedFor = current;
    const target = resource();
    if (!target) return;
    try {
      const record = asRecord(
        await api(`/${target.name}/${encodeURIComponent(current)}`, {
          org: target.scope === "organization",
        }),
      );
      setSelected(record);
      setFailed(!record);
    } catch {
      setSelected(null);
      setFailed(true);
    }
  };

  // Resolve whenever the bound value changes, including the first render.
  createEffect(() => {
    void props.value;
    void resolveSelection();
  });

  const search = (term: string) => {
    window.clearTimeout(searchTimer);
    searchTimer = window.setTimeout(async () => {
      const target = resource();
      if (!target) return;
      setLoading(true);
      try {
        const params = new URLSearchParams({ limit: "20" });
        if (term.trim() && target.search_field) params.set(target.search_field, term.trim());
        setResults(
          asRecords(
            await api(`/${target.name}?${params.toString()}`, {
              org: target.scope === "organization",
            }),
          ),
        );
      } catch {
        setResults([]);
      } finally {
        setLoading(false);
      }
    }, 200);
  };

  const openPicker = () => {
    if (props.disabled) return;
    setOpen(true);
    search(query());
    const onDocument = (event: MouseEvent) => {
      if (container && !container.contains(event.target as Node)) {
        setOpen(false);
        document.removeEventListener("mousedown", onDocument);
      }
    };
    document.addEventListener("mousedown", onDocument);
  };

  const choose = (record: ApiRecord | null) => {
    props.onChange(record ? String(record.id ?? "") : "");
    setSelected(record);
    resolvedFor = record ? String(record.id ?? "") : "";
    setFailed(false);
    setOpen(false);
    setQuery("");
  };

  const buttonLabel = () => {
    if (selected()) return recordLabel(resource(), selected());
    if (props.value && failed()) return `${props.value.slice(0, 8)}… (not readable)`;
    if (props.value) return "Loading…";
    return props.placeholder ?? `Choose a ${resource()?.label.toLowerCase() ?? props.target}`;
  };

  return (
    <div class="relative" ref={container}>
      <button
        type="button"
        disabled={props.disabled}
        onClick={openPicker}
        class={`input flex items-center justify-between gap-2 text-left ${
          selected() || props.value ? "text-ink" : "text-faint"
        }`}
      >
        <span class="truncate">{buttonLabel()}</span>
        <svg
          class="h-3 w-3 shrink-0 text-faint"
          viewBox="0 0 12 12"
          fill="none"
          stroke="currentColor"
          stroke-width="1.4"
        >
          <path d="M2.5 4.5 6 8l3.5-3.5" stroke-linecap="round" stroke-linejoin="round" />
        </svg>
      </button>

      <Show when={open()}>
        <div class="animate-rise absolute z-40 mt-1 w-full overflow-hidden rounded-xl border border-line-strong bg-surface shadow-xl">
          <div class="border-b border-line p-2">
            <input
              class="input"
              autofocus
              placeholder={`Search ${resource()?.plural.toLowerCase() ?? props.target}…`}
              value={query()}
              onInput={(event) => {
                setQuery(event.currentTarget.value);
                search(event.currentTarget.value);
              }}
            />
          </div>
          <div class="max-h-64 overflow-y-auto py-1">
            <Show when={!loading()} fallback={
              <div class="flex items-center gap-2 px-3 py-3 text-xs text-faint">
                <Spinner /> Searching…
              </div>
            }>
              <Show
                when={results().length}
                fallback={<p class="px-3 py-3 text-xs text-faint">Nothing matched.</p>}
              >
                <Show when={props.value}>
                  <button
                    type="button"
                    class="w-full px-3 py-2 text-left text-[0.8125rem] text-faint transition-colors hover:bg-surface-2"
                    onClick={() => choose(null)}
                  >
                    Clear selection
                  </button>
                </Show>
                <For each={results()}>
                  {(record) => (
                    <button
                      type="button"
                      class={`w-full px-3 py-2 text-left text-[0.8125rem] transition-colors hover:bg-surface-2 ${
                        String(record.id ?? "") === props.value ? "bg-accent-soft text-ink" : "text-muted"
                      }`}
                      onClick={() => choose(record)}
                    >
                      {recordLabel(resource(), record)}
                    </button>
                  )}
                </For>
              </Show>
            </Show>
          </div>
        </div>
      </Show>
    </div>
  );
}

/** Whether the signed-in operator is the owner of a row, where that is knowable. */
export function ownsRecord(resource: ResourceManifest, record: ApiRecord | null): boolean {
  if (!record || !session.userId) return false;
  const owner = record[resource.owner_field];
  if (typeof owner === "string") return owner === session.userId;
  // A resource with no owner column owns itself — this is the `user` case.
  return String(record.id ?? "") === session.userId;
}
