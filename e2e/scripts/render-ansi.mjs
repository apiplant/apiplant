/**
 * Turn the console's recorded output into pictures.
 *
 * `cli-shots.py` saves what the program wrote to its terminal: raw bytes, cursor
 * moves, colours and all. Nothing in that stream is a picture — deciding what it
 * looks like is a terminal's job. So this hands each capture to a real terminal
 * emulator (xterm.js, in a headless browser) and photographs the result.
 *
 * The point of the split is that the screenshots' typography, palette and size
 * are decided here, and changing any of them costs a rerun of this file rather
 * than another pass over the running app.
 */

import { readdir, readFile, mkdir } from "node:fs/promises";
import { createRequire } from "node:module";
import { basename, join } from "node:path";
import { fileURLToPath } from "node:url";

import { chromium } from "@playwright/test";

const require = createRequire(import.meta.url);
const HERE = fileURLToPath(new URL(".", import.meta.url));
const CAPTURES = join(HERE, "..", ".cli-shots");
const SHOTS_DIR = fileURLToPath(new URL("../../docs/images", import.meta.url));

/**
 * The theme. Deliberately not a terminal's defaults: these sit in the docs
 * beside the dashboard's screenshots, and a picture that shared their palette
 * reads as the same product rather than as a stray terminal.
 */
const THEME = {
  background: "#12141a",
  foreground: "#d6dae4",
  cursor: "#12141a", // hidden — a blinking block is noise in a still picture
  cursorAccent: "#12141a",
  selectionBackground: "#2a3040",
  black: "#12141a",
  red: "#e5707c",
  green: "#7fc98c",
  yellow: "#e0c076",
  blue: "#7aa7e8",
  magenta: "#c095e0",
  cyan: "#6fc4c9",
  white: "#d6dae4",
  brightBlack: "#5c6472",
  brightRed: "#f08a95",
  brightGreen: "#95dda2",
  brightYellow: "#f0d48c",
  brightBlue: "#94bcf5",
  brightMagenta: "#d3aaf0",
  brightCyan: "#8bd8dd",
  brightWhite: "#ffffff",
};

const PADDING = 18;

async function main() {
  const names = (await readdir(CAPTURES).catch(() => []))
    .filter((file) => file.endsWith(".ansi"))
    .sort();
  if (names.length === 0) {
    console.error(`render-ansi: nothing to render in ${CAPTURES} — run cli-shots.py first`);
    return 1;
  }
  await mkdir(SHOTS_DIR, { recursive: true });

  const xtermJs = await readFile(require.resolve("@xterm/xterm/lib/xterm.js"), "utf8");
  const xtermCss = await readFile(require.resolve("@xterm/xterm/css/xterm.css"), "utf8");

  const browser = await chromium.launch();
  const page = await browser.newPage({
    viewport: { width: 1200, height: 800 },
    // 2×, like the browser screenshots, so the text survives a retina screen.
    deviceScaleFactor: 2,
  });

  for (const file of names) {
    const name = basename(file, ".ansi");
    const stream = await readFile(join(CAPTURES, file));
    await render(page, xtermJs, xtermCss, stream);
    const terminal = page.locator("#shot");
    await terminal.screenshot({
      path: join(SHOTS_DIR, `${name}.png`),
      animations: "disabled",
    });
    console.log(`  ${name}.png`);
  }

  await browser.close();
  return 0;
}

/** Load a fresh terminal and replay one capture into it. */
async function render(page, xtermJs, xtermCss, stream) {
  await page.setContent(`
    <style>
      ${xtermCss}
      html, body { margin: 0; background: ${THEME.background}; }
      #shot {
        display: inline-block;
        padding: ${PADDING}px;
        background: ${THEME.background};
        border-radius: 10px;
      }
      /* The cursor is a still picture's only moving part. Take it out. */
      .xterm-cursor-layer, .xterm-cursor { display: none !important; }
    </style>
    <div id="shot"><div id="term"></div></div>
  `);
  await page.addScriptTag({ content: xtermJs });

  await page.evaluate(
    ([data, theme]) =>
      new Promise((resolve) => {
        const term = new window.Terminal({
          cols: 110,
          rows: 32,
          theme,
          fontFamily:
            "'JetBrains Mono', 'Fira Code', 'DejaVu Sans Mono', ui-monospace, monospace",
          fontSize: 13,
          lineHeight: 1.2,
          letterSpacing: 0,
          cursorBlink: false,
          allowTransparency: false,
          convertEol: false,
        });
        term.open(document.getElementById("term"));
        // The stream arrives as a byte array: it contains escape sequences and
        // UTF-8 box drawing, and only the byte form survives the round trip.
        term.write(new Uint8Array(data), () => requestAnimationFrame(() => resolve()));
      }),
    [Array.from(stream), THEME],
  );

  // One more frame, so the last write has certainly been painted.
  await page.waitForTimeout(120);
}

process.exitCode = await main();
