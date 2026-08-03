/**
 * The interface's vocabulary: buttons, cards, fields, tables, dialogs.
 *
 * Everything here is intentionally conventional. The dashboard is used by
 * operators who did not write the application, so its controls should behave
 * like the controls they already know.
 */

import { For, Show, createEffect, createSignal, onCleanup, splitProps } from "solid-js";
import type { JSX, ParentProps } from "solid-js";
import { Portal } from "solid-js/web";
import { theme, toggleTheme } from "./theme";

type ButtonVariant = "primary" | "secondary" | "ghost" | "danger";

const BUTTON_VARIANTS: Record<ButtonVariant, string> = {
  primary: "bg-accent text-on-accent hover:bg-accent-dim font-semibold shadow-sm",
  secondary: "bg-surface-2 text-ink border border-line hover:border-line-strong hover:bg-surface-3",
  ghost: "text-muted hover:text-ink hover:bg-surface-2",
  danger: "bg-transparent text-danger border border-danger-line hover:bg-danger-soft",
};

export function Button(
  props: ParentProps<
    {
      variant?: ButtonVariant;
      size?: "sm" | "md" | "lg";
      loading?: boolean;
      class?: string;
    } & JSX.ButtonHTMLAttributes<HTMLButtonElement>
  >,
) {
  const [local, rest] = splitProps(props, ["variant", "size", "class", "children", "loading"]);
  const sizes = {
    sm: "px-2.5 py-1 text-xs",
    md: "px-3.5 py-2 text-[0.8125rem]",
    lg: "px-4 py-2.5 text-sm",
  };
  return (
    <button
      type="button"
      {...rest}
      disabled={rest.disabled || local.loading}
      class={[
        "inline-flex items-center justify-center gap-1.5 rounded-lg whitespace-nowrap transition-colors duration-100",
        "disabled:pointer-events-none disabled:opacity-40",
        sizes[local.size ?? "md"],
        BUTTON_VARIANTS[local.variant ?? "secondary"],
        local.class ?? "",
      ].join(" ")}
    >
      <Show when={local.loading}>
        <Spinner />
      </Show>
      {local.children}
    </button>
  );
}

export function Spinner(props: { class?: string }) {
  return (
    <svg
      class={`animate-spin ${props.class ?? "h-3.5 w-3.5"}`}
      viewBox="0 0 16 16"
      fill="none"
      aria-hidden="true"
    >
      <circle cx="8" cy="8" r="6.2" stroke="currentColor" stroke-width="1.6" opacity="0.25" />
      <path
        d="M14.2 8A6.2 6.2 0 0 0 8 1.8"
        stroke="currentColor"
        stroke-width="1.6"
        stroke-linecap="round"
      />
    </svg>
  );
}

export function Card(props: ParentProps<{ class?: string }>) {
  return <div class={`card ${props.class ?? ""}`}>{props.children}</div>;
}

export function CardHeader(props: ParentProps<{ title: string; hint?: string; class?: string }>) {
  return (
    <div
      class={`flex flex-wrap items-center justify-between gap-3 border-b border-line px-5 py-3.5 ${props.class ?? ""}`}
    >
      <div class="min-w-0">
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
  props: ParentProps<{ tone?: "neutral" | "accent" | "warn" | "danger" | "success"; class?: string }>,
) {
  const tones = {
    neutral: "bg-surface-3 text-muted border-line",
    accent: "bg-accent-soft text-accent border-accent-line",
    warn: "bg-warn-soft text-warn border-warn-line",
    danger: "bg-danger-soft text-danger border-danger-line",
    success: "bg-accent-soft text-success border-accent-line",
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

export function PageTitle(props: ParentProps<{ title: string; subtitle?: string }>) {
  return (
    <div class="mb-5 flex flex-wrap items-end justify-between gap-3">
      <div class="min-w-0">
        <h1 class="text-xl font-semibold tracking-tight text-ink sm:text-2xl">{props.title}</h1>
        <Show when={props.subtitle}>
          <p class="mt-1 text-sm text-muted">{props.subtitle}</p>
        </Show>
      </div>
      <div class="flex shrink-0 flex-wrap items-center gap-2">{props.children}</div>
    </div>
  );
}

export function Field(
  props: ParentProps<{ label: string; help?: string | null; required?: boolean; error?: string | null }>,
) {
  return (
    <label class="block">
      <span class="field-label">
        {props.label}
        <Show when={props.required}>
          <span class="ml-1 text-danger" aria-hidden="true">
            *
          </span>
        </Show>
      </span>
      {props.children}
      <Show when={props.error}>
        <p class="mt-1 text-[0.6875rem] leading-relaxed text-danger">{props.error}</p>
      </Show>
      <Show when={props.help && !props.error}>
        <p class="mt-1 text-[0.6875rem] leading-relaxed text-faint">{props.help}</p>
      </Show>
    </label>
  );
}

export function Toggle(props: {
  checked: boolean;
  onChange: (value: boolean) => void;
  label: string;
  help?: string | null;
  disabled?: boolean;
}) {
  return (
    <label
      class={`flex items-start gap-3 rounded-xl border border-line bg-surface-2/50 px-3 py-2.5 ${
        props.disabled ? "opacity-60" : "cursor-pointer hover:border-line-strong"
      }`}
    >
      <button
        type="button"
        role="switch"
        aria-checked={props.checked}
        disabled={props.disabled}
        onClick={() => props.onChange(!props.checked)}
        class={`mt-0.5 inline-flex h-5 w-9 shrink-0 items-center rounded-full border transition-colors ${
          props.checked ? "border-accent-line bg-accent" : "border-line bg-surface-3"
        }`}
      >
        <span
          class={`h-3.5 w-3.5 rounded-full bg-surface transition-transform ${
            props.checked ? "translate-x-4.5" : "translate-x-0.5"
          }`}
        />
      </button>
      <span class="min-w-0">
        <span class="block text-sm text-ink">{props.label}</span>
        <Show when={props.help}>
          <span class="mt-0.5 block text-[0.6875rem] leading-relaxed text-faint">{props.help}</span>
        </Show>
      </span>
    </label>
  );
}

export function EmptyState(props: ParentProps<{ title: string; description?: string; icon?: JSX.Element }>) {
  return (
    <div class="flex flex-col items-center justify-center rounded-xl border border-dashed border-line px-6 py-14 text-center">
      <Show when={props.icon}>
        <div class="mb-3 text-faint">{props.icon}</div>
      </Show>
      <h3 class="text-sm font-semibold text-ink">{props.title}</h3>
      <Show when={props.description}>
        <p class="mt-1.5 max-w-sm text-xs leading-relaxed text-muted">{props.description}</p>
      </Show>
      <Show when={props.children}>
        <div class="mt-5 flex items-center gap-2">{props.children}</div>
      </Show>
    </div>
  );
}

export function HeadMark(props: { class?: string; src?: string | null }) {
  return (
    <Show
      when={props.src}
      fallback={<span class={`logo-head ${props.class ?? ""}`} role="img" aria-label="apiplant" />}
    >
      <img src={props.src!} alt="" class={`w-auto object-contain ${props.class ?? ""}`} />
    </Show>
  );
}

/** Initials in a tinted circle — enough to tell people apart in a list. */
export function Avatar(props: { name: string; size?: "sm" | "md" }) {
  const initials = () =>
    props.name
      .split(/[\s@._-]+/)
      .filter(Boolean)
      .slice(0, 2)
      .map((part) => part[0]?.toUpperCase() ?? "")
      .join("") || "?";
  return (
    <span
      class={`inline-flex shrink-0 items-center justify-center rounded-full border border-accent-line bg-accent-soft font-semibold text-accent ${
        props.size === "sm" ? "h-6 w-6 text-[0.625rem]" : "h-8 w-8 text-xs"
      }`}
    >
      {initials()}
    </span>
  );
}

/** A modal. Used for confirmations and for anything that would otherwise be a
 *  second page for a ten-second task. */
export function Dialog(
  props: ParentProps<{
    open: boolean;
    title: string;
    description?: string;
    onClose: () => void;
    footer?: JSX.Element;
  }>,
) {
  createEffect(() => {
    if (!props.open) return;
    const onKey = (event: KeyboardEvent) => {
      if (event.key === "Escape") props.onClose();
    };
    window.addEventListener("keydown", onKey);
    onCleanup(() => window.removeEventListener("keydown", onKey));
  });

  return (
    <Show when={props.open}>
      <Portal>
        <div class="fixed inset-0 z-50 flex items-center justify-center p-4">
          <div
            class="absolute inset-0 bg-canvas/70 backdrop-blur-sm"
            onClick={props.onClose}
            aria-hidden="true"
          />
          <div
            role="dialog"
            aria-modal="true"
            aria-label={props.title}
            class="animate-rise relative w-full max-w-lg overflow-hidden rounded-2xl border border-line-strong bg-surface shadow-2xl"
          >
            <div class="border-b border-line px-5 py-4">
              <h2 class="text-sm font-semibold tracking-tight text-ink">{props.title}</h2>
              <Show when={props.description}>
                <p class="mt-1 text-xs leading-relaxed text-muted">{props.description}</p>
              </Show>
            </div>
            <div class="max-h-[60vh] overflow-y-auto px-5 py-4">{props.children}</div>
            <Show when={props.footer}>
              <div class="flex items-center justify-end gap-2 border-t border-line bg-surface-2/40 px-5 py-3">
                {props.footer}
              </div>
            </Show>
          </div>
        </div>
      </Portal>
    </Show>
  );
}

/** A confirmation with a typed-in phrase when the action is destructive. */
export function ConfirmDialog(props: {
  open: boolean;
  title: string;
  description: string;
  confirmLabel: string;
  danger?: boolean;
  busy?: boolean;
  onConfirm: () => void;
  onCancel: () => void;
}) {
  return (
    <Dialog
      open={props.open}
      title={props.title}
      onClose={props.onCancel}
      footer={
        <>
          <Button variant="ghost" onClick={props.onCancel}>
            Cancel
          </Button>
          <Button
            variant={props.danger ? "danger" : "primary"}
            loading={props.busy}
            onClick={props.onConfirm}
          >
            {props.confirmLabel}
          </Button>
        </>
      }
    >
      <p class="text-sm leading-relaxed text-muted">{props.description}</p>
    </Dialog>
  );
}

export function ThemeToggle() {
  return (
    <button
      type="button"
      onClick={toggleTheme}
      title={theme() === "dark" ? "Switch to light theme" : "Switch to dark theme"}
      aria-label={theme() === "dark" ? "Switch to light theme" : "Switch to dark theme"}
      class="rounded-lg p-2 text-faint transition-colors hover:bg-surface-2 hover:text-ink"
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

export function SearchInput(props: {
  value: string;
  placeholder: string;
  onInput: (value: string) => void;
  onSubmit: () => void;
}) {
  return (
    <div class="relative">
      <svg
        class="pointer-events-none absolute left-2.5 top-1/2 h-3.5 w-3.5 -translate-y-1/2 text-faint"
        viewBox="0 0 16 16"
        fill="none"
        stroke="currentColor"
        stroke-width="1.5"
      >
        <circle cx="7" cy="7" r="4.5" />
        <path d="m10.5 10.5 3 3" stroke-linecap="round" />
      </svg>
      <input
        class="input pl-8"
        type="search"
        placeholder={props.placeholder}
        value={props.value}
        onInput={(event) => props.onInput(event.currentTarget.value)}
        onKeyDown={(event) => {
          if (event.key === "Enter") props.onSubmit();
        }}
      />
    </div>
  );
}

/** A toast stack. Errors persist; everything else fades on its own. */
export function ToastStack(props: {
  toasts: { id: number; kind: "success" | "error" | "info"; message: string }[];
  onDismiss: (id: number) => void;
}) {
  const tones = {
    success: "border-accent-line bg-accent-soft text-ink",
    error: "border-danger-line bg-danger-soft text-ink",
    info: "border-line bg-surface text-ink",
  };
  return (
    <Portal>
      <div class="pointer-events-none fixed bottom-4 right-4 z-[60] flex w-full max-w-sm flex-col gap-2">
        <For each={props.toasts}>
          {(toast) => (
            <div
              role={toast.kind === "error" ? "alert" : "status"}
              class={`animate-rise pointer-events-auto flex items-start gap-3 rounded-xl border px-3.5 py-2.5 shadow-lg ${tones[toast.kind]}`}
            >
              <span class="min-w-0 flex-1 text-[0.8125rem] leading-relaxed">{toast.message}</span>
              <button
                type="button"
                class="shrink-0 rounded p-0.5 text-faint transition-colors hover:text-ink"
                aria-label="Dismiss"
                onClick={() => props.onDismiss(toast.id)}
              >
                <svg width="12" height="12" viewBox="0 0 12 12" fill="none" stroke="currentColor" stroke-width="1.6">
                  <path d="m3 3 6 6M9 3l-6 6" stroke-linecap="round" />
                </svg>
              </button>
            </div>
          )}
        </For>
      </div>
    </Portal>
  );
}

/** A dropdown anchored to a trigger — the user menu and row menus use it. */
export function Menu(props: ParentProps<{ trigger: (open: () => void) => JSX.Element; align?: "left" | "right" }>) {
  const [open, setOpen] = createSignal(false);
  let container: HTMLDivElement | undefined;

  createEffect(() => {
    if (!open()) return;
    const onDocument = (event: MouseEvent) => {
      if (container && !container.contains(event.target as Node)) setOpen(false);
    };
    const onKey = (event: KeyboardEvent) => {
      if (event.key === "Escape") setOpen(false);
    };
    document.addEventListener("mousedown", onDocument);
    window.addEventListener("keydown", onKey);
    onCleanup(() => {
      document.removeEventListener("mousedown", onDocument);
      window.removeEventListener("keydown", onKey);
    });
  });

  return (
    <div class="relative" ref={container}>
      {props.trigger(() => setOpen((value) => !value))}
      <Show when={open()}>
        <div
          class={`animate-rise absolute z-40 mt-1.5 min-w-52 overflow-hidden rounded-xl border border-line-strong bg-surface py-1 shadow-xl ${
            props.align === "left" ? "left-0" : "right-0"
          }`}
          onClick={() => setOpen(false)}
        >
          {props.children}
        </div>
      </Show>
    </div>
  );
}

export function MenuItem(
  props: ParentProps<{ onClick?: () => void; href?: string; danger?: boolean; disabled?: boolean }>,
) {
  const cls = `flex w-full items-center gap-2 px-3 py-2 text-left text-[0.8125rem] transition-colors disabled:opacity-40 ${
    props.danger ? "text-danger hover:bg-danger-soft" : "text-muted hover:bg-surface-2 hover:text-ink"
  }`;
  return (
    <Show
      when={props.href}
      fallback={
        <button type="button" class={cls} disabled={props.disabled} onClick={props.onClick}>
          {props.children}
        </button>
      }
    >
      <a class={cls} href={props.href} target="_blank" rel="noreferrer">
        {props.children}
      </a>
    </Show>
  );
}

export function MenuSeparator() {
  return <div class="my-1 border-t border-line" />;
}

/** A skeleton row, so a loading table does not collapse and jump. */
export function SkeletonRows(props: { rows?: number; columns: number }) {
  return (
    <For each={Array.from({ length: props.rows ?? 5 })}>
      {() => (
        <tr>
          <For each={Array.from({ length: props.columns })}>
            {() => (
              <td class="px-4 py-3">
                <div class="h-3 w-full max-w-40 animate-pulse rounded bg-surface-3" />
              </td>
            )}
          </For>
        </tr>
      )}
    </For>
  );
}
