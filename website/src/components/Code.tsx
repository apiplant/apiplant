import { Show, createResource, createSignal } from "solid-js";
import { highlight } from "../lib/docs";

/**
 * A highlighted snippet outside the documentation. Highlighting arrives with
 * the shiki chunk, so the plain text is rendered first and swapped in place.
 * The metrics are identical either way, so nothing shifts when it loads.
 */
export function Code(props: { code: string; lang: string; class?: string; filename?: string }) {
  const [html] = createResource(
    () => [props.code, props.lang] as const,
    ([code, lang]) => highlight(code, lang),
  );

  const frame =
    "overflow-x-auto bg-code-bg px-4 py-3.5 font-mono text-[0.8125rem] leading-relaxed [tab-size:4] " +
    "[&_pre]:m-0 [&_pre]:bg-transparent! [&_.shiki]:bg-transparent!";

  return (
    <div class={`min-w-0 overflow-hidden rounded-xl border border-line bg-code-bg ${props.class ?? ""}`}>
      <Show when={props.filename}>
        <div class="flex items-center gap-2 border-b border-line bg-surface px-4 py-2">
          <span class="flex gap-1.5" aria-hidden="true">
            <span class="h-2.5 w-2.5 rounded-full bg-line-strong" />
            <span class="h-2.5 w-2.5 rounded-full bg-line-strong" />
            <span class="h-2.5 w-2.5 rounded-full bg-line-strong" />
          </span>
          <span class="font-mono text-xs text-faint">{props.filename}</span>
        </div>
      </Show>

      <Show
        when={html()}
        fallback={
          <pre class={`${frame} text-muted`}>
            <code>{props.code}</code>
          </pre>
        }
      >
        <div class={frame} innerHTML={html()} />
      </Show>
    </div>
  );
}

/** A one-line install command with a copy button. */
export function CopyLine(props: { command: string }) {
  const [copied, setCopied] = createSignal(false);

  const copy = async () => {
    try {
      await navigator.clipboard.writeText(props.command);
      setCopied(true);
      setTimeout(() => setCopied(false), 1600);
    } catch {
      /* clipboard denied: the text remains selectable */
    }
  };

  return (
    <div class="flex w-full min-w-0 items-center gap-3 rounded-xl border border-line bg-surface px-4 py-2.5 sm:inline-flex sm:w-auto">
      <code class="min-w-0 flex-1 overflow-x-auto whitespace-nowrap font-mono text-[0.8125rem] text-muted sm:flex-none">
        <span class="select-none text-faint">$ </span>
        {props.command}
      </code>
      <button
        type="button"
        onClick={copy}
        aria-label="Copy command"
        class="inline-flex h-6 w-6 shrink-0 items-center justify-center rounded-md text-faint transition-colors hover:bg-surface-2 hover:text-ink"
      >
        <Show
          when={copied()}
          fallback={
            <svg viewBox="0 0 24 24" class="h-3.5 w-3.5" fill="none" stroke="currentColor" stroke-width="1.8">
              <rect x="9" y="9" width="11" height="11" rx="2" />
              <path d="M5 15V5a2 2 0 0 1 2-2h8" stroke-linecap="round" />
            </svg>
          }
        >
          <svg viewBox="0 0 24 24" class="h-3.5 w-3.5 text-success" fill="none" stroke="currentColor" stroke-width="2">
            <path stroke-linecap="round" stroke-linejoin="round" d="m5 13 4 4L19 7" />
          </svg>
        </Show>
      </button>
    </div>
  );
}
