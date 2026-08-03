export interface PersistedListFilter {
  query: string;
  ownerOnly: boolean;
  /** Column the table is sorted by, or "" for the resource's own default. */
  sortField?: string;
  sortDescending?: boolean;
}

interface StoredFilters {
  resources?: Record<string, PersistedListFilter>;
  conversations?: Record<string, PersistedListFilter>;
}

const STORAGE_KEY = "apiplant-admin-filters";

function readStoredFilters(): StoredFilters {
  try {
    const raw = localStorage.getItem(STORAGE_KEY);
    if (!raw) return {};
    const parsed = JSON.parse(raw);
    return parsed && typeof parsed === "object" ? (parsed as StoredFilters) : {};
  } catch {
    localStorage.removeItem(STORAGE_KEY);
    return {};
  }
}

function writeStoredFilters(next: StoredFilters) {
  localStorage.setItem(STORAGE_KEY, JSON.stringify(next));
}

function normaliseFilter(
  value: PersistedListFilter | undefined,
  defaults: PersistedListFilter,
): PersistedListFilter {
  return {
    query: typeof value?.query === "string" ? value.query : defaults.query,
    ownerOnly: typeof value?.ownerOnly === "boolean" ? value.ownerOnly : defaults.ownerOnly,
    sortField: typeof value?.sortField === "string" ? value.sortField : defaults.sortField,
    sortDescending:
      typeof value?.sortDescending === "boolean" ? value.sortDescending : defaults.sortDescending,
  };
}

export function loadResourceFilter(resourceName: string, defaults: PersistedListFilter): PersistedListFilter {
  return normaliseFilter(readStoredFilters().resources?.[resourceName], defaults);
}

export function saveResourceFilter(resourceName: string, filter: PersistedListFilter) {
  const stored = readStoredFilters();
  writeStoredFilters({
    ...stored,
    resources: {
      ...(stored.resources ?? {}),
      [resourceName]: filter,
    },
  });
}

export function loadConversationFilter(agentName: string, defaults: PersistedListFilter): PersistedListFilter {
  return normaliseFilter(readStoredFilters().conversations?.[agentName], defaults);
}

export function saveConversationFilter(agentName: string, filter: PersistedListFilter) {
  const stored = readStoredFilters();
  writeStoredFilters({
    ...stored,
    conversations: {
      ...(stored.conversations ?? {}),
      [agentName]: filter,
    },
  });
}
