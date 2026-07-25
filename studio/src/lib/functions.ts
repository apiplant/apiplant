/**
 * Reading `functions/` the way `apiplant build` does: an entry is either a
 * source file or a directory, its language comes from what it holds, and the
 * library it produces is `lib<entry>.so` beside it.
 */

import { extensionOf, type ScannedFile } from "./fs";
import { LANGUAGE_EXT, type FunctionEntry, type FunctionFile, type Language } from "./types";

const EXT_LANGUAGE: Record<string, Language> = { rs: "rust", c: "c", zig: "zig", go: "go" };
const LIB_EXTENSIONS = ["so", "dylib", "dll"];

/**
 * Function names a library exports, read out of its source.
 *
 * The manifest is a compile-time constant in every language — `name: "x"` in the
 * `function!` macro, `"name": "x"` in the JSON the C/Zig/Go sources return — so
 * a scan is enough to populate the hook pickers without building anything.
 */
export function extractExports(sources: string[]): string[] {
  const names = new Set<string>();
  const patterns = [
    // `name: "greet"` — the Rust function!/functions! macro.
    /\bname\s*:\s*"([A-Za-z_][A-Za-z0-9_]*)"/g,
    // `"name": "hello"` in a JSON manifest, escaped when it lives in a C string.
    /\\?"name\\?"\s*:\s*\\?"([A-Za-z_][A-Za-z0-9_]*)\\?"/g,
  ];
  for (const source of sources) {
    for (const pattern of patterns) {
      for (const match of source.matchAll(pattern)) names.add(match[1]);
    }
  }
  return [...names].sort((a, b) => a.localeCompare(b));
}

function directoryLanguage(files: FunctionFile[]): Language {
  const names = files.map((f) => f.path.slice(f.path.lastIndexOf("/") + 1));
  if (names.includes("Cargo.toml")) return "rust";
  if (names.includes("go.mod")) return "go";
  const extensions = files.map((f) => extensionOf(f.path));
  if (extensions.includes("zig")) return "zig";
  if (extensions.includes("c")) return "c";
  if (extensions.includes("rs")) return "rust";
  return "c";
}

export function libraryName(name: string): string {
  return `lib${name}.so`;
}

/** Group everything under `functions/` into libraries, configs and artifacts. */
export function detectFunctions(files: ScannedFile[]): {
  entries: FunctionEntry[];
  orphanConfigs: string[];
} {
  const inFunctions = files.filter((f) => f.path.startsWith("functions/"));
  const byName = new Map<string, FunctionEntry>();
  const configs: ScannedFile[] = [];
  const libraries: ScannedFile[] = [];
  const directoryFiles = new Map<string, FunctionFile[]>();

  for (const file of inFunctions) {
    const rest = file.path.slice("functions/".length);
    const slash = rest.indexOf("/");

    if (slash >= 0) {
      const dirName = rest.slice(0, slash);
      const list = directoryFiles.get(dirName) ?? [];
      list.push({ path: file.path, text: file.text, size: file.size });
      directoryFiles.set(dirName, list);
      continue;
    }

    const ext = extensionOf(rest);
    const stem = rest.slice(0, rest.length - (ext ? ext.length + 1 : 0));

    if (EXT_LANGUAGE[ext]) {
      byName.set(stem, {
        name: stem,
        language: EXT_LANGUAGE[ext],
        layout: "file",
        files: [{ path: file.path, text: file.text, size: file.size }],
        configs: [],
        libPath: null,
        libSize: 0,
        exports: extractExports(file.text ? [file.text] : []),
      });
    } else if (ext === "toml") {
      configs.push(file);
    } else if (LIB_EXTENSIONS.includes(ext)) {
      libraries.push(file);
    }
  }

  for (const [dirName, dirFiles] of directoryFiles) {
    dirFiles.sort((a, b) => a.path.localeCompare(b.path));
    byName.set(dirName, {
      name: dirName,
      language: directoryLanguage(dirFiles),
      layout: "directory",
      files: dirFiles,
      configs: [],
      libPath: null,
      libSize: 0,
      exports: extractExports(dirFiles.map((f) => f.text).filter((t): t is string => !!t)),
    });
  }

  for (const lib of libraries) {
    const base = lib.path.slice("functions/".length);
    const ext = extensionOf(base);
    const stem = base.slice(0, base.length - ext.length - 1).replace(/^lib/, "");
    const entry = byName.get(stem);
    if (entry) {
      entry.libPath = lib.path;
      entry.libSize = lib.size;
    }
  }

  const orphanConfigs: string[] = [];
  for (const config of configs) {
    const stem = config.path.slice("functions/".length).replace(/\.toml$/, "");
    // A config is keyed by function name, so it may belong to a library that
    // exports it under a different name (a hooks library, typically).
    const owner =
      byName.get(stem) ?? [...byName.values()].find((entry) => entry.exports.includes(stem)) ?? null;
    if (owner) owner.configs.push({ name: stem, path: config.path });
    else orphanConfigs.push(config.path);
  }

  const entries = [...byName.values()].sort((a, b) => a.name.localeCompare(b.name));
  for (const entry of entries) entry.configs.sort((a, b) => a.name.localeCompare(b.name));
  return { entries, orphanConfigs };
}

/** Where a new single-file function's source goes. */
export function sourcePathFor(name: string, language: Language, layout: "file" | "directory"): string {
  if (layout === "file") return `functions/${name}.${LANGUAGE_EXT[language]}`;
  switch (language) {
    case "rust":
      return `functions/${name}/src/lib.rs`;
    case "go":
      return `functions/${name}/main.go`;
    default:
      return `functions/${name}/${name}.${LANGUAGE_EXT[language]}`;
  }
}

/** A valid function/library name: a lowercase SQL-and-Rust-safe identifier. */
export function isValidFunctionName(name: string): boolean {
  return /^[a-z_][a-z0-9_]*$/.test(name);
}
