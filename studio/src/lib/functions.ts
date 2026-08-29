/**
 * Reading `functions/` the way `apiplant build` does: an entry is a source file
 * or a directory, its language comes from what it holds, and its output sits
 * beside it — `lib<entry>.so` for the compiled languages, `<entry>.js` for
 * TypeScript (run in a V8 isolate, not loaded as a library).
 */

import { extensionOf, type ScannedFile } from "./fs";
import { LANGUAGE_EXT, type FunctionEntry, type FunctionFile, type Language } from "./types";

const EXT_LANGUAGE: Record<string, Language> = {
  rs: "rust",
  ts: "typescript",
  c: "c",
  zig: "zig",
  go: "go",
};
const LIB_EXTENSIONS = ["so", "dylib", "dll"];

/**
 * Files in `functions/` that end in `.ts` but are not functions: the ambient
 * declarations `apiplant build` writes, and any other `.d.ts` an app keeps
 * beside its sources. They are types, and there is nothing in them to build.
 */
function isDeclarations(fileName: string): boolean {
  return fileName.endsWith(".d.ts");
}

/**
 * Function names a library exports, read out of its source. The manifest is a
 * compile-time constant in every language, so a scan populates the hook pickers
 * without building anything.
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
    for (const name of definedFunctions(source)) names.add(name);
  }
  return [...names].sort((a, b) => a.localeCompare(b));
}

/**
 * The keys of a TypeScript module's `defineFunctions({...})` — the key *is*
 * the name, so there is no `name:` to match. Braces are counted rather than the
 * whole call parsed: enough to know which keys are top-level.
 */
function definedFunctions(source: string): string[] {
  const call = source.indexOf("defineFunctions(");
  if (call < 0) return [];
  const open = source.indexOf("{", call);
  if (open < 0) return [];

  const names: string[] = [];
  let depth = 0;
  let atKey = false;
  for (let i = open; i < source.length; i++) {
    const ch = source[i];
    if (ch === "{" || ch === "[" || ch === "(") {
      depth++;
      atKey = depth === 1;
      continue;
    }
    if (ch === "}" || ch === "]" || ch === ")") {
      depth--;
      if (depth === 0) break;
      continue;
    }
    if (ch === ",") {
      atKey = depth === 1;
      continue;
    }
    if (!atKey || depth !== 1) continue;
    if (/\s/.test(ch)) continue;

    const rest = source.slice(i);
    // A key, quoted or not, followed by its colon.
    const key = /^["']?([A-Za-z_$][A-Za-z0-9_$]*)["']?\s*:/.exec(rest);
    if (key) {
      names.push(key[1]);
      i += key[0].length - 1;
    }
    atKey = false;
  }
  return names;
}

function directoryLanguage(files: FunctionFile[]): Language {
  const names = files.map((f) => f.path.slice(f.path.lastIndexOf("/") + 1));
  if (names.includes("Cargo.toml")) return "rust";
  if (names.includes("go.mod")) return "go";
  // A TypeScript directory is an npm project, and package.json is what says so.
  if (names.includes("package.json")) return "typescript";
  const extensions = files.map((f) => extensionOf(f.path));
  if (extensions.includes("zig")) return "zig";
  if (extensions.includes("ts")) return "typescript";
  if (extensions.includes("c")) return "c";
  if (extensions.includes("rs")) return "rust";
  return "c";
}

/**
 * What `apiplant build` writes for this entry.
 *
 * TypeScript is the one language that produces no shared library: there is
 * nothing to link, so the server loads the JavaScript directly.
 */
export function libraryName(name: string, language: Language): string {
  return language === "typescript" ? `${name}.js` : `lib${name}.so`;
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

    // apiplant.d.ts and tsconfig.json are what `apiplant build` writes for your
    // editor, not functions.
    if (isDeclarations(rest) || rest === "tsconfig.json") continue;

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
    } else if (LIB_EXTENSIONS.includes(ext) || ext === "js") {
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
    const bare = base.slice(0, base.length - ext.length - 1);
    // `libgreet.so` belongs to `greet`; `greet.js` is already named for it.
    const stem = ext === "js" ? bare : bare.replace(/^lib/, "");
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
    case "typescript":
      return `functions/${name}/src/index.ts`;
    default:
      return `functions/${name}/${name}.${LANGUAGE_EXT[language]}`;
  }
}

/** A valid function/library name: a lowercase SQL-and-Rust-safe identifier. */
export function isValidFunctionName(name: string): boolean {
  return /^[a-z_][a-z0-9_]*$/.test(name);
}
