import { createSignal } from "solid-js";

export type Theme = "dark" | "light";

const STORAGE_KEY = "apiplant-admin-theme";

function systemTheme(): Theme {
  return window.matchMedia?.("(prefers-color-scheme: light)").matches ? "light" : "dark";
}

function storedTheme(): Theme | null {
  const value = localStorage.getItem(STORAGE_KEY);
  return value === "dark" || value === "light" ? value : null;
}

const [theme, setThemeSignal] = createSignal<Theme>(storedTheme() ?? systemTheme());

function applyTheme(next: Theme) {
  const root = document.documentElement;
  root.classList.toggle("light", next === "light");
  root.style.colorScheme = next;
}

applyTheme(theme());

export function setTheme(next: Theme) {
  setThemeSignal(next);
  localStorage.setItem(STORAGE_KEY, next);
  applyTheme(next);
}

export function toggleTheme() {
  setTheme(theme() === "dark" ? "light" : "dark");
}

window.matchMedia?.("(prefers-color-scheme: light)").addEventListener("change", () => {
  if (storedTheme()) return;
  const next = systemTheme();
  setThemeSignal(next);
  applyTheme(next);
});

export { theme };
