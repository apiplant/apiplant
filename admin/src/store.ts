/**
 * Session, API access, and the handful of derived facts every screen needs.
 *
 * The dashboard is a static bundle talking to a deployed API, so all of this is
 * client-side: a token in `localStorage`, an active organisation, and the
 * caller's role in it. Permission checks here decide what to *offer* — the
 * server decides what to allow, and is the only thing that can.
 */

import { createSignal } from "solid-js";
import { createMutable } from "solid-js/store";
import type {
  Action,
  AdminManifest,
  ApiRecord,
  FunctionManifest,
  ResourceManifest,
  Route,
  Toast,
  ToastKind,
} from "./types";

const SESSION_KEY = "apiplant-admin-session";

export interface Session {
  token: string;
  apiKey: string;
  userId: string | null;
  profile: ApiRecord | null;
  organizations: ApiRecord[];
  organizationId: string;
  role: string | null;
  loading: boolean;
}

export const session = createMutable<Session>({
  token: "",
  apiKey: "",
  userId: null,
  profile: null,
  organizations: [],
  organizationId: "",
  role: null,
  loading: false,
});

export const [manifest, setManifest] = createSignal<AdminManifest | null>(null);
export const [route, setRouteSignal] = createSignal<Route>({ kind: "dashboard" });
export const [toasts, setToasts] = createSignal<Toast[]>([]);

// --- notifications ---------------------------------------------------------

let toastId = 0;

export function notify(kind: ToastKind, message: string) {
  const id = ++toastId;
  setToasts((current) => [...current, { id, kind, message }]);
  // Errors stay until dismissed: they usually say something the operator needs
  // to act on, and a message that vanishes mid-read is worse than none.
  if (kind !== "error") {
    window.setTimeout(() => dismissToast(id), 4000);
  }
}

export function dismissToast(id: number) {
  setToasts((current) => current.filter((toast) => toast.id !== id));
}

export function reportError(error: unknown) {
  notify("error", error instanceof Error ? error.message : String(error));
}

// --- routing ---------------------------------------------------------------

/** Serialise a route into a URL hash, so refreshing lands where you were. */
function routeToHash(next: Route): string {
  switch (next.kind) {
    case "dashboard":
      return "#/";
    case "resource":
      return `#/r/${encodeURIComponent(next.name)}`;
    case "new":
      return `#/r/${encodeURIComponent(next.name)}/new`;
    case "record":
      return `#/r/${encodeURIComponent(next.name)}/${encodeURIComponent(next.id)}`;
    case "action":
      return `#/a/${encodeURIComponent(next.name)}`;
    case "cli":
      // The console handoff carries its callback in the hash's query, and this
      // is the one route whose address is not reconstructible from its kind —
      // so keep whatever is already there.
      return window.location.hash || "#/cli";
    default:
      return `#/${next.kind}`;
  }
}

export function parseHash(hash: string): Route {
  // Only the console handoff uses a query string, and it belongs to the page,
  // not to the path.
  const [path] = hash.split("?");
  const parts = path.replace(/^#\/?/, "").split("/").filter(Boolean).map(decodeURIComponent);
  if (parts[0] === "r" && parts[1]) {
    if (parts[2] === "new") return { kind: "new", name: parts[1] };
    if (parts[2]) return { kind: "record", name: parts[1], id: parts[2] };
    return { kind: "resource", name: parts[1] };
  }
  if (parts[0] === "a" && parts[1]) return { kind: "action", name: parts[1] };
  if (parts[0] === "account") return { kind: "account" };
  if (parts[0] === "team") return { kind: "team" };
  if (parts[0] === "organization") return { kind: "organization" };
  if (parts[0] === "keys") return { kind: "keys" };
  if (parts[0] === "cli") return { kind: "cli" };
  return { kind: "dashboard" };
}

export function navigate(next: Route) {
  setRouteSignal(next);
  const hash = routeToHash(next);
  if (window.location.hash !== hash) window.history.pushState(null, "", hash);
}

/** Adopt whatever the address bar currently says (back/forward, or a paste). */
export function syncRouteFromHash() {
  setRouteSignal(parseHash(window.location.hash));
}

// --- session persistence ---------------------------------------------------

function decodeJwtSubject(token: string): string | null {
  try {
    const payload = token.split(".")[1];
    if (!payload) return null;
    const normalised = payload.replaceAll("-", "+").replaceAll("_", "/");
    const padded = normalised.padEnd(Math.ceil(normalised.length / 4) * 4, "=");
    const claims = JSON.parse(atob(padded));
    return typeof claims.sub === "string" ? claims.sub : null;
  } catch {
    return null;
  }
}

export function restoreSession() {
  try {
    const raw = localStorage.getItem(SESSION_KEY);
    if (!raw) return;
    const stored = JSON.parse(raw);
    session.token = typeof stored.token === "string" ? stored.token : "";
    session.apiKey = typeof stored.apiKey === "string" ? stored.apiKey : "";
    session.organizationId = typeof stored.organizationId === "string" ? stored.organizationId : "";
    session.userId = session.token ? decodeJwtSubject(session.token) : null;
  } catch {
    localStorage.removeItem(SESSION_KEY);
  }
}

export function persistSession() {
  localStorage.setItem(
    SESSION_KEY,
    JSON.stringify({
      token: session.token,
      apiKey: session.apiKey,
      organizationId: session.organizationId,
    }),
  );
}

export function isSignedIn() {
  return Boolean(session.token || session.apiKey);
}

export function signOut() {
  session.token = "";
  session.apiKey = "";
  session.userId = null;
  session.profile = null;
  session.organizations = [];
  session.organizationId = "";
  session.role = null;
  persistSession();
  navigate({ kind: "dashboard" });
}

// --- API -------------------------------------------------------------------

export type ApiError = Error & { status?: number };

export interface RequestOptions {
  method?: string;
  body?: unknown;
  /** Send the active organisation header — needed by org-scoped endpoints. */
  org?: boolean;
}

export async function api(path: string, options: RequestOptions = {}): Promise<unknown> {
  const current = manifest();
  if (!current) throw new Error("The dashboard is still loading.");

  const headers: Record<string, string> = { Accept: "application/json" };
  if (session.apiKey) headers["X-Api-Key"] = session.apiKey;
  if (session.token) headers.Authorization = `Bearer ${session.token}`;
  if (options.body !== undefined) headers["Content-Type"] = "application/json";
  if (options.org && session.organizationId) headers["X-Organization"] = session.organizationId;

  const response = await fetch(`${current.api_base_url}${path}`, {
    method: options.method ?? "GET",
    headers,
    body: options.body === undefined ? undefined : JSON.stringify(options.body),
  });

  if (response.status === 204) return null;
  const text = await response.text();
  let payload: unknown = null;
  try {
    payload = text ? JSON.parse(text) : null;
  } catch {
    payload = text;
  }

  if (!response.ok) {
    const detail =
      payload && typeof payload === "object" && typeof (payload as ApiRecord).error === "string"
        ? ((payload as ApiRecord).error as string)
        : humanStatus(response.status);
    const error = new Error(detail) as ApiError;
    error.status = response.status;
    throw error;
  }
  return payload;
}

/**
 * Plain-language fallbacks. A status code is not a message to show someone who
 * did not ask for one.
 */
function humanStatus(status: number): string {
  switch (status) {
    case 401:
      return "Your session has expired. Please sign in again.";
    case 403:
      return "You do not have permission to do that.";
    case 404:
      return "That could not be found.";
    case 409:
      return "That conflicts with something that already exists.";
    case 422:
      return "Some of those details were not accepted.";
    default:
      return status >= 500 ? "The server had a problem. Please try again." : "That did not work.";
  }
}

export function asRecord(value: unknown): ApiRecord | null {
  return value && typeof value === "object" && !Array.isArray(value) ? (value as ApiRecord) : null;
}

export function asRecords(value: unknown): ApiRecord[] {
  return Array.isArray(value) ? (value as ApiRecord[]) : [];
}

// --- session lifecycle -----------------------------------------------------

export async function refreshSession() {
  if (!isSignedIn()) return;
  session.loading = true;
  try {
    const [organizations, profile] = await Promise.all([
      api("/organization").then(asRecords).catch(() => []),
      session.userId
        ? api(`/user/${encodeURIComponent(session.userId)}`)
            .then(asRecord)
            .catch(() => null)
        : Promise.resolve(null),
    ]);
    session.organizations = organizations;
    session.profile = profile;

    // Pick the only organisation automatically; nobody should have to choose
    // between one option.
    const known = organizations.map((organization) => String(organization.id ?? ""));
    if (!known.includes(session.organizationId)) {
      session.organizationId = known.length ? known[0] : "";
    }
    persistSession();
    await refreshRole();
  } finally {
    session.loading = false;
  }
}

/** Find the caller's role in the active organisation, for permission checks. */
export async function refreshRole() {
  session.role = null;
  if (!session.organizationId || !session.userId) return;
  try {
    const members = asRecords(
      await api(`/membership?user_id=${encodeURIComponent(session.userId)}&limit=1`, { org: true }),
    );
    const mine = members[0];
    session.role = typeof mine?.role === "string" ? mine.role : null;
  } catch {
    // A member who cannot list memberships still gets to use the dashboard;
    // they simply see the actions their role allows, which is none of the
    // role-gated ones.
    session.role = null;
  }
}

export async function setActiveOrganization(id: string) {
  session.organizationId = id;
  persistSession();
  await refreshRole();
}

export function currentOrganization(): ApiRecord | null {
  return (
    session.organizations.find(
      (organization) => String(organization.id ?? "") === session.organizationId,
    ) ?? null
  );
}

export function organizationLabel(organization: ApiRecord | null): string {
  if (!organization) return "No organization";
  return String(organization.name ?? organization.slug ?? organization.id ?? "Organization");
}

/** The name to show for whoever is signed in. */
export function currentUserLabel(): string {
  const current = manifest();
  const profile = session.profile;
  if (profile && current) {
    const identity = profile[current.auth.identity_field];
    if (typeof identity === "string" && identity) return identity;
    if (typeof profile.display_name === "string" && profile.display_name) return profile.display_name;
  }
  return session.apiKey ? "API key session" : "Signed in";
}

// --- permissions -----------------------------------------------------------

/**
 * Whether the current session may perform an action, as far as the manifest
 * can tell. `owner` is answered optimistically for lists and creates — whether
 * a *particular* row is yours is something only the server knows, and it
 * already filters rows you do not own out of the list.
 */
export function can(resource: ResourceManifest, action: Action): boolean {
  const policy = resource.permissions[action];
  switch (policy.value) {
    case "public":
      return true;
    case "private":
      return false;
    case "authenticated":
      return isSignedIn();
    case "member":
    case "owner":
      // Org-scoped work needs somewhere to do it.
      return isSignedIn() && (resource.scope === "global" || Boolean(session.organizationId));
    default:
      return policy.role ? session.role === policy.role : false;
  }
}

/** Whether a resource belongs in this operator's navigation. */
export function isResourceVisible(resource: ResourceManifest): boolean {
  if (!resource.visible) return false;
  if (resource.roles.length && !resource.roles.includes(session.role ?? "")) return false;
  return can(resource, "list");
}

/** Whether an action belongs in this operator's action list. */
export function isFunctionVisible(fn: FunctionManifest): boolean {
  if (!fn.visible) return false;
  if (fn.roles.length && !fn.roles.includes(session.role ?? "")) return false;
  switch (fn.permission) {
    case "public":
      return true;
    case "authenticated":
      return isSignedIn();
    case "member":
      return isSignedIn() && Boolean(session.organizationId);
    case "private":
      return false;
    default:
      return fn.role ? session.role === fn.role : false;
  }
}

/**
 * Whether the person signed in has nowhere to work yet.
 *
 * Almost everything in an app is scoped to an organisation, so a session with
 * none is a session where most screens are empty and every write fails. That is
 * a state to resolve on the way in, not one to let someone wander around in.
 */
export function needsOrganization(): boolean {
  return isSignedIn() && !session.loading && session.organizations.length === 0;
}

/**
 * Whether this app lets a member start an organisation of their own.
 *
 * The built-in `organization` resource says `create = "authenticated"`, so they
 * can — but an app that provisions tenants itself will have narrowed that, and
 * then the only way in is for someone who is already an admin to add them. A
 * `role:` policy can never be satisfied by someone with no organisation, so it
 * counts as "no" here.
 */
export function mayCreateOrganization(): boolean {
  const policy = resourceByName("organization")?.permissions.create;
  return policy?.value === "public" || policy?.value === "authenticated";
}

export function resourceByName(name: string | null | undefined): ResourceManifest | null {
  return manifest()?.resources.find((resource) => resource.name === name) ?? null;
}

export function functionByName(name: string | null | undefined): FunctionManifest | null {
  return manifest()?.functions.find((fn) => fn.name === name) ?? null;
}

/** Resources the navigation should list, grouped and ordered as configured. */
export function navigationGroups(): { group: string | null; resources: ResourceManifest[] }[] {
  const visible = (manifest()?.resources ?? []).filter(isResourceVisible);
  const groups = new Map<string | null, ResourceManifest[]>();
  for (const resource of visible) {
    const key = resource.group ?? null;
    if (!groups.has(key)) groups.set(key, []);
    groups.get(key)!.push(resource);
  }
  for (const resources of groups.values()) {
    resources.sort((left, right) => left.order - right.order || left.plural.localeCompare(right.plural));
  }
  // Named groups first, alphabetically; the ungrouped remainder last.
  return [...groups.entries()]
    .sort(([left], [right]) => {
      if (left === right) return 0;
      if (left === null) return 1;
      if (right === null) return -1;
      return left.localeCompare(right);
    })
    .map(([group, resources]) => ({ group, resources }));
}

export function visibleFunctions(): FunctionManifest[] {
  return (manifest()?.functions ?? []).filter(isFunctionVisible);
}
