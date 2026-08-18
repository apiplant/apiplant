/**
 * A File System Access API the studio can be photographed against.
 *
 * `showDirectoryPicker()` opens a native dialog no browser automation can
 * reach, so the studio cannot be driven at all without replacing it. This
 * installs an in-memory implementation of exactly the surface `studio/src/lib/
 * fs.ts` uses — `entries`, `getDirectoryHandle`, `getFileHandle`,
 * `createWritable`, `removeEntry`, `queryPermission`, `requestPermission` —
 * seeded with a real example app read off disk.
 *
 * The studio itself is untouched: it goes on believing it holds a directory
 * handle, and writes land in the object graph instead of the filesystem, which
 * is what makes these runs safe to point at a checked-in example.
 */

import { readdirSync, readFileSync, statSync } from "node:fs";
import { join } from "node:path";

/** A directory as a plain object: file name → contents, subdirectory → tree. */
export interface Tree {
  [name: string]: string | Tree;
}

/** Directories that are build output or version control, never app content. */
const SKIP = new Set([".apiplant-build", "target", "node_modules", ".git", "pgdata", "dist"]);

/** Read an app directory off disk into the tree the shim is seeded with. */
export function readTree(root: string, dir = root): Tree {
  const tree: Tree = {};
  for (const entry of readdirSync(dir, { withFileTypes: true })) {
    if (SKIP.has(entry.name)) continue;
    const path = join(dir, entry.name);
    if (entry.isDirectory()) {
      tree[entry.name] = readTree(root, path);
    } else if (statSync(path).size <= 512 * 1024) {
      tree[entry.name] = readFileSync(path, "utf8");
    }
  }
  return tree;
}

/**
 * The script that runs in the page before any of the studio's own code. It is
 * serialised by Playwright, so it may close over nothing: everything it needs
 * arrives in `argument`.
 */
export function installFileSystemAccess({ name, tree }: { name: string; tree: Tree }): void {
  type Node = string | Record<string, unknown>;

  const isDirectory = (node: Node): node is Record<string, Node> => typeof node !== "string";

  function fileHandle(parent: Record<string, Node>, fileName: string) {
    return {
      kind: "file" as const,
      name: fileName,
      async getFile() {
        const text = String(parent[fileName] ?? "");
        return new File([text], fileName, { type: "text/plain" });
      },
      async createWritable() {
        let buffer = "";
        return {
          async write(chunk: unknown) {
            buffer += typeof chunk === "string" ? chunk : String(chunk);
          },
          async close() {
            parent[fileName] = buffer;
          },
        };
      },
    };
  }

  function directoryHandle(node: Record<string, Node>, dirName: string): unknown {
    const handle = {
      kind: "directory" as const,
      name: dirName,

      async *entries(): AsyncGenerator<[string, unknown]> {
        for (const key of Object.keys(node)) {
          const child = node[key];
          yield [key, isDirectory(child) ? directoryHandle(child, key) : fileHandle(node, key)];
        }
      },

      async *values(): AsyncGenerator<unknown> {
        for await (const [, child] of handle.entries()) yield child;
      },

      async getDirectoryHandle(childName: string, options?: { create?: boolean }) {
        const child = node[childName];
        if (child === undefined) {
          if (!options?.create) throw new DOMException(childName, "NotFoundError");
          node[childName] = {};
          return directoryHandle(node[childName] as Record<string, Node>, childName);
        }
        if (!isDirectory(child)) throw new DOMException(childName, "TypeMismatchError");
        return directoryHandle(child, childName);
      },

      async getFileHandle(childName: string, options?: { create?: boolean }) {
        if (node[childName] === undefined) {
          if (!options?.create) throw new DOMException(childName, "NotFoundError");
          node[childName] = "";
        }
        return fileHandle(node, childName);
      },

      async removeEntry(childName: string) {
        if (node[childName] === undefined) throw new DOMException(childName, "NotFoundError");
        delete node[childName];
      },

      async queryPermission() {
        return "granted" as PermissionState;
      },
      async requestPermission() {
        return "granted" as PermissionState;
      },
      async isSameEntry(other: { name?: string }) {
        return other?.name === dirName;
      },
    };

    return handle;
  }

  const root = directoryHandle(tree as Record<string, Node>, name);

  Object.defineProperty(window, "showDirectoryPicker", {
    configurable: true,
    value: async () => root,
  });
}
