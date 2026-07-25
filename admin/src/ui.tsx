import { Show, splitProps, type JSX, type ParentProps } from "solid-js";

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
  const [local, rest] = splitProps(props, ["variant", "size", "class", "children"]);
  return (
    <button
      {...rest}
      class={[
        "inline-flex items-center justify-center gap-1.5 rounded-lg whitespace-nowrap transition-colors duration-100",
        "disabled:pointer-events-none disabled:opacity-40",
        local.size === "sm" ? "px-2.5 py-1 text-xs" : "px-3 py-1.5 text-[0.8125rem]",
        BUTTON_VARIANTS[local.variant ?? "secondary"],
        local.class ?? "",
      ].join(" ")}
    >
      {local.children}
    </button>
  );
}

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

export function HeadMark(props: { class?: string }) {
  return <span class={`logo-head ${props.class ?? ""}`} role="img" aria-label="apiplant" />;
}

export function Mono(props: ParentProps<{ class?: string }>) {
  return (
    <code class={`rounded bg-surface-3 px-1.5 py-0.5 font-mono text-[0.75rem] text-muted ${props.class ?? ""}`}>
      {props.children}
    </code>
  );
}
