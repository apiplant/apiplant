const META_KEY = "apiplant-studio-last-project";
const DB_NAME = "apiplant-studio";
const STORE_NAME = "handles";
const HANDLE_KEY = "last-project";

interface RememberedProjectMeta {
  name: string;
}

export interface RememberedProject {
  name: string;
  handle: FileSystemDirectoryHandle;
}

function readMeta(): RememberedProjectMeta | null {
  try {
    const raw = localStorage.getItem(META_KEY);
    if (!raw) return null;
    const parsed = JSON.parse(raw) as unknown;
    if (typeof parsed === "object" && parsed !== null && typeof (parsed as { name?: unknown }).name === "string") {
      return { name: (parsed as { name: string }).name };
    }
  } catch {
    // Private browsing and corrupted storage both mean "nothing remembered".
  }
  return null;
}

function writeMeta(meta: RememberedProjectMeta) {
  try {
    localStorage.setItem(META_KEY, JSON.stringify(meta));
  } catch {
    // Best effort only.
  }
}

function clearMeta() {
  try {
    localStorage.removeItem(META_KEY);
  } catch {
    // Best effort only.
  }
}

function openDatabase(): Promise<IDBDatabase | null> {
  if (typeof indexedDB === "undefined") return Promise.resolve(null);
  return new Promise((resolve, reject) => {
    const request = indexedDB.open(DB_NAME, 1);
    request.onupgradeneeded = () => {
      const db = request.result;
      if (!db.objectStoreNames.contains(STORE_NAME)) db.createObjectStore(STORE_NAME);
    };
    request.onsuccess = () => resolve(request.result);
    request.onerror = () => reject(request.error ?? new Error("Could not open IndexedDB"));
  });
}

async function withStore<T>(
  mode: IDBTransactionMode,
  run: (store: IDBObjectStore) => IDBRequest<T>,
): Promise<T | undefined> {
  const db = await openDatabase();
  if (!db) return undefined;
  return new Promise((resolve, reject) => {
    const transaction = db.transaction(STORE_NAME, mode);
    const store = transaction.objectStore(STORE_NAME);
    const request = run(store);
    request.onsuccess = () => resolve(request.result);
    request.onerror = () => reject(request.error ?? new Error("IndexedDB request failed"));
    transaction.oncomplete = () => db.close();
    transaction.onerror = () => {
      db.close();
      reject(transaction.error ?? new Error("IndexedDB transaction failed"));
    };
    transaction.onabort = () => {
      db.close();
      reject(transaction.error ?? new Error("IndexedDB transaction aborted"));
    };
  });
}

export async function rememberProject(handle: FileSystemDirectoryHandle): Promise<void> {
  writeMeta({ name: handle.name });
  await withStore("readwrite", (store) => store.put(handle, HANDLE_KEY));
}

export async function clearRememberedProject(): Promise<void> {
  clearMeta();
  await withStore("readwrite", (store) => store.delete(HANDLE_KEY));
}

export async function loadRememberedProject(): Promise<RememberedProject | null> {
  const meta = readMeta();
  if (!meta) return null;
  const handle = await withStore<FileSystemDirectoryHandle | undefined>("readonly", (store) => store.get(HANDLE_KEY));
  if (!handle) {
    await clearRememberedProject();
    return null;
  }
  return { name: meta.name, handle };
}
