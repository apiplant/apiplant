import { createSignal } from "solid-js";

export type View =
  | { kind: "overview" }
  | { kind: "config"; section?: string }
  | { kind: "resource"; name: string }
  | { kind: "function"; name: string }
  | { kind: "agent"; name: string }
  | { kind: "changes" };

function parsePath(pathname: string): View {
  const parts = pathname
    .split("/")
    .filter(Boolean)
    .map((part) => decodeURIComponent(part));

  if (parts[0] === "config") return parts[1] ? { kind: "config", section: parts[1] } : { kind: "config" };
  if (parts[0] === "changes") return { kind: "changes" };
  if (parts[0] === "resources" && parts[1]) return { kind: "resource", name: parts[1] };
  if (parts[0] === "functions" && parts[1]) return { kind: "function", name: parts[1] };
  if (parts[0] === "agents" && parts[1]) return { kind: "agent", name: parts[1] };
  return { kind: "overview" };
}

function pathFor(view: View): string {
  switch (view.kind) {
    case "overview":
      return "/";
    case "config":
      return view.section ? `/config/${encodeURIComponent(view.section)}` : "/config";
    case "changes":
      return "/changes";
    case "resource":
      return `/resources/${encodeURIComponent(view.name)}`;
    case "function":
      return `/functions/${encodeURIComponent(view.name)}`;
    case "agent":
      return `/agents/${encodeURIComponent(view.name)}`;
  }
}

const [view, setViewSignal] = createSignal<View>(
  typeof window === "undefined" ? { kind: "overview" } : parsePath(window.location.pathname),
);

export { view };

export function setView(next: View, options: { replace?: boolean } = {}) {
  setViewSignal(next);
  if (typeof window === "undefined") return;
  const path = pathFor(next);
  if (window.location.pathname === path) return;
  if (options.replace) window.history.replaceState(null, "", path);
  else window.history.pushState(null, "", path);
}

export function syncViewFromLocation() {
  if (typeof window === "undefined") return;
  setViewSignal(parsePath(window.location.pathname));
}

export function isActive(current: View, target: View): boolean {
  if (current.kind !== target.kind) return false;
  if (current.kind === "resource" && target.kind === "resource") return current.name === target.name;
  if (current.kind === "function" && target.kind === "function") return current.name === target.name;
  if (current.kind === "agent" && target.kind === "agent") return current.name === target.name;
  return true;
}
