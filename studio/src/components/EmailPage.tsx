/**
 * One email template: the file on the left, what it will send on the right.
 * The preview renders with LiquidJS against sample values the user can change.
 */

import { For, Show, createMemo, createSignal } from "solid-js";
import {
  addEmailTextHalf,
  deleteEmailTemplate,
  deleteEmailTextHalf,
  fileState,
  fileText,
  setFileText,
} from "../lib/store";
import {
  builtinEmail,
  formVariables,
  frontMatterError,
  renderEmail,
  sampleValues,
  splitFrontMatter,
  subjectSource,
  iteratedVariables,
  usedVariablesIn,
  withSubject,
  type EmailEntry,
  type FormVariable,
} from "../lib/emails";
import {
  Badge,
  Button,
  Card,
  CardHeader,
  CodeEditor,
  Mono,
  Tabs,
  TextInput,
  type EditorHandle,
} from "./ui";

type Pane = "html" | "text";
type Preview = "rendered" | "source" | "plain";

export function EmailPage(props: { entry: EmailEntry }) {
  const [pane, setPane] = createSignal<Pane>("html");
  const [preview, setPreview] = createSignal<Preview>("rendered");
  const [confirming, setConfirming] = createSignal(false);
  const [overrides, setOverrides] = createSignal<Record<string, string>>({});
  let editor: EditorHandle | undefined;

  const source = () => fileText(props.entry.path) ?? "";
  const textSource = () => (props.entry.textPath ? fileText(props.entry.textPath) : null);

  const builtin = createMemo(() => builtinEmail(props.entry.name));

  /**
   * What the template reads, right now. Half-typed markup does not parse, so a
   * scan that fails keeps the last one that worked rather than emptying the
   * form — and taking the cursor with it — between two keystrokes.
   */
  const used = createMemo<string[] | undefined>(
    (previous) => usedVariablesIn(source(), textSource()) ?? previous ?? [],
  );

  const iterated = createMemo(() => iteratedVariables(`${source()}\n${textSource() ?? ""}`));
  const variables = createMemo(() => formVariables(props.entry.name, used() ?? [], iterated()));

  /**
   * Samples for what the framework passes, empty strings for everything else
   * the template reads, and whatever the user typed over the top of both.
   */
  const values = createMemo(() => {
    const base: Record<string, string> = sampleValues(props.entry.name);
    for (const variable of variables()) base[variable.name] ??= variable.sample;
    return { ...base, ...overrides() };
  });


  const rendered = createMemo(() =>
    renderEmail(props.entry.name, source(), values(), textSource()),
  );
  const problem = createMemo(() => frontMatterError(source()) ?? rendered().error);

  /** The subject as written — a template itself, so it is edited as text. */
  const subject = () => subjectSource(source()) ?? "";

  const setSubject = (next: string) => setFileText(props.entry.path, withSubject(source(), next));

  /**
   * The pane the editor is showing. The plain-text half is a separate file, so
   * switching panes switches which file the same editor is bound to.
   */
  const activePath = () => (pane() === "text" && props.entry.textPath ? props.entry.textPath : props.entry.path);

  const dirty = () => {
    const state = fileState(activePath());
    return !!state && state.current !== state.original;
  };

  return (
    <div class="animate-rise mx-auto w-full max-w-6xl px-6 py-6">
      <header class="mb-5">
        <div class="flex flex-wrap items-center gap-2.5">
          <h1 class="font-mono text-xl font-semibold tracking-tight">{props.entry.name}</h1>
          <Show when={props.entry.builtin} fallback={<Badge>custom</Badge>}>
            <Badge tone="accent">overrides a built-in</Badge>
          </Show>
          <Show when={props.entry.textPath}>
            <Badge tone="info">written text half</Badge>
          </Show>
          <Show when={problem()}>
            <Badge tone="danger">will not render</Badge>
          </Show>
        </div>

        <p class="mt-2 max-w-3xl text-xs leading-relaxed text-muted">
          <Show
            when={builtin()}
            fallback={
              <>
                A custom template. Sent only when a function requests it by name through <Mono>send_email</Mono>,
                passing the variables it uses.
              </>
            }
          >
            {(entry) => <>{entry().description} This file replaces the message the framework would send.</>}
          </Show>{" "}
          Lives at <Mono>{props.entry.path}</Mono> and is compiled at boot: a template that does not parse stops
          the app.
        </p>
      </header>

      <Show when={problem()}>
        {(message) => (
          <div class="mb-4 rounded-lg border border-danger-line bg-danger-soft px-3 py-2">
            <p class="font-mono text-[0.6875rem] leading-relaxed text-danger">{message()}</p>
          </div>
        )}
      </Show>

      <div class="grid min-w-0 gap-4 xl:grid-cols-[minmax(0,1fr)_22rem]">
        <div class="min-w-0 space-y-4">
          <Card>
            <CardHeader
              title="Subject"
              hint="TOML front matter; also a template, so it can name the app."
            />
            <div class="px-3 pb-3 pt-1">
              <TextInput
                mono
                value={subject()}
                placeholder={builtin()?.subject ?? "A message from {{ app_name }}"}
                onInput={setSubject}
              />
              <p class="mt-1.5 text-[0.6875rem] leading-relaxed text-faint">
                <Show
                  when={subject().trim()}
                  fallback={
                    builtin()
                      ? `Empty — the message keeps the built-in subject, "${builtin()!.subject}".`
                      : "Empty — the message is sent with the app's name as its subject."
                  }
                >
                  Renders as <span class="text-muted">{rendered().subject}</span>
                </Show>
              </p>
            </div>
          </Card>

          <div class="flex flex-wrap items-center justify-between gap-2">
            <Tabs
              tabs={[
                { id: "html" as Pane, label: "HTML" },
                { id: "text" as Pane, label: props.entry.textPath ? "Plain text" : "Plain text (derived)" },
              ]}
              active={pane()}
              onChange={setPane}
            />
            <div class="flex items-center gap-2">
              <Show when={dirty()}>
                <span class="text-[0.6875rem] text-accent">unsaved</span>
              </Show>
              <Mono class="hidden sm:inline">{activePath()}</Mono>
            </div>
          </div>

          <Show
            when={pane() === "html" || props.entry.textPath}
            fallback={
              <Card>
                <div class="space-y-2 px-3 py-4">
                  <p class="text-xs leading-relaxed text-muted">
                    No <Mono>{props.entry.name}.text.liquid</Mono>, so the text half is derived from the
                    rendered HTML — tags dropped, links kept as their URL. An HTML-only message is scored as
                    spam by most filters.
                  </p>
                  <Button size="sm" variant="ghost" onClick={() => addEmailTextHalf(props.entry.name)}>
                    Write one instead
                  </Button>
                </div>
              </Card>
            }
          >
            <CodeEditor
              language="liquid"
              value={fileText(activePath()) ?? ""}
              onInput={(text) => setFileText(activePath(), text)}
              onReady={(handle) => (editor = handle)}
              minHeight="32rem"
            />
          </Show>

          <Show when={pane() === "text" && props.entry.textPath}>
            <div class="flex justify-end">
              <Button size="sm" variant="ghost" onClick={() => deleteEmailTextHalf(props.entry.name)}>
                Delete the text half and derive it again
              </Button>
            </div>
          </Show>
        </div>

        <div class="min-w-0 space-y-4">
          <Card>
            <CardHeader
              title="Values"
              hint="Values used to render this message: those passed to it plus those the template reads. Edited here only to preview."
            />
            <div class="space-y-3 px-3 py-3">
              <For each={variables()}>
                {(variable: FormVariable) => (
                  <div>
                    <div class="flex items-center justify-between gap-2">
                      <span class="flex min-w-0 items-center gap-1.5">
                        <Mono class="truncate">{variable.name}</Mono>
                        <Show when={!variable.declared}>
                          <span
                            class="shrink-0 text-[0.625rem] text-warn"
                            title="Not passed to this message — it renders as nothing."
                          >
                            not passed
                          </span>
                        </Show>
                        <Show when={variable.iterated}>
                          <span class="shrink-0 text-[0.625rem] text-faint">list</span>
                        </Show>
                        <Show when={variable.declared && !variable.used}>
                          <span class="shrink-0 text-[0.625rem] text-faint">unused</span>
                        </Show>
                      </span>
                      <button
                        type="button"
                        class="rounded px-1.5 py-0.5 text-[0.6875rem] text-faint transition-colors hover:bg-surface-2 hover:text-accent"
                        title={
                          variable.iterated
                            ? `Insert a loop over ${variable.name} at the cursor`
                            : `Insert {{ ${variable.name} }} at the cursor`
                        }
                        onClick={() =>
                          editor?.insert(
                            variable.iterated
                              ? `{% for item in ${variable.name} %}{{ item }}{% endfor %}`
                              : `{{ ${variable.name} }}`,
                          )
                        }
                      >
                        insert
                      </button>
                    </div>
                    <TextInput
                      mono
                      class="mt-1"
                      disabled={variable.iterated}
                      value={values()[variable.name] ?? ""}
                      placeholder={variable.sample || "(empty)"}
                      onInput={(next) =>
                        setOverrides((current) => ({ ...current, [variable.name]: next }))
                      }
                    />
                    <p class="mt-1 text-[0.6875rem] leading-relaxed text-faint">{variable.description}</p>
                  </div>
                )}
              </For>
              <Show when={Object.keys(overrides()).length}>
                <Button size="sm" variant="ghost" onClick={() => setOverrides({})}>
                  Reset to samples
                </Button>
              </Show>
            </div>
          </Card>

          <Card class="border-danger-line">
            <div class="flex items-center justify-between gap-2 px-3 py-3">
              <span class="text-xs text-muted">
                {props.entry.builtin ? "Delete — the built-in message comes back" : "Delete template"}
              </span>
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
                  <Button size="sm" variant="danger" onClick={() => deleteEmailTemplate(props.entry.name)}>
                    Confirm
                  </Button>
                </div>
              </Show>
            </div>
          </Card>
        </div>
      </div>

      <div class="mt-6 min-w-0">
        <div class="mb-2 flex flex-wrap items-center justify-between gap-2">
          <Tabs
            tabs={[
              { id: "rendered" as Preview, label: "Preview" },
              { id: "plain" as Preview, label: "Plain text" },
              { id: "source" as Preview, label: "Rendered HTML" },
            ]}
            active={preview()}
            onChange={setPreview}
          />
          <span class="text-[0.6875rem] text-faint">
            Subject: <span class="text-muted">{rendered().subject}</span>
          </span>
        </div>

        <Show when={preview() === "rendered"}>
          {/*
            An iframe, and a sandboxed one: the template is HTML from a file on
            disk, and the preview should not be able to script the studio it is
            being previewed in. `allow-same-origin` is deliberately absent.
          */}
          <iframe
            title={`${props.entry.name} preview`}
            class="h-[36rem] w-full rounded-lg border border-line bg-white"
            sandbox=""
            srcdoc={rendered().html}
          />
        </Show>

        <Show when={preview() === "plain"}>
          <pre class="h-[36rem] overflow-auto rounded-lg border border-line bg-surface p-4 font-mono text-[0.75rem] leading-relaxed text-muted">
            {rendered().text}
          </pre>
        </Show>

        <Show when={preview() === "source"}>
          <CodeEditor language="html" value={rendered().html} readOnly minHeight="36rem" />
        </Show>

        <p class="mt-2 text-[0.6875rem] leading-relaxed text-faint">
          Rendered in the browser with LiquidJS against the values above.
          <Show when={splitFrontMatter(source()).frontMatter === null}>
            {" "}
            This file has no front matter, so it keeps the subject of the message it replaces.
          </Show>
        </p>
      </div>
    </div>
  );
}
