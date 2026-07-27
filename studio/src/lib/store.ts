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
  createSubdirectory,
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
import { BUILTIN_FILENAME, BUILTIN_NAMES, builtinResource, type BuiltinName } from "./builtins";
import { detectFunctions, extractExports } from "./functions";
import { emitResource, emitTable, parseResource, parseTable } from "./toml";
import { scaffoldFunction, scaffoldMainToml, type TemplateKind } from "./templates";
import {
  AUTH_HOOK_EVENTS,
  emptyResource,
  type FileState,
  type FunctionEntry,
  type Language,
  type Resource,
  type ResourceEntry,
  type TomlTable,
} from "./types";

export interface Project {
  handle: FileSystemDirectoryHandle;
  /** Directory name, e.g. `07-functions`. */
  name: string;
  files: Record<string, FileState>;
  resources: ResourceEntry[];
  functions: FunctionEntry[];
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
  return ["main.toml", "models", "functions", "https"].some((name) => entryNames.includes(name));
}

export interface AppCandidate {
  name: string;
  handle: FileSystemDirectoryHandle;
}

/** Pick a directory, then work out whether it is an app or contains apps. */
export async function chooseDirectory(): Promise<
  | { kind: "app"; handle: FileSystemDirectoryHandle }
  | {
      kind: "candidates";
      parent: FileSystemDirectoryHandle;
      /** Everything in the parent, so a new app can be named without collision. */
      parentEntryNames: string[];
      candidates: AppCandidate[];
    }
  | null
> {
  const handle = await pickDirectory();
  if (!handle) return null;

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

function buildResources(scanned: ScannedFile[]): {
  resources: ResourceEntry[];
  problems: { path: string; message: string }[];
} {
  const problems: { path: string; message: string }[] = [];
  const byName = new Map<string, ResourceEntry>();

  for (const name of BUILTIN_NAMES) {
    byName.set(name, { name, path: null, builtin: true, resource: builtinResource(name) });
  }

  for (const file of scanned) {
    if (!file.path.startsWith("models/") || !file.path.endsWith(".toml") || file.text === null) continue;
    // Only top-level files in models/ are loaded by the framework.
    if (file.path.slice("models/".length).includes("/")) continue;
    try {
      const resource = parseResource(file.text);
      byName.set(resource.name, {
        name: resource.name,
        path: file.path,
        builtin: (BUILTIN_NAMES as readonly string[]).includes(resource.name),
        resource,
      });
    } catch (error) {
      problems.push({ path: file.path, message: error instanceof Error ? error.message : String(error) });
    }
  }

  const resources = [...byName.values()].sort((a, b) => a.name.localeCompare(b.name));
  return { resources, problems };
}

async function buildProject(handle: FileSystemDirectoryHandle): Promise<Project> {
  const scanned = await scanDirectory(handle);
  const { resources, problems } = buildResources(scanned);
  const { entries, orphanConfigs } = detectFunctions(scanned);

  let config: TomlTable = {};
  const main = scanned.find((f) => f.path === CONFIG_PATH);
  if (main?.text) {
    try {
      config = parseTable(main.text);
    } catch (error) {
      problems.push({ path: CONFIG_PATH, message: error instanceof Error ? error.message : String(error) });
    }
  }

  return {
    handle,
    name: handle.name,
    files: buildFileMap(scanned),
    resources,
    functions: entries,
    orphanConfigs,
    config,
    pendingDirDeletes: [],
    problems,
    hasTls: scanned.some((f) => f.path.startsWith("https/")),
  };
}

export async function openProject(
  handle: FileSystemDirectoryHandle,
  options: { quiet?: boolean } = {},
): Promise<void> {
  setState("loading", true);
  try {
    setState("project", await buildProject(handle));
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

export function setConfigValue(section: string, key: string, value: string | number | boolean | undefined) {
  if (!state.project) return;
  setState(
    "project",
    "config",
    produce((config: TomlTable) => {
      const table = (config[section] ??= {}) as TomlTable;
      if (value === undefined || value === "") delete table[key];
      else table[key] = value;
      if (Object.keys(table).length === 0) delete config[section];
    }),
  );
  syncConfigFile();
}

export function configValue(section: string, key: string): string | number | boolean | undefined {
  const table = state.project?.config[section];
  if (!table || typeof table !== "object" || Array.isArray(table)) return undefined;
  const value = (table as TomlTable)[key];
  if (typeof value === "string" || typeof value === "number" || typeof value === "boolean") return value;
  return undefined;
}

/** Replace main.toml wholesale from the raw editor. */
export function setConfigFromToml(text: string) {
  const parsed = parseTable(text);
  setState("project", "config", parsed);
  setFileText(CONFIG_PATH, text);
}

export function ensureMainToml() {
  const project = state.project;
  if (!project || project.files[CONFIG_PATH]) return;
  const text = scaffoldMainToml(project.name);
  setState("project", "config", parseTable(text));
  setFileText(CONFIG_PATH, text);
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
  toast(`${name} is now a file you own — models/${BUILTIN_FILENAME[name as BuiltinName]}`, "success");
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
    setState("project", "resources", index, {
      path: null,
      resource: builtinResource(name as BuiltinName),
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
