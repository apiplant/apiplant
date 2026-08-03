/**
 * Running a function.
 *
 * This screen is deliberately not a JSON textarea. A function's `input_schema`,
 * which the `function!` macro derives from the handler's own input type, is
 * turned into ordinary labelled inputs, so running an action means filling in a
 * short form. A function without a usable schema falls back to raw JSON.
 */

import { For, Show, createEffect, createMemo, createSignal, untrack } from "solid-js";
import { createMutable } from "solid-js/store";
import { Badge, Button, Card, CardHeader, ConfirmDialog, EmptyState, Field, PageTitle, Toggle } from "../ui";
import { apiStream, notify, reportError, session } from "../store";
import type { FunctionManifest, JsonSchema, JsonValue } from "../types";

/** One input the form should render, flattened out of the schema. */
interface SchemaField {
  name: string;
  label: string;
  description?: string;
  kind: "string" | "number" | "integer" | "boolean" | "enum" | "json";
  required: boolean;
  options: string[];
  format?: string;
  default?: JsonValue;
}

/** Follow a local `$ref` into the schema's own `$defs`/`definitions`. */
function deref(schema: JsonSchema, root: JsonSchema): JsonSchema {
  if (!schema.$ref) return schema;
  const name = schema.$ref.split("/").pop();
  if (!name) return schema;
  return root.$defs?.[name] ?? root.definitions?.[name] ?? schema;
}

/** The first concrete branch of an `anyOf`/`oneOf`, ignoring the `null` arm —
 *  which is how an `Option<T>` arrives from schemars. */
function unwrapNullable(schema: JsonSchema, root: JsonSchema): { schema: JsonSchema; optional: boolean } {
  const branches = schema.anyOf ?? schema.oneOf;
  if (!branches?.length) return { schema, optional: false };
  const concrete = branches
    .map((branch) => deref(branch, root))
    .filter((branch) => branch.type !== "null");
  return {
    schema: concrete[0] ?? schema,
    optional: concrete.length < branches.length,
  };
}

function titleCase(name: string): string {
  const spaced = name.replace(/[_-]+/g, " ").replace(/([a-z])([A-Z])/g, "$1 $2");
  return spaced.charAt(0).toUpperCase() + spaced.slice(1);
}

/**
 * Flatten a schema's top-level properties into form fields. Only one level
 * deep: nested values are offered as JSON rather than forced into a form that
 * does not fit them.
 */
export function schemaFields(root: JsonSchema | null): SchemaField[] | null {
  if (!root) return null;
  const resolved = deref(root, root);
  const properties = resolved.properties;
  if (!properties || !Object.keys(properties).length) return null;

  const required = new Set(resolved.required ?? []);
  const fields: SchemaField[] = [];

  for (const [name, raw] of Object.entries(properties)) {
    const { schema: unwrapped, optional } = unwrapNullable(deref(raw, root), root);
    const schema = deref(unwrapped, root);
    const types = Array.isArray(schema.type) ? schema.type : schema.type ? [schema.type] : [];
    const type = types.find((entry) => entry !== "null");
    const enumValues = (schema.enum ?? []).filter(
      (value): value is string => typeof value === "string",
    );

    let kind: SchemaField["kind"] = "json";
    if (enumValues.length) kind = "enum";
    else if (type === "string") kind = "string";
    else if (type === "integer") kind = "integer";
    else if (type === "number") kind = "number";
    else if (type === "boolean") kind = "boolean";

    fields.push({
      name,
      label: schema.title ?? titleCase(name),
      description: schema.description,
      kind,
      required: required.has(name) && !optional,
      options: enumValues,
      format: schema.format,
      default: schema.default,
    });
  }

  return fields;
}

function initialDraft(fields: SchemaField[]): Record<string, string | boolean> {
  const draft: Record<string, string | boolean> = {};
  for (const field of fields) {
    if (field.kind === "boolean") {
      draft[field.name] = field.default === true;
    } else if (field.default !== undefined && field.default !== null) {
      draft[field.name] =
        typeof field.default === "object" ? JSON.stringify(field.default, null, 2) : String(field.default);
    } else {
      draft[field.name] = "";
    }
  }
  return draft;
}

export function ActionPage(props: { fn: FunctionManifest }) {
  const fields = createMemo(() => schemaFields(props.fn.input_schema));
  const usesForm = createMemo(() => props.fn.method !== "GET" && fields() !== null);
  const needsBody = () => props.fn.method !== "GET";

  const draft = createMutable<Record<string, string | boolean>>({});
  const [rawInput, setRawInput] = createSignal("{}");
  const [running, setRunning] = createSignal(false);
  const [confirming, setConfirming] = createSignal(false);
  const [streamed, setStreamed] = createSignal("");
  const [result, setResult] = createSignal<unknown>(undefined);
  const [failure, setFailure] = createSignal<string | null>(null);

  // Seed the form (and reset it) whenever the action changes.
  createEffect(() => {
    void props.fn.name;
    const schema = fields();
    // Untracked: reading the draft's keys in order to clear them would make
    // this effect depend on its own writes.
    untrack(() => {
      for (const key of Object.keys(draft)) delete draft[key];
      if (schema) Object.assign(draft, initialDraft(schema));
    });
    setRawInput("{}");
    setStreamed("");
    setResult(undefined);
    setFailure(null);
  });

  const blockedByOrganization = () => props.fn.requires_org && !session.organizationId;

  const buildBody = (): Record<string, unknown> | undefined => {
    if (!needsBody()) return undefined;

    const schema = fields();
    if (!schema) {
      const text = rawInput().trim();
      if (!text) return {};
      const parsed = JSON.parse(text);
      if (!parsed || typeof parsed !== "object" || Array.isArray(parsed)) {
        throw new Error("The parameters must be a JSON object.");
      }
      return parsed as Record<string, unknown>;
    }

    const body: Record<string, unknown> = {};
    for (const field of schema) {
      const value = draft[field.name];
      if (field.kind === "boolean") {
        body[field.name] = Boolean(value);
        continue;
      }
      const text = typeof value === "string" ? value.trim() : "";
      if (!text) {
        if (field.required) throw new Error(`${field.label} is required.`);
        continue;
      }
      switch (field.kind) {
        case "integer":
        case "number": {
          const parsed = Number(text);
          if (Number.isNaN(parsed)) throw new Error(`${field.label} must be a number.`);
          if (field.kind === "integer" && !Number.isInteger(parsed)) {
            throw new Error(`${field.label} must be a whole number.`);
          }
          body[field.name] = parsed;
          break;
        }
        case "json":
          try {
            body[field.name] = JSON.parse(text);
          } catch {
            throw new Error(`${field.label} is not valid JSON.`);
          }
          break;
        default:
          body[field.name] = text;
      }
    }
    return body;
  };

  const run = async () => {
    setConfirming(false);
    setRunning(true);
    setStreamed("");
    setFailure(null);
    setResult(undefined);
    try {
      const body = buildBody();
      const output = await apiStream(
        `/functions/${encodeURIComponent(props.fn.name)}/stream`,
        {
          method: props.fn.method,
          body,
          org: props.fn.requires_org,
        },
        (chunk) => setStreamed((current) => current + chunk),
      );
      setResult(output ?? null);
      notify("success", `${props.fn.label} finished.`);
    } catch (error) {
      const message = error instanceof Error ? error.message : String(error);
      setFailure(message);
      reportError(error);
    } finally {
      setRunning(false);
    }
  };

  const start = () => {
    if (props.fn.confirm) {
      // Validate before prompting, so a confirmation is never followed by a
      // validation error.
      try {
        buildBody();
      } catch (error) {
        reportError(error);
        return;
      }
      setConfirming(true);
      return;
    }
    void run();
  };

  return (
    <>
      <PageTitle title={props.fn.label} subtitle={props.fn.description || undefined}>
        <Badge tone={props.fn.permission === "public" ? "neutral" : "accent"}>
          {props.fn.permission_note}
        </Badge>
      </PageTitle>

      <Show when={blockedByOrganization()}>
        <Card class="mb-4 border-warn-line bg-warn-soft/40">
          <p class="px-4 py-3 text-[0.8125rem] text-ink">
            Choose an organization from the top bar before running this.
          </p>
        </Card>
      </Show>

      <div class="grid gap-4 xl:grid-cols-2">
        <Card>
          <CardHeader
            title={usesForm() || !needsBody() ? "Details" : "Parameters"}
            hint={needsBody() ? undefined : "This action takes no input."}
          />
          <div data-ai-assist-scope class="space-y-4 px-5 py-5">
            <Show when={needsBody()}>
              <Show
                when={fields()}
                fallback={
                  <Field
                    label="Parameters"
                    help="This action did not describe its input, so it is sent as raw JSON."
                  >
                    <textarea
                      class="input min-h-40 font-mono text-[0.78125rem]"
                      value={rawInput()}
                      onInput={(event) => setRawInput(event.currentTarget.value)}
                    />
                  </Field>
                }
              >
                {(schema) => (
                  <For each={schema()}>
                    {(field) => (
                      <Show
                        when={field.kind !== "boolean"}
                        fallback={
                          <Toggle
                            checked={Boolean(draft[field.name])}
                            onChange={(value) => {
                              draft[field.name] = value;
                            }}
                            label={field.label}
                            help={field.description}
                          />
                        }
                      >
                        <Field label={field.label} help={field.description} required={field.required}>
                          <Show
                            when={field.kind === "enum"}
                            fallback={
                              <Show
                                when={field.kind !== "json"}
                                fallback={
                                  <textarea
                                    class="input min-h-28 font-mono text-[0.78125rem]"
                                    value={String(draft[field.name] ?? "")}
                                    onInput={(event) => {
                                      draft[field.name] = event.currentTarget.value;
                                    }}
                                  />
                                }
                              >
                                <input
                                  class="input"
                                  type={inputTypeFor(field)}
                                  step={field.kind === "number" ? "any" : undefined}
                                  value={String(draft[field.name] ?? "")}
                                  onInput={(event) => {
                                    draft[field.name] = event.currentTarget.value;
                                  }}
                                />
                              </Show>
                            }
                          >
                            <select
                              class="input"
                              value={String(draft[field.name] ?? "")}
                              onChange={(event) => {
                                draft[field.name] = event.currentTarget.value;
                              }}
                            >
                              <Show when={!field.required}>
                                <option value="">Not set</option>
                              </Show>
                              <For each={field.options}>
                                {(option) => <option value={option}>{titleCase(option)}</option>}
                              </For>
                            </select>
                          </Show>
                        </Field>
                      </Show>
                    )}
                  </For>
                )}
              </Show>
            </Show>

            <Show when={!needsBody()}>
              <p class="text-[0.8125rem] leading-relaxed text-muted">
                Nothing to fill in — run it when you are ready.
              </p>
            </Show>

            <div class="flex items-center gap-2 pt-1">
              <Button
                variant="primary"
                loading={running()}
                disabled={blockedByOrganization()}
                onClick={start}
              >
                {props.fn.run_label}
              </Button>
              <Show when={props.fn.confirm}>
                <span class="text-[0.6875rem] text-faint">You will be asked to confirm.</span>
              </Show>
            </div>
          </div>
        </Card>

        <Card>
          <CardHeader title="Output" />
          <div class="px-5 py-5">
            <Show
              when={running() || streamed() || result() !== undefined || failure()}
              fallback={
                <EmptyState
                  title="No result yet"
                  description={`Run ${props.fn.label.toLowerCase()} to see what it returns.`}
                />
              }
            >
              <div class="space-y-4">
                <Show when={running() || streamed()}>
                  <div class="space-y-2">
                    <p class="text-[0.6875rem] font-semibold tracking-[0.16em] text-faint uppercase">
                      Live output
                    </p>
                    <pre class="max-h-64 overflow-auto rounded-xl border border-line bg-surface-2/50 p-3.5 font-mono text-[0.78125rem] leading-6 text-muted">
                      {streamed() || "Waiting for streamed output..."}
                    </pre>
                  </div>
                </Show>

                <Show
                  when={result() !== undefined || failure()}
                  fallback={<p class="text-[0.8125rem] leading-relaxed text-muted">Waiting for the function to finish.</p>}
                >
                  <div class="space-y-2">
                    <Show when={streamed()}>
                      <p class="text-[0.6875rem] font-semibold tracking-[0.16em] text-faint uppercase">
                        Final result
                      </p>
                    </Show>
                    <Show
                      when={!failure()}
                      fallback={
                        <div class="rounded-xl border border-danger-line bg-danger-soft px-4 py-3">
                          <p class="text-[0.8125rem] leading-relaxed text-ink">{failure()}</p>
                        </div>
                      }
                    >
                      <ResultView value={result()} />
                    </Show>
                  </div>
                </Show>
              </div>
            </Show>
          </div>
        </Card>
      </div>

      <ConfirmDialog
        open={confirming()}
        title={props.fn.label}
        description={props.fn.confirm ?? ""}
        confirmLabel={props.fn.run_label}
        busy={running()}
        onConfirm={() => void run()}
        onCancel={() => setConfirming(false)}
      />
    </>
  );
}

function inputTypeFor(field: SchemaField): string {
  if (field.kind === "integer" || field.kind === "number") return "number";
  if (field.format === "email") return "email";
  if (field.format === "uri" || field.format === "url") return "url";
  if (field.format === "date") return "date";
  if (field.format === "date-time") return "datetime-local";
  return "text";
}

/**
 * Render a result in the most readable form: a flat object becomes a small
 * definition list, and anything else stays as JSON. Most functions return a few
 * scalars, which read better as a list than as raw JSON.
 */
function ResultView(props: { value: unknown }) {
  const flat = createMemo(() => {
    const value = props.value;
    if (!value || typeof value !== "object" || Array.isArray(value)) return null;
    const entries = Object.entries(value as Record<string, unknown>);
    if (!entries.length || entries.length > 12) return null;
    const scalar = entries.every(
      ([, entry]) => entry === null || ["string", "number", "boolean"].includes(typeof entry),
    );
    return scalar ? entries : null;
  });

  return (
    <Show
      when={flat()}
      fallback={
        <pre class="max-h-96 overflow-auto rounded-xl border border-line bg-surface-2/50 p-3.5 font-mono text-[0.78125rem] leading-6 text-muted">
          {JSON.stringify(props.value, null, 2)}
        </pre>
      }
    >
      {(entries) => (
        <dl class="divide-y divide-line">
          <For each={entries()}>
            {([key, value]) => (
              <div class="flex items-baseline justify-between gap-4 py-2.5">
                <dt class="text-[0.8125rem] text-muted">{titleCase(key)}</dt>
                <dd class="text-right text-[0.8125rem] font-medium text-ink">
                  {value === null ? "—" : typeof value === "boolean" ? (value ? "Yes" : "No") : String(value)}
                </dd>
              </div>
            )}
          </For>
        </dl>
      )}
    </Show>
  );
}
