/** Shared presentational pieces. Nothing here knows about apiplant. */

import { For, Show, createSignal, createUniqueId, omit, type ParentProps } from "solid-js";
import type { JSX } from "@solidjs/web";
import { theme, toggleTheme } from "../lib/theme";

// ---- buttons ----------------------------------------------------------------

type ButtonVariant = "primary" | "secondary" | "ghost" | "danger";

const BUTTON_VARIANTS: Record<ButtonVariant, string> = {
  primary: "bg-accent text-on-accent hover:bg-accent-dim active:bg-accent-dim font-semibold",
  secondary: "bg-surface-2 text-ink border border-line hover:border-line-strong hover:bg-surface-3",
  ghost: "text-muted hover:text-ink hover:bg-surface-2",
  danger: "bg-transparent text-danger border border-danger-line hover:bg-danger-soft",
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
  const rest = omit(props, "variant", "size", "class", "children");
  return (
    <button
      {...rest}
      class={[
        "inline-flex items-center justify-center gap-1.5 rounded-lg whitespace-nowrap transition-colors duration-100",
        "disabled:opacity-40 disabled:pointer-events-none",
        props.size === "sm" ? "px-2.5 py-1 text-xs" : "px-3 py-1.5 text-[0.8125rem]",
        BUTTON_VARIANTS[props.variant ?? "secondary"],
        props.class ?? "",
      ].join(" ")}
    >
      {props.children}
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
    accent: "bg-accent-soft text-accent border-accent-line",
    warn: "bg-warn-soft text-warn border-warn-line",
    danger: "bg-danger-soft text-danger border-danger-line",
    info: "bg-accent-soft text-accent border-accent-line",
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
    lowercase?: boolean;
    autofocus?: boolean;
    onKeyDown?: (event: KeyboardEvent) => void;
  },
) {
  // Mobile keyboards capitalise the first letter by default, which silently breaks
  // identifiers. `lowercase` turns that off and folds anything typed or pasted.
  const normalise = (value: string) => (props.lowercase ? value.toLowerCase() : value);

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
      // Runs once, when the element is created — which is exactly the moment
      // wanted: a control revealed by expanding a row takes the caret, and one
      // merely re-rendered never steals it.
      ref={(element) => {
        if (props.autofocus) queueMicrotask(() => element.select());
      }}
      autocapitalize={props.lowercase ? "none" : undefined}
      autocorrect={props.lowercase ? "off" : undefined}
      onInput={(event) => {
        const value = normalise(event.currentTarget.value);
        if (event.currentTarget.value !== value) event.currentTarget.value = value;
        props.onInput(value);
      }}
      onKeyDown={(event) => props.onKeyDown?.(event)}
    />
  );
}

export function TextArea(
  props: {
    value: string;
    onInput: (value: string) => void;
    placeholder?: string;
    mono?: boolean;
    class?: string;
    disabled?: boolean;
  },
) {
  return (
    <textarea
      class={`input ${props.mono ? "font-mono text-[0.78125rem]" : ""} ${props.class ?? ""}`}
      value={props.value}
      placeholder={props.placeholder}
      disabled={props.disabled}
      spellcheck={!props.mono}
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
  lowercase?: boolean;
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
      autocapitalize={props.lowercase ? "none" : undefined}
      autocorrect={props.lowercase ? "off" : undefined}
      onInput={(event) => {
        if (props.lowercase) event.currentTarget.value = event.currentTarget.value.toLowerCase();
        setDraft(event.currentTarget.value);
      }}
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
          ? "border-accent-line bg-accent-soft text-accent"
          : "border-line bg-surface-2 text-faint hover:border-line-strong hover:text-muted",
      ].join(" ")}
    >
      <span
        class={`h-1.5 w-1.5 rounded-full ${props.checked ? "bg-accent" : "bg-line-strong"}`}
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
      aria-checked={props.checked ? "true" : "false"}
      onClick={() => props.onChange(!props.checked)}
      class="inline-flex items-center gap-2 text-[0.8125rem] text-ink"
    >
      <span
        class={`relative h-5 w-9 shrink-0 rounded-full border transition-colors duration-150 ${
          props.checked ? "border-accent-line bg-accent" : "border-line bg-surface-3"
        }`}
      >
        {/* `left` is explicit because a button centres its text, and an
            absolutely-positioned child with no `left` inherits that as its
            static position. */}
        <span
          class={`absolute left-0.5 top-0.5 h-3.5 w-3.5 rounded-full transition-transform duration-150 ${
            props.checked ? "translate-x-4 bg-on-accent" : "translate-x-0 bg-faint"
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
    <div class="flex flex-wrap items-center gap-1 rounded-lg border border-line bg-surface p-1">
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

// The editor lives on its own because CodeMirror brings its own world with it.
export { CodeEditor, type EditorHandle } from "./CodeEditor";

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

/**
 * The apiplant mark — masked in dark mode, swapped to the rendered light asset
 * in light mode.
 */
export function HeadMark(props: { class?: string }) {
  return <span class={`logo-head ${props.class ?? ""}`} role="img" aria-label="apiplant" />;
}

/** The light/dark switch: sun and moon, current state filled in. */
export function ThemeToggle() {
  return (
    <button
      type="button"
      onClick={toggleTheme}
      title={theme() === "dark" ? "Switch to light" : "Switch to dark"}
      aria-label={theme() === "dark" ? "Switch to light theme" : "Switch to dark theme"}
      class="rounded-lg p-1.5 text-faint transition-colors hover:bg-surface-2 hover:text-ink"
    >
      <Show
        when={theme() === "dark"}
        fallback={
          <svg width="16" height="16" viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.4">
            <path d="M13.2 9.6A5.4 5.4 0 0 1 6.4 2.8a5.6 5.6 0 1 0 6.8 6.8z" stroke-linejoin="round" />
          </svg>
        }
      >
        <svg width="16" height="16" viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.4">
          <circle cx="8" cy="8" r="3.1" />
          <path
            d="M8 1.4v1.5M8 13.1v1.5M14.6 8h-1.5M2.9 8H1.4M12.67 3.33l-1.06 1.06M4.39 11.61l-1.06 1.06M12.67 12.67l-1.06-1.06M4.39 4.39L3.33 3.33"
            stroke-linecap="round"
          />
        </svg>
      </Show>
    </button>
  );
}

export function Mono(props: ParentProps<{ class?: string }>) {
  return (
    <code class={`rounded bg-surface-3 px-1.5 py-0.5 font-mono text-[0.75rem] text-muted ${props.class ?? ""}`}>
      {props.children}
    </code>
  );
}
