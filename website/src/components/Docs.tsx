import {
  For,
  Show,
  createEffect,
  createMemo,
  createSignal,
  Errored,
  latest,
  onCleanup,
} from "solid-js";
import { useLocation, useNavigate, useParams } from "@solidjs/router";
import { DOCS, DOC_GROUPS, findDoc, loadDoc, neighbours, type DocMeta } from "../lib/docs";
import { highlightParts, searchDocs, warmSearch, type SearchHit } from "../lib/search";
import { GITHUB_URL, STUDIO_URL } from "../lib/links";

/**
 * The results of a full-text query, section by section. The index is the one
 * built from `docs/` at build time, so this is a lookup in memory rather than
 * a request, and every word of every guide is reachable — not only the titles
 * the nav lists.
 */
function SearchResults(props: { hits: SearchHit[]; query: string; onNavigate?: () => void }) {
  return (
    <Show
      when={props.hits.length > 0}
      fallback={<p class="px-2 py-4 text-xs text-faint">Nothing matches “{props.query}”.</p>}
    >
      <ul class="grid gap-0.5">
        <For each={props.hits}>
          {(hit) => (
            <li>
              <a
                href={hit.href}
                onClick={() => props.onNavigate?.()}
                class="block rounded-lg px-2 py-2 transition-colors hover:bg-surface-2"
              >
                <span class="flex items-baseline gap-1 text-[0.8125rem] font-medium text-ink">
                  <span class="truncate">{hit.heading || hit.doc}</span>
                  <Show when={hit.heading}>
                    <span class="shrink-0 text-[0.6875rem] font-normal text-faint">
                      in {hit.doc}
                    </span>
                  </Show>
                </span>
                <span class="mt-0.5 block text-[0.75rem] leading-5 text-faint [overflow-wrap:anywhere]">
                  <For each={highlightParts(hit.snippet, hit.terms)}>
                    {(part) => (
                      <Show when={part.hit} fallback={part.text}>
                        <mark class="bg-transparent font-medium text-accent">{part.text}</mark>
                      </Show>
                    )}
                  </For>
                </span>
              </a>
            </li>
          )}
        </For>
      </ul>
    </Show>
  );
}

/** The nav, with search: a plain list is unwieldy at nineteen guides. */
function DocsNav(props: { onNavigate?: () => void }) {
  const params = useParams();
  const [query, setQuery] = createSignal("");
  // The typed value, settled: searching on every keystroke would re-render the
  // list faster than it can be read, and the first keystroke also has to wait
  // for the index chunk.
  const [settled, setSettled] = createSignal("");
  let timer: ReturnType<typeof setTimeout> | undefined;
  onCleanup(() => clearTimeout(timer));

  const onInput = (value: string) => {
    setQuery(value);
    clearTimeout(timer);
    timer = setTimeout(() => setSettled(value.trim()), 120);
  };

  const hitsResource = createMemo(async () => {
    const term = settled().length > 1 ? settled() : null;
    return term ? searchDocs(term) : undefined;
  });
  // `latest` keeps the previous results on screen while the next query runs,
  // rather than the whole nav dropping to the boundary's fallback.
  const hits = () => latest(hitsResource);

  const searching = () => query().trim().length > 1;
  const current = () => params.slug ?? "";

  return (
    <div class="flex h-full flex-col gap-4">
      <label class="relative block">
        <span class="sr-only">Search the documentation</span>
        <svg
          viewBox="0 0 24 24"
          class="pointer-events-none absolute left-2.5 top-1/2 h-3.5 w-3.5 -translate-y-1/2 text-faint"
          fill="none"
          stroke="currentColor"
          stroke-width="2"
          aria-hidden="true"
        >
          <circle cx="11" cy="11" r="6" />
          <path stroke-linecap="round" d="m20 20-3.5-3.5" />
        </svg>
        <input
          type="search"
          value={query()}
          onInput={(event) => onInput(event.currentTarget.value)}
          onFocus={warmSearch}
          placeholder="Search the docs"
          class="w-full rounded-lg border border-line bg-surface-2 py-1.5 pl-8 pr-2.5 text-[0.8125rem] text-ink transition-colors placeholder:text-faint hover:border-line-strong focus:border-accent focus:bg-surface focus:outline-none"
        />
      </label>

      <nav class="min-h-0 flex-1 overflow-y-auto pb-8">
        <Show when={!searching()} fallback={
          <Show
            when={hits()}
            fallback={<p class="px-2 py-4 text-xs text-faint">Searching…</p>}
          >
            {(found) => (
              <SearchResults hits={found()} query={query().trim()} onNavigate={props.onNavigate} />
            )}
          </Show>
        }>
          <For each={DOC_GROUPS}>
            {(group) => (
              <div class="mb-5">
                <h2 class="px-2 text-[0.6875rem] font-semibold uppercase tracking-[0.12em] text-faint">
                  {group.group}
                </h2>
                <ul class="mt-1.5 grid gap-0.5">
                  <For each={group.docs}>
                    {(doc) => (
                      <li>
                        <a
                          href={`/docs${doc.slug ? `/${doc.slug}` : ""}`}
                          onClick={() => props.onNavigate?.()}
                          class={`block rounded-lg px-2 py-1.5 text-[0.8125rem] transition-colors ${
                            current() === doc.slug
                              ? "bg-accent-soft font-medium text-accent"
                              : "text-muted hover:bg-surface-2 hover:text-ink"
                          }`}
                        >
                          {doc.title}
                        </a>
                      </li>
                    )}
                  </For>
                </ul>
              </div>
            )}
          </For>
        </Show>
      </nav>
    </div>
  );
}

/**
 * The on-page contents, with the heading nearest the top highlighted. An
 * IntersectionObserver over the rendered headings suffices, so there is no
 * scroll handler, and it re-arms whenever the document changes.
 */
function Contents(props: { headings: { id: string; text: string; level: number }[] }) {
  const [active, setActive] = createSignal<string | null>(null);

  createEffect(
    () => props.headings.map((heading) => heading.id),
    (ids) => {
      if (ids.length === 0) return;

      const visible = new Set<string>();
      const observer = new IntersectionObserver(
        (entries) => {
          for (const entry of entries) {
            if (entry.isIntersecting) visible.add(entry.target.id);
            else visible.delete(entry.target.id);
          }
          const first = ids.find((id) => visible.has(id));
          if (first) setActive(first);
        },
        { rootMargin: "-88px 0px -70% 0px", threshold: 0 },
      );

      for (const id of ids) {
        const element = document.getElementById(id);
        if (element) observer.observe(element);
      }

      return () => observer.disconnect();
    },
  );

  return (
    <Show when={props.headings.length > 1}>
      <div class="sticky top-20 hidden max-h-[calc(100vh-6rem)] overflow-y-auto xl:block">
        <h2 class="text-[0.6875rem] font-semibold uppercase tracking-[0.12em] text-faint">
          On this page
        </h2>
        <ul class="mt-3 grid gap-0.5 border-l border-line">
          <For each={props.headings}>
            {(heading) => (
              <li>
                <a
                  href={`#${heading.id}`}
                  class={`-ml-px block border-l py-1 text-[0.8125rem] leading-snug transition-colors ${
                    heading.level === 3 ? "pl-6" : "pl-3"
                  } ${
                    active() === heading.id
                      ? "border-accent text-accent"
                      : "border-transparent text-faint hover:border-line-strong hover:text-muted"
                  }`}
                >
                  {heading.text}
                </a>
              </li>
            )}
          </For>
        </ul>
      </div>
    </Show>
  );
}

function Pager(props: { slug: string }) {
  const around = createMemo(() => neighbours(props.slug));

  const link = (doc: DocMeta, side: "prev" | "next") => (
    <a
      href={`/docs${doc.slug ? `/${doc.slug}` : ""}`}
      class={`group flex flex-col gap-1 rounded-xl border border-line bg-surface p-4 transition-colors hover:border-line-strong hover:bg-surface-2 ${
        side === "next" ? "text-right sm:col-start-2" : ""
      }`}
    >
      <span class="text-[0.6875rem] font-semibold uppercase tracking-[0.12em] text-faint">
        {side === "prev" ? "Previous" : "Next"}
      </span>
      <span class="text-sm font-medium text-ink">{doc.title}</span>
    </a>
  );

  return (
    <Show when={around().prev || around().next}>
      <div class="mt-14 grid gap-3 border-t border-line pt-8 sm:grid-cols-2">
        <Show when={around().prev}>{(prev) => link(prev(), "prev")}</Show>
        <Show when={around().next}>{(next) => link(next(), "next")}</Show>
      </div>
    </Show>
  );
}

/**
 * A screenshot, full screen. The guides show them at column width, which is
 * narrower than the application they picture, so every one of them is worth a
 * closer look; Escape, or a click anywhere, closes it again.
 */
function Lightbox(props: { src: string; alt: string; onClose: () => void }) {
  createEffect(
    () => props.src,
    () => {
      const onKey = (event: KeyboardEvent) => {
        if (event.key === "Escape") props.onClose();
      };
      window.addEventListener("keydown", onKey);
      return () => window.removeEventListener("keydown", onKey);
    },
  );

  return (
    <div
      class="fixed inset-0 z-[60] flex cursor-zoom-out items-center justify-center bg-canvas/85 p-4 backdrop-blur-sm sm:p-10"
      onClick={props.onClose}
      role="dialog"
      aria-modal="true"
      aria-label={props.alt || "Screenshot"}
    >
      <img
        src={props.src}
        alt={props.alt}
        class="max-h-full max-w-full rounded-lg border border-line shadow-2xl"
      />
      <button
        type="button"
        onClick={props.onClose}
        aria-label="Close image"
        class="absolute right-4 top-4 inline-flex h-9 w-9 items-center justify-center rounded-lg border border-line bg-surface text-muted hover:text-ink"
      >
        <svg viewBox="0 0 24 24" class="h-4 w-4" fill="none" stroke="currentColor" stroke-width="2">
          <path stroke-linecap="round" d="M6 6l12 12M18 6L6 18" />
        </svg>
      </button>
    </div>
  );
}

function Article(props: { slug: string }) {
  const navigate = useNavigate();
  const location = useLocation();
  const [zoomed, setZoomed] = createSignal<{ src: string; alt: string } | null>(null);
  const docResource = createMemo(async () => loadDoc(props.slug));
  const doc = () => latest(docResource);

  const sourceFile = () => `docs/${props.slug || "README"}.md`;

  // Rendered links are plain <a href="/docs/…">; catching them here keeps the
  // router in charge instead of reloading the whole app.
  const intercept = (event: MouseEvent) => {
    if (event.defaultPrevented || event.button !== 0 || event.metaKey || event.ctrlKey || event.shiftKey)
      return;
    const target = event.target as HTMLElement;
    // A screenshot outside a link opens full screen instead of navigating.
    if (target instanceof HTMLImageElement && !target.closest("a")) {
      event.preventDefault();
      setZoomed({ src: target.currentSrc || target.src, alt: target.alt });
      return;
    }
    const anchor = target.closest("a");
    const href = anchor?.getAttribute("href");
    if (!anchor || !href || !href.startsWith("/")) return;
    event.preventDefault();
    navigate(href);
  };

  // A fresh document starts at the top; an in-page anchor is honoured instead.
  //
  // On a cold load the guide's markup is inserted in the same pass that runs
  // this, so the anchor does not exist yet the first time we look — which is
  // why a link opened directly at `/docs/configuration#docs` used to land at
  // the top of the page. Look again over the next few frames before deciding
  // the fragment names nothing.
  createEffect(
    () => [doc(), location.hash] as const,
    ([loaded, hash]) => {
      if (!loaded) return;
      const fragment = decodeURIComponent(hash.slice(1));
      if (!fragment) {
        window.scrollTo({ top: 0 });
        return;
      }
      let frame = 0;
      const settle = (attempt: number) => {
        const target = document.getElementById(fragment);
        if (target) return target.scrollIntoView();
        if (attempt < 5) frame = requestAnimationFrame(() => settle(attempt + 1));
        else window.scrollTo({ top: 0 });
      };
      settle(0);
      return () => cancelAnimationFrame(frame);
    },
  );

  createEffect(
    () => findDoc(props.slug),
    (meta) => {
      document.title = meta ? `${meta.title} — apiplant docs` : "apiplant docs";
    },
  );

  return (
    <div class="mx-auto grid w-full max-w-6xl gap-10 px-5 py-10 xl:grid-cols-[minmax(0,1fr)_14rem]">
      <article class="min-w-0">
        {/* A guide that fails to load has no page to show, so the boundary
            carries the "no such file" message the read would otherwise throw. */}
        <Errored
          fallback={
            <div class="rounded-xl border border-line bg-surface p-8">
              <h1 class="text-xl font-semibold text-ink">That guide doesn't exist</h1>
              <p class="mt-2 text-sm text-muted">
                There is no <code class="font-mono">{sourceFile()}</code> in the repository.
              </p>
              <a href="/docs" class="mt-4 inline-block text-sm text-accent hover:text-accent-dim">
                Back to the documentation
              </a>
            </div>
          }
        >
        <Show
          when={doc()}
          fallback={
            <>
              {/* A page-shaped placeholder while the chunk and its highlighter load. */}
              <div class="animate-pulse">
                <div class="h-9 w-2/3 rounded-lg bg-surface-2" />
                <div class="mt-6 grid gap-3">
                  <div class="h-4 w-full rounded bg-surface-2" />
                  <div class="h-4 w-11/12 rounded bg-surface-2" />
                  <div class="h-4 w-9/12 rounded bg-surface-2" />
                </div>
                <div class="mt-8 h-40 w-full rounded-xl bg-surface-2" />
              </div>
            </>
          }
        >
          {(loaded) => (
            <>
              <div class="mb-6 flex flex-wrap items-center gap-2 text-xs text-faint">
                <a href="/docs" class="hover:text-ink">
                  Documentation
                </a>
                <span aria-hidden="true">/</span>
                <span class="text-muted">{loaded().title}</span>
              </div>

              <div
                class="max-w-none text-[0.9375rem] leading-7 text-muted"
                onClick={intercept}
                innerHTML={loaded().html}
              />

              <div class="mt-12 flex flex-wrap items-center justify-between gap-3 text-xs text-faint">
                <span>
                  Rendered from <code class="font-mono">{sourceFile()}</code>
                </span>
                <a
                  href={`${GITHUB_URL}/blob/master/${sourceFile()}`}
                  target="_blank"
                  rel="noreferrer noopener"
                  class="text-accent hover:text-accent-dim"
                >
                  Edit this page on GitHub
                </a>
              </div>

              <Pager slug={props.slug} />
            </>
          )}
        </Show>
        </Errored>
      </article>

      <Show when={doc()}>{(loaded) => <Contents headings={loaded().headings} />}</Show>

      <Show when={zoomed()}>
        {(image) => (
          <Lightbox src={image().src} alt={image().alt} onClose={() => setZoomed(null)} />
        )}
      </Show>
    </div>
  );
}

/**
 * Whether the mobile nav drawer is open. It lives here rather than in the
 * route because the button that opens it is in the header, above the router.
 */
const [drawerOpen, setDrawerOpen] = createSignal(false);
export { drawerOpen, setDrawerOpen };

/** The docs shell: a sidebar that becomes a drawer, and the article. */
export function DocsPage() {
  const params = useParams();
  const slug = () => params.slug ?? "";
  const closeDrawer = () => setDrawerOpen(false);

  return (
    <div class="mx-auto flex w-full max-w-6xl lg:px-5">
      <aside class="sticky top-14 hidden h-[calc(100vh-3.5rem)] w-56 shrink-0 py-6 pr-6 lg:block">
        <DocsNav />
      </aside>

      {/* Mobile: the same nav, as a drawer over the page. */}
      <Show when={drawerOpen()}>
        <div class="fixed inset-0 z-50 lg:hidden">
          <div
            class="absolute inset-0 bg-canvas/70 backdrop-blur-sm"
            onClick={closeDrawer}
            aria-hidden="true"
          />
          {/* On the right, under the one menu button that opens it. */}
          <div class="absolute inset-y-0 right-0 flex w-72 max-w-[85vw] flex-col border-l border-line bg-surface p-4 shadow-2xl">
            <div class="mb-3 flex items-center justify-between">
              <span class="text-sm font-semibold text-ink">Documentation</span>
              <button
                type="button"
                onClick={closeDrawer}
                aria-label="Close navigation"
                class="inline-flex h-8 w-8 items-center justify-center rounded-lg text-muted hover:bg-surface-2 hover:text-ink"
              >
                <svg viewBox="0 0 24 24" class="h-4 w-4" fill="none" stroke="currentColor" stroke-width="2">
                  <path stroke-linecap="round" d="M6 6l12 12M18 6L6 18" />
                </svg>
              </button>
            </div>
            <div class="min-h-0 flex-1">
              <DocsNav onNavigate={closeDrawer} />
            </div>

            {/* The two site links the header's nav would otherwise carry, kept
                small: in the docs, the docs are the menu. */}
            <div class="mt-3 flex shrink-0 items-center gap-3 border-t border-line pt-3 text-xs text-muted">
              <a
                href={`${GITHUB_URL}/tree/master/examples`}
                target="_blank"
                rel="noreferrer noopener"
                class="hover:text-ink"
                onClick={closeDrawer}
              >
                Examples ↗
              </a>
              <a
                href={STUDIO_URL}
                target="_blank"
                rel="noreferrer noopener"
                class="hover:text-ink"
                onClick={closeDrawer}
              >
                Studio ↗
              </a>
            </div>
          </div>
        </div>
      </Show>

      <div class="min-w-0 flex-1 lg:border-l lg:border-line lg:pl-8">
        <Article slug={slug()} />
      </div>
    </div>
  );
}

/** `/docs` with no slug renders `docs/README.md`. */
export const DOCS_COUNT = DOCS.length;
