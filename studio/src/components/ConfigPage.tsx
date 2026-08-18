import { For, Show, createEffect, createMemo, createSignal } from "solid-js";
import {
  CONFIG_PATH,
  allFunctionNames,
  configEntries,
  configList,
  configValue,
  setConfigEntries,
  setConfigList,
  ensureMainToml,
  fileText,
  setConfigFromToml,
  setConfigValue,
  setSubscriptions,
  studio,
  subscriptions,
} from "../lib/store";
import type { ConfigEntry, Subscription } from "../lib/store";
import { setView, view } from "../lib/nav";
import {
  Badge,
  Button,
  Card,
  CardHeader,
  CodeEditor,
  Labelled,
  Mono,
  Select,
  Switch,
  Tabs,
  TextArea,
  TextInput,
} from "./ui";

/** A section id, or the raw-file tab that always sits last. */
type TabId = string;
type FieldKind = "text" | "number" | "select" | "textarea" | "boolean" | "list";

interface ConfigField {
  key: string;
  label: string;
  placeholder?: string;
  kind: FieldKind;
  options?: readonly string[] | readonly { value: string; label: string }[];
  hint?: string;
  /** Writes to another table than the section's own — `[email.smtp]` under Email. */
  section?: string;
  /**
   * What the server does with this key absent, for a switch whose default is
   * `true`. Without it an unset key draws as off, which reads as "this is
   * disabled" about something the server has switched on. Toggling back to the
   * default deletes the key rather than writing it, so the file keeps saying
   * nothing about a setting nobody changed.
   */
  defaultOn?: boolean;
  /**
   * Heading this field sits under inside its section. Fields carrying the same
   * group are shown together, in the order the group is first named; a field
   * with no group leads the section, above every heading.
   */
  group?: string;
}

interface FieldGroup {
  title: string | null;
  fields: ConfigField[];
}

interface ConfigSection {
  id: string;
  title: string;
  hint: string;
  /** A function when the fields depend on what is already configured. */
  fields: ConfigField[] | (() => ConfigField[]);
}

const DEFAULT_SECTION_ID = "application";
const TOML_TAB_ID = "toml";

const EMAIL_PROVIDERS = [
  { value: "none", label: "none" },
  { value: "smtp", label: "smtp" },
  { value: "ses", label: "ses" },
  { value: "sendgrid", label: "sendgrid" },
  { value: "brevo", label: "brevo" },
  { value: "mailjet", label: "mailjet" },
  { value: "mailgun", label: "mailgun" },
  { value: "postmark", label: "postmark" },
  { value: "resend", label: "resend" },
] as const;

const SMTP_ENCRYPTION = ["starttls", "tls", "none"] as const;

/**
 * What each mailer needs, and nothing else. A key is worth showing only where
 * the provider reads it: `region` means something to SES alone, `domain` to
 * Mailgun alone, and the `[email.smtp]` table only to SMTP — so each provider
 * names its own credentials rather than the form listing every provider's.
 */
const EMAIL_PROVIDER_FIELDS: Record<string, ConfigField[]> = {
  none: [],
  smtp: [
    { section: "email.smtp", key: "host", label: "host", placeholder: "smtp.example.com", kind: "text" as const },
    {
      section: "email.smtp",
      key: "port",
      label: "port",
      placeholder: "0",
      kind: "number" as const,
      hint: "0 picks the usual port for the encryption.",
    },
    { section: "email.smtp", key: "username", label: "username", placeholder: "mailer", kind: "text" as const },
    { section: "email.smtp", key: "password", label: "password", placeholder: "app password", kind: "text" as const },
    {
      section: "email.smtp",
      key: "encryption",
      label: "encryption",
      kind: "select" as const,
      options: SMTP_ENCRYPTION,
      hint: "`starttls`, `tls` or `none`.",
    },
  ],
  ses: [
    { key: "api_key", label: "access key id", placeholder: "$AWS_ACCESS_KEY_ID", kind: "text" as const },
    { key: "api_secret", label: "secret access key", placeholder: "$AWS_SECRET_ACCESS_KEY", kind: "text" as const },
    {
      key: "region",
      label: "region",
      placeholder: "eu-west-1",
      kind: "text" as const,
      hint: "Picks the endpoint: email.<region>.amazonaws.com.",
    },
  ],
  sendgrid: [{ key: "api_key", label: "api key", placeholder: "$SENDGRID_API_KEY", kind: "text" as const }],
  brevo: [{ key: "api_key", label: "api key", placeholder: "$BREVO_API_KEY", kind: "text" as const }],
  mailjet: [
    { key: "api_key", label: "api key", placeholder: "$MAILJET_API_KEY", kind: "text" as const, hint: "The public key." },
    {
      key: "api_secret",
      label: "api secret",
      placeholder: "$MAILJET_SECRET_KEY",
      kind: "text" as const,
      hint: "The private key.",
    },
  ],
  mailgun: [
    { key: "api_key", label: "api key", placeholder: "$MAILGUN_API_KEY", kind: "text" as const },
    {
      key: "domain",
      label: "sending domain",
      placeholder: "mg.example.com",
      kind: "text" as const,
      hint: "The domain the messages are posted to.",
    },
  ],
  postmark: [{ key: "api_key", label: "server token", placeholder: "$POSTMARK_SERVER_TOKEN", kind: "text" as const }],
  resend: [{ key: "api_key", label: "api key", placeholder: "$RESEND_API_KEY", kind: "text" as const }],
};

const PAYMENT_PROVIDERS = [
  { value: "none", label: "none" },
  { value: "stripe", label: "stripe" },
] as const;

const BILLING_ADDRESS = ["auto", "required"] as const;
const AI_PROVIDERS = [
  { value: "none", label: "none" },
  { value: "openai", label: "openai" },
  { value: "anthropic", label: "anthropic" },
  { value: "custom", label: "custom" },
] as const;
const AI_ACCESS_OPTIONS = [
  { value: "public", label: "public — anyone" },
  { value: "authenticated", label: "authenticated — any signed-in caller" },
  { value: "member", label: "member — of the active organisation" },
  { value: "role", label: "role:… — a named org role" },
  { value: "private", label: "private — not exposed" },
] as const;

const QUEUE_PUBLISH_OPTIONS = [
  { value: "private", label: "private — no endpoint at all" },
  { value: "authenticated", label: "authenticated — any signed-in caller" },
  { value: "member", label: "member — of the active organisation" },
  { value: "role", label: "role:… — a named org role" },
  { value: "public", label: "public — anyone" },
] as const;

/**
 * Which option a written access string selects. `role:<name>` collapses to
 * `role`, and the name goes in the box beside it.
 *
 * `fallback` is the *server's* default for the setting, which differs: an
 * unwritten `[ai] access` is `authenticated`, an unwritten `[queues] publish`
 * is `private`. Showing the wrong one would make the form disagree with the
 * app it is describing.
 */
function accessLevelOf(
  value: string | number | boolean | undefined,
  fallback = "authenticated",
) {
  return typeof value === "string" && value.startsWith("role:") ? "role" : String(value ?? fallback);
}

function accessRoleOf(value: string | number | boolean | undefined) {
  return typeof value === "string" && value.startsWith("role:") ? value.slice(5) : "";
}

function aiFields(): ConfigField[] {
  const provider = String(configValue("ai", "provider") ?? "none");
  const endpointHint =
    provider === "custom"
      ? "Required for custom. Point it at any OpenAI-shaped server or gateway."
      : "Optional override. Leave empty to use the provider's own API endpoint.";
  const keyHint =
    provider === "custom"
      ? "Optional. An empty key sends no Authorization header at all."
      : "The provider credential. Leave empty only when the provider truly needs none.";

  return [
    {
      key: "provider",
      label: "provider",
      group: "Provider & model",
      kind: "select" as const,
      options: AI_PROVIDERS,
      hint: "`none` leaves chat and configured agents without a backing model provider.",
    },
    {
      key: "endpoint",
      label: "endpoint",
      group: "Provider & model",
      kind: "text" as const,
      placeholder:
        provider === "anthropic"
          ? "https://api.anthropic.com/v1/messages"
          : "http://localhost:8080 or https://api.openai.com/v1/chat/completions",
      hint: endpointHint,
    },
    {
      key: "model",
      label: "model",
      group: "Provider & model",
      kind: "text" as const,
      placeholder: provider === "custom" ? "local" : "gpt-4o-mini",
      hint: "Used whenever a request or agent does not override the model.",
    },
    {
      key: "api_key",
      label: "api key",
      group: "Provider & model",
      kind: "text" as const,
      placeholder:
        provider === "anthropic"
          ? "$ANTHROPIC_API_KEY"
          : provider === "custom"
            ? "optional"
            : "$OPENAI_API_KEY",
      hint: keyHint,
    },
    {
      key: "system",
      label: "system prompt",
      group: "Generation defaults",
      kind: "textarea" as const,
      placeholder: "Standing instructions for conversations that do not provide their own system prompt.",
      hint: "Applied when the request and message list do not carry a more specific system prompt.",
    },
    {
      key: "max_tokens",
      label: "max tokens",
      group: "Generation defaults",
      kind: "number" as const,
      placeholder: "2048",
      hint: "Cap per reply. Give reasoning models enough room or they can think and answer nothing.",
    },
    {
      key: "temperature",
      label: "temperature",
      group: "Generation defaults",
      kind: "number" as const,
      placeholder: "unset",
      hint: "Leave empty to let the provider decide. Values are sent only when set.",
    },
    {
      key: "reasoning",
      label: "reasoning traces",
      group: "Generation defaults",
      kind: "boolean" as const,
      hint: "Off by default. When on, provider reasoning is surfaced to callers and operators can reveal it per message in admin. Individual agents can override this.",
    },
    {
      key: "access",
      label: "access",
      group: "Access & limits",
      kind: "select" as const,
      options: AI_ACCESS_OPTIONS,
      hint: "Who may call <base>/ai/chat — `public`, `authenticated`, `member`, `role:<name>`, `private`.",
    },
    {
      key: "timeout_secs",
      label: "timeout (s)",
      group: "Access & limits",
      kind: "number" as const,
      placeholder: "300",
      hint: "Per completion. Long local-model replies are slow, not necessarily broken.",
    },
    {
      section: "admin.ai_assistance",
      key: "enabled",
      label: "Admin AI assistance",
      group: "Admin assistance",
      kind: "boolean" as const,
      hint: "Shows the admin’s field-writing AI helper when both this and the app-wide AI provider are configured.",
    },
    {
      section: "admin.ai_assistance",
      key: "system",
      label: "admin AI system prompt",
      group: "Admin assistance",
      kind: "textarea" as const,
      placeholder: "Return only the field content, ready to insert.",
      hint: "Optional system prompt sent only by the admin field helper.",
    },
    {
      section: "admin.ai_assistance",
      key: "prompt_placeholder",
      label: "admin AI prompt placeholder",
      group: "Admin assistance",
      kind: "text" as const,
      placeholder: "Describe what you want AI to write for this field.",
      hint: "Placeholder shown in the admin helper’s floating prompt box.",
    },
  ];
}

/** The `main.toml` sections the studio edits as forms. */
const LOG_FORMATS = [
  { value: "pretty", label: "pretty" },
  { value: "compact", label: "compact" },
  { value: "json", label: "json" },
] as const;

const OTLP_PROTOCOLS = [
  { value: "http/protobuf", label: "http/protobuf" },
  { value: "http/json", label: "http/json" },
] as const;

/**
 * `[observability]`, in the order somebody sets it up.
 *
 * Logs come first and are always shown, because they are the half that works
 * with nothing switched on: a server writes to its terminal whether or not
 * anybody is collecting traces. Everything below them is hidden until
 * `enabled`, since a sample ratio for spans nobody is building is a question
 * with no meaning.
 */
function observabilityFields(): ConfigField[] {
  const enabled = configValue("observability", "enabled") === true;

  const always: ConfigField[] = [
    {
      key: "enabled",
      label: "observability enabled",
      kind: "boolean" as const,
      hint: "Gives every request a span and an X-Trace-Id, and lets the logs carry its trace id. Nothing leaves the process until an OTLP endpoint is set below.",
    },
    {
      section: "observability.logs",
      key: "format",
      label: "log format",
      group: "Logs",
      kind: "select" as const,
      options: LOG_FORMATS,
      hint: "`pretty` for a terminal, `json` for anything that parses the line. This one applies whether or not observability is enabled.",
    },
    {
      section: "observability.logs",
      key: "level",
      label: "log level",
      group: "Logs",
      placeholder: "info,apiplant=debug,ntex_server=warn",
      kind: "text" as const,
      hint: "Used only when RUST_LOG is unset — the environment always wins, so a running container can be turned up without editing this.",
    },
    {
      section: "observability.logs",
      key: "span_fields",
      label: "span fields on every json line",
      group: "Logs",
      kind: "boolean" as const,
      defaultOn: true,
      hint: "JSON only. Puts the request's route, method and trace id on every line written during it — which is what makes “every line from the request that failed” a filter.",
    },
  ];

  if (!enabled) return always;

  return [
    ...always,
    {
      key: "service_name",
      label: "service name",
      group: "Service identity",
      kind: "text" as const,
      hint: "What this service is called in a trace. Unset uses OTEL_SERVICE_NAME, then the app's name.",
    },
    {
      key: "service_version",
      label: "service version",
      group: "Service identity",
      kind: "text" as const,
      hint: "The build being traced. Unset reports the apiplant version.",
    },
    {
      key: "environment",
      label: "environment",
      group: "Service identity",
      placeholder: "production",
      kind: "text" as const,
      hint: "Exported as deployment.environment.name — the attribute most backends group by first.",
    },
    {
      section: "observability.traces",
      key: "enabled",
      label: "build spans",
      group: "Traces",
      kind: "boolean" as const,
      defaultOn: true,
      hint: "On even with no collector: the trace id and the error fields a span carries are worth having in the logs alone.",
    },
    {
      section: "observability.traces",
      key: "sample_ratio",
      label: "sample ratio",
      group: "Traces",
      placeholder: "1.0",
      kind: "number" as const,
      hint: "Fraction of root requests recorded, 0–1. A request that arrives with a traceparent follows its caller instead. Keeping every failure and a fraction of the rest is tail sampling — a collector's job, not this server's.",
    },
    {
      section: "observability.traces",
      key: "response_header",
      label: "return X-Trace-Id",
      group: "Traces",
      kind: "boolean" as const,
      defaultOn: true,
      hint: "Hands the trace id back to the caller, so a bug report becomes a lookup rather than a search by timestamp.",
    },
    {
      section: "observability.traces",
      key: "exclude_paths",
      label: "never trace",
      group: "Traces",
      placeholder: "/_health",
      kind: "list" as const,
      hint: "Path prefixes, under base_path. Comma-separated. Health checks are noise you pay for per span. Empty here uses the default, /_health.",
    },
    {
      section: "observability.traces",
      key: "capture_headers",
      label: "capture request headers",
      group: "Traces",
      placeholder: "x-request-id",
      kind: "list" as const,
      hint: "Copied onto the span. Comma-separated. authorization, cookie and x-api-key are refused even if named — a captured credential is a credential in your log aggregator.",
    },
    {
      section: "observability.metrics",
      key: "enabled",
      label: "record metrics",
      group: "Metrics",
      kind: "boolean" as const,
      defaultOn: true,
      hint: "http.server.request.duration and http.server.active_requests, labelled by a route template so a busy table does not become a time series per row. Needs an endpoint below to go anywhere.",
    },
    {
      section: "observability.metrics",
      key: "interval_secs",
      label: "push interval (seconds)",
      group: "Metrics",
      placeholder: "60",
      kind: "number" as const,
    },
    {
      section: "observability.otlp",
      key: "endpoint",
      label: "collector endpoint",
      group: "OTLP export",
      placeholder: "http://localhost:4318",
      kind: "text" as const,
      hint: "Any OTLP/HTTP receiver — the OpenTelemetry Collector, Jaeger, Tempo, Honeycomb, Datadog, Grafana Cloud. /v1/traces and /v1/metrics are appended. Empty falls back to OTEL_EXPORTER_OTLP_ENDPOINT, and unset in both places keeps everything in-process.",
    },
    {
      section: "observability.otlp",
      key: "protocol",
      label: "protocol",
      group: "OTLP export",
      kind: "select" as const,
      options: OTLP_PROTOCOLS,
      hint: "Every collector accepts http/protobuf. Transport is HTTP either way — there is no gRPC exporter.",
    },
    {
      section: "observability.otlp",
      key: "timeout_secs",
      label: "export timeout (seconds)",
      group: "OTLP export",
      placeholder: "10",
      kind: "number" as const,
      hint: "A batch is dropped rather than blocking the process behind a collector that stopped answering.",
    },
  ];
}

const SECTIONS: ConfigSection[] = [
  {
    id: "application",
    title: "Server",
    hint: "Naming, networking, routing, sign-in defaults and the API docs, each under its own heading.",
    fields: [
      {
        section: "app",
        key: "name",
        label: "name",
        group: "Identity",
        placeholder: "the directory name",
        kind: "text" as const,
        hint: "Heads the admin dashboard as “<name> admin”. Unset uses the directory name.",
      },
      {
        section: "server",
        key: "host",
        label: "host",
        group: "Networking",
        placeholder: "0.0.0.0",
        kind: "text" as const,
        hint: "Bind address. Unset listens on every interface; set it only to narrow the bind.",
      },
      {
        section: "server",
        key: "port",
        label: "port",
        group: "Networking",
        placeholder: "8080",
        kind: "number" as const,
        hint: "TCP port.",
      },
      {
        section: "server",
        key: "domain",
        label: "domain",
        group: "Networking",
        placeholder: "any host",
        kind: "text" as const,
        hint: "When set, other Host headers get no match.",
      },
      {
        section: "server",
        key: "base_path",
        label: "base path",
        group: "Routing",
        placeholder: "/",
        kind: "text" as const,
        hint: "Prefix for every route. /api ⇒ endpoints live at /api/…",
      },
      {
        section: "server",
        key: "workers",
        label: "workers",
        group: "Runtime",
        placeholder: "one per CPU",
        kind: "number" as const,
        hint: "OS worker threads.",
      },
      {
        section: "auth",
        key: "jwt_secret",
        label: "jwt secret",
        group: "Authentication",
        placeholder: "random per boot",
        kind: "text" as const,
        hint: "Set it in production — an empty secret means tokens die with the process.",
      },
      {
        section: "auth",
        key: "session_ttl_secs",
        label: "session ttl (s)",
        group: "Authentication",
        placeholder: "604800",
        kind: "number" as const,
        hint: "Lifetime of issued session tokens.",
      },
      {
        section: "auth",
        key: "allow_registration",
        label: "registration open",
        group: "Authentication",
        kind: "boolean" as const,
        hint: "Whether anybody may create an account for themselves.",
      },
      {
        section: "auth",
        key: "verify_email_redirect",
        label: "verified redirect",
        group: "Authentication",
        placeholder: "nowhere — stay on the confirmation screen",
        kind: "text" as const,
        hint: "Where somebody lands once they confirm their address. An absolute URL or a path on this origin. Confirming signs them in first, so the app is reached already authenticated.",
      },
      {
        section: "admin",
        key: "gravatar",
        label: "gravatar avatars",
        group: "Admin dashboard",
        kind: "boolean" as const,
        hint: "Off by default. When on, an account with no avatar_url is drawn with its Gravatar, which means a request to gravatar.com for every face. Initials are the fallback either way.",
      },
      { section: "docs", key: "enabled", label: "swagger ui enabled", group: "API documentation", kind: "boolean" as const },
      {
        section: "docs",
        key: "path",
        label: "ui path",
        group: "API documentation",
        placeholder: "/docs",
        kind: "text" as const,
      },
      {
        section: "docs",
        key: "title",
        label: "title",
        group: "API documentation",
        placeholder: "the app name",
        kind: "text" as const,
        hint: "Only when the published API answers to a different name than the app.",
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
        group: "Connection URL",
        placeholder: "postgres://user:pass@host:5432/db",
        kind: "text" as const,
        hint: "Full connection URL; leave empty to assemble from the parts below.",
      },
      { key: "host", label: "host", group: "Connection parts", placeholder: "localhost", kind: "text" as const },
      { key: "port", label: "port", group: "Connection parts", placeholder: "5432", kind: "number" as const },
      { key: "name", label: "database", group: "Connection parts", placeholder: "apiplant", kind: "text" as const },
      { key: "user", label: "user", group: "Connection parts", placeholder: "postgres", kind: "text" as const },
      { key: "password", label: "password", group: "Connection parts", placeholder: "postgres", kind: "text" as const },
      { key: "max_connections", label: "pool size", group: "Pool & schema", placeholder: "16", kind: "number" as const },
      {
        key: "auto_migrate",
        label: "auto migrate",
        group: "Pool & schema",
        kind: "boolean" as const,
        hint: "Run pending migrations when the app boots.",
      },
    ],
  },
  {
    id: "organization",
    title: "Organizations",
    hint: "The tenant itself. An organisation's `org_class` decides which `@org_class=` permissions apply inside it, so who may write that column is a deployment decision rather than a row-level one.",
    fields: [
      {
        key: "org_class_editors",
        label: "org class editors",
        group: "Classes",
        placeholder: "private",
        kind: "text" as const,
        hint: "Who may set an organisation's `org_class`, in the `[permissions]` grammar — typically a class of its own, e.g. `member@org_class=staff`. Unset means nobody: the column is server-owned and classes come from seed data or SQL.",
      },
      {
        key: "default_org_class",
        label: "default org class",
        group: "Classes",
        placeholder: "none",
        kind: "text" as const,
        hint: "The class every new organisation starts with, personal ones included. Unset leaves them unclassed, which no `@org_class=` permission matches. A class editor naming one on create is not overridden.",
      },
    ],
  },
  {
    id: "email",
    title: "Email",
    hint: "Outbound mail for functions and auth flows that need a mailbox.",
    fields: () => {
      const provider = String(configValue("email", "provider") ?? "none");
      const credentialsGroup = provider === "smtp" ? "SMTP server" : "Credentials";
      const specific = (EMAIL_PROVIDER_FIELDS[provider] ?? []).map((field) => ({
        ...field,
        group: field.group ?? credentialsGroup,
      }));
      const chooser: ConfigField = {
        key: "provider",
        label: "provider",
        group: "Provider",
        kind: "select" as const,
        options: EMAIL_PROVIDERS,
        hint: "`none` leaves email off.",
      };
      if (provider === "none") return [chooser];
      return [
        chooser,
        {
          key: "from",
          label: "from",
          group: "Sender identity",
          placeholder: "no-reply@example.com",
          kind: "text" as const,
          hint: "Envelope sender. Required once a provider is named.",
        },
        { key: "from_name", label: "from name", group: "Sender identity", placeholder: "Acme Logistics", kind: "text" as const },
        { key: "reply_to", label: "reply to", group: "Sender identity", placeholder: "support@example.com", kind: "text" as const },
        ...specific,
        { key: "timeout_secs", label: "timeout (s)", group: "Delivery", placeholder: "15", kind: "number" as const },
      ];
    },
  },
  {
    id: "cache",
    title: "Caching",
    hint: "An optional Redis functions can reach.",
    fields: [
      {
        key: "url",
        label: "url",
        group: "Connection",
        placeholder: "redis://127.0.0.1:6379",
        kind: "text" as const,
        hint: "Leave empty to keep caching off.",
      },
      {
        key: "enabled",
        label: "enabled",
        group: "Connection",
        kind: "boolean" as const,
        hint: "Keeps the cache off without deleting its settings.",
      },
      { key: "prefix", label: "prefix", group: "Keys & expiry", placeholder: "my-app:", kind: "text" as const },
      {
        key: "default_ttl_secs",
        label: "default ttl (s)",
        group: "Keys & expiry",
        placeholder: "0",
        kind: "number" as const,
      },
      { key: "timeout_secs", label: "timeout (s)", group: "Limits", placeholder: "5", kind: "number" as const },
    ],
  },
  {
    id: "queues",
    title: "Queues",
    hint:
      "Work that happens after the response. `publish` writes a row and fires a Postgres NOTIFY; " +
      "a subscriber claims it and runs a function. No broker to deploy.",
    fields: [
      {
        key: "enabled",
        label: "enabled",
        group: "Handling",
        kind: "boolean" as const,
        hint: "Pauses handling without deleting the subscriptions below. Publishing still records rows, so nothing is lost while it is off.",
      },
      {
        key: "poll_secs",
        label: "poll (s)",
        group: "Handling",
        placeholder: "30",
        kind: "number" as const,
        hint: "The sweep beneath the NOTIFY. Delivery is immediate either way; this is what catches a missed notification.",
      },
      {
        key: "batch",
        label: "batch",
        group: "Handling",
        placeholder: "10",
        kind: "number" as const,
      },
      {
        key: "max_attempts",
        label: "max attempts",
        group: "Retries",
        placeholder: "5",
        kind: "number" as const,
        hint: "Then the message is left `failed` for a person to look at. 1 means no retries.",
      },
      {
        key: "retry_backoff_secs",
        label: "retry backoff (s)",
        group: "Retries",
        placeholder: "10",
        kind: "number" as const,
        hint: "Doubles each attempt: 10s, 20s, 40s, 80s.",
      },
      {
        key: "lease_secs",
        label: "lease (s)",
        group: "Retries",
        placeholder: "300",
        kind: "number" as const,
        hint: "Before a claimed message is offered to another subscriber. Set it above your slowest handler.",
      },
      {
        key: "retain_hours",
        label: "retain (h)",
        group: "Retention",
        placeholder: "24",
        kind: "number" as const,
        hint: "Deletes handled messages after this. 0 keeps them. `failed` rows are never swept.",
      },
      {
        key: "prefix",
        label: "channel prefix",
        group: "Retention",
        placeholder: "apiplant",
        kind: "text" as const,
        hint: "So two apps sharing one database do not wake each other.",
      },
      {
        key: "publish",
        label: "http publish",
        group: "Publishing over HTTP",
        kind: "select" as const,
        options: QUEUE_PUBLISH_OPTIONS,
        hint: "Who may POST <base>/queues/{topic}. `private` — the default — means there is no such endpoint.",
      },
    ],
  },
  {
    id: "payments",
    title: "Payments",
    hint: "Taking money. Naming a provider also adds the billing_* resources and the /billing endpoints.",
    fields: [
      {
        key: "provider",
        label: "provider",
        group: "Provider",
        kind: "select" as const,
        options: PAYMENT_PROVIDERS,
        hint: "`none` leaves payments off — and leaves the billing tables out of the app.",
      },
      {
        key: "secret_key",
        label: "secret key",
        group: "Credentials",
        placeholder: "$STRIPE_SECRET_KEY",
        kind: "text" as const,
        hint: "The `sk_…` key. Required once a provider is named.",
      },
      {
        key: "publishable_key",
        label: "publishable key",
        group: "Credentials",
        placeholder: "$STRIPE_PUBLISHABLE_KEY",
        kind: "text" as const,
        hint: "The `pk_…` key. Not a secret — it is served to the browser.",
      },
      {
        key: "webhook_secret",
        label: "webhook secret",
        group: "Credentials",
        placeholder: "$STRIPE_WEBHOOK_SECRET",
        kind: "text" as const,
        hint: "The `whsec_…` signing secret. Without it nothing bought is ever recorded.",
      },
      {
        key: "currency",
        label: "currency",
        group: "Pricing & tax",
        placeholder: "usd",
        kind: "text" as const,
        hint: "ISO 4217, for prices that don't name one.",
      },
      {
        key: "billing_address",
        label: "billing address",
        group: "Pricing & tax",
        kind: "select" as const,
        options: BILLING_ADDRESS,
        hint: "`auto` collects what the card and tax need; `required` asks for all of it.",
      },
      {
        key: "success_url",
        label: "success url",
        group: "Checkout redirects",
        placeholder: "https://example.com/thanks",
        kind: "text" as const,
        hint: "Where a buyer lands after paying. Empty uses the dashboard's billing screen.",
      },
      {
        key: "cancel_url",
        label: "cancel url",
        group: "Checkout redirects",
        placeholder: "https://example.com/pricing",
        kind: "text" as const,
      },
      { key: "timeout_secs", label: "timeout (s)", group: "Limits", placeholder: "20", kind: "number" as const },
      {
        key: "automatic_tax",
        label: "automatic tax",
        group: "Pricing & tax",
        kind: "boolean" as const,
        hint: "Compute tax automatically and quote prices accordingly.",
      },
    ],
  },
  {
    id: "ai",
    title: "AI",
    hint: "The backing provider for /ai/chat, configured agents, and ctx.chat inside functions.",
    fields: aiFields,
  },
  {
    id: "observability",
    title: "Observability",
    hint: "What the server says about itself: structured logs, OpenTelemetry traces and HTTP metrics, exported over OTLP to any collector.",
    fields: observabilityFields,
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
  const table = field.section ?? sectionId;
  if (table === "app" && field.key === "name") return directory ?? field.placeholder;
  if (table === "docs" && field.key === "title") return appName ?? field.placeholder;
  return field.placeholder;
}

function sectionFields(section: ConfigSection): ConfigField[] {
  return typeof section.fields === "function" ? section.fields() : section.fields;
}

/**
 * The section's fields, in labelled groups. Order follows the field list, so a
 * group appears where it is first named and its fields stay in their declared
 * order; anything without a group leads the section under no heading at all.
 * A section whose fields name no group renders exactly as it did before.
 */
function groupedFields(section: ConfigSection): FieldGroup[] {
  const groups: FieldGroup[] = [];
  for (const field of sectionFields(section)) {
    const title = field.group ?? null;
    const existing = groups.find((group) => group.title === title);
    if (existing) existing.fields.push(field);
    else groups.push({ title, fields: [field] });
  }
  const ungrouped = groups.find((group) => group.title === null);
  if (ungrouped) return [ungrouped, ...groups.filter((group) => group !== ungrouped)];
  return groups;
}

/** Where a field's value lives — its own table when it names one. */
function fieldSection(section: ConfigSection, field: ConfigField): string {
  return field.section ?? section.id;
}

/**
 * Whether a switch draws as on: the value when the key is set, and otherwise
 * what the server would do without it.
 */
function switchIsOn(table: string, field: ConfigField): boolean {
  const current = configValue(table, field.key);
  if (typeof current === "boolean") return current;
  return field.defaultOn === true;
}

function selectDefaultValue(field: ConfigField): string {
  return selectOptionsList(field)[0]?.value ?? "";
}

function hasSelectOption(field: ConfigField, value: string): boolean {
  return selectOptionsList(field).some((option) => option.value === value);
}

function selectOptionsList(field: ConfigField): { value: string; label: string }[] {
  return (field.options ?? []).map((option) => (typeof option === "string" ? { value: option, label: option } : option));
}

function selectOptions(sectionId: string, field: ConfigField) {
  const current = configValue(sectionId, field.key);
  if (typeof current !== "string" || current === "" || hasSelectOption(field, current)) return selectOptionsList(field);
  return [{ value: current, label: `${current} (current)` }, ...selectOptionsList(field)];
}

/**
 * An access string with its optional role, as one control.
 *
 * `role:<name>` is two facts in one value — which level, and which role — so it
 * is a select plus a box that appears only when it means something. Shared by
 * `[ai] access` and `[queues] publish`, which use the same grammar.
 */
function AccessField(props: {
  table: string;
  field: string;
  options: readonly { value: string; label: string }[];
  fallback: string;
}) {
  const current = () => configValue(props.table, props.field);
  const level = () => accessLevelOf(current(), props.fallback);

  return (
    <div class="flex gap-2">
      <Select
        class="flex-1"
        value={level()}
        options={props.options}
        onChange={(value) =>
          setConfigValue(
            props.table,
            props.field,
            value === "role" ? `role:${accessRoleOf(current()) || "admin"}` : value,
          )
        }
      />
      <Show when={level() === "role"}>
        <TextInput
          mono
          class="max-w-[10rem]"
          value={accessRoleOf(current())}
          placeholder="admin"
          onInput={(value) => setConfigValue(props.table, props.field, `role:${value}`)}
        />
      </Show>
    </div>
  );
}

/**
 * A list of strings, edited as one comma-separated line.
 *
 * A row-per-entry editor would be the richer control, but these lists are two
 * or three short tokens — header names, path prefixes — and a line of them is
 * quicker to read and quicker to change than a stack of inputs.
 *
 * The text is held locally while it is being typed: splitting on every
 * keystroke would drop the comma the moment it is pressed, and the caret with
 * it. It is written through on each edit all the same, so nothing waits for a
 * blur to be saved.
 */
function ListField(props: { table: string; field: string; placeholder?: string }) {
  const saved = createMemo(() => configList(props.table, props.field).join(", "));
  const [typed, setTyped] = createSignal<string | null>(null);
  // Anything that changes the list elsewhere — the TOML tab, a project reload —
  // ends the local edit, so the box never disagrees with the file.
  createEffect(saved, () => {
    setTyped(null);
  });

  return (
    <TextInput
      mono
      value={typed() ?? saved()}
      placeholder={props.placeholder}
      onInput={(value) => {
        setTyped(value);
        setConfigList(
          props.table,
          props.field,
          value.split(",").map((item) => item.trim()),
        );
      }}
    />
  );
}

/**
 * `[queues.subscribe]`: which function handles which topic.
 *
 * Its own card because it is a map rather than a set of scalar settings, and
 * because it is the part of `[queues]` anybody actually comes here to change —
 * the rest are knobs with sensible defaults.
 *
 * Nothing here names a *publisher*. A topic is announced by whatever publishes
 * it — a function's `publish`, a model's `[publish]`, the HTTP endpoint — and
 * none of them know this table exists. That indirection is the point: adding a
 * second handler to a topic is one line here and no change anywhere else.
 */
function SubscriptionsCard() {
  const entries = createMemo(subscriptions);
  const known = createMemo(() => allFunctionNames());

  const edit = (update: (draft: Subscription[]) => void) => {
    const draft = entries().map((entry) => ({ topic: entry.topic, functions: [...entry.functions] }));
    update(draft);
    setSubscriptions(draft);
  };

  // A topic mid-edit has no functions yet, so it would be dropped on write.
  // Held here until it names one, which is what lets the row exist at all.
  const [pending, setPending] = createSignal<Subscription[]>([]);
  const rows = createMemo(() => [...entries(), ...pending()]);

  const commit = (index: number, next: Subscription) => {
    const saved = entries().length;
    if (index < saved) {
      edit((draft) => {
        draft[index] = next;
      });
      return;
    }
    // A pending row that now names both is a real subscription.
    if (next.topic.trim() && next.functions.length > 0) {
      setPending((current) => current.filter((_, i) => i !== index - saved));
      edit((draft) => draft.push(next));
      return;
    }
    setPending((current) => current.map((row, i) => (i === index - saved ? next : row)));
  };

  const remove = (index: number) => {
    const saved = entries().length;
    if (index < saved) edit((draft) => draft.splice(index, 1));
    else setPending((current) => current.filter((_, i) => i !== index - saved));
  };

  const unknown = createMemo(() =>
    rows().flatMap((row) => row.functions.filter((name) => name && !known().includes(name))),
  );

  return (
    <Card>
      <CardHeader
        title="Subscriptions"
        hint="Topic → the function(s) that handle it. Each subscriber gets its own retries, so one failing never re-runs another."
      />
      <datalist id="queue-function-names">
        <For each={known()}>{(name) => <option value={name} />}</For>
      </datalist>

      <div class="space-y-3 px-4 py-4">
        <Show
          when={rows().length > 0}
          fallback={
            <p class="text-xs leading-relaxed text-muted">
              Nothing subscribes yet. A message published to a topic with no subscriber is still recorded in{" "}
              <Mono>queue_message</Mono> — so it is never lost — but no function runs.
            </p>
          }
        >
          <For each={rows()}>
            {(row, index) => (
              <div class="grid gap-2 sm:grid-cols-[1fr_1fr_auto] sm:items-end">
                <Labelled label="topic" hint={index() === 0 ? "letters, digits, . _ - :" : undefined}>
                  <TextInput
                    mono
                    value={row.topic}
                    placeholder="order.paid"
                    onInput={(value) => commit(index(), { ...row, topic: value })}
                  />
                </Labelled>
                <Labelled
                  label="functions"
                  hint={index() === 0 ? "comma-separated for several" : undefined}
                >
                  <TextInput
                    mono
                    list="queue-function-names"
                    value={row.functions.join(", ")}
                    placeholder="fulfilOrder"
                    onInput={(value) =>
                      commit(index(), {
                        ...row,
                        functions: value
                          .split(",")
                          .map((name) => name.trim())
                          .filter(Boolean),
                      })
                    }
                  />
                </Labelled>
                <Button variant="ghost" size="sm" onClick={() => remove(index())}>
                  Remove
                </Button>
              </div>
            )}
          </For>
        </Show>

        <Button
          variant="secondary"
          size="sm"
          onClick={() => setPending((current) => [...current, { topic: "", functions: [] }])}
        >
          Add topic
        </Button>
      </div>

      <Show when={unknown().length > 0}>
        <div class="border-t border-line px-4 py-3 text-xs leading-relaxed text-warn">
          <Mono>{unknown().join(", ")}</Mono> — no library in <Mono>functions/</Mono> exports that. Messages on
          its topic will retry and then land in the dead-letter until it is built and dropped in.
        </div>
      </Show>
    </Card>
  );
}

/**
 * A string→string table, edited as rows.
 *
 * Its own card for the same reason `[queues.subscribe]` has one: the *keys* are
 * data, not a fixed set of settings, so there is no field list to render. Used
 * twice — resource attributes and export headers — which is why it takes the
 * table and key rather than knowing either.
 */
function EntriesCard(props: {
  title: string;
  hint: string;
  table: string;
  field: string;
  keyPlaceholder: string;
  valuePlaceholder: string;
  empty: string;
  addLabel: string;
}) {
  const saved = createMemo(() => configEntries(props.table, props.field));
  // A row is not a setting until it has a name, so a blank one has nowhere to
  // live in the config until then. It waits here instead of vanishing as it is
  // typed into.
  const [pending, setPending] = createSignal<ConfigEntry[]>([]);
  const rows = createMemo(() => [...saved(), ...pending()]);

  const commit = (index: number, next: ConfigEntry) => {
    const count = saved().length;
    if (index < count) {
      const draft = saved().map((entry, i) => (i === index ? next : entry));
      // Renaming a row to nothing would silently drop it; keep it pending.
      if (!next.key.trim()) {
        setPending((current) => [...current, next]);
        setConfigEntries(props.table, props.field, draft.filter((_, i) => i !== index));
        return;
      }
      setConfigEntries(props.table, props.field, draft);
      return;
    }
    const at = index - count;
    if (next.key.trim()) {
      setPending((current) => current.filter((_, i) => i !== at));
      setConfigEntries(props.table, props.field, [...saved(), next]);
      return;
    }
    setPending((current) => current.map((row, i) => (i === at ? next : row)));
  };

  const remove = (index: number) => {
    const count = saved().length;
    if (index < count) setConfigEntries(props.table, props.field, saved().filter((_, i) => i !== index));
    else setPending((current) => current.filter((_, i) => i !== index - count));
  };

  return (
    <Card>
      <CardHeader title={props.title} hint={props.hint} />
      <div class="space-y-3 px-4 py-4">
        <Show
          when={rows().length > 0}
          fallback={<p class="text-xs leading-relaxed text-muted">{props.empty}</p>}
        >
          <For each={rows()}>
            {(row, index) => (
              <div class="grid gap-2 sm:grid-cols-[1fr_1fr_auto] sm:items-end">
                <Labelled label="key">
                  <TextInput
                    mono
                    value={row.key}
                    placeholder={props.keyPlaceholder}
                    onInput={(value) => commit(index(), { ...row, key: value })}
                  />
                </Labelled>
                <Labelled label="value" hint={index() === 0 ? "$VAR reads the environment" : undefined}>
                  <TextInput
                    mono
                    value={row.value}
                    placeholder={props.valuePlaceholder}
                    onInput={(value) => commit(index(), { ...row, value })}
                  />
                </Labelled>
                <Button variant="ghost" size="sm" onClick={() => remove(index())}>
                  Remove
                </Button>
              </div>
            )}
          </For>
        </Show>

        <Button
          variant="secondary"
          size="sm"
          onClick={() => setPending((current) => [...current, { key: "", value: "" }])}
        >
          {props.addLabel}
        </Button>
      </div>
    </Card>
  );
}

export function ConfigPage() {
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

  const selectedSectionId = createMemo(() => {
    const current = view();
    if (current.kind !== "config" || !current.section) return null;
    return SECTIONS.some((section) => section.id === current.section) ? current.section : null;
  });

  /**
   * One row of tabs — every section, then the raw file. Sections stay in the
   * route so links keep working; the TOML tab is a view of the same file and
   * lives only here.
   */
  const [tab, setTab] = createSignal<TabId>(selectedSectionId() ?? DEFAULT_SECTION_ID);
  createEffect(selectedSectionId, (id) => {
    if (id) setTab(id);
  });

  const chooseTab = (id: TabId) => {
    setTab(id);
    if (id !== TOML_TAB_ID) setView({ kind: "config", section: id });
  };

  const currentSection = createMemo(() =>
    SECTIONS.find((section) => section.id === tab()) ?? null,
  );

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
              That is a valid app — the server fills in every default. Create one to pin the port, database,
              mailer, cache, AI provider, payments and JWT secret.
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
            onChange={chooseTab}
            tabs={[
              ...SECTIONS.map((section) => ({ id: section.id, label: section.title })),
              { id: TOML_TAB_ID, label: "TOML" },
            ]}
          />
        </div>

        <Show when={currentSection()}>
          {(section) => (
            <div class="space-y-4">
            <Card>
              <CardHeader title={section().title} hint={section().hint} />
              <div class="grid gap-4 px-4 py-4 sm:grid-cols-2">
                <For each={groupedFields(section())}>
                  {(group, groupIndex) => (
                    <>
                      <Show when={group.title}>
                        <h3
                          class={`sm:col-span-2 text-[0.6875rem] font-semibold uppercase tracking-wider text-faint ${
                            groupIndex() === 0 ? "" : "mt-2 border-t border-surface-3 pt-4"
                          }`}
                        >
                          {group.title}
                        </h3>
                      </Show>
                      <For each={group.fields}>
                        {(field) => (
                          <Show
                            when={field.kind !== "boolean"}
                            fallback={
                              <div class="sm:col-span-2">
                                <Switch
                                  checked={switchIsOn(fieldSection(section(), field), field)}
                                  label={field.label}
                                  onChange={(value) =>
                                    setConfigValue(
                                      fieldSection(section(), field),
                                      field.key,
                                      // Back to the server's own default writes
                                      // nothing, so the file only records what
                                      // this app actually changed.
                                      field.defaultOn && value ? undefined : value,
                                    )
                                  }
                                />
                                <Show when={field.hint}>
                                  <p class="mt-1 text-[0.6875rem] leading-relaxed text-faint">{field.hint}</p>
                                </Show>
                              </div>
                            }
                          >
                            <Labelled label={field.label} hint={field.hint}>
                              <Show
                                when={field.kind === "list"}
                                fallback={
                              <Show
                                when={
                                  (section().id === "ai" && field.key === "access") ||
                                  (section().id === "queues" && field.key === "publish")
                                }
                                fallback={
                                  <Show
                                    when={field.kind === "select"}
                                    fallback={
                                      <Show
                                        when={field.kind === "textarea"}
                                        fallback={
                                          <TextInput
                                            mono
                                            type={field.kind === "number" ? "number" : "text"}
                                            value={String(configValue(fieldSection(section(), field), field.key) ?? "")}
                                            placeholder={fallbackPlaceholder(section().id, field)}
                                            onInput={(value) => {
                                              const table = fieldSection(section(), field);
                                              if (value === "") return setConfigValue(table, field.key, undefined);
                                              if (field.kind === "number") {
                                                const parsed = Number(value);
                                                return setConfigValue(
                                                  table,
                                                  field.key,
                                                  Number.isFinite(parsed) ? parsed : undefined,
                                                );
                                              }
                                              setConfigValue(table, field.key, value);
                                            }}
                                          />
                                        }
                                      >
                                        <TextArea
                                          mono
                                          class="min-h-32"
                                          value={String(configValue(fieldSection(section(), field), field.key) ?? "")}
                                          placeholder={fallbackPlaceholder(section().id, field)}
                                          onInput={(value) => {
                                            const table = fieldSection(section(), field);
                                            setConfigValue(table, field.key, value === "" ? undefined : value);
                                          }}
                                        />
                                      </Show>
                                    }
                                  >
                                    <Select
                                      value={String(
                                        configValue(fieldSection(section(), field), field.key) ?? selectDefaultValue(field),
                                      )}
                                      options={selectOptions(fieldSection(section(), field), field)}
                                      onChange={(value) =>
                                        setConfigValue(
                                          fieldSection(section(), field),
                                          field.key,
                                          value === selectDefaultValue(field) ? undefined : value,
                                        )
                                      }
                                    />
                                  </Show>
                                }
                              >
<AccessField
                                  table={section().id}
                                  field={field.key}
                                  options={section().id === "ai" ? AI_ACCESS_OPTIONS : QUEUE_PUBLISH_OPTIONS}
                                  fallback={section().id === "ai" ? "authenticated" : "private"}
                                />
                              </Show>
                                }
                              >
                                <ListField
                                  table={fieldSection(section(), field)}
                                  field={field.key}
                                  placeholder={field.placeholder}
                                />
                              </Show>
                            </Labelled>
                          </Show>
                        )}
                      </For>
                    </>
                  )}
                </For>
              </div>
            </Card>

            <Show when={section().id === "queues"}>
              <SubscriptionsCard />
            </Show>

            <Show when={section().id === "observability" && configValue("observability", "enabled") === true}>
              <EntriesCard
                title="Resource attributes"
                hint="Attached to every span and metric this service reports, beside its name and version."
                table="observability"
                field="resource_attributes"
                keyPlaceholder="region"
                valuePlaceholder="eu-west-1"
                empty="None. Add one where a backend needs to tell this deployment apart from another running the same build — region, cluster, tenant."
                addLabel="Add attribute"
              />
              <EntriesCard
                title="Export headers"
                hint="Sent with every export request. This is where a vendor's ingest key goes — write it as $VAR so the key itself stays out of the file."
                table="observability.otlp"
                field="headers"
                keyPlaceholder="authorization"
                valuePlaceholder="$OTEL_TOKEN"
                empty="None. A collector on your own network usually needs none; a hosted backend needs its API key."
                addLabel="Add header"
              />
            </Show>

            <Show when={section().id === "application"}>
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
            </Show>
            </div>
          )}
        </Show>

        <Show when={tab() === TOML_TAB_ID}>
          <div>
            <div class="mb-2 flex items-center justify-between">
              <p class="text-xs text-muted">Edit the file directly; the other tabs follow.</p>
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
