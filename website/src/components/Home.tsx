import { For, createSignal } from "solid-js";
import { A } from "@solidjs/router";
import { Badge, LinkButton } from "./ui";
import { Code, CopyLine } from "./Code";
import { GITHUB_URL, STUDIO_URL } from "../lib/links";
import { DOC_GROUPS } from "../lib/docs";

/* The hero's tabs: an app is these three files, so the landing page shows all
   three rather than an excerpt of one. */
const TABS = [
  {
    name: "models/post.toml",
    lang: "toml",
    code: `# → GET/POST /api/post, GET/PATCH/DELETE /api/post/{id}
[resource]
name = "post"

[permissions]        # public | authenticated | member | owner | role:<name>
list   = "member"
read   = "member"
create = "member"
update = "owner"     # only the row's owner may edit
delete = "role:admin"

[fields.title]
type = "string"
required = true
max_length = 200

[fields.body]
type = "text"

[fields.owner_id]    # stamped by the server on create
type = "reference"
references = "user"`,
  },
  {
    name: "functions/greet.rs",
    lang: "rust",
    code: `use apiplant_function::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Deserialize, JsonSchema)]
struct Input { name: String }

#[derive(Serialize, JsonSchema)]
struct Output { message: String }

// Compiled separately, loaded at boot over a stable C ABI,
// never linked into the server.
fn greet(ctx: &Context<()>, input: Input) -> Result<Output, String> {
    ctx.info("greet invoked");
    Ok(Output { message: format!("Hello, {}!", input.name) })
}

apiplant_function::function! {
    name: "greet",
    description: "Say hello. This description appears in the OpenAPI docs.",
    method: Post,
    visibility: Public,
    handler: greet,
}`,
  },
  {
    name: "seed/",
    lang: "toml",
    code: `# user.toml: This gives you an account to login as
[[row]]
id = "admin"                  # a name, hashed into a stable uuid
email = "admin@example.com"
password = "password"         # hashed with argon2 on insert
display_name = "Ada Admin"

# membership.toml: This grants this account the admin role in the organisation
[[row]]
user_id = "admin"
organization_id = "acme"
role = "admin"`,
  },
];

const FEATURES = [
  {
    title: "Resources, not controllers",
    body: "One TOML file per resource becomes a Postgres table and a full set of RESTful endpoints, each gated by a per-action permission.",
    href: "/docs/resources",
    icon: "M4 6h16M4 12h16M4 18h10",
  },
  {
    title: "Migrations that aren't files",
    body: "Your schemas are the desired state. On boot apiplant creates missing tables and adds missing columns, idempotently.",
    href: "/docs/resources",
    icon: "M12 3v18M5 10l7-7 7 7",
  },
  {
    title: "Multitenant by default",
    body: "organization is the tenant, membership carries the role, and every query, create and delete is scoped to the active organisation for you.",
    href: "/docs/multitenancy",
    icon: "M7 20a5 5 0 0 1 10 0M12 11a4 4 0 1 0 0-8 4 4 0 0 0 0 8Z",
  },
  {
    title: "Functions in any language",
    body: "Place a .rs, a Go module, an npm project or a C file in functions/. apiplant builds it and loads it over a stable C ABI, with no server rebuild.",
    href: "/docs/functions",
    icon: "m8 8-4 4 4 4M16 8l4 4-4 4M14 5l-4 14",
  },
  {
    title: "Hooks on every operation",
    body: "Attach those same functions to before_create, after_list and the rest, to validate, rewrite or observe any CRUD operation.",
    href: "/docs/hooks",
    icon: "M4 12h6a3 3 0 0 0 3-3V6a3 3 0 1 1 3 3",
  },
  {
    title: "Auth, keys and OAuth built in",
    body: "Users, organisations, sessions and API keys are ordinary resources with sensible defaults. Extend them by adding a file with the same name.",
    href: "/docs/authentication",
    icon: "M12 3 4 6v6c0 4.5 3.3 8.2 8 9 4.7-.8 8-4.5 8-9V6l-8-3Z",
  },
  {
    title: "Email, cache, payments, AI",
    body: "Name a provider and it is wired up: ctx.send_email, ctx.cache_get, a billing catalogue with checkout, and streaming chat agents.",
    href: "/docs/payments",
    icon: "M3 7h18v10H3zM3 7l9 6 9-6",
  },
  {
    title: "An operator dashboard, generated",
    body: "A static admin UI is generated from the same definitions, configurable per resource with [admin], and available in the terminal as apiplant cli.",
    href: "/docs/admin",
    icon: "M4 4h7v7H4zM13 4h7v4h-7zM13 10h7v10h-7zM4 13h7v7H4z",
  },
];

const STEPS = [
  {
    step: "01",
    title: "Start the directory",
    body: "apiplant init my-app writes a working sample containing one resource, seed rows to sign in with and one function, or clones a template of your own from a git URL.",
  },
  {
    step: "02",
    title: "Describe and build",
    body: "Edit models/*.toml until the API is the one you wanted. apiplant build ./my-app wraps every source in functions/ into a library beside it.",
  },
  {
    step: "03",
    title: "Run the binary",
    body: "apiplant run ./my-app migrates the database, mounts the endpoints and hooks, and starts serving. Nothing is generated into your repo.",
  },
];

function Icon(props: { path: string; class?: string }) {
  return (
    <svg
      viewBox="0 0 24 24"
      class={props.class ?? "h-4.5 w-4.5"}
      fill="none"
      stroke="currentColor"
      stroke-width="1.7"
      stroke-linecap="round"
      stroke-linejoin="round"
      aria-hidden="true"
    >
      <path d={props.path} />
    </svg>
  );
}

function Hero() {
  const [tab, setTab] = createSignal(0);

  return (
    <section class="relative overflow-hidden">
      {/* The site's light source: the same blue wash the studio opens with. */}
      <div
        aria-hidden="true"
        class="pointer-events-none absolute inset-0 -z-10 bg-[radial-gradient(70rem_40rem_at_12%_-20%,color-mix(in_srgb,var(--color-accent)_14%,transparent),transparent_60%),radial-gradient(50rem_30rem_at_95%_0%,color-mix(in_srgb,var(--color-accent)_8%,transparent),transparent_55%)]"
      />

      <div class="mx-auto grid w-full max-w-6xl gap-10 px-5 pb-14 pt-10 sm:gap-12 sm:pb-20 sm:pt-16 lg:grid-cols-[minmax(0,1fr)_minmax(0,1.05fr)] lg:items-center lg:pt-24">
        <div class="min-w-0 animate-rise">
          <Badge tone="accent">
            <span class="h-1.5 w-1.5 rounded-full bg-accent" />
            Rust · Postgres · stable ABI plugins
          </Badge>

          <h1 class="mt-5 text-3xl font-semibold leading-[1.1] tracking-tight text-balance text-ink sm:text-4xl md:text-5xl">
            Ship one executable.
            <br />
            Point it at a directory.
          </h1>

          <p class="mt-5 max-w-xl text-base font-medium text-ink sm:text-lg md:text-xl">
            Instant, multi-tenant APIs from a single binary.
          </p>

          <p class="mt-3 max-w-xl text-base leading-relaxed text-muted sm:text-lg">
            Declare your resources in TOML. <code class="font-mono text-[0.9em]">apiplant</code> handles Postgres migrations, auth, tenant
            isolation and CRUD endpoints on boot. Add your own logic where you need it; there is
            no boilerplate to write.
          </p>

          <div class="mt-8 flex flex-wrap items-center gap-3">
            <LinkButton href="/docs" variant="primary" size="lg" class="flex-1 sm:flex-none">
              Read the docs
              <svg viewBox="0 0 24 24" class="h-4 w-4" fill="none" stroke="currentColor" stroke-width="2">
                <path stroke-linecap="round" stroke-linejoin="round" d="M5 12h14m-6-6 6 6-6 6" />
              </svg>
            </LinkButton>
            <LinkButton href={STUDIO_URL} size="lg" class="flex-1 sm:flex-none">
              Open the studio
            </LinkButton>
          </div>

          <div class="mt-6 flex flex-wrap gap-2">
            <CopyLine command="cargo install apiplant" />
            <CopyLine command="apiplant init my-app" />
          </div>
        </div>

        <div class="min-w-0 animate-rise">
          <div class="flex flex-wrap gap-1 rounded-t-xl border border-b-0 border-line bg-surface p-1">
            <For each={TABS}>
              {(item, index) => (
                <button
                  type="button"
                  onClick={() => setTab(index())}
                  class={`rounded-lg px-2.5 py-1.5 font-mono text-[0.6875rem] transition-colors sm:px-3 sm:text-xs ${
                    tab() === index()
                      ? "bg-surface-2 text-ink"
                      : "text-faint hover:bg-surface-2/60 hover:text-muted"
                  }`}
                >
                  {item.name}
                </button>
              )}
            </For>
          </div>
          <Code
            code={TABS[tab()].code}
            lang={TABS[tab()].lang}
            class="rounded-t-none border-t-0"
          />

          <Code
            class="mt-4"
            lang="bash"
            code={`$ apiplant run ./my-app
  INFO apiplant_server: running migrations
  INFO apiplant_server:   fn greet -> /api/functions/greet
  INFO apiplant_server:   hook post.before_create -> post_before_create
  INFO apiplant_server: apiplant listening on http://127.0.0.1:8099/api`}
          />
        </div>
      </div>
    </section>
  );
}

function Anatomy() {
  return (
    <section class="mx-auto w-full max-w-6xl px-5 py-12 sm:py-16">
      <div class="grid gap-8 sm:gap-10 lg:grid-cols-[minmax(0,0.9fr)_minmax(0,1fr)] lg:items-center">
        <div class="min-w-0">
          <p class="text-[0.6875rem] font-semibold uppercase tracking-[0.12em] text-faint">
            The 60-second model
          </p>
          <h2 class="mt-3 text-2xl font-semibold tracking-tight text-ink sm:text-3xl">
            An app is a directory, and every part of it is optional
          </h2>
          <p class="mt-4 leading-relaxed text-muted">
            <code class="font-mono text-[0.9em]">apiplant init</code> writes the directory below,
            and every part of it is optional: an empty directory is a valid app. Add a resource
            and you get a table with CRUD endpoints; add a certificate and the server serves
            HTTPS; add a function and it is mounted at boot. There is no generated code to keep in
            sync.
          </p>
          <A
            href="/docs"
            class="mt-6 inline-flex items-center gap-1.5 text-sm font-medium text-accent hover:text-accent-dim"
          >
            The full model
            <svg viewBox="0 0 24 24" class="h-3.5 w-3.5" fill="none" stroke="currentColor" stroke-width="2">
              <path stroke-linecap="round" stroke-linejoin="round" d="M5 12h14m-6-6 6 6-6 6" />
            </svg>
          </A>
        </div>

        <Code
          filename="apiplant init my-app"
          /* No tag: the renderer reads the block and picks a grammar, the same
             way it does for the untagged fences in the guides. */
          lang=""
          code={`my-app/
├── main.toml       # server / database / auth; safe defaults if absent
├── https/          # cert + key here ⇒ the server runs HTTPS
├── models/         # one <name>.toml per resource ⇒ table + endpoints
├── seed/           # optional <resource>.toml|csv ⇒ initial rows
├── agents/         # optional <name>.toml per configured AI agent
└── functions/      # sources, per-function config, built libraries`}
        />
      </div>
    </section>
  );
}

function Features() {
  return (
    <section class="mx-auto w-full max-w-6xl px-5 py-12 sm:py-16">
      <h2 class="text-2xl font-semibold tracking-tight text-ink sm:text-3xl">What you get at boot</h2>
      <p class="mt-3 max-w-2xl leading-relaxed text-muted">
        Everything below is read from the directory when the process starts. None of it is a
        library you call; it is all behaviour the server provides.
      </p>

      <div class="mt-10 grid gap-4 sm:grid-cols-2 lg:grid-cols-4">
        <For each={FEATURES}>
          {(feature) => (
            <A
              href={feature.href}
              class="group rounded-2xl border border-line bg-surface p-5 transition-colors hover:border-line-strong hover:bg-surface-2"
            >
              <span class="inline-flex h-9 w-9 items-center justify-center rounded-lg border border-accent-line bg-accent-soft text-accent">
                <Icon path={feature.icon} />
              </span>
              <h3 class="mt-4 text-sm font-semibold tracking-tight text-ink">{feature.title}</h3>
              <p class="mt-2 text-sm leading-relaxed text-muted">{feature.body}</p>
            </A>
          )}
        </For>
      </div>
    </section>
  );
}

function Steps() {
  return (
    <section class="mx-auto w-full max-w-6xl px-5 py-12 sm:py-16">
      <div class="grid gap-4 md:grid-cols-3">
        <For each={STEPS}>
          {(step) => (
            <div class="rounded-2xl border border-line bg-surface p-6">
              <span class="font-mono text-xs text-accent">{step.step}</span>
              <h3 class="mt-3 text-base font-semibold tracking-tight text-ink">{step.title}</h3>
              <p class="mt-2 text-sm leading-relaxed text-muted">{step.body}</p>
            </div>
          )}
        </For>
      </div>
    </section>
  );
}

function StudioCallout() {
  return (
    <section class="mx-auto w-full max-w-6xl px-5 py-12 sm:py-16">
      <div class="overflow-hidden rounded-2xl border border-accent-line bg-accent-soft">
        <div class="grid gap-6 p-6 sm:gap-8 sm:p-10 lg:grid-cols-[minmax(0,1fr)_auto] lg:items-center">
          <div class="min-w-0">
            <p class="text-[0.6875rem] font-semibold uppercase tracking-[0.12em] text-accent">
              studio.apiplant.com
            </p>
            <h2 class="mt-3 text-xl font-semibold tracking-tight text-ink sm:text-2xl">
              Edit the directory without leaving the browser
            </h2>
            <p class="mt-3 max-w-2xl leading-relaxed text-muted">
              The studio is a local-first visual editor for app directories: it opens the folder on
              your machine, reads <code class="font-mono text-[0.9em]">main.toml</code>, every model
              and every function, and writes back only what you changed. Nothing is uploaded;
              there is no server behind the page.
            </p>
          </div>
          <LinkButton href={STUDIO_URL} variant="primary" size="lg" class="justify-self-start">
            Open the studio
            <svg viewBox="0 0 24 24" class="h-3.5 w-3.5" fill="none" stroke="currentColor" stroke-width="2">
              <path stroke-linecap="round" stroke-linejoin="round" d="M14 4h6v6M20 4l-9 9" />
            </svg>
          </LinkButton>
        </div>
      </div>
    </section>
  );
}

function DocsIndex() {
  return (
    <section class="mx-auto w-full max-w-6xl px-5 py-12 sm:py-16">
      <div class="flex flex-wrap items-end justify-between gap-4">
        <div>
          <h2 class="text-2xl font-semibold tracking-tight text-ink sm:text-3xl">The documentation</h2>
        </div>
        <LinkButton href="/docs">Browse all guides</LinkButton>
      </div>

      <div class="mt-10 grid gap-8 sm:grid-cols-2 lg:grid-cols-3">
        <For each={DOC_GROUPS}>
          {(group) => (
            <div>
              <h3 class="text-[0.6875rem] font-semibold uppercase tracking-[0.12em] text-faint">
                {group.group}
              </h3>
              <ul class="mt-3 grid gap-1">
                <For each={group.docs}>
                  {(doc) => (
                    <li>
                      <A
                        href={`/docs${doc.slug ? `/${doc.slug}` : ""}`}
                        class="-mx-2 block rounded-lg px-2 py-1.5 transition-colors hover:bg-surface-2"
                      >
                        <span class="text-sm font-medium text-ink">{doc.title}</span>
                        <span class="mt-0.5 block text-xs leading-relaxed text-faint">
                          {doc.summary}
                        </span>
                      </A>
                    </li>
                  )}
                </For>
              </ul>
            </div>
          )}
        </For>
      </div>
    </section>
  );
}

function Closing() {
  return (
    <section class="mx-auto w-full max-w-6xl px-5 pb-16 pt-8 sm:pb-24">
      <div class="rounded-2xl border border-line bg-surface p-6 text-center sm:p-10">
        <h2 class="text-xl font-semibold tracking-tight text-ink sm:text-2xl">
          Start with one command
        </h2>
        <p class="mx-auto mt-3 max-w-xl leading-relaxed text-muted">
          <code class="font-mono text-[0.9em]">init</code> writes a working app you can seed and
          run, or clones your own template from a git URL. An empty directory is also a valid
          app: add one <code class="font-mono text-[0.9em]">models/*.toml</code> and you have an API.
        </p>
        <div class="mt-7 flex flex-wrap items-center justify-center gap-3">
          <CopyLine command="apiplant init my-app" />
          <LinkButton href={`${GITHUB_URL}/tree/master/examples`} size="lg">
            Browse the examples
          </LinkButton>
        </div>
      </div>
    </section>
  );
}

export function Home() {
  return (
    <>
      <Hero />
      <Anatomy />
      <Features />
      <Steps />
      <StudioCallout />
      <DocsIndex />
      <Closing />
    </>
  );
}
