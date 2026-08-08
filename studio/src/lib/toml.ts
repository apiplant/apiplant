/**
 * Reading and writing the TOML apiplant actually consumes.
 *
 * Parsing is `smol-toml`; emitting is ours, because the shape of these files is
 * part of their readability — `[resource]`, `[permissions]`, one `[fields.x]`
 * block per column, `[hooks]` last — and a generic serializer will not produce
 * that order.
 */

import { parse as parseToml } from "smol-toml";
import {
  ACTIONS,
  AUTH_HOOK_EVENTS,
  CONTENT_FORMATS,
  FIELD_TYPES,
  HOOK_EVENTS,
  ON_DELETE,
  SCOPES,
  type Action,
  type ContentFormat,
  type AuthHookEvent,
  type Field,
  type FieldType,
  type HookEvent,
  type OnDelete,
  type Resource,
  type Scope,
  type TomlTable,
  type TomlValue,
} from "./types";

// ---- emitting ---------------------------------------------------------------

const BARE_KEY = /^[A-Za-z0-9_-]+$/;

function emitKey(key: string): string {
  return BARE_KEY.test(key) ? key : emitString(key);
}

function emitString(value: string): string {
  const escaped = value
    .replace(/\\/g, "\\\\")
    .replace(/"/g, '\\"')
    .replace(/\n/g, "\\n")
    .replace(/\r/g, "\\r")
    .replace(/\t/g, "\\t");
  return `"${escaped}"`;
}

function isTable(value: unknown): value is TomlTable {
  return typeof value === "object" && value !== null && !Array.isArray(value) && !(value instanceof Date);
}

function emitValue(value: TomlValue): string {
  if (typeof value === "string") return emitString(value);
  if (typeof value === "number") return Number.isFinite(value) ? String(value) : "nan";
  if (typeof value === "boolean") return String(value);
  if (value instanceof Date) return value.toISOString();
  if (Array.isArray(value)) return `[${value.map(emitValue).join(", ")}]`;
  // An inline table: only reached for a nested value inside an array.
  const pairs = Object.entries(value).map(([k, v]) => `${emitKey(k)} = ${emitValue(v)}`);
  return `{ ${pairs.join(", ")} }`;
}

/**
 * Serialize a table, walking sub-tables into `[header]` blocks in insertion
 * order. Empty sub-tables still get a header, so an intentionally empty
 * `[hooks]` survives a round trip.
 */
export function emitTable(table: TomlTable, prefix: string[] = []): string {
  const scalars: string[] = [];
  const blocks: string[] = [];

  for (const [key, value] of Object.entries(table)) {
    if (value === undefined || value === null) continue;
    if (isTable(value)) {
      const path = [...prefix, key];
      const body = emitTable(value, path);
      // A table holding nothing but sub-tables needs no header of its own:
      // `[fields]` above `[fields.title]` is noise the examples never have.
      const keysOfOwn = Object.values(value).filter((child) => !isTable(child));
      const header = keysOfOwn.length > 0 || Object.keys(value).length === 0;
      blocks.push(header ? `[${path.map(emitKey).join(".")}]\n${body}`.trimEnd() : body.trimEnd());
    } else if (Array.isArray(value) && value.length > 0 && value.every(isTable)) {
      const path = [...prefix, key];
      for (const item of value as TomlTable[]) {
        blocks.push(`[[${path.map(emitKey).join(".")}]]\n${emitTable(item, path)}`.trimEnd());
      }
    } else {
      scalars.push(`${emitKey(key)} = ${emitValue(value)}`);
    }
  }

  const parts: string[] = [];
  if (scalars.length) parts.push(scalars.join("\n"));
  if (blocks.length) parts.push(blocks.join("\n\n"));
  return parts.join("\n\n") + "\n";
}

// ---- parsing ----------------------------------------------------------------

export function parseTable(text: string): TomlTable {
  const parsed = parseToml(text) as unknown;
  if (!isTable(parsed)) throw new Error("expected a TOML table at the top level");
  return parsed;
}

function asString(value: unknown, fallback: string): string {
  return typeof value === "string" ? value : fallback;
}

function asBool(value: unknown, fallback: boolean): boolean {
  return typeof value === "boolean" ? value : fallback;
}

function oneOf<T extends string>(value: unknown, allowed: readonly T[], fallback: T): T {
  return typeof value === "string" && (allowed as readonly string[]).includes(value) ? (value as T) : fallback;
}

/** Parse one `resources/*.toml` into the studio's resource. Throws on invalid TOML. */
export function parseResource(text: string): Resource {
  const table = parseTable(text);
  const meta = isTable(table.resource) ? table.resource : {};
  const name = asString(meta.name, "");
  if (!name) throw new Error("[resource] name is required");

  const permissions: Partial<Record<Action, string>> = {};
  const rawPermissions = isTable(table.permissions) ? table.permissions : {};
  for (const action of ACTIONS) {
    const value = rawPermissions[action];
    if (typeof value === "string") permissions[action] = value;
  }

  const fields: Field[] = [];
  const rawFields = isTable(table.fields) ? table.fields : {};
  for (const [fieldName, raw] of Object.entries(rawFields)) {
    if (!isTable(raw)) continue;
    const type = oneOf<FieldType>(raw.type, FIELD_TYPES, "string");
    const field: Field = { name: fieldName, type };
    if (typeof raw.references === "string") field.references = raw.references;
    if (raw.required === true) field.required = true;
    if (raw.unique === true) field.unique = true;
    if (raw.hidden === true) field.hidden = true;
    if (typeof raw.max_length === "number") field.max_length = raw.max_length;
    if (typeof raw.default === "string" || typeof raw.default === "number" || typeof raw.default === "boolean") {
      field.default = raw.default;
    }
    if (typeof raw.on_delete === "string") field.on_delete = oneOf<OnDelete>(raw.on_delete, ON_DELETE, "restrict");
    const admin = isTable(raw.admin) ? raw.admin : {};
    if (typeof admin.format === "string") {
      field.format = oneOf<ContentFormat>(admin.format, CONTENT_FORMATS, "plain");
    }
    fields.push(field);
  }

  // Auth events live in the same section as the CRUD ones, and only mean
  // anything on `user` — the server rejects them anywhere else.
  const hooks: Partial<Record<HookEvent | AuthHookEvent, string>> = {};
  const rawHooks = isTable(table.hooks) ? table.hooks : {};
  for (const event of [...HOOK_EVENTS, ...AUTH_HOOK_EVENTS]) {
    const value = rawHooks[event];
    if (typeof value === "string" && value) hooks[event] = value;
  }

  const resource: Resource = {
    name,
    table: typeof meta.table === "string" ? meta.table : undefined,
    timestamps: asBool(meta.timestamps, true),
    owner_field: asString(meta.owner_field, "owner_id"),
    scope: oneOf<Scope>(meta.scope, SCOPES, "organization"),
    permissions,
    fields,
    hooks,
  };

  if (isTable(table.auth)) {
    const providers = table.auth.oauth_providers;
    resource.auth = {
      identity_field: asString(table.auth.identity_field, "email"),
      password_field: asString(table.auth.password_field, "password_hash"),
      oauth_providers: Array.isArray(providers) ? providers.filter((p): p is string => typeof p === "string") : [],
    };
  }

  // `[admin]` is presentation; the studio resources one key of it and carries the
  // rest through untouched rather than making the form the authority on a
  // section it only half understands.
  if (isTable(table.admin)) {
    const { search_fields: searchFields, ...rest } = table.admin;
    if (Array.isArray(searchFields)) {
      const named = searchFields.filter((entry): entry is string => typeof entry === "string");
      if (named.length) resource.search_fields = named;
    }
    if (Object.keys(rest).length) resource.admin_extra = rest as TomlTable;
  }

  // Keep anything we do not model so saving never silently drops it.
  const known = new Set(["resource", "permissions", "fields", "hooks", "auth", "admin"]);
  const extra: TomlTable = {};
  for (const [key, value] of Object.entries(table)) {
    if (!known.has(key)) extra[key] = value;
  }
  if (Object.keys(extra).length) resource.extra = extra;

  return resource;
}

/** Serialize a resource back to the file layout apiplant's examples use. */
export function emitResource(resource: Resource): string {
  const meta: TomlTable = { name: resource.name };
  if (resource.table) meta.table = resource.table;
  if (resource.scope !== "organization") meta.scope = resource.scope;
  if (!resource.timestamps) meta.timestamps = false;
  if (resource.owner_field && resource.owner_field !== "owner_id") meta.owner_field = resource.owner_field;

  const out: TomlTable = { resource: meta };

  const permissions: TomlTable = {};
  for (const action of ACTIONS) {
    const value = resource.permissions[action];
    if (value) permissions[action] = value;
  }
  if (Object.keys(permissions).length) out.permissions = permissions;

  if (resource.auth) {
    const auth: TomlTable = {
      identity_field: resource.auth.identity_field,
      password_field: resource.auth.password_field,
    };
    if (resource.auth.oauth_providers.length) auth.oauth_providers = resource.auth.oauth_providers;
    out.auth = auth;
  }

  const admin: TomlTable = { ...(resource.admin_extra ?? {}) };
  if (resource.search_fields?.length) admin.search_fields = [...resource.search_fields];
  if (Object.keys(admin).length) out.admin = admin;

  if (resource.fields.length) {
    const fields: TomlTable = {};
    for (const field of resource.fields) {
      if (!field.name) continue;
      const entry: TomlTable = { type: field.type };
      if (field.type === "reference" && field.references) entry.references = field.references;
      if (field.required) entry.required = true;
      if (field.unique) entry.unique = true;
      if (field.hidden) entry.hidden = true;
      if (field.type === "string" && field.max_length) entry.max_length = field.max_length;
      if (field.default !== undefined && field.default !== "") entry.default = field.default;
      if (field.type === "reference" && field.on_delete) entry.on_delete = field.on_delete;
      if (field.format && field.format !== "plain") entry.admin = { format: field.format };
      fields[field.name] = entry;
    }
    out.fields = fields;
  }

  const hooks: TomlTable = {};
  for (const event of [...HOOK_EVENTS, ...AUTH_HOOK_EVENTS]) {
    const value = resource.hooks[event];
    if (value) hooks[event] = value;
  }
  if (Object.keys(hooks).length) out.hooks = hooks;

  if (resource.extra) Object.assign(out, resource.extra);

  return emitTable(out);
}

/** True when a file carries comments a form-driven rewrite would drop. */
export function hasComments(text: string): boolean {
  return text.split("\n").some((line) => {
    const trimmed = line.trim();
    return trimmed.startsWith("#") && trimmed.length > 1;
  });
}
