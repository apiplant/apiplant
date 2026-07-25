/** Shared presentational pieces. Nothing here knows about apiplant. */

import { For, Show, createSignal, createUniqueId, splitProps, type JSX, type ParentProps } from "solid-js";

// ---- buttons ----------------------------------------------------------------

type ButtonVariant = "primary" | "secondary" | "ghost" | "danger";

const BUTTON_VARIANTS: Record<ButtonVariant, string> = {
  primary:
    "bg-accent text-[#04241a] hover:bg-[#4ade9f] active:bg-accent-dim shadow-[0_1px_0_rgba(255,255,255,0.18)_inset,0_6px_20px_-8px_rgba(52,211,153,0.7)] font-semibold",
  secondary: "bg-surface-2 text-ink border border-line hover:border-line-strong hover:bg-surface-3",
  ghost: "text-muted hover:text-ink hover:bg-surface-2",
  danger: "bg-transparent text-danger border border-[#3c2323] hover:bg-[#2a1616] hover:border-[#5a2f2f]",
};

export function Button(
  props: ParentProps<
    {
      variant?: ButtonVariant;
      size?: "sm" | "md";
      class?: string;
    } & JSX.ButtonHTMLAttributes<HTMLButtonElement>
  >,
) {
  const [local, rest] = splitProps(props, ["variant", "size", "class", "children"]);
  return (
    <button
      {...rest}
      class={[
        "inline-flex items-center justify-center gap-1.5 rounded-lg whitespace-nowrap transition-colors duration-100",
        "disabled:opacity-40 disabled:pointer-events-none",
        local.size === "sm" ? "px-2.5 py-1 text-xs" : "px-3 py-1.5 text-[0.8125rem]",
        BUTTON_VARIANTS[local.variant ?? "secondary"],
        local.class ?? "",
      ].join(" ")}
    >
      {local.children}
    </button>
  );
}

// ---- surfaces ---------------------------------------------------------------

export function Card(props: ParentProps<{ class?: string }>) {
  return <div class={`card ${props.class ?? ""}`}>{props.children}</div>;
}

export function CardHeader(props: ParentProps<{ title: string; hint?: string; class?: string }>) {
  return (
    <div class={`flex items-start justify-between gap-4 border-b border-line px-4 py-3 ${props.class ?? ""}`}>
      <div>
        <h3 class="text-sm font-semibold tracking-tight text-ink">{props.title}</h3>
        <Show when={props.hint}>
          <p class="mt-0.5 text-xs text-muted">{props.hint}</p>
        </Show>
      </div>
      <div class="flex shrink-0 items-center gap-2">{props.children}</div>
    </div>
  );
}

export function Badge(
  props: ParentProps<{ tone?: "neutral" | "accent" | "warn" | "danger" | "info"; class?: string }>,
) {
  const tones = {
    neutral: "bg-surface-3 text-muted border-line",
    accent: "bg-[#0d2b21] text-accent border-[#1c4c3b]",
    warn: "bg-[#2c2413] text-warn border-[#4a3c1a]",
    danger: "bg-[#2c1818] text-danger border-[#4a2626]",
    info: "bg-[#132330] text-[#7dd3fc] border-[#1e3d52]",
  };
  return (
    <span
      class={`inline-flex items-center gap-1 rounded-full border px-2 py-0.5 text-[0.6875rem] font-medium leading-4 ${
        tones[props.tone ?? "neutral"]
      } ${props.class ?? ""}`}
    >
      {props.children}
    </span>
  );
}

// ---- form controls ----------------------------------------------------------

export function Labelled(props: ParentProps<{ label: string; hint?: string; class?: string }>) {
  return (
    <div class={props.class}>
      <label class="field-label">{props.label}</label>
      {props.children}
      <Show when={props.hint}>
        <p class="mt-1 text-[0.6875rem] leading-relaxed text-faint">{props.hint}</p>
      </Show>
    </div>
  );
}

export function TextInput(
  props: {
    value: string;
    onInput: (value: string) => void;
    placeholder?: string;
    mono?: boolean;
    class?: string;
    disabled?: boolean;
    list?: string;
    type?: string;
  },
) {
  return (
    <input
      class={`input ${props.mono ? "font-mono text-[0.78125rem]" : ""} ${props.class ?? ""}`}
      type={props.type ?? "text"}
      value={props.value}
      placeholder={props.placeholder}
      disabled={props.disabled}
      list={props.list}
      spellcheck={false}
      autocomplete="off"
      onInput={(event) => props.onInput(event.currentTarget.value)}
    />
  );
}

/**
 * A text input that commits on blur or Enter rather than on every keystroke.
 *
 * Used where a change is disruptive — renaming a resource moves its file and
 * re-routes the page, which cannot happen per character typed.
 */
export function CommitInput(props: {
  value: string;
  onCommit: (value: string) => void;
  placeholder?: string;
  mono?: boolean;
  disabled?: boolean;
}) {
  const [draft, setDraft] = createSignal<string | null>(null);

  const commit = () => {
    const next = draft();
    setDraft(null);
    if (next !== null && next.trim() && next !== props.value) props.onCommit(next.trim());
  };

  return (
    <input
      class={`input ${props.mono ? "font-mono text-[0.78125rem]" : ""}`}
      value={draft() ?? props.value}
      placeholder={props.placeholder}
      disabled={props.disabled}
      spellcheck={false}
      autocomplete="off"
      onInput={(event) => setDraft(event.currentTarget.value)}
      onBlur={commit}
      onKeyDown={(event) => {
        if (event.key === "Enter") event.currentTarget.blur();
        if (event.key === "Escape") {
          setDraft(null);
          event.currentTarget.blur();
        }
      }}
    />
  );
}

export function Select<T extends string>(props: {
  value: T;
  options: readonly T[] | readonly { value: T; label: string }[];
  onChange: (value: T) => void;
  class?: string;
  disabled?: boolean;
}) {
  const options = () =>
    (props.options as readonly (T | { value: T; label: string })[]).map((option) =>
      typeof option === "string" ? { value: option, label: option } : option,
    );
  return (
    <select
      class={`input ${props.class ?? ""}`}
      disabled={props.disabled}
      value={props.value}
      onChange={(event) => props.onChange(event.currentTarget.value as T)}
    >
      <For each={options()}>{(option) => <option value={option.value}>{option.label}</option>}</For>
    </select>
  );
}

/** A compact on/off pill, used for the many boolean field options. */
export function Toggle(props: { checked: boolean; onChange: (value: boolean) => void; label: string; hint?: string }) {
  return (
    <button
      type="button"
      title={props.hint}
      onClick={() => props.onChange(!props.checked)}
      class={[
        "inline-flex items-center gap-1.5 rounded-md border px-2 py-1 text-xs transition-colors duration-100",
        props.checked
          ? "border-[#1c4c3b] bg-[#0d2b21] text-accent"
          : "border-line bg-surface-2 text-faint hover:border-line-strong hover:text-muted",
      ].join(" ")}
    >
      <span
        class={`h-1.5 w-1.5 rounded-full ${props.checked ? "bg-accent" : "bg-[#2f3f39]"}`}
        aria-hidden="true"
      />
      {props.label}
    </button>
  );
}

export function Switch(props: { checked: boolean; onChange: (value: boolean) => void; label?: string }) {
  return (
    <button
      type="button"
      role="switch"
      aria-checked={props.checked}
      onClick={() => props.onChange(!props.checked)}
      class="inline-flex items-center gap-2 text-[0.8125rem] text-ink"
    >
      <span
        class={`relative h-5 w-9 rounded-full border transition-colors duration-150 ${
          props.checked ? "border-[#1c4c3b] bg-accent-dim/80" : "border-line bg-surface-3"
        }`}
      >
        <span
          class={`absolute top-0.5 h-3.5 w-3.5 rounded-full bg-ink transition-transform duration-150 ${
            props.checked ? "translate-x-[1.125rem]" : "translate-x-0.5"
          }`}
        />
      </span>
      <Show when={props.label}>{props.label}</Show>
    </button>
  );
}

// ---- tabs -------------------------------------------------------------------

export function Tabs<T extends string>(props: {
  tabs: readonly { id: T; label: string; badge?: string | number }[];
  active: T;
  onChange: (id: T) => void;
}) {
  return (
    <div class="flex items-center gap-1 rounded-lg border border-line bg-surface p-1">
      <For each={props.tabs}>
        {(tab) => (
          <button
            type="button"
            onClick={() => props.onChange(tab.id)}
            class={[
              "rounded-md px-2.5 py-1 text-xs font-medium transition-colors duration-100",
              props.active === tab.id
                ? "bg-surface-3 text-ink shadow-[0_1px_0_rgba(255,255,255,0.04)_inset]"
                : "text-muted hover:text-ink",
            ].join(" ")}
          >
            {tab.label}
            <Show when={tab.badge !== undefined && tab.badge !== 0}>
              <span class="ml-1.5 rounded-full bg-surface-3 px-1.5 py-px text-[0.625rem] text-faint">
                {tab.badge}
              </span>
            </Show>
          </button>
        )}
      </For>
    </div>
  );
}

// ---- code editor ------------------------------------------------------------

/**
 * A textarea that behaves like an editor: monospace, tab inserts a tab, and a
 * gutter that scrolls with the text. Deliberately not a full editor component —
 * the studio's job is the structure around the code, not another IDE.
 */
export function CodeEditor(props: {
  value: string;
  onInput?: (value: string) => void;
  readOnly?: boolean;
  minHeight?: string;
  language?: string;
}) {
  let textarea: HTMLTextAreaElement | undefined;
  let gutter: HTMLDivElement | undefined;

  const lines = () => Math.max(props.value.split("\n").length, 1);

  const onKeyDown = (event: KeyboardEvent) => {
    if (event.key !== "Tab" || !textarea || props.readOnly) return;
    event.preventDefault();
    const { selectionStart, selectionEnd, value } = textarea;
    const next = `${value.slice(0, selectionStart)}    ${value.slice(selectionEnd)}`;
    props.onInput?.(next);
    requestAnimationFrame(() => {
      if (!textarea) return;
      textarea.selectionStart = textarea.selectionEnd = selectionStart + 4;
    });
  };

  return (
    <div
      class="relative flex overflow-hidden rounded-lg border border-line bg-[#0a100e]"
      style={{ "min-height": props.minHeight ?? "24rem" }}
    >
      <div
        ref={gutter}
        class="code w-12 shrink-0 select-none overflow-hidden border-r border-line bg-surface/60 py-3 text-right text-faint"
        aria-hidden="true"
      >
        <For each={Array.from({ length: lines() }, (_, index) => index + 1)}>
          {(line) => <div class="pr-2">{line}</div>}
        </For>
      </div>
      <textarea
        ref={textarea}
        class="code w-full resize-none bg-transparent px-3 py-3 text-ink outline-none"
        style={{ "min-height": props.minHeight ?? "24rem" }}
        spellcheck={false}
        readOnly={props.readOnly}
        value={props.value}
        onKeyDown={onKeyDown}
        onScroll={(event) => {
          if (gutter) gutter.scrollTop = event.currentTarget.scrollTop;
        }}
        onInput={(event) => props.onInput?.(event.currentTarget.value)}
      />
      <Show when={props.language}>
        <span class="pointer-events-none absolute right-2 top-2 rounded border border-line bg-surface/90 px-1.5 py-0.5 text-[0.625rem] uppercase tracking-wide text-faint">
          {props.language}
        </span>
      </Show>
    </div>
  );
}

// ---- overlays ---------------------------------------------------------------

export function Modal(props: ParentProps<{ title: string; subtitle?: string; onClose: () => void; width?: string }>) {
  const titleId = createUniqueId();
  return (
    <div
      class="fixed inset-0 z-50 flex items-start justify-center overflow-y-auto bg-black/60 p-6 backdrop-blur-sm"
      onClick={(event) => {
        if (event.target === event.currentTarget) props.onClose();
      }}
    >
      <div
        role="dialog"
        aria-modal="true"
        aria-labelledby={titleId}
        class="animate-rise mt-16 w-full rounded-xl border border-line-strong bg-surface shadow-2xl shadow-black/50"
        style={{ "max-width": props.width ?? "34rem" }}
      >
        <div class="flex items-start justify-between gap-4 border-b border-line px-5 py-4">
          <div>
            <h2 id={titleId} class="text-base font-semibold tracking-tight">
              {props.title}
            </h2>
            <Show when={props.subtitle}>
              <p class="mt-0.5 text-xs text-muted">{props.subtitle}</p>
            </Show>
          </div>
          <button
            type="button"
            onClick={props.onClose}
            class="rounded-md p-1 text-faint transition-colors hover:bg-surface-2 hover:text-ink"
            aria-label="Close"
          >
            <svg width="16" height="16" viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.5">
              <path d="M4 4l8 8M12 4l-8 8" stroke-linecap="round" />
            </svg>
          </button>
        </div>
        <div class="px-5 py-4">{props.children}</div>
      </div>
    </div>
  );
}

export function EmptyState(props: ParentProps<{ title: string; description: string }>) {
  return (
    <div class="flex flex-col items-center justify-center rounded-xl border border-dashed border-line px-6 py-12 text-center">
      <h3 class="text-sm font-semibold text-ink">{props.title}</h3>
      <p class="mt-1 max-w-md text-xs leading-relaxed text-muted">{props.description}</p>
      <Show when={props.children}>
        <div class="mt-4 flex items-center gap-2">{props.children}</div>
      </Show>
    </div>
  );
}

// ---- misc -------------------------------------------------------------------

export function Leaf(props: { class?: string }) {
  return (
    <svg viewBox="0 0 32 32" class={props.class} fill="none" aria-hidden="true">
      <rect width="32" height="32" rx="8" fill="currentColor" />
      <path
        d="M16 25V13m0 0c0-3.3 2.7-6 6-6h3v2.5c0 3-2.5 5.5-5.5 5.5H16zm0 4.5h-2.2A5.3 5.3 0 0 1 8.5 14v-2h2.7c2.9 0 5.3 2.4 5.3 5.3v.2z"
        fill="none"
        stroke="#052e1b"
        stroke-width="2"
        stroke-linecap="round"
        stroke-linejoin="round"
      />
    </svg>
  );
}

export function Mono(props: ParentProps<{ class?: string }>) {
  return (
    <code class={`rounded bg-surface-3 px-1.5 py-0.5 font-mono text-[0.75rem] text-muted ${props.class ?? ""}`}>
      {props.children}
    </code>
  );
}
