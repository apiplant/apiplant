import { For, Show, createResource, createSignal } from "solid-js";
import { A } from "@solidjs/router";
import { Badge, LinkButton } from "./ui";
import { Code, CopyBlock, CopyLine } from "./Code";
import { GITHUB_URL, STUDIO_URL, SKILL_URL, CRATE_URL } from "../lib/links";
import {
  LATEST_RELEASE_URL,
  PLATFORMS,
  RELEASES_URL,
  TAG,
  assetName,
  detectPlatform,
  downloadUrl,
} from "../lib/release";
import { DOC_GROUPS } from "../lib/docs";

/* The hero's tabs: an app is these three files, so the landing page shows all
   three rather than an excerpt of one. */
const TABS = [
  {
    name: "resources/post.toml",
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
    title: "Background work, no broker",
    body: "queue.publish records a message and returns; subscribed functions handle it afterwards, with retries and a dead-letter. The transport is the Postgres you already have.",
    href: "/docs/queues",
    icon: "M4 7h16M4 12h16M4 17h9M17 15l3 2-3 2",
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
    body: "Edit resources/*.toml until the API is the one you wanted. apiplant build ./my-app wraps every source in functions/ into a library beside it.",
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

          <div class="mt-6 flex flex-wrap items-center gap-3">
            <DownloadButton />
            <a href="#install" class="text-sm font-medium text-accent hover:text-accent-dim">
              Other ways to install
            </a>
          </div>

          <div class="mt-4 flex flex-wrap gap-2">
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
├── resources/      # one <name>.toml per resource ⇒ table + endpoints
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

      <div class="mt-10 grid gap-4 sm:grid-cols-2 lg:grid-cols-3">
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

/* The release asset for the machine the page is being read on. Detection is
   asynchronous and can fail — on Windows, on an Intel Mac, on a phone — so the
   releases page is the fallback rather than a broken link to an archive that
   was never built. */
function DownloadButton(props: { class?: string }) {
  const [platform] = createResource(detectPlatform);

  return (
    <Show
      when={platform()}
      fallback={
        <LinkButton href={LATEST_RELEASE_URL} variant="primary" size="lg" class={props.class}>
          Download {TAG}
        </LinkButton>
      }
    >
      {(detected) => (
        <LinkButton
          href={downloadUrl(detected())}
          variant="primary"
          size="lg"
          class={props.class}
          /* A cross-origin download of a binary: let the browser navigate to
             the asset rather than opening a tab that immediately closes. */
          target="_self"
        >
          <svg viewBox="0 0 24 24" class="h-4 w-4" fill="none" stroke="currentColor" stroke-width="2">
            <path stroke-linecap="round" stroke-linejoin="round" d="M12 4v11m0 0 4-4m-4 4-4-4M4 19h16" />
          </svg>
          Download for {detected().short}
        </LinkButton>
      )}
    </Show>
  );
}

function Install() {
  const stepCard =
    "grid min-w-0 gap-5 rounded-2xl border bg-surface p-5 sm:p-6 lg:grid-cols-[minmax(14rem,0.7fr)_minmax(0,1.3fr)] lg:items-start";
  const homebrewCommands = `brew tap apiplant/tap
brew install apiplant/tap/apiplant`;
  const pacmanCommands = `curl -sSfL https://apiplant.github.io/pacman/apiplant.gpg -o /tmp/apiplant.gpg
keyid=$(gpg --show-keys --with-colons /tmp/apiplant.gpg | awk -F: '/^pub:/ { print $5; exit }') && sudo pacman-key --add /tmp/apiplant.gpg && sudo pacman-key --finger "$keyid" && sudo pacman-key --lsign-key "$keyid"
printf '\\n[apiplant]\\nSigLevel = Required DatabaseOptional\\nServer = https://apiplant.github.io/pacman/$arch\\n' | sudo tee -a /etc/pacman.conf > /dev/null
sudo pacman -Sy apiplant`;
  const aptCommands = `curl -sSfL https://apt.apiplant.com/apiplant-archive-keyring.gpg | sudo tee /usr/share/keyrings/apiplant.gpg > /dev/null
echo "deb [signed-by=/usr/share/keyrings/apiplant.gpg] https://apt.apiplant.com stable main" | sudo tee /etc/apt/sources.list.d/apiplant.list > /dev/null
sudo apt update && sudo apt install apiplant`;
  const dockerCommands = `docker pull ghcr.io/apiplant/apiplant:latest
docker run --rm -p 8080:8080 -v "$PWD:/app" ghcr.io/apiplant/apiplant`;

  return (
    <section id="install" class="mx-auto w-full max-w-7xl px-5 py-12 sm:py-16">
      <h2 class="text-2xl font-semibold tracking-tight text-ink sm:text-3xl">Install</h2>
      <p class="mt-3 max-w-2xl leading-relaxed text-muted">
        Use Homebrew, pacman or apt when your platform has it. Otherwise take the prebuilt
        binary, or run the image — building from source is the slowest path.
      </p>

      <div class="mt-8 space-y-4 sm:mt-10">
        <div class={`${stepCard} border-accent-line`}>
          <div>
            <div class="flex items-center gap-2">
              <span class="font-mono text-xs text-accent">01</span>
              <Badge tone="accent">Recommended</Badge>
            </div>
            <h3 class="mt-3 text-base font-semibold tracking-tight text-ink">Use a package manager</h3>
            <p class="mt-2 text-sm leading-relaxed text-muted">
              The quickest path on macOS, Arch, Debian and Ubuntu.
            </p>
          </div>

          <div class="min-w-0 space-y-5">
            <div>
              <p class="mb-2 text-xs font-semibold uppercase tracking-[0.14em] text-faint">
                macOS / Homebrew
              </p>
              <CopyBlock command={homebrewCommands} />
            </div>

            <div>
              <p class="mb-2 text-xs font-semibold uppercase tracking-[0.14em] text-faint">
                Arch Linux / pacman
              </p>
              <CopyBlock command={pacmanCommands} />
            </div>

            <div>
              <p class="mb-2 text-xs font-semibold uppercase tracking-[0.14em] text-faint">
                Debian / Ubuntu
              </p>
              <CopyBlock command={aptCommands} />
              <p class="mt-2 text-xs leading-relaxed text-faint">
                Or pin a release with{" "}
                <code class="font-mono text-[0.9em]">sudo dpkg -i apiplant_0.7.0-1_amd64.deb</code>.
              </p>
            </div>
          </div>
        </div>

        <div class={`${stepCard} border-line`}>
          <div>
            <span class="font-mono text-xs text-accent">02</span>
            <h3 class="mt-3 text-base font-semibold tracking-tight text-ink">Download the binary</h3>
            <p class="mt-2 text-sm leading-relaxed text-muted">
              Unpack it anywhere on your <code class="font-mono text-[0.9em]">PATH</code> and run it.
              Every archive ships with a matching{" "}
              <code class="font-mono text-[0.9em]">.sha256</code>.
            </p>

            <div class="mt-5">
              <DownloadButton class="w-full sm:w-auto" />
            </div>
          </div>

          {/* The whole row is the link. The label never shrinks and the asset
              name absorbs what is left, ellipsised when the column is narrow —
              the full name is in the tooltip and in the URL. */}
          <ul class="min-w-0 space-y-1 border-t border-line pt-4 lg:border-t-0 lg:pt-0">
            <For each={PLATFORMS}>
              {(platform) => (
                <li class="min-w-0">
                  <a
                    href={downloadUrl(platform)}
                    title={assetName(platform)}
                    class="flex min-w-0 items-baseline justify-between gap-3 rounded-md py-1 text-muted transition-colors hover:text-ink"
                  >
                    <span class="shrink-0 text-sm">{platform.label}</span>
                    <span class="min-w-0 truncate font-mono text-xs text-accent">
                      {assetName(platform)}
                    </span>
                  </a>
                </li>
              )}
            </For>
          </ul>

          <a
            href={RELEASES_URL}
            target="_blank"
            rel="noreferrer noopener"
            class="lg:col-start-2 text-sm font-medium text-accent hover:text-accent-dim"
          >
            All releases and checksums
          </a>
        </div>

        <div class={`${stepCard} border-line`}>
          <div>
            <span class="font-mono text-xs text-accent">03</span>
            <h3 class="mt-3 text-base font-semibold tracking-tight text-ink">Run the image</h3>
            <p class="mt-2 text-sm leading-relaxed text-muted">
              Multi-arch (amd64 and arm64) on the GitHub registry. Mount your app directory at{" "}
              <code class="font-mono text-[0.9em]">/app</code> and the server picks it up.
            </p>
          </div>

          <div class="flex min-w-0 flex-col gap-2">
            <CopyBlock command={dockerCommands} />
          </div>

          <p class="lg:col-start-2 text-sm leading-relaxed text-muted">
            The image carries no Rust toolchain, so Rust functions have to be built with{" "}
            <code class="font-mono text-[0.9em]">apiplant build</code> beforehand. TypeScript
            functions run in-process and need nothing.
          </p>
        </div>

        <div class={`${stepCard} border-line`}>
          <div>
            <span class="font-mono text-xs text-faint">04</span>
            <h3 class="mt-3 text-base font-semibold tracking-tight text-ink">Build from source</h3>
            <p class="mt-2 text-sm leading-relaxed text-muted">
              The last resort: this compiles the whole dependency tree. Worth it only if
              you want a target nothing is published for, or your own patches.
            </p>
          </div>

          <div class="min-w-0">
            <CopyBlock command="cargo install apiplant" />
          </div>

          <a
            href={CRATE_URL}
            target="_blank"
            rel="noreferrer noopener"
            class="lg:col-start-2 inline-flex items-center gap-2 text-sm font-medium text-accent hover:text-accent-dim"
          >
            {/* The crates.io mark: a crate in three-quarter view. Drawn rather
                than fetched, since the site inlines every icon. */}
            <svg
              viewBox="0 0 24 24"
              class="h-4 w-4 shrink-0"
              fill="none"
              stroke="currentColor"
              stroke-width="1.7"
              stroke-linecap="round"
              stroke-linejoin="round"
              aria-hidden="true"
            >
              <path d="M12 2.75 3.75 7v10L12 21.25 20.25 17V7L12 2.75Z" />
              <path d="M3.75 7 12 11.25 20.25 7" />
              <path d="M12 11.25v10" />
            </svg>
            apiplant on crates.io
          </a>
        </div>
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
              your machine, reads <code class="font-mono text-[0.9em]">main.toml</code>, every resource
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

/* An app is TOML, which a coding agent writes well — but only if it knows the
   field types and the permission policies rather than guessing them. That is
   what the skill carries, so it belongs next to the studio: the other way to
   edit a directory without writing it by hand. */
function SkillCallout() {
  return (
    <section class="mx-auto w-full max-w-6xl px-5 py-12 sm:py-16">
      <div class="overflow-hidden rounded-2xl border border-line bg-surface">
        <div class="grid gap-6 p-6 sm:gap-8 sm:p-10 lg:grid-cols-[minmax(0,1fr)_auto] lg:items-center">
          <div class="min-w-0">
            <p class="text-[0.6875rem] font-semibold uppercase tracking-[0.12em] text-accent">
              Claude skill
            </p>
            <h2 class="mt-3 text-xl font-semibold tracking-tight text-ink sm:text-2xl">
              Describe the app, get the directory
            </h2>
            <p class="mt-3 max-w-2xl leading-relaxed text-muted">
              Every guide on this site, all twenty-six examples and the build workflow, packaged as
              a Claude Code plugin. Install it and Claude writes resources, permissions, hooks and
              functions from the documentation instead of from guesswork — it loads on its own when
              the work is apiplant work, with nothing to enable.
            </p>
            <div class="mt-6 grid gap-3">
              <CopyBlock
                prompt="> "
                command={"/plugin marketplace add apiplant/apiplant\n/plugin install apiplant-app@apiplant"}
              />
              <LinkButton href={SKILL_URL} class="justify-self-start">
                The skill on GitHub
              </LinkButton>
            </div>
          </div>
          <figure class="min-w-0 rounded-xl border border-line bg-surface-2 p-5 lg:max-w-sm">
            <blockquote class="text-sm leading-relaxed text-ink">
              “build me an apiplant app for tracking client invoices, one organisation per client,
              and a hook that stamps the due date 30 days out”
            </blockquote>
            <figcaption class="mt-3 text-xs leading-relaxed text-faint">
              It works on an app you already have, too — point it at the directory and it reads
              your <code class="font-mono text-[0.9em]">resources/</code> before changing them.
            </figcaption>
          </figure>
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
          app: add one <code class="font-mono text-[0.9em]">resources/*.toml</code> and you have an API.
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
      <Install />
      <Steps />
      <StudioCallout />
      <SkillCallout />
      <DocsIndex />
      <Closing />
    </>
  );
}
