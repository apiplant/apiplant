/**
 * Full-text search over every guide, not just their titles.
 *
 * The index is built from `docs/*.md` at build time (see
 * `build/search-index.ts`) and shipped serialised; all this module does is
 * hand it to ZBSearch and ask questions. It is imported dynamically, so a
 * reader who never opens the search box never downloads it.
 */

import { create, load, search, type AnyZBSearch } from "zbsearch";
import { tokenizerComponents } from "./search-tokenizer";

/** One indexed section: a heading and the prose beneath it. */
export interface SearchSection {
  slug: string;
  doc: string;
  heading: string;
  anchor: string;
  text: string;
}

export interface SearchHit extends SearchSection {
  /** `/docs/queues#retries` — the section itself, not just its guide. */
  href: string;
  /** A window of the section's text around the first matching word. */
  snippet: string;
  /** The matched words, lowercased, for highlighting the snippet. */
  terms: string[];
}

const SCHEMA = {
  slug: "string",
  doc: "string",
  heading: "string",
  anchor: "string",
  text: "string",
} as const;

let db: Promise<AnyZBSearch> | null = null;

function database(): Promise<AnyZBSearch> {
  db ??= (async () => {
    const [{ default: raw }, components] = await Promise.all([
      import("virtual:search-index"),
      tokenizerComponents(),
    ]);
    // The index was built with these components; querying with any others
    // would tokenise the term differently and match nothing.
    const instance = create({ schema: SCHEMA, components });
    load(instance, JSON.parse(raw));
    return instance as AnyZBSearch;
  })();
  return db;
}

/** Start loading the index without searching yet — on focus, say. */
export function warmSearch(): void {
  void database();
}

const SNIPPET = 180;

/**
 * The query reduced to the word stems worth highlighting. The index is
 * stemmed, so "queueing" matches a section that says "queues" — the prefixes
 * here are what lets the snippet mark that word rather than nothing at all.
 */
function termsOf(query: string): string[] {
  return query
    .toLowerCase()
    .split(/[^\w.-]+/)
    .filter((word) => word.length > 1)
    .map((word) => (word.length > 5 ? word.slice(0, word.length - 2) : word));
}

/** `queue` → /\bqueue\w*\/gi: the stem, and whatever ending it was given. */
function matcher(terms: string[]): RegExp | null {
  if (terms.length === 0) return null;
  const escaped = terms.map((term) => term.replace(/[.*+?^${}()|[\]\\-]/g, "\\$&"));
  return new RegExp(`\\b(${escaped.join("|")})\\w*`, "gi");
}

/**
 * A window of `text` around its first match, cut at word boundaries. Falls
 * back to the opening of the section when the hit was on the heading alone.
 */
function snippetOf(text: string, terms: string[]): string {
  const at = matcher(terms)?.exec(text)?.index ?? -1;

  if (at < 0) return text.length > SNIPPET ? text.slice(0, SNIPPET).trimEnd() + "…" : text;

  let start = Math.max(0, at - Math.round(SNIPPET / 3));
  let end = Math.min(text.length, start + SNIPPET);
  if (start > 0) start = text.indexOf(" ", start) + 1 || start;
  if (end < text.length) end = text.lastIndexOf(" ", end) + 1 || end;

  return (start > 0 ? "…" : "") + text.slice(start, end).trim() + (end < text.length ? "…" : "");
}

/**
 * Search the guides. Matches in a heading count for more than matches in the
 * prose, and a guide's own title for more again, so "queues" finds the queues
 * guide before the paragraph in `functions.md` that mentions it.
 */
export async function searchDocs(query: string, limit = 12): Promise<SearchHit[]> {
  const term = query.trim();
  if (term.length < 2) return [];

  const instance = await database();
  const results = await search(instance, {
    term,
    properties: ["doc", "heading", "text"],
    boost: { doc: 3, heading: 2 },
    // A typo in one word of a longer query shouldn't empty the list.
    tolerance: term.length > 5 ? 1 : 0,
    limit,
    threshold: 0,
  });

  const terms = termsOf(term);

  return results.hits.map((hit) => {
    const section = hit.document as unknown as SearchSection;
    return {
      ...section,
      href: `/docs${section.slug ? `/${section.slug}` : ""}${section.anchor ? `#${section.anchor}` : ""}`,
      snippet: snippetOf(section.text, terms),
      terms,
    };
  });
}

/** The snippet split into matched and unmatched runs, for highlighting. */
export function highlightParts(snippet: string, terms: string[]): { text: string; hit: boolean }[] {
  const pattern = matcher(terms);
  if (!pattern) return [{ text: snippet, hit: false }];

  const parts: { text: string; hit: boolean }[] = [];
  let at = 0;
  for (const match of snippet.matchAll(pattern)) {
    if (match.index > at) parts.push({ text: snippet.slice(at, match.index), hit: false });
    parts.push({ text: match[0], hit: true });
    at = match.index + match[0].length;
  }
  if (at < snippet.length) parts.push({ text: snippet.slice(at), hit: false });
  return parts;
}
