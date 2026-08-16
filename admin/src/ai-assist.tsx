import { For, Show, createEffect, createMemo, createSignal } from "solid-js";
import { Portal } from "@solidjs/web";
import { api, asRecord, manifest, reportError } from "./store";
import { Button } from "./ui";

type TextControl = HTMLInputElement | HTMLTextAreaElement;

interface Anchor {
  id: string;
  element: TextControl;
  top: number;
  left: number;
}

const TEXT_INPUT_TYPES = new Set(["", "text", "email", "url"]);
const PANEL_GAP = 8;
const BUTTON_SIZE = 32;
const PANEL_WIDTH = 420;
const PANEL_HEIGHT = 248;

function clamp(value: number, min: number, max: number): number {
  return Math.min(Math.max(value, min), max);
}

function isEligibleControl(element: Element): element is TextControl {
  if (element.closest("[data-ai-assist-root]")) return false;
  if (!(element instanceof HTMLTextAreaElement || element instanceof HTMLInputElement)) return false;
  if (element.disabled || element.readOnly || element.hasAttribute("data-ai-assist-ignore")) return false;
  if (element instanceof HTMLInputElement) {
    const type = (element.getAttribute("type") ?? "text").toLowerCase();
    if (!TEXT_INPUT_TYPES.has(type)) return false;
  }
  return element.isConnected && element.getClientRects().length > 0;
}

function fieldLabel(element: TextControl): string {
  const aria = element.getAttribute("aria-label")?.trim();
  if (aria) return aria;
  const label = element.closest("label");
  const heading = label?.querySelector(".field-label");
  const text = heading?.textContent?.replace(/\*+$/, "").trim();
  if (text) return text;
  const placeholder = element.getAttribute("placeholder")?.trim();
  if (placeholder) return placeholder;
  return "field";
}

function fieldFormat(element: TextControl): string | null {
  const markup = element.closest(".markup");
  const format = markup?.querySelector(".markup-format")?.textContent?.trim();
  return format ? format.toLowerCase() : null;
}

function applyValue(element: TextControl, value: string) {
  element.focus();
  element.value = value;
  element.dispatchEvent(new InputEvent("input", { bubbles: true, composed: true }));
  element.dispatchEvent(new Event("change", { bubbles: true }));
}

function buildPrompt(element: TextControl, instruction: string): string {
  const label = fieldLabel(element);
  const format = fieldFormat(element);
  const current = element.value.trim();
  const kind = element instanceof HTMLTextAreaElement ? "textarea" : "input";

  return [
    "You are filling one admin form field.",
    `Field label: ${label}.`,
    `Field kind: ${kind}.`,
    format ? `Expected format: ${format}.` : null,
    current ? `Current value:\n${current}` : "Current value is empty.",
    "Return only the text that should be inserted into the field.",
    "Do not wrap the answer in quotes, markdown fences, or explanations.",
    `Instruction:\n${instruction.trim()}`,
  ]
    .filter(Boolean)
    .join("\n\n");
}

function promptPlaceholder() {
  return manifest()?.ai_assistance?.prompt_placeholder ?? "Describe what you want AI to write for this field.";
}

export function AdminAiAssist() {
  const config = createMemo(() => manifest()?.ai_assistance ?? null);
  const [anchors, setAnchors] = createSignal<Anchor[]>([]);
  const [target, setTarget] = createSignal<TextControl | null>(null);
  const [prompt, setPrompt] = createSignal("");
  const [busy, setBusy] = createSignal(false);

  let frame = 0;
  let panel: HTMLDivElement | undefined;
  let promptInput: HTMLTextAreaElement | undefined;

  const refresh = () => {
    if (!config()) {
      setAnchors([]);
      setTarget(null);
      return;
    }
    cancelAnimationFrame(frame);
    frame = requestAnimationFrame(() => {
      const next = Array.from(document.querySelectorAll("[data-ai-assist-scope] input, [data-ai-assist-scope] textarea"))
        .filter(isEligibleControl)
        .map((element, index) => {
          const rect = element.getBoundingClientRect();
          const left = clamp(rect.right - BUTTON_SIZE - 6, PANEL_GAP, window.innerWidth - BUTTON_SIZE - PANEL_GAP);
          const top = clamp(rect.top + 6, PANEL_GAP, window.innerHeight - BUTTON_SIZE - PANEL_GAP);
          return {
            id: `${index}:${fieldLabel(element)}:${rect.top}:${rect.left}`,
            element,
            top,
            left,
          };
        });
      setAnchors(next);
      if (target() && !next.some((anchor) => anchor.element === target())) setTarget(null);
    });
  };

  createEffect(config, (settings) => {
    if (!settings) {
      setAnchors([]);
      setTarget(null);
      return;
    }

    refresh();
    const observer = new MutationObserver(refresh);
    observer.observe(document.body, {
      childList: true,
      subtree: true,
      attributes: true,
      attributeFilter: ["class", "style", "disabled", "readonly", "type"],
    });
    window.addEventListener("resize", refresh);
    window.addEventListener("scroll", refresh, true);
    return () => {
      cancelAnimationFrame(frame);
      observer.disconnect();
      window.removeEventListener("resize", refresh);
      window.removeEventListener("scroll", refresh, true);
    };
  });

  createEffect(target, (current) => {
    if (!current) return;
    queueMicrotask(() => promptInput?.focus());
    const onDocument = (event: MouseEvent) => {
      const current = target();
      if (!current) return;
      if (panel?.contains(event.target as Node)) return;
      const anchor = anchors().find((entry) => entry.element === current);
      if (anchor && (event.target as Node) instanceof Node && event.composedPath().includes(anchor.element)) return;
      setTarget(null);
    };
    window.addEventListener("mousedown", onDocument);
    const onKey = (event: KeyboardEvent) => {
      if (event.key === "Escape") setTarget(null);
    };
    window.addEventListener("keydown", onKey);
    return () => {
      window.removeEventListener("mousedown", onDocument);
      window.removeEventListener("keydown", onKey);
    };
  });

  const open = (element: TextControl) => {
    setPrompt("");
    setTarget(element);
    refresh();
  };

  const panelStyle = createMemo(() => {
    const current = target();
    if (!current) return undefined;
    const rect = current.getBoundingClientRect();
    const width = Math.min(PANEL_WIDTH, Math.max(220, window.innerWidth - PANEL_GAP * 2));
    const left = clamp(rect.left, PANEL_GAP, window.innerWidth - width - PANEL_GAP);
    const below = window.innerHeight - rect.bottom;
    const top =
      below >= PANEL_HEIGHT + 12
        ? clamp(rect.bottom + 10, PANEL_GAP, window.innerHeight - PANEL_HEIGHT - PANEL_GAP)
        : clamp(rect.top - PANEL_HEIGHT - 10, PANEL_GAP, window.innerHeight - PANEL_HEIGHT - PANEL_GAP);
    return { top: `${top}px`, left: `${left}px`, width: `${width}px` };
  });

  const submit = async () => {
    const current = target();
    const instruction = prompt().trim();
    if (!current || !instruction || busy()) return;

    setBusy(true);
    try {
      const response = asRecord(
        await api("/ai/chat", {
          method: "POST",
          body: {
            stream: false,
            ...(config()?.system ? { system: config()!.system } : {}),
            messages: [{ role: "user", content: buildPrompt(current, instruction) }],
          },
        }),
      );
      const text = typeof response?.text === "string" ? response.text : null;
      if (text === null) throw new Error("The AI assistant returned no text.");
      applyValue(current, text);
      setTarget(null);
    } catch (error) {
      reportError(error);
    } finally {
      setBusy(false);
    }
  };

  return (
    <Show when={config()}>
      <Portal>
        <div class="pointer-events-none fixed inset-0 z-[55]">
          <For each={anchors()}>
            {(anchor) => (
              <button
                type="button"
                class="pointer-events-auto fixed inline-flex h-7 w-7 items-center justify-center rounded-md border border-line-strong bg-surface/75 text-accent/80 shadow-md shadow-black/5 backdrop-blur-sm transition-colors hover:bg-surface hover:text-accent"
                style={{ top: `${anchor.top}px`, left: `${anchor.left}px` }}
                title="Fill with AI"
                aria-label={`Fill ${fieldLabel(anchor.element)} with AI`}
                onPointerDown={(event) => {
                  event.preventDefault();
                  event.stopPropagation();
                  open(anchor.element);
                }}
              >
                <SparklesIcon />
              </button>
            )}
          </For>

          <Show when={target() && panelStyle()}>
            <div
              ref={panel}
              data-ai-assist-root
              class="pointer-events-auto fixed max-h-[calc(100dvh-1rem)] overflow-y-auto rounded-2xl border border-line-strong bg-surface shadow-2xl shadow-black/15"
              style={panelStyle()}
            >
              <div class="border-b border-line px-4 py-3">
                <div class="flex items-center gap-2 text-sm font-semibold tracking-tight text-ink">
                  <span class="inline-flex h-7 w-7 items-center justify-center rounded-full bg-accent-soft text-accent">
                    <SparklesIcon />
                  </span>
                  Fill {fieldLabel(target()!)} with AI
                </div>
                <p class="mt-1 text-xs leading-relaxed text-muted">
                  Describe what should be written. The reply is inserted directly into the field.
                </p>
              </div>
              <div class="space-y-3 px-4 py-4">
                <textarea
                  ref={promptInput}
                  data-ai-assist-ignore
                  class="input min-h-32 resize-y"
                  placeholder={promptPlaceholder()}
                  value={prompt()}
                  onInput={(event) => setPrompt(event.currentTarget.value)}
                  onKeyDown={(event) => {
                    if ((event.metaKey || event.ctrlKey) && event.key === "Enter") {
                      event.preventDefault();
                      void submit();
                    }
                  }}
                />
                <div class="flex items-center justify-between gap-3">
                  <p class="text-[0.6875rem] text-faint">Ctrl/Cmd + Enter to insert</p>
                  <div class="flex items-center gap-2">
                    <Button variant="ghost" onClick={() => setTarget(null)}>
                      Cancel
                    </Button>
                    <Button variant="primary" loading={busy()} disabled={!prompt().trim()} onClick={() => void submit()}>
                      Insert
                    </Button>
                  </div>
                </div>
              </div>
            </div>
          </Show>
        </div>
      </Portal>
    </Show>
  );
}

function SparklesIcon() {
  return (
    <svg width="15" height="15" viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.5">
      <path d="M8 1.8 9.35 5.4l3.65 1.25-3.65 1.25L8 11.5 6.65 7.9 3 6.65 6.65 5.4 8 1.8Z" />
      <path d="M12.6 10.3 13.3 12l1.7.7-1.7.7-.7 1.7-.7-1.7-1.7-.7 1.7-.7.7-1.7Z" />
    </svg>
  );
}
