/**
 * The apiplant app-directory model, as the studio holds it in memory.
 *
 * Mirrors `crates/apiplant-core/src/schema.rs` and `config.rs`: every default
 * here is the one the server applies when a key is absent, so the studio can
 * omit unchanged keys from what it writes back.
 */

export const FIELD_TYPES = [
  "string",
  "text",
  "integer",
  "big_int",
  "float",
  "boolean",
  "uuid",
  "timestamp",
  "json",
  "file",
  "reference",
] as const;
export type FieldType = (typeof FIELD_TYPES)[number];

export const CONTENT_FORMATS = ["plain", "markdown", "html"] as const;
export type ContentFormat = (typeof CONTENT_FORMATS)[number];

export const ON_DELETE = ["restrict", "set_null", "cascade", "no_action"] as const;
export type OnDelete = (typeof ON_DELETE)[number];

export const SCOPES = ["organization", "global"] as const;
export type Scope = (typeof SCOPES)[number];

export const ACTIONS = ["list", "read", "create", "update", "delete"] as const;
export type Action = (typeof ACTIONS)[number];

/**
 * What separates an access level from the organisation class it is narrowed to,
 * as in `role:admin@org_class=school`. An unqualified policy applies in every
 * organisation.
 */
export const ORG_CLASS_SUFFIX = "@org_class=";

/** The `role:<name>` level is spelled out separately in the UI. */
export const ACCESS_LEVELS = ["public", "authenticated", "member", "owner", "role", "private"] as const;
export type AccessLevel = (typeof ACCESS_LEVELS)[number];

export const HOOK_EVENTS = [
  "before_list",
  "after_list",
  "before_read",
  "after_read",
  "before_create",
  "after_create",
  "before_update",
  "after_update",
  "before_delete",
  "after_delete",
] as const;
export type HookEvent = (typeof HOOK_EVENTS)[number];

export const LANGUAGES = ["rust", "typescript", "c", "zig", "go"] as const;
export type Language = (typeof LANGUAGES)[number];

export type TomlValue = string | number | boolean | Date | TomlValue[] | { [key: string]: TomlValue };
export type TomlTable = { [key: string]: TomlValue };

export interface Field {
  name: string;
  type: FieldType;
  references?: string;
  required?: boolean;
  unique?: boolean;
  hidden?: boolean;
  /** Column DEFAULT; a bare scalar in TOML. */
  default?: string | number | boolean;
  max_length?: number;
  on_delete?: OnDelete;
  format?: ContentFormat;
}

/** Hook events on the built-in auth endpoints; only the `user` resource has them. */
export const AUTH_HOOK_EVENTS = [
  "before_register",
  "after_register",
  "before_login",
  "after_login",
  "before_api_key",
  "after_api_key",
] as const;
export type AuthHookEvent = (typeof AUTH_HOOK_EVENTS)[number];

export interface AuthSpec {
  identity_field: string;
  password_field: string;
  oauth_providers: string[];
}

export interface Resource {
  name: string;
  /** Physical table; defaults to `apiplant_<name>`. */
  table?: string;
  timestamps: boolean;
  owner_field: string;
  scope: Scope;
  permissions: Partial<Record<Action, string>>;
  /** Ordered for the file; the server sorts them anyway. */
  fields: Field[];
  hooks: Partial<Record<HookEvent | AuthHookEvent, string>>;
  /** Only meaningful on the `user` resource. */
  auth?: AuthSpec;
  /**
   * `[admin] search_fields` — the columns one `?search=` term is matched
   * against, in the API and in the dashboard's search box. Empty means the
   * server's default: whichever single field names a record.
   */
  search_fields?: string[];
  /** The rest of `[admin]`, which the studio does not model, kept verbatim. */
  admin_extra?: TomlTable;
  /** Anything the studio does not model, preserved verbatim on save. */
  extra?: TomlTable;
}

/** A resource as it exists in a project: on disk, or still a framework default. */
export interface ResourceEntry {
  name: string;
  /** `resources/<file>.toml`, or null while the resource is an unmodified built-in. */
  path: string | null;
  /** One of the five resources the framework defines with or without a file. */
  builtin: boolean;
  builtinSummary?: string;
  resource: Resource;
}

export interface FunctionFile {
  path: string;
  /** Text sources are editable; compiled libraries are listed by size only. */
  text: string | null;
  size: number;
}

/** Config is keyed by *function* name, which need not match the library's. */
export interface FunctionConfig {
  name: string;
  path: string;
}

export interface FunctionEntry {
  /** What it builds to: `libgreet.so`, or `greet.js` for TypeScript. */
  name: string;
  language: Language;
  layout: "file" | "directory";
  /** Sources, in the order they should be listed. */
  files: FunctionFile[];
  /** `functions/<fn>.toml` files belonging to this library's functions. */
  configs: FunctionConfig[];
  /** The compiled artifact, when `apiplant build` has run. */
  libPath: string | null;
  libSize: number;
  /** Function names the sources appear to export (for the hook pickers). */
  exports: string[];
}

export interface AgentAiOverride {
  provider?: string;
  endpoint?: string;
  model?: string;
  apiKey?: string;
  system?: string;
  temperature?: number;
  maxTokens?: number;
  timeoutSecs?: number;
  reasoning?: boolean;
  thinking?: boolean;
  // How the server hands the model's thinking back: auto, native, tags, implicit.
  reasoningFormat?: string;
}

export interface AgentTool {
  name: string;
  description: string;
  inputSchema: TomlTable;
  outputSchema: TomlTable;
  function: string;
}

export interface AgentEntry {
  path: string;
  name: string;
  fallbackName: string;
  description: string;
  system: string;
  scope: Scope;
  storageEnabled: boolean;
  summaryAfterCharacters?: number;
  chat: string;
  history: string;
  aiOverride: AgentAiOverride | null;
  tools: AgentTool[];
}

/** One file the studio is tracking, with the bytes it was read with. */
export interface FileState {
  /**
   * Content on disk when the project was opened; null when the file is new —
   * or when it is binary, which is why deletion needs its own flag rather than
   * being inferred from a null `current`.
   */
  original: string | null;
  /** Content to write. */
  current: string | null;
  /** Staged for removal from the directory. */
  deleted?: boolean;
  /** Non-text files (compiled libraries, certificates) are listed, never edited. */
  binary: boolean;
  size: number;
}

export const DEFAULT_PERMISSIONS: Record<Action, string> = {
  list: "member",
  read: "member",
  create: "member",
  update: "member",
  delete: "member",
};

export function emptyResource(name: string): Resource {
  return {
    name,
    timestamps: true,
    owner_field: "owner_id",
    scope: "organization",
    permissions: { ...DEFAULT_PERMISSIONS },
    fields: [],
    hooks: {},
  };
}

export const LANGUAGE_LABEL: Record<Language, string> = {
  rust: "Rust",
  typescript: "TypeScript",
  c: "C",
  zig: "Zig",
  go: "Go",
};

export const LANGUAGE_EXT: Record<Language, string> = {
  rust: "rs",
  typescript: "ts",
  c: "c",
  zig: "zig",
  go: "go",
};
