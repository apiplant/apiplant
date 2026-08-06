/**
 * The open project: one directory, everything it holds, and the edits waiting
 * to be written back.
 *
 * Files are the single source of truth for what gets saved. Every form in the
 * studio edits a model (a `Resource`, the config table) and immediately re-emits
 * that model into `files[path].current`; saving then walks the file map and
 * writes only what differs from what was read. That keeps "what will change on
 * disk" answerable at any moment, and makes discarding a rescan.
 */

import { createStore, produce, unwrap } from "solid-js/store";
import {
  agentStorageBuiltinEntries,
  emitAgent,
  fallbackAgentName,
  isAgentStorageBuiltinName,
  scaffoldAgent,
  summarizeAgent,
} from "./agents";
import {
  createSubdirectory,
  ensurePermission,
  deleteDirectory,
  deleteFile,
  isTextPath,
  listEntryNames,
  listSubdirectories,
  pickDirectory,
  scanDirectory,
  writeTextFile,
  type ScannedFile,
} from "./fs";
import {
  ALWAYS_BUILTIN_NAMES,
  BILLING_BUILTIN_NAMES,
  BUILTIN_FILENAME,
  BUILTIN_NAMES,
  BUILTIN_SUMMARY,
  builtinResource,
  type BuiltinName,
} from "./builtins";
import { detectFunctions, extractExports } from "./functions";
import { emitResource, emitTable, parseResource, parseTable } from "./toml";
import { scaffoldFunction, scaffoldMainToml, type TemplateKind } from "./templates";
import {
  type AgentEntry,
  AUTH_HOOK_EVENTS,
  emptyResource,
  type FileState,
  type FunctionEntry,
  type Language,
  type Resource,
  type ResourceEntry,
  type TomlTable,
  type TomlValue,
} from "./types";
import { setView } from "./nav";
import { rememberProject } from "./persistence";

export interface Project {
  handle: FileSystemDirectoryHandle;
  /** Directory name, e.g. `07-functions`. */
  name: string;
  files: Record<string, FileState>;
  resources: ResourceEntry[];
  functions: FunctionEntry[];
  agents: AgentEntry[];
  /** `functions/*.toml` we could not tie to a library. */
  orphanConfigs: string[];
  config: TomlTable;
  /** Directories to remove on save (a deleted function directory). */
  pendingDirDeletes: string[];
  /** Files that failed to parse, so the studio can say so instead of guessing. */
  problems: { path: string; message: string }[];
  hasTls: boolean;
}

export interface Toast {
  id: number;
  kind: "info" | "success" | "error";
  message: string;
}

interface StudioState {
  project: Project | null;
  loading: boolean;
  saving: boolean;
  toasts: Toast[];
}

const [state, setState] = createStore<StudioState>({
  project: null,
  loading: false,
  saving: false,
  toasts: [],
});

export const studio = state;

export const CONFIG_PATH = "main.toml";

// ---- toasts -----------------------------------------------------------------

let toastId = 0;

export function toast(message: string, kind: Toast["kind"] = "info") {
  const id = ++toastId;
  setState("toasts", (list) => [...list, { id, kind, message }]);
  setTimeout(() => setState("toasts", (list) => list.filter((t) => t.id !== id)), 4200);
}

export function dismissToast(id: number) {
  setState("toasts", (list) => list.filter((t) => t.id !== id));
}

// ---- opening ----------------------------------------------------------------

/** An app directory is anything carrying one of the framework's own pieces. */
export function looksLikeApp(entryNames: string[]): boolean {
  return ["main.toml", "models", "agents", "functions", "https"].some((name) => entryNames.includes(name));
}

export interface AppCandidate {
  name: string;
  handle: FileSystemDirectoryHandle;
}

export type DirectorySelection =
  | { kind: "app"; handle: FileSystemDirectoryHandle }
  | {
      kind: "candidates";
      parent: FileSystemDirectoryHandle;
      /** Everything in the parent, so a new app can be named without collision. */
      parentEntryNames: string[];
      candidates: AppCandidate[];
    };

/** Work out whether a handle is an app itself or a parent containing apps. */
export async function inspectDirectory(handle: FileSystemDirectoryHandle): Promise<DirectorySelection> {
  const names = await listEntryNames(handle);
  if (looksLikeApp(names)) return { kind: "app", handle };

  const candidates: AppCandidate[] = [];
  for (const child of await listSubdirectories(handle)) {
    const childHandle = await handle.getDirectoryHandle(child);
    if (looksLikeApp(await listEntryNames(childHandle))) {
      candidates.push({ name: child, handle: childHandle });
    }
  }

  return { kind: "candidates", parent: handle, parentEntryNames: names, candidates };
}

/** Pick a directory, then work out whether it is an app or contains apps. */
export async function chooseDirectory(): Promise<DirectorySelection | null> {
  const handle = await pickDirectory();
  if (!handle) return null;
  return inspectDirectory(handle);
}

/** Pick a directory to hold a new app, with the names already in it. */
export async function chooseParentDirectory(): Promise<
  { handle: FileSystemDirectoryHandle; entryNames: string[] } | null
> {
  const handle = await pickDirectory();
  if (!handle) return null;
  return { handle, entryNames: await listEntryNames(handle) };
}

/** Directory names the studio is willing to create. */
export const APP_NAME_RULE = /^[A-Za-z0-9][A-Za-z0-9._-]*$/;

/**
 * Start a new app in `parent/name`.
 *
 * The directory itself has to exist to hold a handle, so it is created now; its
 * contents are staged like every other edit and land on disk on Save.
 */
export async function createProject(
  parent: FileSystemDirectoryHandle,
  name: string,
  options: { withExampleResource?: boolean } = {},
): Promise<boolean> {
  if (!APP_NAME_RULE.test(name)) {
    toast("An app directory name must start with a letter or digit and hold only letters, digits, . _ -", "error");
    return false;
  }
  if ((await listEntryNames(parent)).includes(name)) {
    toast(`${parent.name} already has an entry named ${name}`, "error");
    return false;
  }

  const handle = await createSubdirectory(parent, name);
  await openProject(handle, { quiet: true });
  ensureMainToml();
  if (options.withExampleResource) addResource("note");
  toast(`New app ${name} — press Save to write it to disk`, "success");
  return true;
}

function buildFileMap(scanned: ScannedFile[]): Record<string, FileState> {
  const files: Record<string, FileState> = {};
  for (const file of scanned) {
    files[file.path] = {
      original: file.text,
      current: file.text,
      binary: file.text === null,
      size: file.size,
    };
  }
  return files;
}

/** Whether `[payments]` names a provider — the condition the billing built-ins ride on. */
function paymentsEnabled(config: TomlTable): boolean {
  const table = configTable(config, "payments", false);
  const provider = table?.provider;
  return typeof provider === "string" && provider !== "" && provider !== "none";
}

function buildResources(
  scanned: ScannedFile[],
  config: TomlTable,
  agents: AgentEntry[],
): {
  resources: ResourceEntry[];
  problems: { path: string; message: string }[];
} {
  const problems: { path: string; message: string }[] = [];
  const byName = new Map<string, ResourceEntry>();

  const present = paymentsEnabled(config) ? BUILTIN_NAMES : ALWAYS_BUILTIN_NAMES;
  for (const name of present) {
    byName.set(name, {
      name,
      path: null,
      builtin: true,
      builtinSummary: BUILTIN_SUMMARY[name],
      resource: builtinResource(name),
    });
  }
  for (const agent of agents) {
    for (const entry of agentStorageBuiltinEntries(agent)) {
      byName.set(entry.name, entry);
    }
  }

  for (const file of scanned) {
    if (!file.path.startsWith("models/") || !file.path.endsWith(".toml") || file.text === null) continue;
    // Only top-level files in models/ are loaded by the framework.
    if (file.path.slice("models/".length).includes("/")) continue;
    try {
      const resource = parseResource(file.text);
      const existing = byName.get(resource.name);
      byName.set(resource.name, {
        name: resource.name,
        path: file.path,
        builtin:
          existing?.builtin ??
          ((present as readonly string[]).includes(resource.name) || isAgentStorageBuiltinName(resource.name)),
        builtinSummary: existing?.builtinSummary,
        resource,
      });
    } catch (error) {
      problems.push({ path: file.path, message: error instanceof Error ? error.message : String(error) });
    }
  }

  const resources = [...byName.values()].sort((a, b) => a.name.localeCompare(b.name));
  return { resources, problems };
}

function buildAgents(scanned: ScannedFile[]): {
  agents: AgentEntry[];
  problems: { path: string; message: string }[];
} {
  const agents: AgentEntry[] = [];
  const problems: { path: string; message: string }[] = [];

  for (const file of scanned) {
    if (!file.path.startsWith("agents/") || !file.path.endsWith(".toml") || file.text === null) continue;
    if (file.path.slice("agents/".length).includes("/")) continue;
    try {
      agents.push(summarizeAgent(file.path, file.text));
    } catch (error) {
      problems.push({ path: file.path, message: error instanceof Error ? error.message : String(error) });
      agents.push({
        path: file.path,
        name: fallbackAgentName(file.path),
        fallbackName: fallbackAgentName(file.path),
        description: "",
        system: "",
        scope: "global",
        storageEnabled: false,
        summaryAfterCharacters: undefined,
        chat: "authenticated",
        history: "owner",
        aiOverride: null,
        tools: [],
      });
    }
  }

  agents.sort((a, b) => a.name.localeCompare(b.name));
  return { agents, problems };
}

async function buildProject(handle: FileSystemDirectoryHandle): Promise<Project> {
  const scanned = await scanDirectory(handle);
  const problems: { path: string; message: string }[] = [];

  // The config comes first: whether the app has the billing resources at all
  // depends on what `[payments]` says.
  let config: TomlTable = {};
  const configProblems: { path: string; message: string }[] = [];
  const main = scanned.find((f) => f.path === CONFIG_PATH);
  if (main?.text) {
    try {
      config = parseTable(main.text);
    } catch (error) {
      configProblems.push({ path: CONFIG_PATH, message: error instanceof Error ? error.message : String(error) });
    }
  }

  const builtAgents = buildAgents(scanned);
  problems.push(...builtAgents.problems);
  const { resources, problems: resourceProblems } = buildResources(scanned, config, builtAgents.agents);
  problems.push(...configProblems);
  problems.push(...resourceProblems);
  const { entries, orphanConfigs } = detectFunctions(scanned);

  return {
    handle,
    name: handle.name,
    files: buildFileMap(scanned),
    resources,
    functions: entries,
    agents: builtAgents.agents,
    orphanConfigs,
    config,
    pendingDirDeletes: [],
    problems,
    hasTls: scanned.some((f) => f.path.startsWith("https/")),
  };
}

export async function openProject(
  handle: FileSystemDirectoryHandle,
  options: { quiet?: boolean; preserveView?: boolean } = {},
): Promise<void> {
  setState("loading", true);
  try {
    if (!(await ensurePermission(handle))) {
      throw new Error(`Permission to read and write ${handle.name} was not granted`);
    }
    setState("project", await buildProject(handle));
    try {
      await rememberProject(handle);
    } catch {
      // Opening the project matters more than caching the handle for next time.
    }
    if (!options.preserveView) setView({ kind: "overview" }, { replace: true });
    if (!options.quiet) toast(`Opened ${handle.name}`, "success");
  } finally {
    setState("loading", false);
  }
}

/** Re-read everything from disk, dropping unsaved edits. */
export async function reloadProject(): Promise<void> {
  const project = state.project;
  if (!project) return;
  setState("loading", true);
  try {
    setState("project", await buildProject(project.handle));
  } finally {
    setState("loading", false);
  }
}

export function closeProject() {
  setState("project", null);
}

// ---- file plumbing ----------------------------------------------------------

export function fileText(path: string): string | null {
  return state.project?.files[path]?.current ?? null;
}

export function fileState(path: string): FileState | undefined {
  return state.project?.files[path];
}

export function setFileText(path: string, text: string) {
  if (!state.project) return;
  setState(
    "project",
    "files",
    produce((files: Record<string, FileState>) => {
      const existing = files[path];
      if (existing) {
        existing.current = text;
        existing.size = text.length;
        existing.deleted = false;
      } else {
        files[path] = { original: null, current: text, binary: !isTextPath(path), size: text.length };
      }
    }),
  );
}

function markDeleted(path: string) {
  if (!state.project) return;
  setState(
    "project",
    "files",
    produce((files: Record<string, FileState>) => {
      const existing = files[path];
      if (!existing) return;
      // A file the studio itself created was never on disk: it just stops being
      // tracked. Anything read from the directory — including a compiled
      // library, which has no text to compare — is staged for removal.
      if (existing.original === null && !existing.binary) delete files[path];
      else {
        existing.deleted = true;
        existing.current = null;
      }
    }),
  );
}

export type ChangeKind = "added" | "modified" | "deleted";

export interface Change {
  path: string;
  kind: ChangeKind;
}

export function pendingChanges(): Change[] {
  const project = state.project;
  if (!project) return [];
  const changes: Change[] = [];
  for (const [path, file] of Object.entries(project.files)) {
    if (file.deleted) changes.push({ path, kind: "deleted" });
    else if (file.binary || file.current === file.original) continue;
    else if (file.original === null) changes.push({ path, kind: "added" });
    else changes.push({ path, kind: "modified" });
  }
  return changes.sort((a, b) => a.path.localeCompare(b.path));
}

export async function saveAll(): Promise<void> {
  const project = state.project;
  if (!project) return;
  const changes = pendingChanges();
  if (!changes.length && !project.pendingDirDeletes.length) {
    toast("Nothing to save");
    return;
  }

  setState("saving", true);
  try {
    for (const change of changes) {
      if (change.kind === "deleted") await deleteFile(project.handle, change.path);
      else await writeTextFile(project.handle, change.path, project.files[change.path].current!);
    }
    for (const dir of unwrap(project.pendingDirDeletes)) {
      await deleteDirectory(project.handle, dir);
    }

    setState(
      "project",
      "files",
      produce((files: Record<string, FileState>) => {
        for (const change of changes) {
          if (change.kind === "deleted") delete files[change.path];
          else files[change.path].original = files[change.path].current;
        }
      }),
    );
    setState("project", "pendingDirDeletes", []);
    toast(`Saved ${changes.length} file${changes.length === 1 ? "" : "s"} to ${project.name}`, "success");
  } catch (error) {
    toast(error instanceof Error ? error.message : String(error), "error");
    throw error;
  } finally {
    setState("saving", false);
  }
}

// ---- config -----------------------------------------------------------------

function syncConfigFile() {
  const project = state.project;
  if (!project) return;
  setFileText(CONFIG_PATH, emitTable(unwrap(project.config)));
}

function isConfigTable(value: unknown): value is TomlTable {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function configTable(config: TomlTable, sectionPath: string, create: boolean): TomlTable | undefined {
  let table = config;
  for (const segment of sectionPath.split(".")) {
    const next = table[segment];
    if (isConfigTable(next)) {
      table = next;
      continue;
    }
    if (!create) return undefined;
    table = table[segment] = {};
  }
  return table;
}

export function setConfigValue(section: string, key: string, value: string | number | boolean | undefined) {
  writeConfig(section, key, value === "" ? undefined : value);
  if (section === "payments" && key === "provider") syncBillingBuiltins();
}

/**
 * Write one key, then delete every table the write left empty.
 *
 * The pruning is what keeps the file readable: a setting turned back to its
 * default should leave no trace, not a `[observability.otlp]` header with
 * nothing under it. Shared by every writer below, since a list and a map empty
 * out exactly like a scalar does.
 */
function writeConfig(section: string, key: string, value: TomlValue | undefined) {
  if (!state.project) return;
  setState(
    "project",
    "config",
    produce((config: TomlTable) => {
      const parents: TomlTable[] = [config];
      let table = config;
      for (const segment of section.split(".")) {
        const next = configTable(table, segment, true);
        table = next!;
        parents.push(table);
      }
      if (value === undefined) delete table[key];
      else table[key] = value;
      const segments = section.split(".");
      for (let i = segments.length - 1; i >= 0; i--) {
        if (Object.keys(parents[i + 1]).length > 0) break;
        delete parents[i][segments[i]];
      }
    }),
  );
  syncConfigFile();
}

/**
 * A list-of-strings setting — `[observability.traces] capture_headers`.
 *
 * Values are trimmed and blanks dropped, so a form that edits them as one
 * comma-separated line cannot write `["a", ""]`.
 */
export function configList(section: string, key: string): string[] {
  const table = state.project ? configTable(state.project.config, section, false) : undefined;
  const value = table?.[key];
  if (!Array.isArray(value)) return [];
  return value.filter((item): item is string => typeof item === "string").map((item) => item.trim()).filter(Boolean);
}

export function setConfigList(section: string, key: string, values: string[]) {
  const usable = values.map((value) => value.trim()).filter(Boolean);
  // An empty list is not the same statement as an absent one for every key —
  // `exclude_paths = []` really does mean "trace the health check too" — but
  // the form has no way to say one and not the other, and the defaults are
  // written to be the useful answer. Absent it is.
  writeConfig(section, key, usable.length > 0 ? usable : undefined);
}

/** One entry of a string→string table, as a form edits it. */
export interface ConfigEntry {
  key: string;
  value: string;
}

/**
 * A map setting — `[observability] resource_attributes`, `[observability.otlp]
 * headers`. Like `[queues.subscribe]`, it is read and written whole, because a
 * key of the map is data rather than a known setting name.
 */
export function configEntries(section: string, key: string): ConfigEntry[] {
  const table = state.project ? configTable(state.project.config, section, false) : undefined;
  const value = table?.[key];
  if (!isConfigTable(value)) return [];
  return Object.entries(value)
    .filter(([, item]) => typeof item === "string" || typeof item === "number" || typeof item === "boolean")
    .map(([entryKey, item]) => ({ key: entryKey, value: String(item) }));
}

export function setConfigEntries(section: string, key: string, entries: ConfigEntry[]) {
  const table: TomlTable = {};
  for (const entry of entries) {
    // A row mid-edit has a value and no name yet; it is not a setting until it
    // has both, and writing it would produce `"" = "…"`.
    if (!entry.key.trim()) continue;
    table[entry.key.trim()] = entry.value;
  }
  writeConfig(section, key, Object.keys(table).length > 0 ? table : undefined);
}

/**
 * Follow `[payments].provider` with the billing resources.
 *
 * The framework adds the six `billing_*` built-ins when a provider is named and
 * leaves them out otherwise, so naming one in the studio has to make them
 * appear then and there — a resource list that only agreed with the app at the
 * moment the project was opened would be worse than none. A billing resource
 * the app has a file for stays either way; turning payments off just makes it
 * an ordinary resource of the app's own, which is what it then is.
 */
function syncBillingBuiltins() {
  const project = state.project;
  if (!project) return;
  const enabled = paymentsEnabled(project.config);

  setState(
    "project",
    "resources",
    produce((resources: ResourceEntry[]) => {
      for (const name of BILLING_BUILTIN_NAMES) {
        const index = resources.findIndex((entry) => entry.name === name);
        if (enabled) {
          if (index < 0) {
            resources.push({
              name,
              path: null,
              builtin: true,
              builtinSummary: BUILTIN_SUMMARY[name],
              resource: builtinResource(name),
            });
          } else {
            resources[index].builtin = true;
            resources[index].builtinSummary = BUILTIN_SUMMARY[name];
          }
        } else if (index >= 0) {
          if (resources[index].path) resources[index].builtin = false;
          else resources.splice(index, 1);
        }
      }
      resources.sort((a, b) => a.name.localeCompare(b.name));
    }),
  );
}

function syncAgentStorageBuiltins() {
  const project = state.project;
  if (!project) return;
  const generated = new Map(project.agents.flatMap((agent) => agentStorageBuiltinEntries(agent).map((entry) => [entry.name, entry])));

  setState(
    "project",
    "resources",
    produce((resources: ResourceEntry[]) => {
      for (const [name, entry] of generated) {
        const index = resources.findIndex((resource) => resource.name === name);
        if (index < 0) {
          resources.push(entry);
          continue;
        }
        resources[index].builtin = true;
        resources[index].builtinSummary = entry.builtinSummary;
        if (!resources[index].path) resources[index].resource = entry.resource;
      }

      for (let index = resources.length - 1; index >= 0; index -= 1) {
        const resource = resources[index];
        if (!isAgentStorageBuiltinName(resource.name) || generated.has(resource.name)) continue;
        if (resource.path) {
          resource.builtin = false;
          delete resource.builtinSummary;
        } else {
          resources.splice(index, 1);
        }
      }

      resources.sort((a, b) => a.name.localeCompare(b.name));
    }),
  );
}

export function configValue(section: string, key: string): string | number | boolean | undefined {
  const table = state.project ? configTable(state.project.config, section, false) : undefined;
  if (!table) return undefined;
  const value = table[key];
  if (typeof value === "string" || typeof value === "number" || typeof value === "boolean") return value;
  return undefined;
}

// ---- queue subscriptions ----------------------------------------------------
//
// `[queues.subscribe]` is a map, not a list of scalars, so it cannot go through
// `configValue`/`setConfigValue` like every other setting. It is read and
// written whole instead, which also lets the studio normalise the two spellings
// the server accepts — `"topic" = "one"` and `"topic" = ["one", "two"]` — into
// one shape for the form.

/** One `[queues.subscribe]` entry, as the form edits it. */
export interface Subscription {
  topic: string;
  /** The function(s) that handle it, in declaration order. */
  functions: string[];
}

/** The app's subscriptions, in the order the file lists them. */
export function subscriptions(): Subscription[] {
  const config = state.project?.config;
  const table = config ? configTable(config, "queues.subscribe", false) : undefined;
  if (!table) return [];
  return Object.entries(table).map(([topic, value]) => ({
    topic,
    functions: (Array.isArray(value) ? value : [value])
      .filter((name): name is string => typeof name === "string")
      .map((name) => name.trim())
      .filter(Boolean),
  }));
}

/**
 * Replace the whole subscription table.
 *
 * A single subscriber is written as a bare string rather than a one-element
 * list, because that is what somebody reading the file would have written —
 * and the server reads both.
 *
 * An entry with no topic, or a topic with no functions, is dropped: it is a row
 * mid-edit in the form, and neither spelling means anything to the server.
 */
export function setSubscriptions(list: Subscription[]) {
  if (!state.project) return;
  setState(
    "project",
    "config",
    produce((config: TomlTable) => {
      const usable = list.filter((entry) => entry.topic.trim() && entry.functions.length > 0);
      if (usable.length === 0) {
        const queues = configTable(config, "queues", false);
        if (queues) delete queues.subscribe;
        // A `[queues]` left with nothing in it is noise in the file.
        if (queues && Object.keys(queues).length === 0) delete config.queues;
        return;
      }
      const table = configTable(config, "queues.subscribe", true)!;
      for (const key of Object.keys(table)) delete table[key];
      for (const entry of usable) {
        table[entry.topic.trim()] =
          entry.functions.length === 1 ? entry.functions[0] : [...entry.functions];
      }
    }),
  );
  syncConfigFile();
}

/** Replace main.toml wholesale from the raw editor. */
export function setConfigFromToml(text: string) {
  const parsed = parseTable(text);
  setState("project", "config", parsed);
  setFileText(CONFIG_PATH, text);
  syncBillingBuiltins();
}

export function ensureMainToml() {
  const project = state.project;
  if (!project || project.files[CONFIG_PATH]) return;
  const text = scaffoldMainToml(project.name);
  setState("project", "config", parseTable(text));
  setFileText(CONFIG_PATH, text);
  syncBillingBuiltins();
}

// ---- resources --------------------------------------------------------------

export function resourceEntry(name: string): ResourceEntry | undefined {
  return state.project?.resources.find((entry) => entry.name === name);
}

function pathForResource(name: string): string {
  if ((BUILTIN_NAMES as readonly string[]).includes(name)) return `models/${BUILTIN_FILENAME[name as BuiltinName]}`;
  return `models/${name}.toml`;
}

/**
 * Apply an edit to a resource and re-emit its file.
 *
 * Editing a built-in that has no file materialises one — which is exactly what
 * the framework means by "drop a same-named model in to replace the default".
 */
export function updateResource(name: string, update: (resource: Resource) => void) {
  const project = state.project;
  if (!project) return;
  const index = project.resources.findIndex((entry) => entry.name === name);
  if (index < 0) return;

  setState(
    "project",
    "resources",
    index,
    produce((entry: ResourceEntry) => {
      update(entry.resource);
      if (!entry.path) entry.path = pathForResource(entry.resource.name);
    }),
  );

  const entry = state.project!.resources[index];
  const emitted = emitResource(unwrap(entry.resource));

  // A rename moves the file for custom resources.
  if (entry.name !== entry.resource.name) {
    const oldPath = entry.path;
    const newPath = pathForResource(entry.resource.name);
    if (oldPath && oldPath !== newPath) markDeleted(oldPath);
    setState("project", "resources", index, { name: entry.resource.name, path: newPath });
  }

  setFileText(state.project!.resources[index].path!, emitted);
}

/** Write a built-in's default definition to disk so it can be extended. */
export function customizeBuiltin(name: string) {
  updateResource(name, () => {});
  toast(`${name} is now a file you own — ${pathForResource(name)}`, "success");
}

export function addResource(name: string, options: { scope?: "organization" | "global" } = {}): boolean {
  const project = state.project;
  if (!project) return false;
  if (project.resources.some((entry) => entry.name === name)) {
    toast(`A resource named ${name} already exists`, "error");
    return false;
  }

  const resource = emptyResource(name);
  if (options.scope) resource.scope = options.scope;
  resource.fields = [{ name: "title", type: "string", required: true }];

  const path = pathForResource(name);
  setState(
    "project",
    "resources",
    produce((resources: ResourceEntry[]) => {
      resources.push({ name, path, builtin: false, resource });
      resources.sort((a, b) => a.name.localeCompare(b.name));
    }),
  );
  setFileText(path, emitResource(resource));
  return true;
}

/**
 * Remove a resource's file. A custom resource disappears; a built-in reverts to
 * the definition the framework ships.
 */
export function deleteResource(name: string) {
  const project = state.project;
  if (!project) return;
  const index = project.resources.findIndex((entry) => entry.name === name);
  if (index < 0) return;
  const entry = project.resources[index];
  if (entry.path) markDeleted(entry.path);

  if (entry.builtin) {
    const builtin = (BUILTIN_NAMES as readonly string[]).includes(name)
      ? {
          builtinSummary: BUILTIN_SUMMARY[name as BuiltinName],
          resource: builtinResource(name as BuiltinName),
        }
      : state.project?.agents
          .flatMap((agent) => agentStorageBuiltinEntries(agent))
          .find((resource) => resource.name === name);
    if (!builtin) return;
    setState("project", "resources", index, {
      path: null,
      builtinSummary: builtin.builtinSummary,
      resource: builtin.resource,
    });
    toast(`${name} reverted to the built-in definition`);
  } else {
    setState("project", "resources", (resources) => resources.filter((r) => r.name !== name));
    toast(`Deleted ${name}`);
  }
}

/** Replace a resource from hand-edited TOML. Throws if it does not parse. */
export function setResourceFromToml(name: string, text: string) {
  const project = state.project;
  if (!project) return;
  const index = project.resources.findIndex((entry) => entry.name === name);
  if (index < 0) return;
  const parsed = parseResource(text);
  const path = project.resources[index].path ?? pathForResource(parsed.name);
  setState("project", "resources", index, { name: parsed.name, path, resource: parsed });
  setFileText(path, text);
}

// ---- functions --------------------------------------------------------------

export function functionEntry(name: string): FunctionEntry | undefined {
  return state.project?.functions.find((entry) => entry.name === name);
}

export function agentEntry(name: string): AgentEntry | undefined {
  return state.project?.agents.find((entry) => entry.name === name);
}

/** Exports recomputed from the current (possibly edited) sources. */
export function functionExports(entry: FunctionEntry): string[] {
  const sources = entry.files
    .map((file) => fileText(file.path))
    .filter((text): text is string => typeof text === "string");
  const found = extractExports(sources);
  return found.length ? found : [entry.name];
}

/** Every function name in the app, for the hook pickers. */
export function allFunctionNames(): string[] {
  const project = state.project;
  if (!project) return [];
  const names = new Set<string>();
  for (const entry of project.functions) for (const name of functionExports(entry)) names.add(name);
  return [...names].sort((a, b) => a.localeCompare(b));
}

export function addFunction(
  name: string,
  language: Language,
  layout: "file" | "directory",
  kind: TemplateKind,
  withConfig: boolean,
): boolean {
  const project = state.project;
  if (!project) return false;
  if (project.functions.some((entry) => entry.name === name)) {
    toast(`functions/ already has an entry named ${name}`, "error");
    return false;
  }

  const generated = scaffoldFunction(name, language, layout, kind, withConfig);
  for (const file of generated) setFileText(file.path, file.text);

  const sources = generated.filter((file) => !file.path.endsWith(`${name}.toml`));
  const entry: FunctionEntry = {
    name,
    language,
    layout,
    files: sources.map((file) => ({ path: file.path, text: file.text, size: file.text.length })),
    configs: withConfig ? [{ name, path: `functions/${name}.toml` }] : [],
    libPath: null,
    libSize: 0,
    exports: [name],
  };

  setState(
    "project",
    "functions",
    produce((functions: FunctionEntry[]) => {
      functions.push(entry);
      functions.sort((a, b) => a.name.localeCompare(b.name));
    }),
  );
  return true;
}

/** Add a `functions/<fn>.toml` for a function that has none. */
export function addFunctionConfig(entryName: string, functionName: string) {
  const project = state.project;
  if (!project) return;
  const index = project.functions.findIndex((entry) => entry.name === entryName);
  if (index < 0) return;
  const path = `functions/${functionName}.toml`;
  if (project.functions[index].configs.some((config) => config.path === path)) return;

  setFileText(path, `# Configuration for the \`${functionName}\` function.\n`);
  setState(
    "project",
    "functions",
    index,
    "configs",
    produce((configs: { name: string; path: string }[]) => {
      configs.push({ name: functionName, path });
      configs.sort((a, b) => a.name.localeCompare(b.name));
    }),
  );
}

/** Remove a function: its sources, its config, and the library it built to. */
export function deleteFunction(name: string) {
  const project = state.project;
  if (!project) return;
  const entry = project.functions.find((fn) => fn.name === name);
  if (!entry) return;

  for (const file of entry.files) markDeleted(file.path);
  for (const config of entry.configs) markDeleted(config.path);
  if (entry.libPath) markDeleted(entry.libPath);
  if (entry.layout === "directory") {
    setState("project", "pendingDirDeletes", (dirs) => [...dirs, `functions/${name}`]);
  }

  setState("project", "functions", (functions) => functions.filter((fn) => fn.name !== name));
  toast(`Deleted function ${name}`);
}

/** Add another source file to a function directory. */
export function addFunctionFile(entryName: string, fileName: string, text: string) {
  const project = state.project;
  if (!project) return;
  const index = project.functions.findIndex((entry) => entry.name === entryName);
  if (index < 0) return;
  const path = `functions/${entryName}/${fileName}`;
  setFileText(path, text);
  setState(
    "project",
    "functions",
    index,
    "files",
    produce((files: { path: string; text: string | null; size: number }[]) => {
      files.push({ path, text, size: text.length });
      files.sort((a, b) => a.path.localeCompare(b.path));
    }),
  );
}

// ---- agents -----------------------------------------------------------------

export function addAgent(name: string, storageEnabled: boolean): string | null {
  const project = state.project;
  if (!project) return null;
  const path = `agents/${name}.toml`;
  if (project.agents.some((entry) => entry.path === path || entry.name === name)) {
    toast(`An agent named ${name} already exists`, "error");
    return null;
  }

  const text = scaffoldAgent(name, storageEnabled);
  const entry = summarizeAgent(path, text);
  setFileText(path, text);
  setState(
    "project",
    "agents",
    produce((agents: AgentEntry[]) => {
      agents.push(entry);
      agents.sort((a, b) => a.name.localeCompare(b.name));
    }),
  );
  syncAgentStorageBuiltins();
  return path;
}

export function updateAgent(name: string, update: (agent: AgentEntry) => void) {
  const project = state.project;
  if (!project) return;
  const index = project.agents.findIndex((entry) => entry.name === name);
  if (index < 0) return;
  const before = project.agents[index];

  setState("project", "agents", index, produce((entry: AgentEntry) => update(entry)));

  const updated = state.project!.agents[index];
  const nextPath = `agents/${updated.name}.toml`;
  if (before.path !== nextPath) {
    markDeleted(before.path);
    setState("project", "agents", index, "path", nextPath);
  }
  setState("project", "agents", index, "fallbackName", updated.name);
  setFileText(state.project!.agents[index].path, emitAgent(unwrap(state.project!.agents[index])));
  syncAgentStorageBuiltins();
}

export function setAgentFromToml(name: string, text: string): string | null {
  const project = state.project;
  if (!project) return null;
  const index = project.agents.findIndex((entry) => entry.name === name);
  if (index < 0) return null;
  const parsed = summarizeAgent(project.agents[index].path, text);
  if (project.agents.some((entry, other) => other !== index && entry.name === parsed.name)) {
    throw new Error(`Another agent already uses the name ${parsed.name}`);
  }
  if (project.agents[index].path !== `agents/${parsed.name}.toml`) {
    markDeleted(project.agents[index].path);
    parsed.path = `agents/${parsed.name}.toml`;
  }
  setState("project", "agents", index, parsed);
  setFileText(parsed.path, text);
  syncAgentStorageBuiltins();
  return parsed.name;
}

export function deleteAgent(name: string) {
  const project = state.project;
  if (!project) return;
  const entry = project.agents.find((agent) => agent.name === name);
  if (!entry) return;
  markDeleted(entry.path);
  setState("project", "agents", (agents) => agents.filter((agent) => agent.name !== name));
  syncAgentStorageBuiltins();
  toast(`Deleted agent ${entry.name}`);
}

// ---- validation -------------------------------------------------------------

/** The load-time rules from `schema.rs`, checked while you type instead. */
export function validateResource(resource: Resource, knownResources: string[]): string[] {
  const issues: string[] = [];
  if (!/^[A-Za-z_][A-Za-z0-9_]*$/.test(resource.name)) {
    issues.push("Resource name must be a valid SQL identifier (letters, digits, underscore).");
  }
  const seen = new Set<string>();
  for (const field of resource.fields) {
    if (!field.name) {
      issues.push("A field has no name.");
      continue;
    }
    if (field.name === "id") issues.push("`id` is reserved — every resource gets a uuid primary key.");
    if (seen.has(field.name)) issues.push(`Duplicate field \`${field.name}\`.`);
    seen.add(field.name);
    if (!/^[A-Za-z_][A-Za-z0-9_]*$/.test(field.name)) {
      issues.push(`\`${field.name}\` is not a valid column name.`);
    }
    if (field.type === "reference") {
      if (!field.references) issues.push(`\`${field.name}\` is a reference with no target resource.`);
      else if (!knownResources.includes(field.references)) {
        issues.push(`\`${field.name}\` references \`${field.references}\`, which no resource defines.`);
      }
    }
    if (field.format && field.format !== "plain" && !["string", "text"].includes(field.type)) {
      issues.push(`\`${field.name}\` uses ${field.format} formatting but is not a string or text field.`);
    }
  }
  // Searching means matching part of a value, so the server refuses to load a
  // search field that is missing, not text, or hidden — the same three checks,
  // made here before the file is written.
  for (const name of resource.search_fields ?? []) {
    const field = resource.fields.find((entry) => entry.name === name);
    if (!field) issues.push(`\`search_fields\` names \`${name}\`, which no field defines.`);
    else if (!["string", "text"].includes(field.type)) {
      issues.push(`\`${name}\` is not a string or text field, so it cannot be searched.`);
    } else if (field.hidden) {
      issues.push(`\`${name}\` is hidden, so searching it would reveal what responses withhold.`);
    }
  }
  // Only `user` owns the auth endpoints, so the same key elsewhere names a
  // function nothing would ever call — the server refuses to load it.
  if (resource.name !== "user") {
    for (const event of AUTH_HOOK_EVENTS) {
      if (resource.hooks[event]) {
        issues.push(`\`${event}\` only exists on the \`user\` resource, which owns the auth endpoints.`);
      }
    }
  }
  return issues;
}
