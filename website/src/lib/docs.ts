/**
 * The documentation, rendered from `docs/*.md` at build time.
 *
 * Nothing here is a copy of the docs: the markdown files in the repository are
 * the only source, pulled in raw by Vite's glob import. Adding `docs/foo.md`
 * and one line to `SECTIONS` publishes it. Without the line the file still
 * appears under "More", since the nav is built from the glob and `SECTIONS`
 * only orders it.
 */

import MarkdownIt from "markdown-it";
import anchor from "markdown-it-anchor";
import { fromHighlighter } from "@shikijs/markdown-it/core";
import { createHighlighterCore } from "shiki/core";
import { createJavaScriptRegexEngine } from "shiki/engine/javascript";

/** Where a relative link that leaves `docs/` (`../examples/…`) should point. */
const REPO_BLOB = "https://github.com/apiplant/apiplant/blob/master/";

/** Raw markdown, keyed by path, loaded lazily so a doc is its own chunk. */
const SOURCES = import.meta.glob("@docs/*.md", {
  query: "?raw",
  import: "default",
}) as Record<string, () => Promise<string>>;

export interface DocMeta {
  /** URL segment: `configuration`, or `""` for the docs index. */
  slug: string;
  title: string;
  /** One line under the title in the nav and on the index. */
  summary: string;
  group: string;
}

export interface Heading {
  id: string;
  text: string;
  level: number;
}

export interface Doc extends DocMeta {
  html: string;
  headings: Heading[];
}

/**
 * The reading order, grouped. Titles and summaries live here rather than being
 * scraped from each file's first paragraph, since the nav needs a short phrase
 * while the documents begin with full sentences.
 */
const SECTIONS: { group: string; docs: [slug: string, title: string, summary: string][] }[] = [
  {
    group: "Start here",
    docs: [
      ["", "Overview", "What apiplant is, and the 60-second model of an app directory"],
      ["configuration", "Configuration", "main.toml reference: TLS, database, workers"],
      ["resources", "Resources", "Defining resources, field types, scope and migrations"],
      ["seed", "Seed data", "An app's initial rows, in TOML or CSV"],
    ],
  },
  {
    group: "Access",
    docs: [
      ["permissions", "Permissions", "The access model, per-action policies, ownership, roles"],
      ["multitenancy", "Multitenancy", "Organisations, memberships, automatic tenant isolation"],
      ["authentication", "Authentication", "Users, organisations, API keys, sessions"],
      ["relationships", "Relationships", "References, has_many, expansion, filtering, on_delete"],
    ],
  },
  {
    group: "Code",
    docs: [
      ["functions", "Functions", "Compiled plugins over the stable ABI, in any language"],
      ["hooks", "Lifecycle hooks", "Running functions before and after every CRUD operation"],
    ],
  },
  {
    group: "Services",
    docs: [
      ["email", "Sending email", "One [email] provider: SMTP, SES, SendGrid, Brevo and others"],
      ["caching", "Caching", "The optional Redis a function can reach"],
      ["queues", "Queues", "Background work on Postgres alone: publish, subscribe, retries"],
      ["storage", "File storage", "The file field type, on a directory or an S3-compatible bucket"],
      ["payments", "Payments", "Catalogue, subscriptions, checkout and tax"],
      ["ai", "AI", "Agents, streaming chat, ctx.chat and live action output"],
    ],
  },
  {
    group: "Operate",
    docs: [
      ["admin", "Admin dashboard", "The built-in operator UI and its [admin] config"],
      ["cli", "The console", "apiplant cli: the dashboard's functionality in a terminal"],
      ["security", "Security model", "What the server enforces, and what you must configure"],
      ["api-reference", "API reference", "Every endpoint, query parameter and status code"],
      ["openapi", "OpenAPI & Swagger UI", "The generated spec and interactive docs"],
    ],
  },
];

function slugOf(path: string): string {
  const name = path.slice(path.lastIndexOf("/") + 1).replace(/\.md$/, "");
  return name === "README" ? "" : name;
}

function titleCase(slug: string): string {
  const words = slug.replace(/-/g, " ");
  return words.charAt(0).toUpperCase() + words.slice(1);
}

/**
 * Every doc that exists on disk, in reading order. Files not listed in
 * `SECTIONS` are appended under "More", so a new guide is never hidden.
 */
export const DOCS: DocMeta[] = (() => {
  const available = new Set(Object.keys(SOURCES).map(slugOf));
  const listed: DocMeta[] = [];

  for (const section of SECTIONS) {
    for (const [slug, title, summary] of section.docs) {
      if (!available.delete(slug)) continue;
      listed.push({ slug, title, summary, group: section.group });
    }
  }

  for (const slug of [...available].sort()) {
    listed.push({ slug, title: titleCase(slug), summary: "", group: "More" });
  }

  return listed;
})();

export const DOC_GROUPS: { group: string; docs: DocMeta[] }[] = (() => {
  const groups: { group: string; docs: DocMeta[] }[] = [];
  for (const doc of DOCS) {
    const last = groups[groups.length - 1];
    if (last && last.group === doc.group) last.docs.push(doc);
    else groups.push({ group: doc.group, docs: [doc] });
  }
  return groups;
})();

export function findDoc(slug: string): DocMeta | undefined {
  return DOCS.find((doc) => doc.slug === slug);
}

/** The previous/next pair for the footer pager. */
export function neighbours(slug: string): { prev?: DocMeta; next?: DocMeta } {
  const index = DOCS.findIndex((doc) => doc.slug === slug);
  if (index < 0) return {};
  return { prev: DOCS[index - 1], next: DOCS[index + 1] };
}

// ---- rendering --------------------------------------------------------------

/**
 * The reading theme, as Tailwind utilities rather than a stylesheet of element
 * selectors: markdown-it hands every tag through a renderer rule, so the class
 * lists below are attached on the way out and the CSS file stays variables.
 */
const DOC_CLASSES: Record<string, string> = {
  h1: "group scroll-mt-24 text-3xl sm:text-4xl font-semibold tracking-tight text-ink leading-tight mt-0 mb-4",
  h2: "group scroll-mt-24 text-2xl font-semibold tracking-tight text-ink leading-snug mt-10 mb-3 pt-6 border-t border-line",
  h3: "group scroll-mt-24 text-lg font-semibold tracking-tight text-ink leading-snug mt-8 mb-2",
  h4: "group scroll-mt-24 text-base font-semibold text-ink mt-6 mb-2",
  h5: "group scroll-mt-24 text-sm font-semibold text-ink mt-5 mb-2",
  h6: "group scroll-mt-24 text-sm font-semibold text-faint mt-5 mb-2",
  p: "my-4 leading-7 text-muted [overflow-wrap:anywhere]",
  ul: "my-4 list-disc pl-6 marker:text-faint",
  ol: "my-4 list-decimal pl-6 marker:text-faint",
  li: "my-1.5 leading-7 [&>ul]:my-1.5 [&>ol]:my-1.5",
  blockquote:
    "my-5 rounded-r-lg border-l-[3px] border-accent-line bg-accent-soft px-4 py-3 [&>*:first-child]:mt-0 [&>*:last-child]:mb-0",
  a: "text-accent underline decoration-1 underline-offset-2 hover:text-accent-dim",
  strong: "font-semibold text-ink",
  em: "italic",
  hr: "my-8 border-0 border-t border-line",
  img: "max-w-full rounded-lg",
  // Not `whitespace-nowrap`: a long path or symbol in prose would otherwise
  // push the whole page wider than a phone screen.
  code_inline:
    "font-mono text-[0.85em] [overflow-wrap:anywhere] rounded-[0.3125rem] border border-line bg-surface-2 px-1.5 py-0.5 text-ink",
  table: "w-full min-w-[34rem] border-collapse text-sm",
  thead: "bg-surface-2",
  th: "border-b border-line px-3 py-2 text-left align-top font-semibold text-ink whitespace-nowrap",
  // Inside a table the wrapper scrolls, so a token is better kept whole than
  // broken across two lines in a column a phone has squeezed to nothing.
  td: "border-b border-line px-3 py-2 text-left align-top [&_code]:whitespace-nowrap",
  tr: "last:[&>td]:border-b-0",
};

/**
 * The grammars, by their own name. Shiki's default entry makes every language
 * it ships reachable — three hundred grammars — so the ones these docs use are
 * listed here instead, and loaded one document at a time.
 */
const GRAMMARS: Record<string, () => Promise<unknown>> = {
  toml: () => import("shiki/langs/toml.mjs"),
  rust: () => import("shiki/langs/rust.mjs"),
  bash: () => import("shiki/langs/bash.mjs"),
  json: () => import("shiki/langs/json.mjs"),
  http: () => import("shiki/langs/http.mjs"),
  c: () => import("shiki/langs/c.mjs"),
  sql: () => import("shiki/langs/sql.mjs"),
  typescript: () => import("shiki/langs/typescript.mjs"),
  javascript: () => import("shiki/langs/javascript.mjs"),
  go: () => import("shiki/langs/go.mjs"),
  zig: () => import("shiki/langs/zig.mjs"),
  yaml: () => import("shiki/langs/yaml.mjs"),
  python: () => import("shiki/langs/python.mjs"),
  diff: () => import("shiki/langs/diff.mjs"),
  ini: () => import("shiki/langs/ini.mjs"),
  csv: () => import("shiki/langs/csv.mjs"),
  log: () => import("shiki/langs/log.mjs"),
};

/** What a fence may be tagged with, when that is not the grammar's own name. */
const ALIASES: Record<string, string> = {
  rs: "rust",
  sh: "bash",
  shell: "bash",
  console: "bash",
  ts: "typescript",
  js: "javascript",
  yml: "yaml",
  jsonc: "json",
};

/**
 * A language for a fence that carries none.
 *
 * Two dozen blocks in the guides are directory trees, endpoint sketches, server
 * output and query-string examples — real content, written without a tag
 * because none of them is a program. Reading the first lines picks the grammar
 * that colours each usefully, so no block on the site is left grey.
 */
function guessLanguage(code: string): string {
  const lines = code.split("\n").filter((line) => line.trim().length > 0);
  if (lines.length === 0) return "text";
  const head = lines.slice(0, 6);

  // A JSON document, whole or excerpted.
  if (/^[[{]/.test(lines[0].trim())) return "json";

  // Shell sessions and directory trees: both are `#`-commented, and the tree
  // drawings read as comments and paths, which bash colours well.
  if (head.some((line) => /^\s*[$>] /.test(line))) return "bash";
  if (head.some((line) => /[├└│]/.test(line) || /^\S+\/$/.test(line.trim()))) return "bash";

  // Requests and SSE frames — `POST /path`, `event: delta`, `data: {…}`.
  if (head.some((line) => /^(GET|POST|PATCH|PUT|DELETE|HEAD|OPTIONS)\s+\S/.test(line.trim())))
    return "http";
  if (head.every((line) => /^[\w-]+:\s/.test(line.trim()))) return "http";

  // Server output: a level word, or a timestamp, at the start of the line.
  if (head.some((line) => /^\s*(INFO|WARN|ERROR|DEBUG|TRACE)\b/.test(line))) return "log";

  // `[section]` or `key = value` is configuration, whatever it is about.
  if (head.some((line) => /^\s*\[[\w.$-]+\]/.test(line) || /^\s*[\w.-]+\s*=\s*\S/.test(line)))
    return "toml";

  // A query string, a path, a `#`-commented line: bash again, for the comment.
  if (head.some((line) => /^\s*[?/#]/.test(line))) return "bash";

  // Whatever is left is program output or an error message. The log grammar
  // treats it as one — numbers, quoted strings and punctuation coloured, plain
  // words left alone — which is exactly right for a block of prose the server
  // printed, and never wrong enough to be worse than flat grey.
  return "log";
}

/** Fence tags that mean "no language", and so get a guess instead. */
const PLAIN = new Set(["", "text", "txt", "plain", "plaintext", "none"]);

let core: Awaited<ReturnType<typeof createHighlighterCore>> | null = null;
const loaded = new Set<string>();

/**
 * Load the grammars a piece of markdown asks for, and no others. A guide that
 * is all TOML never pays for the TypeScript grammar, which is twenty times its
 * size; `http` — the largest of the set, because it colours request bodies too —
 * only arrives with the API reference.
 */
/**
 * The grammar a fence should be highlighted with, or `null` for none we have.
 * An untagged (or `text`-tagged) block is read to see what it looks like.
 */
function grammarFor(tag: string, code: string): string | null {
  const wanted = PLAIN.has(tag) ? guessLanguage(code) : (ALIASES[tag] ?? tag);
  return GRAMMARS[wanted] ? wanted : null;
}

async function ensureLangs(source: string): Promise<void> {
  const wanted = new Set<string>();
  // Fence by fence, because an untagged one needs its body read to know which
  // grammar it wants.
  for (const match of source.matchAll(/^ {0,3}```+[ \t]*([\w-]*)[^\n]*\n([\s\S]*?)^ {0,3}```/gm)) {
    const name = grammarFor(match[1].toLowerCase(), match[2]);
    if (name && !loaded.has(name)) wanted.add(name);
  }
  if (wanted.size === 0 || !core) return;

  await Promise.all(
    [...wanted].map(async (name) => {
      await core!.loadLanguage((await GRAMMARS[name]()) as never);
      loaded.add(name);
    }),
  );
}

let renderer: Promise<MarkdownIt> | null = null;

/**
 * One markdown-it, built once. Shiki loads its grammars asynchronously, which
 * is the only reason this is a promise; after that rendering is synchronous.
 */
function markdown(): Promise<MarkdownIt> {
  if (renderer) return renderer;

  renderer = (async () => {
    const md = MarkdownIt({ html: true, linkify: true, typographer: false });

    core = await createHighlighterCore({
      themes: [import("shiki/themes/github-dark.mjs"), import("shiki/themes/github-light.mjs")],
      langs: [],
      // The JavaScript regex engine, rather than the wasm build of Oniguruma:
      // half a megabyte less, and these grammars all compile under it.
      engine: createJavaScriptRegexEngine({ forgiving: true }),
    });

    // The cast is shiki's generics only: `fromHighlighter` is typed for the
    // bundled highlighter, whose language union a core one doesn't carry.
    md.use(fromHighlighter(core as Parameters<typeof fromHighlighter>[0], {
      themes: { light: "github-light", dark: "github-dark" },
      // Both themes in one pass: the dark colours are inline and the light
      // ones ride along in a CSS variable that `:root.light` switches on.
      defaultColor: false,
      cssVariablePrefix: "--shiki-",
      // Deliberately no `fallbackLanguage`: the plugin reads the highlighter's
      // loaded languages once, when it is installed — before this module has
      // loaded a single grammar — and would rewrite every fence to the
      // fallback for ever after. The fence rule below names a language we know
      // is loaded instead, which is the same guarantee without the snapshot.
    }));

    // Every opening tag picks up its utilities here; `code_inline` is a leaf
    // rule and has to be wrapped separately, below.
    const openRule = (tokens: Parameters<MarkdownIt["renderer"]["renderToken"]>[0], index: number) => {
      const token = tokens[index];
      const classes = DOC_CLASSES[token.tag];
      if (classes) token.attrJoin("class", classes);
    };

    const defaultRender = md.renderer.renderToken.bind(md.renderer);
    md.renderer.renderToken = (tokens, index, options) => {
      const token = tokens[index];
      if (token.nesting >= 0) openRule(tokens, index);
      return defaultRender(tokens, index, options);
    };

    const defaultCodeInline = md.renderer.rules.code_inline;
    md.renderer.rules.code_inline = (tokens, index, options, env, self) => {
      tokens[index].attrJoin("class", DOC_CLASSES.code_inline);
      return defaultCodeInline
        ? defaultCodeInline(tokens, index, options, env, self)
        : `<code${self.renderAttrs(tokens[index])}>${md.utils.escapeHtml(tokens[index].content)}</code>`;
    };

    md.use(anchor, {
      level: [1, 2, 3, 4],
      slugify: (heading: string) =>
        heading
          .toLowerCase()
          .replace(/[^\w\s-]/g, "")
          .trim()
          .replace(/\s+/g, "-"),
      // A link that only appears when its heading is hovered: the prose stays
      // clean, and every section is still addressable.
      permalink: anchor.permalink.linkInsideHeader({
        symbol: "#",
        class:
          "float-left -ml-[1.1em] pr-[0.35em] text-accent no-underline opacity-0 transition-opacity " +
          "group-hover:opacity-100 focus-visible:opacity-100 max-md:hidden",
        placement: "before",
        ariaHidden: true,
      }),
    });

    // Shiki has already turned the fence into a coloured <pre>; it only needs
    // the block's own frame and its horizontal scroller.
    const shikiFence = md.renderer.rules.fence!;
    md.renderer.rules.fence = (tokens, index, options, env, self) => {
      const token = tokens[index];
      // `text` and friends say "this is not a program", not "leave it grey" —
      // and anything whose grammar isn't loaded falls back to plain text here,
      // because shiki throws on a language it does not have.
      token.info = grammarFor(token.info.trim().toLowerCase(), token.content) ?? "text";
      return shikiFence(tokens, index, options, env, self).replace(
        /^<pre class="/,
        '<pre class="my-5 overflow-x-auto rounded-[0.625rem] border border-line bg-code-bg! px-4 py-3.5 ' +
          'font-mono text-[0.8125rem] leading-relaxed [tab-size:4] ',
      );
    };

    const defaultImage = md.renderer.rules.image!;
    md.renderer.rules.image = (tokens, index, options, env, self) => {
      tokens[index].attrJoin("class", DOC_CLASSES.img);
      return defaultImage(tokens, index, options, env, self);
    };

    // Links: `configuration.md#tls` is a route on this site, anything that
    // climbs out of docs/ is a file on GitHub, and off-site links open away.
    const defaultLink =
      md.renderer.rules.link_open ??
      ((tokens, index, options, _env, self) => self.renderToken(tokens, index, options));

    md.renderer.rules.link_open = (tokens, index, options, env, self) => {
      const token = tokens[index];
      const href = token.attrGet("href");
      if (href) token.attrSet("href", rewriteHref(href));
      const rewritten = token.attrGet("href") ?? "";
      if (/^https?:/.test(rewritten)) {
        token.attrSet("target", "_blank");
        token.attrSet("rel", "noreferrer noopener");
      }
      return defaultLink(tokens, index, options, env, self);
    };

    // Wrap tables so a wide reference scrolls inside itself instead of
    // widening the page.
    md.renderer.rules.table_open = () =>
      `<div class="my-5 overflow-x-auto rounded-[0.625rem] border border-line"><table class="${DOC_CLASSES.table}">`;
    md.renderer.rules.table_close = () => "</table></div>";

    return md;
  })();

  return renderer;
}

/** `resources.md#fields` → `/docs/resources#fields`; `../examples/x` → GitHub. */
export function rewriteHref(href: string): string {
  if (/^(https?:|mailto:|#|\/)/.test(href)) return href;

  if (href.startsWith("../")) return REPO_BLOB + href.replace(/^(\.\.\/)+/, "");

  const match = /^\.?\/?([\w.-]+)\.md(#.*)?$/.exec(href);
  if (!match) return href;

  const slug = match[1] === "README" ? "" : match[1];
  const hash = match[2] ?? "";
  return `/docs${slug ? `/${slug}` : ""}${hash}`;
}

/** Every h2/h3 in the rendered document, for the on-page table of contents. */
function headingsOf(md: MarkdownIt, source: string): Heading[] {
  const headings: Heading[] = [];
  const tokens = md.parse(source, {});

  for (let index = 0; index < tokens.length; index++) {
    const token = tokens[index];
    if (token.type !== "heading_open") continue;
    const level = Number(token.tag.slice(1));
    if (level < 2 || level > 3) continue;
    const id = token.attrGet("id");
    const inline = tokens[index + 1];
    if (!id || !inline) continue;
    headings.push({ id, level, text: inline.content.replace(/`/g, "") });
  }

  return headings;
}

/**
 * One highlighted code block, for the pages that aren't documents. It goes
 * through the same markdown pipeline so the landing page's snippets and the
 * guides' are colored by the same themes.
 */
export async function highlight(code: string, lang: string): Promise<string> {
  const fenced = "```" + lang + "\n" + code.replace(/\n$/, "") + "\n```";
  const md = await markdown();
  await ensureLangs(fenced);
  return md.render(fenced);
}

const rendered = new Map<string, Doc>();

/** Load and render one guide. Results are cached for the session. */
export async function loadDoc(slug: string): Promise<Doc> {
  const cached = rendered.get(slug);
  if (cached) return cached;

  const meta = findDoc(slug);
  const path = Object.keys(SOURCES).find((candidate) => slugOf(candidate) === slug);
  if (!meta || !path) throw new Error(`No such guide: ${slug || "index"}`);

  const [md, source] = await Promise.all([markdown(), SOURCES[path]()]);
  await ensureLangs(source);

  // markdown-it's anchor ids are assigned during render, so the table of
  // contents has to be taken from the same pass that produced them.
  const html = md.render(source);
  const doc: Doc = { ...meta, html, headings: headingsOf(md, source) };
  rendered.set(slug, doc);
  return doc;
}
