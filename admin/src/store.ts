/**
 * Session, API access, and the handful of derived facts every screen needs.
 *
 * The dashboard is a static bundle talking to a deployed API, so all of this is
 * client-side: a token in `localStorage`, an active organisation, and the
 * caller's role in it. Permission checks here decide what to *offer*; the
 * server decides what to allow, and is the only place that can enforce it.
 */

import { createSignal } from "solid-js";
import { createMutable } from "solid-js/store";
import type {
  Action,
  ActionPermissionManifest,
  AdminManifest,
  AgentManifest,
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
  /**
   * Every role held in the active organisation: the membership's primary role
   * plus its `membership_role` rows. A user can hold several, so this is a set
   * rather than a single value.
   */
  roles: string[];
  loading: boolean;
}

export const session = createMutable<Session>({
  token: "",
  apiKey: "",
  userId: null,
  profile: null,
  organizations: [],
  organizationId: "",
  roles: [],
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
  // Errors stay until dismissed: they usually require action, and a message
  // that disappears while being read is worse than none.
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
    case "agent":
      return next.threadId
        ? `#/g/${encodeURIComponent(next.name)}/${encodeURIComponent(next.threadId)}`
        : `#/g/${encodeURIComponent(next.name)}`;
    case "cli":
      // The console handoff carries its callback in the hash's query, and this
      // is the one route whose address cannot be reconstructed from its kind,
      // so whatever is already there is preserved.
      return window.location.hash || "#/cli";
    case "accept-invite":
    case "verify-email":
    case "reset-password":
      // These are entered directly rather than navigated to: the address was
      // composed by the server and opened from an email. Keeping the token in
      // the hash means a refresh mid-form does not discard the link.
      return `#/${next.kind}?token=${encodeURIComponent(next.token)}`;
    default:
      return `#/${next.kind}`;
  }
}

export function parseHash(hash: string): Route {
  // The console handoff and the three emailed links use a query string, which
  // belongs to the page rather than the path.
  const [path, query = ""] = hash.split("?");
  const parts = path.replace(/^#\/?/, "").split("/").filter(Boolean).map(decodeURIComponent);

  if (parts[0] === "accept-invite" || parts[0] === "verify-email" || parts[0] === "reset-password") {
    const token = new URLSearchParams(query).get("token") ?? "";
    return { kind: parts[0], token };
  }
  if (parts[0] === "r" && parts[1]) {
    if (parts[2] === "new") return { kind: "new", name: parts[1] };
    if (parts[2]) return { kind: "record", name: parts[1], id: parts[2] };
    return { kind: "resource", name: parts[1] };
  }
  if (parts[0] === "a" && parts[1]) return { kind: "action", name: parts[1] };
  if (parts[0] === "g" && parts[1]) {
    return parts[2] ? { kind: "agent", name: parts[1], threadId: parts[2] } : { kind: "agent", name: parts[1] };
  }
  if (parts[0] === "account") return { kind: "account" };
  if (parts[0] === "team") return { kind: "team" };
  if (parts[0] === "organization") return { kind: "organization" };
  if (parts[0] === "keys") return { kind: "keys" };
  if (parts[0] === "billing") return { kind: "billing" };
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
  session.roles = [];
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

function requestHeaders(options: RequestOptions, accept: string): Record<string, string> {
  const headers: Record<string, string> = { Accept: accept };
  if (session.apiKey) headers["X-Api-Key"] = session.apiKey;
  if (session.token) headers.Authorization = `Bearer ${session.token}`;
  if (options.body !== undefined) headers["Content-Type"] = "application/json";
  if (options.org && session.organizationId) headers["X-Organization"] = session.organizationId;
  return headers;
}

async function readPayload(response: Response): Promise<unknown> {
  const text = await response.text();
  if (!text) return null;
  try {
    return JSON.parse(text);
  } catch {
    return text;
  }
}

function responseError(response: Response, payload: unknown): ApiError {
  const detail =
    payload && typeof payload === "object" && typeof (payload as ApiRecord).error === "string"
      ? ((payload as ApiRecord).error as string)
      : humanStatus(response.status);
  const error = new Error(detail) as ApiError;
  error.status = response.status;
  return error;
}

export async function api(path: string, options: RequestOptions = {}): Promise<unknown> {
  const current = manifest();
  if (!current) throw new Error("The dashboard is still loading.");

  const response = await fetch(`${current.api_base_url}${path}`, {
    method: options.method ?? "GET",
    headers: requestHeaders(options, "application/json"),
    body: options.body === undefined ? undefined : JSON.stringify(options.body),
  });

  if (response.status === 204) return null;
  const payload = await readPayload(response);
  if (!response.ok) throw responseError(response, payload);
  return payload;
}

interface StreamState {
  error: string | null;
  result: unknown;
  done: boolean;
}

function parseStreamEvent(frame: string): { name: string; data: unknown } | null {
  let name = "message";
  const data: string[] = [];

  for (const line of frame.split(/\r?\n/)) {
    if (!line || line.startsWith(":")) continue;
    const cut = line.indexOf(":");
    const field = cut >= 0 ? line.slice(0, cut) : line;
    let value = cut >= 0 ? line.slice(cut + 1) : "";
    if (value.startsWith(" ")) value = value.slice(1);
    if (field === "event") name = value;
    if (field === "data") data.push(value);
  }

  if (!data.length) return null;

  const joined = data.join("\n");
  try {
    return { name, data: JSON.parse(joined) };
  } catch {
    return { name, data: joined };
  }
}

function applyStreamFrame(
  frame: string,
  state: StreamState,
  onDelta?: (text: string) => void,
  onReasoning?: (text: string) => void,
) {
  const event = parseStreamEvent(frame);
  if (!event) return;

  const payload = asRecord(event.data);
  if (event.name === "delta") {
    const text = payload?.text;
    if (typeof text === "string") onDelta?.(text);
    return;
  }
  if (event.name === "reasoning") {
    const text = payload?.text;
    if (typeof text === "string") onReasoning?.(text);
    return;
  }
  if (event.name === "error") {
    const message = payload?.error;
    state.error = typeof message === "string" ? message : "The stream failed.";
    return;
  }
  if (event.name === "done") {
    state.done = true;
    state.result =
      payload && Object.prototype.hasOwnProperty.call(payload, "result") ? payload.result : event.data;
  }
}

function drainStreamFrames(
  buffer: string,
  state: StreamState,
  onDelta?: (text: string) => void,
  onReasoning?: (text: string) => void,
  flush = false,
): string {
  for (;;) {
    const match = buffer.match(/\r?\n\r?\n/);
    if (!match || match.index === undefined) break;
    applyStreamFrame(buffer.slice(0, match.index), state, onDelta, onReasoning);
    buffer = buffer.slice(match.index + match[0].length);
  }

  if (flush && buffer.trim()) {
    applyStreamFrame(buffer.trimEnd(), state, onDelta, onReasoning);
    return "";
  }

  return buffer;
}

function finishStream(state: StreamState): unknown {
  if (!state.done) throw new Error(state.error ?? "The stream ended before the function finished.");
  if (state.error) throw new Error(state.error);
  return state.result;
}

async function readEventStream(
  response: Response,
  onDelta?: (text: string) => void,
  onReasoning?: (text: string) => void,
): Promise<unknown> {
  const state: StreamState = { error: null, result: null, done: false };
  const body = response.body;
  if (!body) {
    const text = await response.text();
    drainStreamFrames(text, state, onDelta, onReasoning, true);
    return finishStream(state);
  }

  const reader = body.getReader();
  const decoder = new TextDecoder();
  let buffer = "";

  for (;;) {
    const { value, done } = await reader.read();
    if (done) break;
    buffer += decoder.decode(value, { stream: true });
    buffer = drainStreamFrames(buffer, state, onDelta, onReasoning);
  }

  buffer += decoder.decode();
  drainStreamFrames(buffer, state, onDelta, onReasoning, true);
  return finishStream(state);
}

export async function apiStream(
  path: string,
  options: RequestOptions = {},
  onDelta?: (text: string) => void,
  onReasoning?: (text: string) => void,
): Promise<unknown> {
  const current = manifest();
  if (!current) throw new Error("The dashboard is still loading.");

  const response = await fetch(`${current.api_base_url}${path}`, {
    method: options.method ?? "GET",
    headers: requestHeaders(options, "text/event-stream, application/json"),
    body: options.body === undefined ? undefined : JSON.stringify(options.body),
  });

  if (response.status === 204) return null;
  if (!response.ok) {
    const payload = await readPayload(response);
    throw responseError(response, payload);
  }

  const contentType = response.headers.get("content-type") ?? "";
  if (!contentType.includes("text/event-stream")) return readPayload(response);
  return readEventStream(response, onDelta, onReasoning);
}

/**
 * Plain-language fallbacks, since a bare status code is not a useful message
 * for an operator.
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

/**
 * Check with the server whether the stored credential is still valid.
 *
 * A token in `localStorage` proves nothing on its own: it may be signed with a
 * secret the server has since rotated, or name a deleted account. `/auth/me`
 * covers both cases, and a 401 means the credential is invalid, so it is
 * dropped here rather than failing every subsequent request.
 *
 * Returns whether a usable session remains. Any response other than a 401, such
 * as the server being down or a transient network error, leaves the credential
 * in place, since an unreachable server is not evidence of a bad token.
 */
export async function verifySession(): Promise<boolean> {
  if (!isSignedIn()) return false;
  try {
    const identity = asRecord(await api("/auth/me"));
    // An API-key session has no JWT to read a subject from, so this is the only
    // source of its user id.
    const userId = identity && typeof identity.user_id === "string" ? identity.user_id : null;
    if (userId) session.userId = userId;
    return true;
  } catch (error) {
    if ((error as ApiError).status === 401) {
      signOut();
      return false;
    }
    throw error;
  }
}

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

    // Select the sole organisation automatically; there is nothing to choose.
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

/**
 * Whether the caller may act as `role` in the active organisation.
 *
 * An `admin` satisfies every role the app defines, so a `role:billing` screen
 * opens for them without `billing` having been granted. The server applies the same
 * rule; this only decides what to *offer*.
 */
export function hasRole(role: string): boolean {
  return session.roles.includes(role) || session.roles.includes("admin");
}

/** The caller's primary role — the one worth printing next to their name. */
export function primaryRole(): string | null {
  return session.roles[0] ?? null;
}

/** Load every role the caller holds in the active organisation. */
export async function refreshRole() {
  session.roles = [];
  if (!session.organizationId || !session.userId) return;
  try {
    const members = asRecords(
      await api(`/membership?user_id=${encodeURIComponent(session.userId)}&limit=1`, { org: true }),
    );
    const mine = members[0];
    if (!mine) return;
    // The primary role first, then the additional grants, deduplicated — the
    // same order the server builds them in.
    const extra = asRecords(
      await api(`/membership_role?membership_id=${encodeURIComponent(String(mine.id ?? ""))}&limit=100`, {
        org: true,
      }),
    ).map((row) => String(row.role ?? ""));
    session.roles = [...new Set([String(mine.role ?? ""), ...extra])].filter(Boolean);
  } catch {
    // A member who cannot list memberships still gets to use the dashboard;
    // they simply see the actions their role allows, which is none of the
    // role-gated ones.
    session.roles = [];
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
  return canPermission(resource.permissions[action], resource.scope === "global");
}

function canPermission(policy: ActionPermissionManifest, global: boolean): boolean {
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
      return isSignedIn() && (global || Boolean(session.organizationId));
    default:
      return policy.role ? hasRole(policy.role) : false;
  }
}

/** Whether a resource belongs in this operator's navigation. */
export function isResourceVisible(resource: ResourceManifest): boolean {
  if (!resource.visible) return false;
  if (resource.roles.length && !resource.roles.some(hasRole)) return false;
  return can(resource, "list");
}

/** Whether an action belongs in this operator's action list. */
export function isFunctionVisible(fn: FunctionManifest): boolean {
  if (!fn.visible) return false;
  if (fn.roles.length && !fn.roles.some(hasRole)) return false;
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
      return fn.role ? hasRole(fn.role) : false;
  }
}

export function isAgentVisible(agent: AgentManifest): boolean {
  return canPermission(agent.chat, agent.scope === "global");
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

export function resourceByName(name: string | null | undefined): ResourceManifest | null {
  return manifest()?.resources.find((resource) => resource.name === name) ?? null;
}

export function functionByName(name: string | null | undefined): FunctionManifest | null {
  return manifest()?.functions.find((fn) => fn.name === name) ?? null;
}

export function agentByName(name: string | null | undefined): AgentManifest | null {
  return manifest()?.agents.find((agent) => agent.name === name) ?? null;
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

export function visibleAgents(): AgentManifest[] {
  return [...(manifest()?.agents ?? [])]
    .filter(isAgentVisible)
    .sort((left, right) => left.label.localeCompare(right.label) || left.name.localeCompare(right.name));
}
