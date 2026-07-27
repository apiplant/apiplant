/**
 * Editing markup: Markdown and HTML.
 *
 * A `text` field can declare what it holds (`[fields.x.admin] format =
 * "markdown"`). Nothing about storage changes — the API stores and returns the
 * same characters — but an operator writing a product description deserves
 * better than a grey box: the editor colours the markup and shows the rendered
 * result beside it, or behind a tab when the screen is too narrow for two
 * columns.
 *
 * Everything here is deliberately dependency-free. The renderer covers the
 * Markdown people actually type into a description, and the HTML preview goes
 * through a sanitiser, because the stored text is operator input and must never
 * be able to run script in the dashboard.
 */

import { Show, createEffect, createMemo, createSignal, onCleanup } from "solid-js";
import type { ContentFormat } from "./types";

// --- escaping and urls -----------------------------------------------------

const ESCAPES: Record<string, string> = {
  "&": "&amp;",
  "<": "&lt;",
  ">": "&gt;",
  '"': "&quot;",
};

function escapeHtml(text: string): string {
  return text.replace(/[&<>"]/g, (char) => ESCAPES[char]);
}

/** Only URLs that cannot execute anything. Everything else becomes inert. */
function safeUrl(raw: string): string {
  const url = raw.trim();
  if (/^(https?:|mailto:|tel:)/i.test(url)) return url;
  if (/^[./#?]/.test(url)) return url;
  if (/^[a-z][a-z0-9+.-]*:/i.test(url)) return "#";
  return url;
}

// --- markdown to html ------------------------------------------------------

/** Inline spans, applied to text that is already HTML-escaped. */
function inlineMarkdown(text: string): string {
  const code: string[] = [];
  // Code spans win over every other marker, so hold them aside first.
  let out = text.replace(/`([^`]+)`/g, (_match, body: string) => {
    code.push(`<code>${body}</code>`);
    return `\u0000${code.length - 1}\u0000`;
  });

  out = out
    .replace(
      /!\[([^\]]*)\]\(([^)\s]+)\)/g,
      (_m, alt: string, url: string) => `<img src="${safeUrl(url)}" alt="${alt}" />`,
    )
    .replace(
      /\[([^\]]+)\]\(([^)\s]+)\)/g,
      (_m, label: string, url: string) =>
        `<a href="${safeUrl(url)}" target="_blank" rel="noreferrer noopener">${label}</a>`,
    )
    .replace(/(\*\*|__)(?=\S)([\s\S]*?\S)\1/g, "<strong>$2</strong>")
    .replace(/(^|[^*\w])\*(?=\S)([^*]*?\S)\*/g, "$1<em>$2</em>")
    .replace(/(^|[^_\w])_(?=\S)([^_]*?\S)_/g, "$1<em>$2</em>")
    .replace(/~~(?=\S)([\s\S]*?\S)~~/g, "<del>$1</del>");

  return out.replace(/\u0000(\d+)\u0000/g, (_m, index: string) => code[Number(index)]);
}

/**
 * A small block-level Markdown renderer: headings, fenced code, quotes, lists,
 * rules and paragraphs. Raw HTML in the source stays escaped — someone writing
 * Markdown gets Markdown, not a second way to inject markup.
 */
export function renderMarkdown(source: string): string {
  const lines = source.replace(/\r\n?/g, "\n").split("\n");
  const html: string[] = [];
  let paragraph: string[] = [];
  let list: "ul" | "ol" | null = null;
  let quote: string[] | null = null;

  const closeParagraph = () => {
    if (!paragraph.length) return;
    html.push(`<p>${inlineMarkdown(paragraph.join("\n")).replace(/\n/g, "<br />")}</p>`);
    paragraph = [];
  };
  const closeList = () => {
    if (!list) return;
    html.push(`</${list}>`);
    list = null;
  };
  const closeQuote = () => {
    if (!quote) return;
    html.push(`<blockquote>${renderMarkdown(quote.join("\n"))}</blockquote>`);
    quote = null;
  };
  const closeAll = () => {
    closeParagraph();
    closeList();
    closeQuote();
  };

  for (let index = 0; index < lines.length; index += 1) {
    const line = lines[index];

    const fence = /^\s*```+\s*([\w-]*)\s*$/.exec(line);
    if (fence) {
      closeAll();
      const body: string[] = [];
      index += 1;
      while (index < lines.length && !/^\s*```+\s*$/.test(lines[index])) {
        body.push(lines[index]);
        index += 1;
      }
      html.push(`<pre><code>${escapeHtml(body.join("\n"))}</code></pre>`);
      continue;
    }

    if (/^\s*$/.test(line)) {
      closeAll();
      continue;
    }

    const quoted = /^\s*>\s?(.*)$/.exec(line);
    if (quoted) {
      closeParagraph();
      closeList();
      quote = quote ?? [];
      quote.push(quoted[1]);
      continue;
    }
    closeQuote();

    if (/^\s*(?:-{3,}|\*{3,}|_{3,})\s*$/.test(line)) {
      closeAll();
      html.push("<hr />");
      continue;
    }

    const heading = /^\s*(#{1,6})\s+(.*?)\s*#*\s*$/.exec(line);
    if (heading) {
      closeAll();
      const level = heading[1].length;
      html.push(`<h${level}>${inlineMarkdown(escapeHtml(heading[2]))}</h${level}>`);
      continue;
    }

    const bullet = /^\s*[-*+]\s+(.*)$/.exec(line);
    const numbered = /^\s*\d+[.)]\s+(.*)$/.exec(line);
    if (bullet || numbered) {
      closeParagraph();
      const wanted = bullet ? "ul" : "ol";
      if (list !== wanted) {
        closeList();
        html.push(`<${wanted}>`);
        list = wanted;
      }
      html.push(`<li>${inlineMarkdown(escapeHtml((bullet ?? numbered)![1]))}</li>`);
      continue;
    }
    closeList();

    paragraph.push(escapeHtml(line));
  }

  closeAll();
  return html.join("\n");
}

// --- html sanitising -------------------------------------------------------

const BANNED_TAGS = new Set([
  "SCRIPT",
  "STYLE",
  "IFRAME",
  "FRAME",
  "FRAMESET",
  "OBJECT",
  "EMBED",
  "LINK",
  "META",
  "BASE",
  "FORM",
  "INPUT",
  "BUTTON",
  "TEXTAREA",
  "SELECT",
]);

const URL_ATTRIBUTES = new Set(["href", "src", "action", "poster", "cite"]);

/**
 * Strip anything that could execute or navigate somewhere unexpected.
 *
 * The preview renders text an operator typed, which is exactly the input a
 * stored-XSS lives in; a permissive preview would run it inside a session that
 * can edit every record.
 */
export function sanitizeHtml(source: string): string {
  const doc = new DOMParser().parseFromString(source, "text/html");

  const walk = (node: Element) => {
    for (const child of Array.from(node.children)) {
      if (BANNED_TAGS.has(child.tagName)) {
        child.remove();
        continue;
      }
      for (const attribute of Array.from(child.attributes)) {
        const name = attribute.name.toLowerCase();
        if (name.startsWith("on") || name === "srcdoc") {
          child.removeAttribute(attribute.name);
          continue;
        }
        if (URL_ATTRIBUTES.has(name)) child.setAttribute(attribute.name, safeUrl(attribute.value));
      }
      if (child.tagName === "A") {
        child.setAttribute("target", "_blank");
        child.setAttribute("rel", "noreferrer noopener");
      }
      walk(child);
    }
  };

  walk(doc.body);
  return doc.body.innerHTML;
}

export function renderPreview(source: string, format: ContentFormat): string {
  if (format === "html") return sanitizeHtml(source);
  return renderMarkdown(source);
}

// --- highlighting ----------------------------------------------------------

function highlightMarkdown(source: string): string {
  const lines = escapeHtml(source).split("\n");
  let fenced = false;

  return lines
    .map((line) => {
      if (/^\s*```/.test(line)) {
        fenced = !fenced;
        return `<span class="tok-code">${line}</span>`;
      }
      if (fenced) return `<span class="tok-code">${line}</span>`;
      if (/^\s*(?:-{3,}|\*{3,}|_{3,})\s*$/.test(line)) return `<span class="tok-mark">${line}</span>`;
      if (/^\s*#{1,6}\s/.test(line)) return `<span class="tok-head">${line}</span>`;
      if (/^\s*&gt;/.test(line)) return `<span class="tok-quote">${line}</span>`;

      const marker = /^(\s*)([-*+]|\d+[.)])(\s+)/.exec(line);
      const rest = marker ? line.slice(marker[0].length) : line;
      const prefix = marker ? `${marker[1]}<span class="tok-mark">${marker[2]}</span>${marker[3]}` : "";
      return prefix + inlineHighlight(rest);
    })
    .join("\n");
}

/** Inline markers, on already-escaped text. */
function inlineHighlight(text: string): string {
  return text
    .replace(/`[^`]+`/g, (match) => `<span class="tok-code">${match}</span>`)
    .replace(/(\*\*|__)(?=\S)[\s\S]*?\S\1/g, (match) => `<span class="tok-strong">${match}</span>`)
    .replace(/(^|[^*\w<])(\*(?=\S)[^*]*?\S\*)/g, (_m, before: string, body: string) =>
      `${before}<span class="tok-em">${body}</span>`,
    )
    .replace(
      /(!?\[[^\]]*\])(\([^)\s]*\))/g,
      (_m, label: string, url: string) =>
        `<span class="tok-link">${label}</span><span class="tok-url">${url}</span>`,
    );
}

function highlightHtmlSource(source: string): string {
  const comments: string[] = [];
  let text = escapeHtml(source).replace(/&lt;!--[\s\S]*?--&gt;/g, (match) => {
    comments.push(`<span class="tok-comment">${match}</span>`);
    return `\u0000${comments.length - 1}\u0000`;
  });

  text = text.replace(/&lt;\/?[a-zA-Z][^]*?&gt;/g, (tag) => {
    // One pass over the tag, so a class name injected for the tag itself is
    // never re-read as an attribute of the markup being highlighted.
    const inner = tag.replace(
      /(&lt;\/?)([a-zA-Z][\w:-]*)|([\w:-]+)(?==)|(&quot;[\s\S]*?&quot;)/g,
      (match, open: string, name: string, attribute: string, value: string) => {
        if (name) return `${open}<span class="tok-tag">${name}</span>`;
        if (attribute) return `<span class="tok-attr">${attribute}</span>`;
        if (value) return `<span class="tok-str">${value}</span>`;
        return match;
      },
    );
    return `<span class="tok-punct">${inner}</span>`;
  });

  return text.replace(/\u0000(\d+)\u0000/g, (_m, index: string) => comments[Number(index)]);
}

export function highlight(source: string, format: ContentFormat): string {
  // A trailing space keeps the final (often empty) line tall enough to sit
  // under the caret.
  const text = format === "html" ? highlightHtmlSource(source) : highlightMarkdown(source);
  return `${text} `;
}

// --- the editor ------------------------------------------------------------

/** Whether there is room to show the editor and the preview side by side. */
function useWideScreen(): () => boolean {
  const query = window.matchMedia("(min-width: 900px)");
  const [wide, setWide] = createSignal(query.matches);
  const onChange = (event: MediaQueryListEvent) => setWide(event.matches);
  query.addEventListener("change", onChange);
  onCleanup(() => query.removeEventListener("change", onChange));
  return wide;
}

/**
 * A textarea with the markup coloured behind it and the rendered result beside
 * it — one column each on a wide screen, two tabs when there is no room.
 *
 * The colouring is a `<pre>` sitting under a transparent-text textarea, which
 * is the only way to keep native editing (undo, spellcheck, the mobile
 * keyboard) and still show syntax. The two must share every metric that
 * affects layout; see `.markup-input` and `.markup-layer`.
 */
export function MarkupEditor(props: {
  value: string;
  format: ContentFormat;
  onChange: (next: string) => void;
  disabled?: boolean;
  placeholder?: string;
}) {
  const wide = useWideScreen();
  const [tab, setTab] = createSignal<"write" | "preview">("write");
  const [text, setText] = createSignal(props.value);

  let layer: HTMLPreElement | undefined;
  let input: HTMLTextAreaElement | undefined;

  // The draft is a plain object, so a reset elsewhere (a cancelled edit, a
  // record swap) has to be picked up here.
  createEffect(() => setText(props.value));

  const preview = createMemo(() => renderPreview(text(), props.format));
  const coloured = createMemo(() => highlight(text(), props.format));

  const syncScroll = () => {
    if (!layer || !input) return;
    layer.scrollTop = input.scrollTop;
    layer.scrollLeft = input.scrollLeft;
  };

  const showEditor = () => wide() || tab() === "write";
  const showPreview = () => wide() || tab() === "preview";
  const label = () => (props.format === "html" ? "HTML" : "Markdown");

  return (
    <div class="markup">
      <div class="markup-head">
        <span class="markup-format">{label()}</span>
        <Show when={!wide()}>
          <div class="markup-tabs">
            <button
              type="button"
              class={`markup-tab ${tab() === "write" ? "is-active" : ""}`}
              onClick={() => setTab("write")}
            >
              Write
            </button>
            <button
              type="button"
              class={`markup-tab ${tab() === "preview" ? "is-active" : ""}`}
              onClick={() => setTab("preview")}
            >
              Preview
            </button>
          </div>
        </Show>
      </div>

      <div class={`markup-body ${wide() ? "is-split" : ""}`}>
        <Show when={showEditor()}>
          <div class="markup-editor">
            <pre class="markup-layer" aria-hidden="true" ref={layer} innerHTML={coloured()} />
            <textarea
              ref={input}
              class="markup-input"
              spellcheck={props.format !== "html"}
              disabled={props.disabled}
              placeholder={props.placeholder}
              value={text()}
              onScroll={syncScroll}
              onInput={(event) => {
                setText(event.currentTarget.value);
                props.onChange(event.currentTarget.value);
                syncScroll();
              }}
            />
          </div>
        </Show>
        <Show when={showPreview()}>
          <div class="markup-preview">
            <Show when={text().trim()} fallback={<p class="markup-empty">Nothing to preview yet.</p>}>
              <div class="prose" innerHTML={preview()} />
            </Show>
          </div>
        </Show>
      </div>
    </div>
  );
}

/** The same rendering, for read-only screens. */
export function MarkupView(props: { value: string; format: ContentFormat }) {
  const html = createMemo(() => renderPreview(props.value, props.format));
  return <div class="prose" innerHTML={html()} />;
}
