/**
 * Which palette the screenshots in the guides are shown in.
 *
 * Every application shot is photographed twice (`cd e2e && pnpm shots`), and a
 * reader who runs the admin dashboard in dark mode should see it that way in
 * the documentation — so the default follows the site's own theme. A reader who
 * picks a side on one screenshot has picked it for the article: the choice is a
 * page-wide signal rather than per-picture state, because a guide whose shots
 * disagree with each other reads as two applications.
 */

import { createSignal } from "solid-js";
import { theme, type Theme } from "./theme";

/** `null` until the reader chooses, meaning "whatever the site is wearing". */
const [chosen, setChosen] = createSignal<Theme | null>(null);

/** The palette to show screenshots in right now. */
export function shotTheme(): Theme {
  return chosen() ?? theme();
}

/** Flip the screenshots, and stop following the site's theme. */
export function toggleShotTheme() {
  setChosen(shotTheme() === "dark" ? "light" : "dark");
}
