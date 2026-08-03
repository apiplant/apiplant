/**
 * The File System Access API, narrowed to what the studio needs: pick a
 * directory once, read it, write back only what changed.
 *
 * Nothing here leaves the machine — there is no server, and the app never
 * uploads a byte. The directory handle is the whole permission model.
 */

const TEXT_EXTENSIONS = new Set([
  "toml",
  "rs",
  "ts",
  "js",
  "mjs",
  "cjs",
  "c",
  "h",
  "zig",
  "go",
  "mod",
  "sum",
  "md",
  "txt",
  "json",
  "lock",
  "yaml",
  "yml",
  "sql",
  "sh",
  "pem",
  "crt",
  "key",
]);

/** Directories that are build output or version control, never app content. */
const SKIP_DIRS = new Set([".apiplant-build", "target", "node_modules", ".git", "pgdata", "dist"]);

/** Text is read in full up to this size; larger files are listed, not loaded. */
const MAX_TEXT_BYTES = 512 * 1024;

export interface ScannedFile {
  /** Path relative to the app root, using forward slashes. */
  path: string;
  text: string | null;
  size: number;
}

/**
 * The picker and the permission calls are still shipping ahead of the DOM
 * typings in some TypeScript releases, so they are reached through these
 * intersections rather than by merging into `lib.dom`.
 */
type DirectoryHandle = FileSystemDirectoryHandle & {
  entries(): AsyncIterable<[string, FileSystemHandle]>;
  queryPermission(options: { mode: "read" | "readwrite" }): Promise<PermissionState>;
  requestPermission(options: { mode: "read" | "readwrite" }): Promise<PermissionState>;
};

type PickerWindow = Window & {
  showDirectoryPicker(options?: { mode?: "read" | "readwrite"; id?: string }): Promise<FileSystemDirectoryHandle>;
};

type DroppedHandleItem = DataTransferItem & {
  getAsFileSystemHandle?: () => Promise<FileSystemHandle | null>;
};

export function isSupported(): boolean {
  return typeof window !== "undefined" && "showDirectoryPicker" in window;
}

export function extensionOf(path: string): string {
  const base = path.slice(path.lastIndexOf("/") + 1);
  const dot = base.lastIndexOf(".");
  return dot > 0 ? base.slice(dot + 1).toLowerCase() : "";
}

export function isTextPath(path: string): boolean {
  return TEXT_EXTENSIONS.has(extensionOf(path));
}

export async function pickDirectory(): Promise<FileSystemDirectoryHandle | null> {
  try {
    return await (window as unknown as PickerWindow).showDirectoryPicker({
      mode: "readwrite",
      id: "apiplant-studio",
    });
  } catch (error) {
    // The user dismissing the picker is not a failure worth reporting.
    if (error instanceof DOMException && error.name === "AbortError") return null;
    throw error;
  }
}

/** The first directory dragged into the page, or null when the drop is not a directory. */
export async function droppedDirectoryHandle(
  items: DataTransferItemList | readonly DataTransferItem[] | null | undefined,
): Promise<FileSystemDirectoryHandle | null> {
  if (!items) return null;
  for (const item of Array.from(items)) {
    if (item.kind !== "file") continue;
    const handle = await (item as DroppedHandleItem).getAsFileSystemHandle?.();
    if (handle?.kind === "directory") return handle as FileSystemDirectoryHandle;
  }
  return null;
}

export async function ensurePermission(handle: FileSystemDirectoryHandle): Promise<boolean> {
  const options = { mode: "readwrite" as const };
  const dir = handle as DirectoryHandle;
  if ((await dir.queryPermission(options)) === "granted") return true;
  return (await dir.requestPermission(options)) === "granted";
}

export async function permissionState(handle: FileSystemDirectoryHandle): Promise<PermissionState> {
  return (handle as DirectoryHandle).queryPermission({ mode: "readwrite" });
}

async function* entriesOf(dir: FileSystemDirectoryHandle) {
  for await (const entry of (dir as DirectoryHandle).entries()) yield entry;
}

/** Recursively read a directory into a flat list of files. */
export async function scanDirectory(
  dir: FileSystemDirectoryHandle,
  prefix = "",
  depth = 0,
): Promise<ScannedFile[]> {
  const files: ScannedFile[] = [];
  if (depth > 8) return files;

  for await (const [name, handle] of entriesOf(dir)) {
    const path = prefix ? `${prefix}/${name}` : name;
    if (handle.kind === "directory") {
      if (SKIP_DIRS.has(name)) continue;
      files.push(...(await scanDirectory(handle as FileSystemDirectoryHandle, path, depth + 1)));
      continue;
    }
    const file = await (handle as FileSystemFileHandle).getFile();
    const readable = isTextPath(path) && file.size <= MAX_TEXT_BYTES;
    files.push({ path, size: file.size, text: readable ? await file.text() : null });
  }

  files.sort((a, b) => a.path.localeCompare(b.path));
  return files;
}

/** Names of the immediate subdirectories, for finding an app inside a folder. */
export async function listSubdirectories(dir: FileSystemDirectoryHandle): Promise<string[]> {
  const names: string[] = [];
  for await (const [name, handle] of entriesOf(dir)) {
    if (handle.kind === "directory" && !SKIP_DIRS.has(name) && !name.startsWith(".")) names.push(name);
  }
  return names.sort((a, b) => a.localeCompare(b));
}

/**
 * Create (or open) a subdirectory. Used to start a new app inside the folder
 * the user picked — the only directory the studio ever creates on its own.
 */
export async function createSubdirectory(
  parent: FileSystemDirectoryHandle,
  name: string,
): Promise<FileSystemDirectoryHandle> {
  return parent.getDirectoryHandle(name, { create: true });
}

export async function listEntryNames(dir: FileSystemDirectoryHandle): Promise<string[]> {
  const names: string[] = [];
  for await (const [name] of entriesOf(dir)) names.push(name);
  return names;
}

async function resolveParent(
  root: FileSystemDirectoryHandle,
  segments: string[],
  create: boolean,
): Promise<FileSystemDirectoryHandle> {
  let dir = root;
  for (const segment of segments) {
    dir = await dir.getDirectoryHandle(segment, { create });
  }
  return dir;
}

export async function writeTextFile(root: FileSystemDirectoryHandle, path: string, text: string): Promise<void> {
  const segments = path.split("/");
  const name = segments.pop()!;
  const dir = await resolveParent(root, segments, true);
  const handle = await dir.getFileHandle(name, { create: true });
  const writable = await handle.createWritable();
  await writable.write(text);
  await writable.close();
}

export async function deleteFile(root: FileSystemDirectoryHandle, path: string): Promise<void> {
  const segments = path.split("/");
  const name = segments.pop()!;
  try {
    const dir = await resolveParent(root, segments, false);
    await dir.removeEntry(name, { recursive: true });
  } catch (error) {
    // Already gone is the outcome we wanted.
    if (error instanceof DOMException && error.name === "NotFoundError") return;
    throw error;
  }
}

/** Remove a directory (used when deleting a whole function directory). */
export async function deleteDirectory(root: FileSystemDirectoryHandle, path: string): Promise<void> {
  await deleteFile(root, path);
}

export function formatBytes(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`;
  return `${(bytes / (1024 * 1024)).toFixed(1)} MB`;
}
