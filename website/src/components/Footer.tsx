import { For } from "solid-js";
import { A } from "@solidjs/router";
import { HeadMark } from "./ui";
import { COMPANY_URL, CRATE_URL, GITHUB_URL, STUDIO_URL } from "../lib/links";
import { DOCS } from "../lib/docs";

const COLUMNS = [
  {
    title: "Product",
    links: [
      { label: "Documentation", href: "/docs" },
      { label: "Studio", href: STUDIO_URL },
      { label: "Examples", href: `${GITHUB_URL}/tree/master/examples` },
    ],
  },
  {
    title: "Reference",
    links: [
      { label: "Configuration", href: "/docs/configuration" },
      { label: "API reference", href: "/docs/api-reference" },
      { label: "Security model", href: "/docs/security" },
    ],
  },
  {
    title: "Source",
    links: [
      { label: "GitHub", href: GITHUB_URL },
      { label: "crates.io", href: CRATE_URL },
      { label: "Issues", href: `${GITHUB_URL}/issues` },
      { label: "apiplant.com", href: COMPANY_URL },
    ],
  },
];

function FooterLink(props: { href: string; label: string }) {
  return props.href.startsWith("/") ? (
    <A href={props.href} class="text-muted transition-colors hover:text-ink">
      {props.label}
    </A>
  ) : (
    <a
      href={props.href}
      target="_blank"
      rel="noreferrer noopener"
      class="text-muted transition-colors hover:text-ink"
    >
      {props.label}
    </a>
  );
}

export function Footer() {
  return (
    <footer class="border-t border-line">
      <div class="mx-auto grid w-full max-w-6xl gap-10 px-5 py-12 sm:grid-cols-2 lg:grid-cols-4">
        <div>
          <div class="flex items-center gap-2">
            <HeadMark class="h-7" />
            <span class="text-[0.9375rem] font-semibold tracking-tight text-ink">
              apiplant <span class="text-accent">framework</span>
            </span>
          </div>
          <p class="mt-3 max-w-xs text-sm leading-relaxed text-faint">
            One executable, pointed at a directory. Your API is configuration and compiled plugins,
            not a server you maintain.
          </p>
          <p class="mt-4 text-xs text-faint">
            {DOCS.length} guides · rendered from the repository's <code class="font-mono">docs/</code>
          </p>
        </div>

        <For each={COLUMNS}>
          {(column) => (
            <div>
              <h2 class="text-[0.6875rem] font-semibold uppercase tracking-[0.12em] text-faint">
                {column.title}
              </h2>
              <ul class="mt-3 grid gap-2 text-sm">
                <For each={column.links}>
                  {(link) => (
                    <li>
                      <FooterLink href={link.href} label={link.label} />
                    </li>
                  )}
                </For>
              </ul>
            </div>
          )}
        </For>
      </div>

      <div class="border-t border-line">
        <div class="mx-auto flex w-full max-w-6xl flex-wrap items-center justify-between gap-2 px-5 py-5 text-xs text-faint">
          <span>© {new Date().getFullYear()} apiplant</span>
          <span>framework.apiplant.com</span>
        </div>
      </div>
    </footer>
  );
}
