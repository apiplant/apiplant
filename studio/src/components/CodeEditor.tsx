/**
 * The code editor: CodeMirror 6, one instance per mounted editor. Colours come
 * from the CSS variables in app.css, so light/dark re-paints it with everything
 * else. Languages reconfigure through a compartment, keeping cursor and history.
 */

import { createEffect, onSettled, Show } from "solid-js";
import { EditorState, Compartment, type Extension } from "@codemirror/state";
import {
  EditorView,
  highlightActiveLine,
  highlightActiveLineGutter,
  keymap,
  lineNumbers,
  placeholder as cmPlaceholder,
} from "@codemirror/view";
import { defaultKeymap, history, historyKeymap, indentWithTab } from "@codemirror/commands";
import {
  HighlightStyle,
  StreamLanguage,
  bracketMatching,
  indentUnit,
  syntaxHighlighting,
} from "@codemirror/language";
import { highlightSelectionMatches, search, searchKeymap } from "@codemirror/search";
import { tags } from "@lezer/highlight";
import { javascript } from "@codemirror/lang-javascript";
import { json } from "@codemirror/lang-json";
import { rust } from "@codemirror/lang-rust";
import { cpp } from "@codemirror/lang-cpp";
import { go } from "@codemirror/lang-go";
import { html } from "@codemirror/lang-html";
import { toml } from "@codemirror/legacy-modes/mode/toml";
import { theme } from "../lib/theme";

// ---- languages --------------------------------------------------------------

/**
 * Zig has no CodeMirror grammar, so this is a stream tokenizer: enough to get
 * keywords, builtins, strings, comments and numbers right, which is all the
 * studio's editor needs to make a scaffold readable.
 */
const ZIG_KEYWORDS = new Set(
  `align allowzero and anyframe anytype asm async await break callconv catch comptime const continue
   defer else enum errdefer error export extern fn for if inline noalias noinline nosuspend opaque or
   orelse packed pub resume return linksection struct suspend switch test threadlocal try union
   unreachable usingnamespace var volatile while`.split(/\s+/),
);

const ZIG_TYPES = new Set(
  `bool void noreturn type anyerror comptime_int comptime_float isize usize
   i8 u8 i16 u16 i32 u32 i64 u64 i128 u128 f16 f32 f64 f80 f128 c_int c_uint c_long c_ulong
   true false null undefined`.split(/\s+/),
);

const zig = StreamLanguage.define<{ inComment: boolean }>({
  name: "zig",
  startState: () => ({ inComment: false }),
  token(stream) {
    if (stream.match("//")) {
      stream.skipToEnd();
      return "comment";
    }
    if (stream.match(/^[@][A-Za-z_][A-Za-z0-9_]*/)) return "macroName";
    if (stream.match(/^\\\\.*/)) return "string"; // multiline string literal
    const quote = stream.peek();
    if (quote === '"' || quote === "'") {
      stream.next();
      let escaped = false;
      for (let ch = stream.next(); ch; ch = stream.next()) {
        if (ch === quote && !escaped) break;
        escaped = !escaped && ch === "\\";
      }
      return "string";
    }
    if (stream.match(/^0[xXbBoO][0-9a-fA-F_]+|^\d[\d_]*(\.[\d_]+)?([eE][+-]?\d+)?/)) return "number";
    if (stream.match(/^[A-Za-z_][A-Za-z0-9_]*/)) {
      const word = stream.current();
      if (ZIG_KEYWORDS.has(word)) return "keyword";
      if (ZIG_TYPES.has(word)) return "typeName";
      return null;
    }
    if (stream.match(/^[+\-*/%!<>=&|^~?.]+/)) return "operator";
    stream.next();
    return null;
  },
  languageData: { commentTokens: { line: "//" } },
});

/** Map the loose language names the pages pass in onto a CodeMirror mode. */
export function languageExtension(name: string | undefined): Extension {
  switch ((name ?? "").toLowerCase()) {
    case "rs":
    case "rust":
      return rust();
    case "c":
    case "h":
    case "cpp":
      return cpp();
    case "go":
    case "go.mod":
    case "mod":
    case "go.sum":
    case "sum":
      return go();
    case "zig":
      return zig;
    // One grammar, two dialects: `typescript` turns on the type syntax, which
    // is the only difference that matters for highlighting a function.
    case "ts":
    case "typescript":
      return javascript({ typescript: true });
    case "js":
    case "mjs":
    case "cjs":
    case "javascript":
      return javascript();
    case "json":
      return json();
    // Liquid is HTML with `{{ }}` in it: the HTML grammar leaves the tags alone
    // and gets the markup around them right, which is what the editor is for.
    case "html":
    case "liquid":
      return html();
    case "toml":
      return StreamLanguage.define(toml);
    default:
      return [];
  }
}

// ---- highlighting -----------------------------------------------------------

const highlightStyle = HighlightStyle.define([
  { tag: [tags.comment, tags.lineComment, tags.blockComment, tags.docComment], color: "var(--color-syn-comment)", fontStyle: "italic" },
  { tag: [tags.keyword, tags.modifier, tags.controlKeyword, tags.moduleKeyword, tags.operatorKeyword], color: "var(--color-syn-keyword)" },
  { tag: [tags.string, tags.special(tags.string), tags.regexp, tags.escape], color: "var(--color-syn-string)" },
  { tag: [tags.number, tags.bool, tags.null, tags.atom, tags.literal], color: "var(--color-syn-number)" },
  { tag: [tags.typeName, tags.className, tags.namespace, tags.standard(tags.typeName)], color: "var(--color-syn-type)" },
  { tag: [tags.function(tags.variableName), tags.function(tags.propertyName), tags.macroName, tags.labelName], color: "var(--color-syn-func)" },
  { tag: [tags.propertyName, tags.attributeName], color: "var(--color-syn-number)" },
  { tag: [tags.variableName, tags.definition(tags.variableName)], color: "var(--color-syn-name)" },
  { tag: [tags.operator, tags.punctuation, tags.separator, tags.bracket], color: "var(--color-faint)" },
  { tag: [tags.meta, tags.annotation, tags.processingInstruction], color: "var(--color-syn-type)" },
  { tag: tags.heading, color: "var(--color-syn-number)", fontWeight: "600" },
  { tag: tags.invalid, color: "var(--color-danger)" },
]);

/** Structural styles; every colour is a variable, so themes need no rebuild. */
function editorTheme(dark: boolean): Extension {
  return EditorView.theme(
    {
      // The editor never dictates the page width: it takes what the layout gives
      // it and scrolls a long line sideways inside its own box.
      "&": {
        color: "var(--color-ink)",
        backgroundColor: "var(--color-editor-bg)",
        width: "100%",
        maxWidth: "100%",
      },
      ".cm-scroller": { overflowX: "auto", overflowY: "auto" },
      ".cm-content": { padding: "0.75rem 0", caretColor: "var(--color-accent)" },
      ".cm-line": { padding: "0 0.75rem" },
      ".cm-cursor, .cm-dropCursor": { borderLeftColor: "var(--color-accent)", borderLeftWidth: "2px" },
      "&.cm-focused .cm-selectionBackground, .cm-selectionBackground, .cm-content ::selection": {
        backgroundColor: "var(--color-editor-select)",
      },
      ".cm-activeLine": { backgroundColor: "var(--color-editor-active)" },
      ".cm-activeLineGutter": { backgroundColor: "var(--color-editor-active)", color: "var(--color-muted)" },
      ".cm-gutters": { border: "none", borderRight: "1px solid var(--color-line)" },
      ".cm-lineNumbers .cm-gutterElement": { padding: "0 0.5rem 0 0.75rem", minWidth: "2.5rem" },
      ".cm-selectionMatch": { backgroundColor: "color-mix(in srgb, var(--color-accent) 18%, transparent)" },
      "&.cm-focused .cm-matchingBracket": {
        backgroundColor: "color-mix(in srgb, var(--color-accent) 22%, transparent)",
        outline: "none",
      },
      ".cm-searchMatch": { backgroundColor: "color-mix(in srgb, var(--color-warn) 30%, transparent)" },
      ".cm-searchMatch.cm-searchMatch-selected": {
        backgroundColor: "color-mix(in srgb, var(--color-accent) 40%, transparent)",
      },
      ".cm-panel.cm-search": { padding: "0.5rem", backgroundColor: "var(--color-surface)" },
      ".cm-panel.cm-search input, .cm-panel.cm-search button": {
        backgroundColor: "var(--color-surface-2)",
        color: "var(--color-ink)",
        border: "1px solid var(--color-line)",
        borderRadius: "0.35rem",
        padding: "0.15rem 0.4rem",
      },
      ".cm-panel.cm-search label": { color: "var(--color-muted)" },
    },
    { dark },
  );
}

// ---- the component ----------------------------------------------------------

/** What a page can do to a mounted editor from outside it. */
export interface EditorHandle {
  /** Drop text in at the cursor (or over the selection) and focus the editor. */
  insert(text: string): void;
}

export function CodeEditor(props: {
  value: string;
  onInput?: (value: string) => void;
  readOnly?: boolean;
  minHeight?: string;
  language?: string;
  placeholder?: string;
  /** Called once the view exists, with the handle for inserting into it. */
  onReady?: (handle: EditorHandle) => void;
}) {
  let host!: HTMLDivElement;
  let view: EditorView | undefined;

  const language = new Compartment();
  const editable = new Compartment();
  const appearance = new Compartment();

  onSettled(() => {
    view = new EditorView({
      parent: host,
      state: EditorState.create({
        doc: props.value,
        extensions: [
          lineNumbers(),
          highlightActiveLine(),
          highlightActiveLineGutter(),
          highlightSelectionMatches(),
          history(),
          bracketMatching(),
          search({ top: true }),
          indentUnit.of("    "),
          EditorState.tabSize.of(4),
          keymap.of([...defaultKeymap, ...historyKeymap, ...searchKeymap, indentWithTab]),
          syntaxHighlighting(highlightStyle),
          cmPlaceholder(props.placeholder ?? ""),
          language.of(languageExtension(props.language)),
          appearance.of(editorTheme(theme() === "dark")),
          editable.of([
            EditorView.editable.of(!props.readOnly),
            EditorState.readOnly.of(!!props.readOnly),
          ]),
          EditorView.updateListener.of((update) => {
            if (!update.docChanged) return;
            const next = update.state.doc.toString();
            // Only a user edit differs from what the parent already holds.
            if (next !== props.value) props.onInput?.(next);
          }),
        ],
      }),
    });
    props.onReady?.({
      insert(text) {
        if (!view) return;
        const { from, to } = view.state.selection.main;
        view.dispatch({
          changes: { from, to, insert: text },
          selection: { anchor: from + text.length },
        });
        view.focus();
      },
    });
    return () => view?.destroy();
  });

  // An external change (a different file, a form edit, a discard) replaces the
  // document; an edit the user just made is already in the view, so it is a
  // no-op rather than a cursor-losing round trip.
  createEffect(
    () => props.value,
    (value) => {
      if (!view || value === view.state.doc.toString()) return;
      view.dispatch({ changes: { from: 0, to: view.state.doc.length, insert: value } });
    },
    { defer: true },
  );

  createEffect(
    () => props.language,
    (name) => void view?.dispatch({ effects: language.reconfigure(languageExtension(name)) }),
    { defer: true },
  );

  createEffect(
    theme,
    (current) => void view?.dispatch({ effects: appearance.reconfigure(editorTheme(current === "dark")) }),
    { defer: true },
  );

  createEffect(
    () => props.readOnly,
    (readOnly) =>
      void view?.dispatch({
        effects: editable.reconfigure([
          EditorView.editable.of(!readOnly),
          EditorState.readOnly.of(!!readOnly),
        ]),
      }),
    { defer: true },
  );

  return (
    <div
      class="relative w-full min-w-0 max-w-full overflow-hidden rounded-lg border border-line"
      style={{ height: props.minHeight ?? "24rem" }}
    >
      <div ref={host} class="h-full w-full min-w-0 max-w-full overflow-hidden" />
      <Show when={props.language}>
        <span class="pointer-events-none absolute right-2 top-2 z-10 rounded border border-line bg-surface/90 px-1.5 py-0.5 text-[0.625rem] uppercase tracking-wide text-faint">
          {props.language}
        </span>
      </Show>
    </div>
  );
}
