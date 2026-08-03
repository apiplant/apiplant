import {
  For,
  Show,
  createEffect,
  createMemo,
  createResource,
  createSignal,
  onCleanup,
} from "solid-js";
import { A, useLocation, useNavigate, useParams } from "@solidjs/router";
import { DOCS, DOC_GROUPS, findDoc, loadDoc, neighbours, type DocMeta } from "../lib/docs";
import { GITHUB_URL } from "../lib/links";

/** The nav, with a filter: a plain list is unwieldy at nineteen guides. */
function DocsNav(props: { onNavigate?: () => void }) {
  const params = useParams();
  const [query, setQuery] = createSignal("");

  const current = () => params.slug ?? "";

  const groups = createMemo(() => {
    const needle = query().trim().toLowerCase();
    if (!needle) return DOC_GROUPS;
    return DOC_GROUPS.map((group) => ({
      group: group.group,
      docs: group.docs.filter(
        (doc) =>
          doc.title.toLowerCase().includes(needle) || doc.summary.toLowerCase().includes(needle),
      ),
    })).filter((group) => group.docs.length > 0);
  });

  return (
    <div class="flex h-full flex-col gap-4">
      <label class="relative block">
        <span class="sr-only">Filter guides</span>
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
          onInput={(event) => setQuery(event.currentTarget.value)}
          placeholder="Filter guides"
          class="w-full rounded-lg border border-line bg-surface-2 py-1.5 pl-8 pr-2.5 text-[0.8125rem] text-ink transition-colors placeholder:text-faint hover:border-line-strong focus:border-accent focus:bg-surface focus:outline-none"
        />
      </label>

      <nav class="min-h-0 flex-1 overflow-y-auto pb-8">
        <Show
          when={groups().length > 0}
          fallback={<p class="px-2 py-4 text-xs text-faint">No guide matches “{query()}”.</p>}
        >
          <For each={groups()}>
            {(group) => (
              <div class="mb-5">
                <h2 class="px-2 text-[0.6875rem] font-semibold uppercase tracking-[0.12em] text-faint">
                  {group.group}
                </h2>
                <ul class="mt-1.5 grid gap-0.5">
                  <For each={group.docs}>
                    {(doc) => (
                      <li>
                        <A
                          href={`/docs${doc.slug ? `/${doc.slug}` : ""}`}
                          onClick={() => props.onNavigate?.()}
                          class={`block rounded-lg px-2 py-1.5 text-[0.8125rem] transition-colors ${
                            current() === doc.slug
                              ? "bg-accent-soft font-medium text-accent"
                              : "text-muted hover:bg-surface-2 hover:text-ink"
                          }`}
                        >
                          {doc.title}
                        </A>
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

  createEffect(() => {
    const ids = props.headings.map((heading) => heading.id);
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

    onCleanup(() => observer.disconnect());
  });

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
    <A
      href={`/docs${doc.slug ? `/${doc.slug}` : ""}`}
      class={`group flex flex-col gap-1 rounded-xl border border-line bg-surface p-4 transition-colors hover:border-line-strong hover:bg-surface-2 ${
        side === "next" ? "text-right sm:col-start-2" : ""
      }`}
    >
      <span class="text-[0.6875rem] font-semibold uppercase tracking-[0.12em] text-faint">
        {side === "prev" ? "Previous" : "Next"}
      </span>
      <span class="text-sm font-medium text-ink">{doc.title}</span>
    </A>
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

function Article(props: { slug: string }) {
  const navigate = useNavigate();
  const location = useLocation();
  const [doc] = createResource(() => props.slug, loadDoc);

  const sourceFile = () => `docs/${props.slug || "README"}.md`;

  // Rendered links are plain <a href="/docs/…">; catching them here keeps the
  // router in charge instead of reloading the whole app.
  const intercept = (event: MouseEvent) => {
    if (event.defaultPrevented || event.button !== 0 || event.metaKey || event.ctrlKey || event.shiftKey)
      return;
    const anchor = (event.target as HTMLElement).closest("a");
    const href = anchor?.getAttribute("href");
    if (!anchor || !href || !href.startsWith("/")) return;
    event.preventDefault();
    navigate(href);
  };

  // A fresh document starts at the top; an in-page anchor is honoured instead.
  createEffect(() => {
    if (!doc.loading && doc()) {
      const hash = location.hash.slice(1);
      const target = hash ? document.getElementById(decodeURIComponent(hash)) : null;
      if (target) target.scrollIntoView();
      else window.scrollTo({ top: 0 });
    }
  });

  createEffect(() => {
    const meta = findDoc(props.slug);
    document.title = meta ? `${meta.title} — apiplant docs` : "apiplant docs";
  });

  return (
    <div class="mx-auto grid w-full max-w-6xl gap-10 px-5 py-10 xl:grid-cols-[minmax(0,1fr)_14rem]">
      <article class="min-w-0">
        <Show
          when={doc()}
          fallback={
            <Show
              when={!doc.error}
              fallback={
                <div class="rounded-xl border border-line bg-surface p-8">
                  <h1 class="text-xl font-semibold text-ink">That guide doesn't exist</h1>
                  <p class="mt-2 text-sm text-muted">
                    There is no <code class="font-mono">{sourceFile()}</code> in the repository.
                  </p>
                  <A href="/docs" class="mt-4 inline-block text-sm text-accent hover:text-accent-dim">
                    Back to the documentation
                  </A>
                </div>
              }
            >
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
            </Show>
          }
        >
          {(loaded) => (
            <>
              <div class="mb-6 flex flex-wrap items-center gap-2 text-xs text-faint">
                <A href="/docs" class="hover:text-ink">
                  Documentation
                </A>
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
      </article>

      <Show when={doc()}>{(loaded) => <Contents headings={loaded().headings} />}</Show>
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
          <div class="absolute inset-y-0 left-0 w-72 max-w-[85vw] border-r border-line bg-surface p-4 shadow-2xl">
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
            <div class="h-[calc(100%-2.75rem)]">
              <DocsNav onNavigate={closeDrawer} />
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
