/**
 * The documentation search index, built once at build time.
 *
 * Every guide in `docs/` is split at its headings into sections, each section
 * becomes a document in a ZBSearch index, and the whole index is serialised
 * into the virtual module `virtual:search-index`. The browser never tokenises
 * anything: it loads the finished index and calls `search`.
 *
 * The index is a build input, not a checked-in artefact — editing a guide
 * rebuilds it, in `vite build` and in the dev server alike.
 */

import { readFileSync, readdirSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { join } from "node:path";
import type { Plugin } from "vite";
import { create, insertMultiple, save } from "zbsearch";
import { tokenizerComponents } from "../src/lib/search-tokenizer.ts";

const VIRTUAL_ID = "virtual:search-index";
const RESOLVED_ID = "\0" + VIRTUAL_ID;

const DOCS_DIR = fileURLToPath(new URL("../../docs", import.meta.url));

/** Mirrors `SearchSection` in `src/lib/search.ts`. */
export const SEARCH_SCHEMA = {
  /** The guide's URL segment; `""` for the docs index. */
  slug: "string",
  /** The guide's own title — the h1 of the file. */
  doc: "string",
  /** The nearest heading above this text, empty for a file's preamble. */
  heading: "string",
  /** The heading's anchor id, for linking straight to the section. */
  anchor: "string",
  text: "string",
} as const;

interface Section {
  slug: string;
  doc: string;
  heading: string;
  anchor: string;
  text: string;
}

/** The same slugify markdown-it-anchor is configured with in `lib/docs.ts`. */
function slugify(heading: string): string {
  return heading
    .toLowerCase()
    .replace(/[^\w\s-]/g, "")
    .trim()
    .replace(/\s+/g, "-");
}

/**
 * Markdown reduced to the words a reader would search for. Fences keep their
 * body — `ctx.chat` and `on_delete` are looked up far more often than any
 * sentence — but lose the backticks and the language tag, and link syntax
 * keeps its text rather than its URL.
 */
function toPlainText(markdown: string): string {
  return markdown
    .replace(/^ {0,3}```+[^\n]*\n([\s\S]*?)^ {0,3}```[^\n]*$/gm, "$1")
    .replace(/<!--[\s\S]*?-->/g, "")
    .replace(/!\[([^\]]*)\]\([^)]*\)/g, "$1")
    .replace(/\[([^\]]*)\]\([^)]*\)/g, "$1")
    .replace(/^\s{0,3}>\s?/gm, "")
    .replace(/^\s*\|/gm, " ")
    .replace(/[|`*_~]/g, " ")
    .replace(/[ \t]+/g, " ")
    .replace(/\n{2,}/g, "\n")
    .trim();
}

/** A file's h1, or its slug title-cased when it has none. */
function documentTitle(source: string, slug: string): string {
  const heading = /^#\s+(.+)$/m.exec(source);
  if (heading) return heading[1].replace(/[`*]/g, "").trim();
  const words = (slug || "overview").replace(/-/g, " ");
  return words.charAt(0).toUpperCase() + words.slice(1);
}

/**
 * One guide, cut into searchable sections at every h2/h3 — the same levels the
 * on-page contents lists, so every hit has somewhere to link to. Deeper
 * headings stay with their section: a match on `h4` still lands the reader on
 * the right screenful.
 */
function sectionsOf(source: string, slug: string): Section[] {
  const doc = documentTitle(source, slug);
  const sections: Section[] = [];

  // Fences first: a `#` at the start of a line inside a shell block is a
  // comment, not a heading, and must not cut the document in half.
  const fenced = source.replace(/^ {0,3}(```+)[^\n]*\n[\s\S]*?^ {0,3}\1[^\n]*$/gm, (block) =>
    block.replace(/^#/gm, " "),
  );

  let heading = "";
  let anchor = "";
  let buffer: string[] = [];

  const flush = () => {
    const text = toPlainText(buffer.join("\n"));
    buffer = [];
    if (!text) return;
    sections.push({ slug, doc, heading, anchor, text });
  };

  for (const line of fenced.split("\n")) {
    const match = /^(#{1,3})\s+(.+?)\s*$/.exec(line);
    if (!match) {
      buffer.push(line);
      continue;
    }
    flush();
    const title = match[2].replace(/[`*]/g, "").trim();
    // The h1 names the document, and its anchor is the top of the page.
    heading = match[1].length === 1 ? "" : title;
    anchor = match[1].length === 1 ? "" : slugify(match[2]);
  }
  flush();

  return sections;
}

function collectSections(): Section[] {
  const files = readdirSync(DOCS_DIR).filter((name) => name.endsWith(".md"));
  const sections: Section[] = [];

  for (const file of files.sort()) {
    const name = file.replace(/\.md$/, "");
    const slug = name === "README" ? "" : name;
    sections.push(...sectionsOf(readFileSync(join(DOCS_DIR, file), "utf8"), slug));
  }

  return sections;
}

/** The serialised index, as the JSON the browser will hand to `load`. */
async function buildIndex(): Promise<string> {
  const db = create({ schema: SEARCH_SCHEMA, components: await tokenizerComponents() });
  await insertMultiple(db, collectSections() as never);
  return JSON.stringify(save(db));
}

export function searchIndexPlugin(): Plugin {
  let cached: Promise<string> | null = null;

  return {
    name: "apiplant-search-index",

    resolveId(id) {
      return id === VIRTUAL_ID ? RESOLVED_ID : undefined;
    },

    async load(id) {
      if (id !== RESOLVED_ID) return undefined;
      cached ??= buildIndex();
      // A string literal, not an object: parsing JSON is faster than letting
      // the JS engine evaluate an object literal this size, and it keeps the
      // index out of the module graph's own analysis.
      return `export default ${JSON.stringify(await cached)};`;
    },

    // In `vite dev`, editing a guide rebuilds the index and reloads the page.
    handleHotUpdate(context) {
      if (!context.file.startsWith(DOCS_DIR) || !context.file.endsWith(".md")) return;
      cached = null;
      const module = context.server.moduleGraph.getModuleById(RESOLVED_ID);
      if (module) context.server.moduleGraph.invalidateModule(module);
    },
  };
}
