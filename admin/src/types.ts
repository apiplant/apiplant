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
  signup_fields: FieldManifest[];
  profile_fields: FieldManifest[];
  known_roles: string[];
}

export interface AdminManifest {
  title: string;
  app_name: string;
  /** The app's own mark, when configured; otherwise the apiplant one is used. */
  logo: string | null;
  api_base_url: string;
  docs_url: string | null;
  auth: AuthManifest;
  resources: ResourceManifest[];
  functions: FunctionManifest[];
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
  | { kind: "account" }
  | { kind: "team" }
  | { kind: "organization" }
  | { kind: "keys" }
  /** The `apiplant cli` handoff — see `pages/cli.tsx`. */
  | { kind: "cli" };
