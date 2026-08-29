/**
 * Light/dark, the way apiplant.com does it: dark by default, with a `light`
 * class on `<html>` swapping the palette. The choice is remembered; without one,
 * the system preference decides. An inline script in index.html sets the class
 * so the first paint is already the right theme.
 */

import { createSignal } from "solid-js";

export type Theme = "dark" | "light";

const STORAGE_KEY = "apiplant-studio-theme";

function systemTheme(): Theme {
  return window.matchMedia?.("(prefers-color-scheme: light)").matches ? "light" : "dark";
}

function stored(): Theme | null {
  const value = localStorage.getItem(STORAGE_KEY);
  return value === "dark" || value === "light" ? value : null;
}

const [theme, setThemeSignal] = createSignal<Theme>(stored() ?? systemTheme());

function apply(next: Theme) {
  const root = document.documentElement;
  root.classList.toggle("light", next === "light");
  root.style.colorScheme = next;
}

apply(theme());

export function setTheme(next: Theme) {
  setThemeSignal(next);
  localStorage.setItem(STORAGE_KEY, next);
  apply(next);
}

export function toggleTheme() {
  setTheme(theme() === "dark" ? "light" : "dark");
}

/** Follow the system until the user picks a side. */
window.matchMedia?.("(prefers-color-scheme: light)").addEventListener("change", () => {
  if (!stored()) setThemeSignal((_) => {
    const next = systemTheme();
    apply(next);
    return next;
  });
});

export { theme };
