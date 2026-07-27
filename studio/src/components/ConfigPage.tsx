import { For, Show, createMemo, createSignal } from "solid-js";
import {
  CONFIG_PATH,
  configValue,
  ensureMainToml,
  fileText,
  setConfigFromToml,
  setConfigValue,
  studio,
} from "../lib/store";
import { Badge, Button, Card, CardHeader, CodeEditor, Labelled, Mono, Switch, Tabs, TextInput } from "./ui";

type TabId = "form" | "toml";

interface ConfigField {
  key: string;
  label: string;
  placeholder: string;
  kind: "text" | "number";
  hint?: string;
}

interface ConfigSection {
  id: string;
  title: string;
  hint: string;
  fields: ConfigField[];
}

/** Every key the framework reads, with the default it falls back to. */
const SECTIONS: ConfigSection[] = [
  {
    id: "app",
    title: "App",
    hint: "What the app calls itself, wherever it is named to a person.",
    fields: [
      {
        key: "name",
        label: "name",
        placeholder: "the directory name",
        kind: "text" as const,
        hint: "Heads the admin dashboard as “<name> admin”. Unset uses the directory name.",
      },
    ],
  },
  {
    id: "server",
    title: "Server",
    hint: "Where the API binds and what it answers to.",
    fields: [
      { key: "host", label: "host", placeholder: "0.0.0.0", kind: "text" as const, hint: "Bind address." },
      { key: "port", label: "port", placeholder: "8080", kind: "number" as const, hint: "TCP port." },
      {
        key: "base_path",
        label: "base path",
        placeholder: "/",
        kind: "text" as const,
        hint: "Prefix for every route. /api ⇒ endpoints live at /api/…",
      },
      {
        key: "domain",
        label: "domain",
        placeholder: "any host",
        kind: "text" as const,
        hint: "When set, other Host headers get no match.",
      },
      {
        key: "workers",
        label: "workers",
        placeholder: "one per CPU",
        kind: "number" as const,
        hint: "OS worker threads.",
      },
    ],
  },
  {
    id: "database",
    title: "Database",
    hint: "PostgreSQL. `url` wins over the individual parts.",
    fields: [
      {
        key: "url",
        label: "url",
        placeholder: "postgres://user:pass@host:5432/db",
        kind: "text" as const,
        hint: "Full connection URL; leave empty to assemble from the parts below.",
      },
      { key: "host", label: "host", placeholder: "localhost", kind: "text" as const },
      { key: "port", label: "port", placeholder: "5432", kind: "number" as const },
      { key: "name", label: "database", placeholder: "apiplant", kind: "text" as const },
      { key: "user", label: "user", placeholder: "postgres", kind: "text" as const },
      { key: "password", label: "password", placeholder: "postgres", kind: "text" as const },
      { key: "max_connections", label: "pool size", placeholder: "16", kind: "number" as const },
    ],
  },
  {
    id: "auth",
    title: "Authentication",
    hint: "Session tokens and registration.",
    fields: [
      {
        key: "jwt_secret",
        label: "jwt secret",
        placeholder: "random per boot",
        kind: "text" as const,
        hint: "Set it in production — an empty secret means tokens die with the process.",
      },
      {
        key: "session_ttl_secs",
        label: "session ttl (s)",
        placeholder: "604800",
        kind: "number" as const,
        hint: "Lifetime of issued session tokens.",
      },
    ],
  },
  {
    id: "docs",
    title: "OpenAPI & Swagger UI",
    hint: "Generated from your resources and functions; served with no configuration.",
    fields: [
      { key: "path", label: "ui path", placeholder: "/docs", kind: "text" as const },
      {
        key: "title",
        label: "title",
        placeholder: "the app name",
        kind: "text" as const,
        hint: "Only when the published API answers to a different name than the app.",
      },
    ],
  },
];

/**
 * What a field shows when it is empty: the value the server would actually use,
 * where the studio knows it. The app's name falls back to the directory, and
 * the docs title falls back to the app's name — so both can be shown rather
 * than described.
 */
function fallbackPlaceholder(sectionId: string, field: ConfigField): string | undefined {
  const directory = studio.project?.name;
  const appName = (configValue("app", "name") as string | undefined)?.trim() || directory;
  if (sectionId === "app" && field.key === "name") return directory ?? field.placeholder;
  if (sectionId === "docs" && field.key === "title") return appName ?? field.placeholder;
  return field.placeholder;
}

export function ConfigPage() {
  const [tab, setTab] = createSignal<TabId>("form");
  const [draft, setDraft] = createSignal<string | null>(null);
  const [error, setError] = createSignal<string | null>(null);

  const exists = () => !!studio.project?.files[CONFIG_PATH];
  const canonical = createMemo(() => fileText(CONFIG_PATH) ?? "");

  const commit = (text: string) => {
    setDraft(text);
    try {
      setConfigFromToml(text);
      setError(null);
    } catch (failure) {
      setError(failure instanceof Error ? failure.message : String(failure));
    }
  };

  const url = createMemo(() => {
    const host = configValue("server", "host") ?? "0.0.0.0";
    const port = configValue("server", "port") ?? 8080;
    const base = String(configValue("server", "base_path") ?? "").replace(/\/$/, "");
    const scheme = studio.project?.hasTls ? "https" : "http";
    return `${scheme}://${host === "0.0.0.0" ? "127.0.0.1" : host}:${port}${base}`;
  });

  return (
    <div class="animate-rise mx-auto w-full max-w-4xl px-6 py-6">
      <header class="mb-5">
        <div class="flex flex-wrap items-center gap-2.5">
          <h1 class="text-xl font-semibold tracking-tight">Configuration</h1>
          <Mono>main.toml</Mono>
          <Show when={studio.project?.hasTls}>
            <Badge tone="accent">https/ found — TLS on</Badge>
          </Show>
        </div>
        <p class="mt-2 max-w-3xl text-xs leading-relaxed text-muted">
          Every key is optional; anything left blank keeps the framework's default. With these settings the API
          answers at <Mono>{url()}</Mono>.
        </p>
      </header>

      <Show
        when={exists()}
        fallback={
          <Card class="px-4 py-8 text-center">
            <p class="text-sm text-ink">This app has no main.toml.</p>
            <p class="mx-auto mt-1 max-w-md text-xs leading-relaxed text-muted">
              That is a valid app — the server fills in every default. Create one to pin the port, the database
              and the JWT secret.
            </p>
            <Button class="mx-auto mt-4" variant="primary" size="sm" onClick={ensureMainToml}>
              Create main.toml
            </Button>
          </Card>
        }
      >
        <div class="mb-4">
          <Tabs
            active={tab()}
            onChange={setTab}
            tabs={[
              { id: "form", label: "Settings" },
              { id: "toml", label: "TOML" },
            ]}
          />
        </div>

        <Show when={tab() === "form"}>
          <div class="space-y-3">
            <For each={SECTIONS}>
              {(section) => (
                <Card>
                  <CardHeader title={section.title} hint={section.hint}>
                    <Show when={section.id === "database"}>
                      <Switch
                        checked={configValue("database", "auto_migrate") !== false}
                        label="auto migrate"
                        onChange={(value) => setConfigValue("database", "auto_migrate", value)}
                      />
                    </Show>
                    <Show when={section.id === "auth"}>
                      <Switch
                        checked={configValue("auth", "allow_registration") !== false}
                        label="registration open"
                        onChange={(value) => setConfigValue("auth", "allow_registration", value)}
                      />
                    </Show>
                    <Show when={section.id === "docs"}>
                      <Switch
                        checked={configValue("docs", "enabled") !== false}
                        label="enabled"
                        onChange={(value) => setConfigValue("docs", "enabled", value)}
                      />
                    </Show>
                  </CardHeader>
                  <div class="grid gap-4 px-4 py-4 sm:grid-cols-2">
                    <For each={section.fields}>
                      {(field) => (
                        <Labelled label={field.label} hint={field.hint}>
                          <TextInput
                            mono
                            type={field.kind === "number" ? "number" : "text"}
                            value={String(configValue(section.id, field.key) ?? "")}
                            // The app name falls back to the directory, and the
                            // studio knows which directory this is — so the
                            // placeholder can show the name that will be used
                            // rather than describe it.
                            placeholder={fallbackPlaceholder(section.id, field)}
                            onInput={(value) => {
                              if (value === "") return setConfigValue(section.id, field.key, undefined);
                              if (field.kind === "number") {
                                const parsed = Number(value);
                                return setConfigValue(
                                  section.id,
                                  field.key,
                                  Number.isFinite(parsed) ? parsed : undefined,
                                );
                              }
                              setConfigValue(section.id, field.key, value);
                            }}
                          />
                        </Labelled>
                      )}
                    </For>
                  </div>
                </Card>
              )}
            </For>

            <Card>
              <CardHeader title="HTTPS" hint="Not configured here — inferred from the app directory." />
              <p class="px-4 py-4 text-xs leading-relaxed text-muted">
                Drop a certificate and a key into <Mono>https/</Mono> and the server serves TLS. Recognised
                names: <Mono>cert.pem</Mono>, <Mono>fullchain.pem</Mono>, <Mono>certificate.pem</Mono>,{" "}
                <Mono>server.crt</Mono> for the certificate; <Mono>key.pem</Mono>, <Mono>privkey.pem</Mono>,{" "}
                <Mono>server.key</Mono>, <Mono>private.pem</Mono> for the key.
                <Show when={studio.project?.hasTls}>
                  {" "}
                  This app has one, so it will serve HTTPS.
                </Show>
              </p>
            </Card>
          </div>
        </Show>

        <Show when={tab() === "toml"}>
          <div>
            <div class="mb-2 flex items-center justify-between">
              <p class="text-xs text-muted">Edit the file directly; the form above follows.</p>
              <Show when={error()}>
                <span class="text-xs text-danger">{error()}</span>
              </Show>
            </div>
            <CodeEditor language="toml" value={draft() ?? canonical()} onInput={commit} minHeight="26rem" />
          </div>
        </Show>
      </Show>
    </div>
  );
}
