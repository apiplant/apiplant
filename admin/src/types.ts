/**
 * The shape of `apiplant-admin.json`, written by `apiplant admin`.
 *
 * Everything app-specific lives here; the shipped JavaScript is identical for
 * every application. Keep this in step with `crates/apiplant/src/admin.rs`.
 */

export type JsonValue = null | boolean | number | string | JsonValue[] | { [key: string]: JsonValue };

export type FieldType =
  | "string"
  | "text"
  | "integer"
  | "big_int"
  | "float"
  | "boolean"
  | "uuid"
  | "timestamp"
  | "json"
  | "file"
  | "reference";

/** Concrete input to render. The generator resolves `auto` for us. */
export type Widget =
  | "text"
  | "textarea"
  | "select"
  | "email"
  | "url"
  | "password"
  | "color"
  | "date"
  | "date_time"
  | "json"
  | "switch"
  | "number"
  | "file"
  | "reference";

/** What a free-text field holds. `plain` is an ordinary textarea. */
export type ContentFormat = "plain" | "markdown" | "html";

export interface FieldOption {
  value: string;
  label: string;
}

export interface FieldManifest {
  name: string;
  label: string;
  type: FieldType;
  widget: Widget;
  help: string | null;
  placeholder: string | null;
  /** Markup the editor highlights and previews; `plain` for everything else. */
  format: ContentFormat;
  options: FieldOption[];
  required: boolean;
  unique: boolean;
  /** Stripped from API responses entirely. */
  hidden: boolean;
  /** Present in the API but deliberately not shown in the dashboard. */
  admin_visible: boolean;
  readonly: boolean;
  max_length: number | null;
  references: string | null;
  relation: string | null;
  on_delete: "restrict" | "set_null" | "cascade" | "no_action" | null;
  default_value: JsonValue;
  writable: boolean;
}

export interface RelationManifest {
  field: string;
  relation: string;
  target: string;
  label: string;
  required: boolean;
}

export interface ChildManifest {
  resource: string;
  field: string;
  label: string;
}

/** One of the five CRUD actions, in the shared `[permissions]` grammar. */
export interface ActionPermissionManifest {
  value: string;
  role: string | null;
  note: string;
  requires_org: boolean;
}

export interface ActionPermissionsManifest {
  list: ActionPermissionManifest;
  read: ActionPermissionManifest;
  create: ActionPermissionManifest;
  update: ActionPermissionManifest;
  delete: ActionPermissionManifest;
}

export type Action = keyof ActionPermissionsManifest;

export interface ResourceManifest {
  name: string;
  label: string;
  plural: string;
  group: string | null;
  order: number;
  builtin: boolean;
  auth_resource: boolean;
  visible: boolean;
  roles: string[];
  scope: "organization" | "global";
  owner_field: string;
  display_field: string | null;
  search_field: string | null;
  /** Every field `?search=` covers; empty when nothing is searchable. */
  search_fields: string[];
  columns: string[];
  fields: FieldManifest[];
  relations: RelationManifest[];
  children: ChildManifest[];
  permissions: ActionPermissionsManifest;
}

export interface FunctionManifest {
  name: string;
  label: string;
  description: string;
  group: string | null;
  order: number;
  method: "GET" | "POST" | "PUT" | "DELETE";
  permission: string;
  role: string | null;
  permission_note: string;
  requires_org: boolean;
  visible: boolean;
  roles: string[];
  confirm: string | null;
  run_label: string;
  input_schema: JsonSchema | null;
  output_schema: JsonSchema | null;
}

export interface AgentManifest {
  name: string;
  label: string;
  description: string;
  scope: "organization" | "global";
  storage: boolean;
  reasoning_enabled: boolean;
  thread_resource: string | null;
  message_resource: string | null;
  chat: ActionPermissionManifest;
  history: ActionPermissionManifest;
  delete_history: ActionPermissionManifest;
}

/** The slice of JSON Schema the action form understands. */
export interface JsonSchema {
  type?: string | string[];
  title?: string;
  description?: string;
  properties?: Record<string, JsonSchema>;
  required?: string[];
  enum?: JsonValue[];
  format?: string;
  default?: JsonValue;
  items?: JsonSchema;
  minimum?: number;
  maximum?: number;
  $ref?: string;
  $defs?: Record<string, JsonSchema>;
  definitions?: Record<string, JsonSchema>;
  anyOf?: JsonSchema[];
  oneOf?: JsonSchema[];
  allOf?: JsonSchema[];
}

export interface AuthManifest {
  identity_field: string;
  identity_label: string;
  allow_registration: boolean;
  /** Whether the server can send email; the three flags below depend on it. */
  email_enabled: boolean;
  /** New accounts must confirm their address before they can sign in. */
  require_email_verification: boolean;
  /** The team screen may invite somebody who has no account yet. */
  invitations_enabled: boolean;
  /** "Forgot your password?" is worth offering. */
  password_reset_enabled: boolean;
  signup_fields: FieldManifest[];
  profile_fields: FieldManifest[];
  known_roles: string[];
  /**
   * Third-party sign-ins this deployment offers, in the order to draw them.
   * Empty unless `main.toml` has an `[oauth.<provider>]` block with
   * credentials, which is what keeps a sign-in screen from offering a button
   * that would land on a 404.
   */
  oauth_providers: OAuthProviderManifest[];
}

export interface OAuthProviderManifest {
  /** The name it is configured under: `github`, `google`, … */
  provider: string;
  /** What the button says. */
  label: string;
  /**
   * Where the button goes, as a path under the API's base path. The endpoint
   * answers with a redirect to the provider, so this is a plain link.
   */
  start_url: string;
  /**
   * False for a provider that releases no address — X. An account created
   * through one carries a placeholder, which is worth saying out loud before
   * somebody presses the button.
   */
  provides_email: boolean;
  /**
   * A logo the app supplied for a provider apiplant does not draw itself —
   * `[oauth.<provider>] icon`, usually a path into `public/`. Empty means fall
   * back to whatever the client shows for an unknown provider.
   */
  icon: string;
}

export interface AdminManifest {
  title: string;
  app_name: string;
  /** The app's own mark, when configured; otherwise the apiplant one is used. */
  logo: string | null;
  /**
   * Whether an account with no `avatar_url` may fall back to its Gravatar.
   * Off unless `[admin] gravatar` turns it on, since it means a request to a
   * third party for every face drawn; initials are the fallback either way.
   */
  gravatar?: boolean;
  api_base_url: string;
  docs_url: string | null;
  ai_assistance?: AdminAiAssistanceManifest;
  auth: AuthManifest;
  resources: ResourceManifest[];
  functions: FunctionManifest[];
  agents: AgentManifest[];
  /** Present only in an app whose `[payments]` section names a provider. */
  billing?: BillingManifest;
}

/** What the app's `[payments]` section amounts to, for the billing screen. */
export interface BillingManifest {
  provider: string;
  /** Intended for use in a browser; this is not a secret. */
  publishable_key: string;
  currency: string;
  /** Whether the amounts in the price list are quoted before tax. */
  automatic_tax: boolean;
  tax_id_collection: boolean;
  /** Whether purchases will actually be recorded. See `pages/billing.tsx`. */
  webhooks_configured: boolean;
}

export interface AdminAiAssistanceManifest {
  prompt_placeholder: string;
  system?: string | null;
}

export type ApiRecord = Record<string, unknown>;

export type ToastKind = "success" | "error" | "info";

export interface Toast {
  id: number;
  kind: ToastKind;
  message: string;
}

/** Where the interface is. Persisted to the URL hash so links survive a reload. */
export type Route =
  | { kind: "dashboard" }
  | { kind: "resource"; name: string }
  | { kind: "record"; name: string; id: string }
  | { kind: "new"; name: string }
  | { kind: "action"; name: string }
  | { kind: "agent"; name: string; threadId?: string }
  | { kind: "account" }
  | { kind: "team" }
  | { kind: "organization" }
  | { kind: "keys" }
  | { kind: "billing" }
  /** The `apiplant cli` handoff; see `pages/cli.tsx`. */
  | { kind: "cli" }
  /**
   * The three screens reached from a link in an email rather than from the
   * interface. Each takes its single-use token from the URL, and each is shown
   * to a caller who is not signed in.
   */
  | { kind: "accept-invite"; token: string }
  | { kind: "verify-email"; token: string }
  | { kind: "reset-password"; token: string };
