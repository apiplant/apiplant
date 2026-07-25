import { createSignal } from "solid-js";

export type View =
  | { kind: "overview" }
  | { kind: "config" }
  | { kind: "resource"; name: string }
  | { kind: "function"; name: string }
  | { kind: "changes" };

const [view, setView] = createSignal<View>({ kind: "overview" });

export { view, setView };

export function isActive(current: View, target: View): boolean {
  if (current.kind !== target.kind) return false;
  if (current.kind === "resource" && target.kind === "resource") return current.name === target.name;
  if (current.kind === "function" && target.kind === "function") return current.name === target.name;
  return true;
}
