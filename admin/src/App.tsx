import { For, Match, Show, Switch, createMemo, createSignal, onMount, type ParentProps } from "solid-js";
import { createMutable } from "solid-js/store";
import { Badge, Button, Card, CardHeader, EmptyState, HeadMark, Mono } from "./ui";
import { theme, toggleTheme } from "./theme";
import type {
  AdminManifest,
  AdminState,
  ApiRecord,
  FieldManifest,
  FunctionManifest,
  FunctionState,
  JsonValue,
  NoticeKind,
  ResourceManifest,
  ResourceState,
} from "./types";

const STORAGE_KEY = "apiplant-admin-session";
const MANIFEST_URL = "./apiplant-admin.json";

type ApiError = Error & { status?: number; payload?: unknown };
type BadgeTone = "neutral" | "accent" | "warn" | "danger" | "info";

const state = createMutable<AdminState>({
  manifest: null,
  page: { kind: "auth", name: null },
  notice: null,
  auth: {
    mode: "bearer",
    bearerToken: "",
    apiKey: "",
    userId: null,
    profile: null,
    organizations: [],
    selectedOrgId: "",
    role: null,
    refreshing: false,
  },
  forms: {
    loginIdentity: "",
    loginPassword: "",
    registerIdentity: "",
    registerPassword: "",
    registerExtra: "{}",
    manualBearerToken: "",
    manualApiKey: "",
    createOrgName: "",
    createOrgSlug: "",
    editOrgName: "",
    editOrgSlug: "",
    inviteIdentity: "",
    inviteUserId: "",
    inviteRole: "member",
  },
  organizations: {
    loadingMembers: false,
    membersError: null,
    members: [],
    memberRoleDrafts: {},
    inviteLookup: null,
    inviteLookupError: null,
    inviteLookupLoading: false,
  },
  resources: {},
  functions: {},
});

function humanize(value: string) {
  return value.replaceAll("_", " ").replace(/\b\w/g, (match) => match.toUpperCase());
}

function prettyJson(value: unknown) {
  return JSON.stringify(value, null, 2);
}

function compactValue(value: unknown) {
  if (value === null || value === undefined) return "—";
  if (typeof value === "object") {
    const text = JSON.stringify(value);
    return text.length > 80 ? `${text.slice(0, 77)}…` : text;
  }
  const text = String(value);
  return text.length > 80 ? `${text.slice(0, 77)}…` : text;
}

function safeJson(text: string) {
  try {
    return JSON.parse(text);
  } catch {
    return text;
  }
}

function asRecord(value: unknown): ApiRecord | null {
  return value && typeof value === "object" && !Array.isArray(value) ? (value as ApiRecord) : null;
}

function decodeJwtUserId(token: string) {
  try {
    const parts = token.split(".");
    if (parts.length !== 3) return null;
    const payload = JSON.parse(atob(parts[1].replaceAll("-", "+").replaceAll("_", "/").padEnd(Math.ceil(parts[1].length / 4) * 4, "=")));
    return typeof payload.sub === "string" ? payload.sub : null;
  } catch {
    return null;
  }
}

function defaultFieldValue(field: FieldManifest): JsonValue | boolean {
  if (field.default_value !== null) return field.default_value;
  if (field.type === "boolean") return false;
  return null;
}

function draftValueForField(field: FieldManifest, value: unknown): string | boolean {
  const resolved = value ?? defaultFieldValue(field);
  if (field.type === "boolean") return Boolean(resolved);
  if (resolved === null || resolved === undefined) return "";
  if (field.type === "json") return prettyJson(resolved);
  return String(resolved);
}

function visibleResourceFields(resource: ResourceManifest) {
  return resource.fields.filter((field) => !field.hidden);
}

function editableFields(resource: ResourceManifest) {
  return resource.fields.filter((field) => field.writable);
}

function filterableFields(resource: ResourceManifest) {
  return visibleResourceFields(resource).filter((field) => field.type !== "json");
}

function createFormDraft(resource: ResourceManifest, record: ApiRecord | null) {
  const draft: Record<string, string | boolean> = {};
  for (const field of editableFields(resource)) {
    draft[field.name] = draftValueForField(field, record?.[field.name]);
  }
  return draft;
}

function resourceByName(name: string | null) {
  return state.manifest?.resources.find((resource) => resource.name === name) ?? null;
}

function functionByName(name: string | null) {
  return state.manifest?.functions.find((fn) => fn.name === name) ?? null;
}

function currentOrganization() {
  return state.auth.organizations.find((organization) => String(organization.id ?? "") === state.auth.selectedOrgId) ?? null;
}

function createResourceState(resource: ResourceManifest): ResourceState {
  return {
    loading: false,
    saving: false,
    error: null,
    rows: [],
    selectedId: "",
    selectedRecord: null,
    formDraft: createFormDraft(resource, null),
    filterField: filterableFields(resource)[0]?.name ?? "",
    filterValue: "",
    limit: "50",
    offset: "0",
  };
}

function ensureResourceState(resource: ResourceManifest) {
  if (!state.resources[resource.name]) {
    state.resources[resource.name] = createResourceState(resource);
  }
  return state.resources[resource.name];
}

function createFunctionState(fn: FunctionManifest): FunctionState {
  return {
    input: fn.method === "GET" ? "" : "{}",
    loading: false,
    error: null,
    output: null,
  };
}

function ensureFunctionState(fn: FunctionManifest) {
  if (!state.functions[fn.name]) {
    state.functions[fn.name] = createFunctionState(fn);
  }
  return state.functions[fn.name];
}

function setNotice(kind: NoticeKind, message: string) {
  state.notice = { kind, message };
}

function clearNotice() {
  state.notice = null;
}

function readStoredAuth() {
  try {
    const raw = localStorage.getItem(STORAGE_KEY);
    if (!raw) return;
    const parsed = safeJson(raw);
    const record = asRecord(parsed);
    if (!record) return;
    state.auth.mode = record.mode === "apiKey" ? "apiKey" : "bearer";
    state.auth.bearerToken = typeof record.bearerToken === "string" ? record.bearerToken : "";
    state.auth.apiKey = typeof record.apiKey === "string" ? record.apiKey : "";
    state.auth.selectedOrgId = typeof record.selectedOrgId === "string" ? record.selectedOrgId : "";
    state.forms.manualBearerToken = state.auth.bearerToken;
    state.forms.manualApiKey = state.auth.apiKey;
    state.auth.userId = state.auth.mode === "bearer" ? decodeJwtUserId(state.auth.bearerToken) : null;
  } catch {
    localStorage.removeItem(STORAGE_KEY);
  }
}

function persistAuth() {
  localStorage.setItem(
    STORAGE_KEY,
    JSON.stringify({
      mode: state.auth.mode,
      bearerToken: state.auth.bearerToken,
      apiKey: state.auth.apiKey,
      selectedOrgId: state.auth.selectedOrgId,
    }),
  );
}

function isAuthenticated() {
  return Boolean(state.auth.bearerToken || state.auth.apiKey);
}

function authHeaders() {
  const headers: Record<string, string> = {};
  if (state.auth.mode === "apiKey" && state.auth.apiKey) headers["X-Api-Key"] = state.auth.apiKey;
  if (state.auth.bearerToken) headers.Authorization = `Bearer ${state.auth.bearerToken}`;
  return headers;
}

async function apiRequest(path: string, options: { method?: string; body?: unknown; headers?: HeadersInit; requiresOrg?: boolean } = {}) {
  if (!state.manifest) throw new Error("Manifest not loaded.");
  if (options.requiresOrg && !state.auth.selectedOrgId && state.auth.organizations.length > 1) {
    throw new Error("Choose an organization before continuing.");
  }

  const headers: Record<string, string> = {
    Accept: "application/json",
    ...authHeaders(),
    ...(options.headers as Record<string, string> | undefined),
  };
  if (options.body !== undefined) headers["Content-Type"] = "application/json";
  if (options.requiresOrg && state.auth.selectedOrgId) headers["X-Organization"] = state.auth.selectedOrgId;

  const response = await fetch(`${state.manifest.api_base_url}${path}`, {
    method: options.method ?? "GET",
    headers,
    body: options.body === undefined ? undefined : JSON.stringify(options.body),
  });

  if (response.status === 204) return null;
  const text = await response.text();
  const payload = text ? safeJson(text) : null;
  if (!response.ok) {
    const details = asRecord(payload);
    const error = new Error(typeof details?.error === "string" ? details.error : `Request failed with ${response.status}`) as ApiError;
    error.status = response.status;
    error.payload = payload;
    throw error;
  }
  return payload;
}

function parseJsonObject(text: string, label: string) {
  try {
    const parsed = JSON.parse(text || "{}");
    if (!parsed || typeof parsed !== "object" || Array.isArray(parsed)) {
      throw new Error(`${label} must be a JSON object.`);
    }
    return parsed as Record<string, unknown>;
  } catch (error) {
    throw new Error(`Invalid ${label}: ${error instanceof Error ? error.message : String(error)}`);
  }
}

function buildResourcePayload(resource: ResourceManifest, draft: Record<string, string | boolean>) {
  const payload: Record<string, unknown> = {};
  for (const field of editableFields(resource)) {
    const raw = draft[field.name];
    switch (field.type) {
      case "boolean":
        payload[field.name] = Boolean(raw);
        break;
      case "integer":
      case "big_int":
      case "float":
        if (raw === "") {
          payload[field.name] = field.required ? 0 : null;
          break;
        }
        if (typeof raw !== "string" || Number.isNaN(Number(raw))) {
          throw new Error(`${humanize(field.name)} must be a number.`);
        }
        payload[field.name] = Number(raw);
        break;
      case "json":
        if (typeof raw !== "string" || raw.trim() === "") {
          payload[field.name] = field.required ? {} : null;
          break;
        }
        payload[field.name] = JSON.parse(raw);
        break;
      case "reference":
      case "uuid":
      case "timestamp":
        payload[field.name] = raw === "" ? null : raw;
        break;
      default:
        payload[field.name] = raw;
        break;
    }
  }
  return payload;
}

function selectResourceRecord(resource: ResourceManifest, record: ApiRecord | null) {
  const entry = ensureResourceState(resource);
  entry.selectedRecord = record;
  entry.selectedId = record?.id ? String(record.id) : "";
  entry.formDraft = createFormDraft(resource, record);
}

function startNewRecord(resource: ResourceManifest) {
  const entry = ensureResourceState(resource);
  entry.selectedRecord = null;
  entry.selectedId = "";
  entry.formDraft = createFormDraft(resource, null);
}

async function openResourceRecord(resourceName: string, id: string) {
  const resource = resourceByName(resourceName);
  if (!resource) {
    setNotice("error", `Unknown related resource: ${resourceName}.`);
    return;
  }

  navigate("resource", resource.name);
  const entry = ensureResourceState(resource);
  const existing = entry.rows.find((row) => String(row.id ?? "") === id);
  if (existing) {
    selectResourceRecord(resource, existing);
    return;
  }

  entry.loading = true;
  entry.error = null;
  try {
    const record = asRecord(
      await apiRequest(`/${resource.name}/${encodeURIComponent(id)}`, {
        requiresOrg: resource.scope === "organization",
      }),
    );
    if (!record) {
      throw new Error(`Could not load ${humanize(resource.name).toLowerCase()} ${id}.`);
    }
    selectResourceRecord(resource, record);
    clearNotice();
  } catch (error) {
    entry.error = error instanceof Error ? error.message : String(error);
    setNotice("error", entry.error);
  } finally {
    entry.loading = false;
  }
}

function organizationNeedsSelection() {
  return state.auth.organizations.length > 1 && !state.auth.selectedOrgId;
}

function syncOrganizationForms() {
  const current = currentOrganization();
  state.forms.editOrgName = typeof current?.name === "string" ? current.name : "";
  state.forms.editOrgSlug = typeof current?.slug === "string" ? current.slug : "";
}

async function refreshSession() {
  state.auth.refreshing = true;
  try {
    state.auth.profile = null;
    state.auth.role = null;
    state.auth.organizations = [];
    state.organizations.members = [];
    state.organizations.memberRoleDrafts = {};

    if (!isAuthenticated()) return;

    const tasks: [Promise<unknown>, Promise<unknown>] = [
      apiRequest("/organization"),
      state.auth.mode === "bearer" && state.auth.userId
        ? apiRequest(`/user/${encodeURIComponent(state.auth.userId)}`).catch(() => null)
        : Promise.resolve(null),
    ];
    const [organizations, profile] = await Promise.all(tasks);
    state.auth.organizations = Array.isArray(organizations) ? (organizations as ApiRecord[]) : [];
    state.auth.profile = asRecord(profile);

    const onlyOrg = state.auth.organizations.length === 1 ? String(state.auth.organizations[0].id ?? "") : "";
    if (onlyOrg && !state.auth.selectedOrgId) state.auth.selectedOrgId = onlyOrg;
    if (
      state.auth.selectedOrgId &&
      !state.auth.organizations.some((organization) => String(organization.id ?? "") === state.auth.selectedOrgId)
    ) {
      state.auth.selectedOrgId = onlyOrg;
    }
    persistAuth();
    syncOrganizationForms();

    if (state.auth.selectedOrgId) {
      await loadOrganizationMembers(false);
    }
  } finally {
    state.auth.refreshing = false;
  }
}

async function loadOrganizationMembers(showNoticeOnError = true) {
  if (!state.auth.selectedOrgId) {
    state.organizations.members = [];
    state.organizations.memberRoleDrafts = {};
    state.auth.role = null;
    return;
  }

  state.organizations.loadingMembers = true;
  state.organizations.membersError = null;
  try {
    const rows = await apiRequest("/membership?limit=200&expand=user", { requiresOrg: true });
    const members = Array.isArray(rows) ? (rows as ApiRecord[]) : [];
    state.organizations.members = members;
    state.organizations.memberRoleDrafts = Object.fromEntries(
      members.map((member) => [String(member.id ?? ""), typeof member.role === "string" ? member.role : "member"]),
    );
    if (state.auth.userId) {
      const mine = members.find((member) => String(member.user_id ?? "") === state.auth.userId);
      state.auth.role = typeof mine?.role === "string" ? mine.role : null;
    }
  } catch (error) {
    state.organizations.members = [];
    state.organizations.memberRoleDrafts = {};
    state.organizations.membersError = error instanceof Error ? error.message : String(error);
    if (showNoticeOnError) setNotice("error", state.organizations.membersError);
  } finally {
    state.organizations.loadingMembers = false;
  }
}

async function setActiveOrganization(orgId: string) {
  state.auth.selectedOrgId = orgId;
  persistAuth();
  syncOrganizationForms();
  await loadOrganizationMembers(false);

  const current = state.page.kind === "resource" ? resourceByName(state.page.name) : null;
  if (current?.scope === "organization") {
    await loadResourceCollection(current, false);
  }
}

async function login() {
  if (!state.manifest) return;
  try {
    clearNotice();
    const payload = {
      [state.manifest.auth.identity_field]: state.forms.loginIdentity.trim(),
      password: state.forms.loginPassword,
    };
    const response = asRecord(await apiRequest("/auth/login", { method: "POST", body: payload }));
    const token = typeof response?.token === "string" ? response.token : "";
    state.auth.mode = "bearer";
    state.auth.bearerToken = token;
    state.auth.apiKey = "";
    state.auth.userId = decodeJwtUserId(token);
    state.forms.loginPassword = "";
    state.forms.manualBearerToken = token;
    persistAuth();
    await refreshSession();
    state.page = { kind: "dashboard", name: null };
    setNotice("success", "Welcome back.");
  } catch (error) {
    setNotice("error", error instanceof Error ? error.message : String(error));
  }
}

async function register() {
  if (!state.manifest) return;
  try {
    clearNotice();
    const extra = parseJsonObject(state.forms.registerExtra, "registration details");
    const payload = {
      ...extra,
      [state.manifest.auth.identity_field]: state.forms.registerIdentity.trim(),
      password: state.forms.registerPassword,
    };
    const response = asRecord(await apiRequest("/auth/register", { method: "POST", body: payload }));
    const token = typeof response?.token === "string" ? response.token : "";
    state.auth.mode = "bearer";
    state.auth.bearerToken = token;
    state.auth.apiKey = "";
    state.auth.userId = decodeJwtUserId(token);
    state.forms.registerPassword = "";
    state.forms.registerExtra = "{}";
    state.forms.manualBearerToken = token;
    persistAuth();
    await refreshSession();
    state.page = { kind: "dashboard", name: null };
    setNotice("success", "Account created.");
  } catch (error) {
    setNotice("error", error instanceof Error ? error.message : String(error));
  }
}

function signOut() {
  state.auth.mode = "bearer";
  state.auth.bearerToken = "";
  state.auth.apiKey = "";
  state.auth.userId = null;
  state.auth.profile = null;
  state.auth.organizations = [];
  state.auth.selectedOrgId = "";
  state.auth.role = null;
  state.forms.manualBearerToken = "";
  state.forms.manualApiKey = "";
  state.page = { kind: "auth", name: null };
  persistAuth();
}

async function createOrganization() {
  try {
    clearNotice();
    const payload: Record<string, unknown> = { name: state.forms.createOrgName.trim() };
    if (state.forms.createOrgSlug.trim()) payload.slug = state.forms.createOrgSlug.trim();
    const organization = asRecord(await apiRequest("/organization", { method: "POST", body: payload }));
    state.forms.createOrgName = "";
    state.forms.createOrgSlug = "";
    state.auth.selectedOrgId = String(organization?.id ?? "");
    persistAuth();
    await refreshSession();
    setNotice("success", "Organization created.");
  } catch (error) {
    setNotice("error", error instanceof Error ? error.message : String(error));
  }
}

async function updateCurrentOrganization() {
  const organization = currentOrganization();
  if (!organization) {
    setNotice("error", "Choose an organization first.");
    return;
  }
  try {
    clearNotice();
    await apiRequest(`/organization/${encodeURIComponent(String(organization.id ?? ""))}`, {
      method: "PATCH",
      body: { name: state.forms.editOrgName.trim(), slug: state.forms.editOrgSlug.trim() || null },
    });
    await refreshSession();
    setNotice("success", "Organization details updated.");
  } catch (error) {
    setNotice("error", error instanceof Error ? error.message : String(error));
  }
}

async function lookupInvitee() {
  if (!state.manifest) return;
  const directUserId = state.forms.inviteUserId.trim();
  const identity = state.forms.inviteIdentity.trim();
  state.organizations.inviteLookup = null;
  state.organizations.inviteLookupError = null;
  state.organizations.inviteLookupLoading = true;

  try {
    if (directUserId) {
      state.organizations.inviteLookup = { id: directUserId, label: directUserId };
      return;
    }
    if (!identity) throw new Error(`Enter a ${state.manifest.auth.identity_field}, username, or user id first.`);
    const rows = await apiRequest(
      `/user?${encodeURIComponent(state.manifest.auth.identity_field)}=${encodeURIComponent(identity)}&limit=1`,
    );
    const found = Array.isArray(rows) ? asRecord(rows[0]) : null;
    if (!found) throw new Error("No matching user was found.");
    const label =
      (typeof found[state.manifest.auth.identity_field] === "string" && String(found[state.manifest.auth.identity_field])) ||
      (typeof found.display_name === "string" && found.display_name) ||
      String(found.email ?? found.id ?? "User");
    state.organizations.inviteLookup = { id: String(found.id ?? ""), label };
  } catch (error) {
    state.organizations.inviteLookupError = error instanceof Error ? error.message : String(error);
  } finally {
    state.organizations.inviteLookupLoading = false;
  }
}

async function inviteMember() {
  if (!state.auth.selectedOrgId) {
    setNotice("error", "Choose an organization before inviting anyone.");
    return;
  }
  try {
    clearNotice();
    const lookup = state.organizations.inviteLookup;
    const userId = state.forms.inviteUserId.trim() || lookup?.id;
    if (!userId) throw new Error("Look up a user or paste their user id first.");
    await apiRequest("/membership", {
      method: "POST",
      body: { user_id: userId, role: state.forms.inviteRole || "member" },
      requiresOrg: true,
    });
    state.forms.inviteIdentity = "";
    state.forms.inviteUserId = "";
    state.forms.inviteRole = "member";
    state.organizations.inviteLookup = null;
    state.organizations.inviteLookupError = null;
    await loadOrganizationMembers(false);
    setNotice("success", "Member added to the current organization.");
  } catch (error) {
    setNotice("error", error instanceof Error ? error.message : String(error));
  }
}

async function saveMembershipRole(membershipId: string) {
  const role = state.organizations.memberRoleDrafts[membershipId];
  if (!role) {
    setNotice("error", "Choose a role first.");
    return;
  }
  try {
    clearNotice();
    await apiRequest(`/membership/${encodeURIComponent(membershipId)}`, {
      method: "PATCH",
      body: { role },
      requiresOrg: true,
    });
    await loadOrganizationMembers(false);
    setNotice("success", "Member role updated.");
  } catch (error) {
    setNotice("error", error instanceof Error ? error.message : String(error));
  }
}

async function removeMembership(membershipId: string) {
  if (!window.confirm("Remove this person from the current organization?")) return;
  try {
    clearNotice();
    await apiRequest(`/membership/${encodeURIComponent(membershipId)}`, {
      method: "DELETE",
      requiresOrg: true,
    });
    await loadOrganizationMembers(false);
    setNotice("success", "Member removed.");
  } catch (error) {
    setNotice("error", error instanceof Error ? error.message : String(error));
  }
}

async function loadResourceCollection(resource: ResourceManifest, announce = true) {
  const entry = ensureResourceState(resource);
  entry.loading = true;
  entry.error = null;
  try {
    const params = new URLSearchParams();
    params.set("limit", String(Number(entry.limit || 50) || 50));
    params.set("offset", String(Number(entry.offset || 0) || 0));
    if (entry.filterField && entry.filterValue.trim() !== "") params.set(entry.filterField, entry.filterValue.trim());
    if (resource.relations.length) params.set("expand", resource.relations.map((relation) => relation.relation).join(","));
    const rows = await apiRequest(`/${resource.name}?${params.toString()}`, {
      requiresOrg: resource.scope === "organization",
    });
    entry.rows = Array.isArray(rows) ? (rows as ApiRecord[]) : [];
    const selected = entry.selectedId ? entry.rows.find((row) => String(row.id ?? "") === entry.selectedId) ?? null : null;
    if (selected) {
      selectResourceRecord(resource, selected);
    } else if (entry.selectedId) {
      startNewRecord(resource);
    }
    if (announce) {
      if (!entry.rows.length) {
        setNotice("success", `No ${humanize(resource.name).toLowerCase()} records matched those filters.`);
      } else {
        clearNotice();
      }
    }
  } catch (error) {
    entry.error = error instanceof Error ? error.message : String(error);
    if (announce) setNotice("error", entry.error);
  } finally {
    entry.loading = false;
  }
}

async function saveResourceRecord(resource: ResourceManifest, mode: "create" | "update") {
  const entry = ensureResourceState(resource);
  try {
    clearNotice();
    entry.saving = true;
    const payload = buildResourcePayload(resource, entry.formDraft);
    const response = asRecord(
      await apiRequest(
        mode === "create" ? `/${resource.name}` : `/${resource.name}/${encodeURIComponent(entry.selectedId)}`,
        {
          method: mode === "create" ? "POST" : "PATCH",
          body: payload,
          requiresOrg: resource.scope === "organization",
        },
      ),
    );
    selectResourceRecord(resource, response);
    setNotice("success", mode === "create" ? "Record created." : "Record updated.");
    await loadResourceCollection(resource, false);
  } catch (error) {
    setNotice("error", error instanceof Error ? error.message : String(error));
  } finally {
    entry.saving = false;
  }
}

async function deleteResourceRecord(resource: ResourceManifest) {
  const entry = ensureResourceState(resource);
  if (!entry.selectedId) {
    setNotice("error", "Pick a record before deleting.");
    return;
  }
  if (!window.confirm("Delete this record?")) return;
  try {
    clearNotice();
    entry.saving = true;
    await apiRequest(`/${resource.name}/${encodeURIComponent(entry.selectedId)}`, {
      method: "DELETE",
      requiresOrg: resource.scope === "organization",
    });
    startNewRecord(resource);
    await loadResourceCollection(resource, false);
    setNotice("success", "Record deleted.");
  } catch (error) {
    setNotice("error", error instanceof Error ? error.message : String(error));
  } finally {
    entry.saving = false;
  }
}

async function invokeFunction(fn: FunctionManifest) {
  const entry = ensureFunctionState(fn);
  entry.loading = true;
  entry.error = null;
  entry.output = null;
  try {
    const body = fn.method === "GET" ? undefined : parseJsonObject(entry.input || "{}", `${fn.name} input`);
    entry.output = await apiRequest(`/functions/${fn.name}`, {
      method: fn.method,
      body,
      requiresOrg: fn.visibility === "role",
    });
    clearNotice();
  } catch (error) {
    entry.error = error instanceof Error ? error.message : String(error);
    setNotice("error", entry.error);
  } finally {
    entry.loading = false;
  }
}

function navigate(kind: AdminState["page"]["kind"], name: string | null = null) {
  state.page = { kind, name };
  clearNotice();
  if (kind === "resource" && name) {
    const resource = resourceByName(name);
    if (resource) {
      const entry = ensureResourceState(resource);
      if (!entry.rows.length && !(resource.scope === "organization" && !state.auth.selectedOrgId)) {
        void loadResourceCollection(resource, false);
      }
    }
  }
  if (kind === "organization" && state.auth.selectedOrgId && !state.organizations.members.length) {
    void loadOrganizationMembers(false);
  }
}

function currentUserLabel() {
  if (state.auth.profile && state.manifest) {
    const identity = state.auth.profile[state.manifest.auth.identity_field];
    return typeof identity === "string"
      ? identity
      : typeof state.auth.profile.display_name === "string"
        ? state.auth.profile.display_name
        : String(state.auth.profile.id ?? "Signed in");
  }
  return state.auth.mode === "apiKey" ? "API key session" : "Signed in";
}

function noticeClasses(kind: NoticeKind) {
  switch (kind) {
    case "success":
      return "border-accent-line bg-accent-soft text-accent";
    case "error":
      return "border-danger-line bg-danger-soft text-danger";
    case "warn":
      return "border-warn-line bg-warn-soft text-warn";
    default:
      return "border-line bg-surface text-muted";
  }
}

function AdminThemeToggle() {
  return (
    <button
      type="button"
      onClick={toggleTheme}
      title={theme() === "dark" ? "Switch to light" : "Switch to dark"}
      aria-label={theme() === "dark" ? "Switch to light theme" : "Switch to dark theme"}
      class="rounded-lg p-1.5 text-faint transition-colors hover:bg-surface-2 hover:text-ink"
    >
      <Show
        when={theme() === "dark"}
        fallback={
          <svg width="16" height="16" viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.4">
            <path d="M13.2 9.6A5.4 5.4 0 0 1 6.4 2.8a5.6 5.6 0 1 0 6.8 6.8z" stroke-linejoin="round" />
          </svg>
        }
      >
        <svg width="16" height="16" viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.4">
          <circle cx="8" cy="8" r="3.1" />
          <path
            d="M8 1.4v1.5M8 13.1v1.5M14.6 8h-1.5M2.9 8H1.4M12.67 3.33l-1.06 1.06M4.39 11.61l-1.06 1.06M12.67 12.67l-1.06-1.06M4.39 4.39L3.33 3.33"
            stroke-linecap="round"
          />
        </svg>
      </Show>
    </button>
  );
}

function Notice(props: { kind: NoticeKind; message: string }) {
  return (
    <div class={`rounded-xl border px-3 py-2 text-sm ${noticeClasses(props.kind)}`}>
      {props.message}
    </div>
  );
}

function PageHeader(props: { title: string; copy: string; badges?: { tone?: BadgeTone; label: string }[]; eyebrow?: string }) {
  return (
    <div class="mb-5">
      <Show when={props.eyebrow}>
        <p class="mb-2 text-[0.6875rem] font-semibold uppercase tracking-[0.18em] text-faint">{props.eyebrow}</p>
      </Show>
      <h1 class="text-2xl font-semibold tracking-tight text-ink">{props.title}</h1>
      <p class="mt-2 max-w-4xl text-sm leading-7 text-muted">{props.copy}</p>
      <Show when={props.badges?.length}>
        <div class="mt-3 flex flex-wrap gap-2">
          <For each={props.badges}>{(badge) => <Badge tone={badge.tone}>{badge.label}</Badge>}</For>
        </div>
      </Show>
    </div>
  );
}

function MetricCard(props: { label: string; value: string; hint: string; tone?: BadgeTone }) {
  return (
    <Card class="border-line-strong/70 bg-surface/85">
      <div class="space-y-2 px-4 py-4">
        <Badge tone={props.tone}>{props.label}</Badge>
        <p class="text-2xl font-semibold tracking-tight text-ink">{props.value}</p>
        <p class="text-xs leading-6 text-muted">{props.hint}</p>
      </div>
    </Card>
  );
}

function FormField(props: ParentProps<{ label: string; hint?: string }>) {
  return (
    <label class="block">
      <span class="field-label">{props.label}</span>
      {props.children}
      <Show when={props.hint}>
        <p class="mt-1 text-[0.6875rem] leading-relaxed text-faint">{props.hint}</p>
      </Show>
    </label>
  );
}

function ResourceFieldEditor(props: { resource: ResourceManifest; field: FieldManifest; entry: ResourceState }) {
  const value = () => props.entry.formDraft[props.field.name];
  const relatedId = createMemo(() => {
    const current = value();
    return typeof current === "string" ? current.trim() : "";
  });
  if (props.field.type === "text" || props.field.type === "json") {
    return (
      <FormField
        label={humanize(props.field.name)}
        hint={
          props.field.type === "json"
            ? "JSON object or array"
            : props.field.references
              ? `References ${props.field.references}`
              : undefined
        }
      >
        <textarea
          class={`input min-h-36 ${props.field.type === "json" ? "font-mono text-[0.78125rem]" : ""}`}
          value={String(typeof value() === "string" ? value() : "")}
          onInput={(event) => {
            props.entry.formDraft[props.field.name] = event.currentTarget.value;
          }}
        />
      </FormField>
    );
  }

  if (props.field.type === "boolean") {
    return (
      <label class="flex items-start gap-3 rounded-xl border border-line bg-surface-2/60 px-3 py-3">
        <input
          type="checkbox"
          class="mt-1 h-4 w-4 accent-current"
          checked={Boolean(value())}
          onChange={(event) => {
            props.entry.formDraft[props.field.name] = event.currentTarget.checked;
          }}
        />
        <span class="space-y-1">
          <span class="block text-sm font-medium text-ink">{humanize(props.field.name)}</span>
          <span class="block text-xs text-faint">{props.field.required ? "Required" : "Optional"}</span>
        </span>
      </label>
    );
  }

  const inputType =
    props.field.type === "integer" || props.field.type === "big_int" || props.field.type === "float" ? "number" : "text";

  return (
    <FormField
      label={humanize(props.field.name)}
      hint={
        props.field.references
          ? `References ${props.field.references}`
          : props.field.type === "timestamp"
            ? "Use an RFC 3339 timestamp"
            : props.field.required
              ? "Required"
              : "Optional"
      }
    >
      <input
        type={inputType}
        class={`input ${props.field.type === "uuid" || props.field.type === "reference" ? "font-mono text-[0.78125rem]" : ""}`}
        value={String(typeof value() === "string" ? value() : "")}
        onInput={(event) => {
          props.entry.formDraft[props.field.name] = event.currentTarget.value;
        }}
      />
      <Show when={props.field.type === "reference" && props.field.references && relatedId()}>
        <div class="mt-2">
          <Button
            size="sm"
            variant="ghost"
            onClick={() => void openResourceRecord(props.field.references!, relatedId())}
          >
            Open related {humanize(props.field.references!)}
          </Button>
        </div>
      </Show>
    </FormField>
  );
}

function AuthPage() {
  const manifest = () => state.manifest!;
  const [showRegister, setShowRegister] = createSignal(false);

  return (
    <div class="relative z-10 flex min-h-screen flex-col">
      <header class="flex items-center justify-between px-4 pt-4 sm:px-6">
        <div class="flex items-center gap-2">
          <HeadMark class="h-7 text-accent" />
          <span class="text-sm font-semibold tracking-tight">
            apiplant <span class="text-accent">admin</span>
          </span>
        </div>
        <AdminThemeToggle />
      </header>

      <main class="flex flex-1 items-center justify-center p-4 sm:p-6">
        <section class="w-full max-w-md">
          <Card class="border-line-strong/70">
            <CardHeader title="Sign in" hint={`Use your ${manifest().auth.identity_field} and password to open the admin panel.`} />
            <div class="space-y-4 px-5 py-5">
              <div class="space-y-1">
                <Badge tone="accent">{manifest().app_name}</Badge>
                <p class="text-sm leading-6 text-muted">This panel talks directly to <Mono>{manifest().api_base_url}</Mono>.</p>
              </div>
              <FormField label={manifest().auth.identity_field}>
                <input
                  class="input"
                  value={state.forms.loginIdentity}
                  onInput={(event) => {
                    state.forms.loginIdentity = event.currentTarget.value;
                  }}
                />
              </FormField>
              <FormField label="Password">
                <input
                  type="password"
                  class="input"
                  value={state.forms.loginPassword}
                  onInput={(event) => {
                    state.forms.loginPassword = event.currentTarget.value;
                  }}
                />
              </FormField>
              <div class="flex flex-wrap gap-2">
                <Button variant="primary" onClick={() => void login()}>
                  Sign in
                </Button>
              </div>
              <Show when={manifest().auth.allow_registration}>
                <button
                  type="button"
                  class="text-xs font-semibold text-accent hover:underline"
                  onClick={() => setShowRegister((value) => !value)}
                >
                  {showRegister() ? "Hide registration" : "Need an account? Register"}
                </button>
              </Show>
              <Show when={!manifest().auth.allow_registration}>
                <p class="text-xs leading-6 text-muted">Registration is disabled for this app. Ask an administrator to add your account.</p>
              </Show>
            </div>
          </Card>

          <Show when={showRegister() && manifest().auth.allow_registration}>
            <Card class="mt-4 border-line-strong/70">
              <CardHeader title="Register" hint="Create an account, then continue into the admin panel." />
              <div class="space-y-4 px-5 py-5">
                <FormField label={manifest().auth.identity_field}>
                  <input
                    class="input"
                    value={state.forms.registerIdentity}
                    onInput={(event) => {
                      state.forms.registerIdentity = event.currentTarget.value;
                    }}
                  />
                </FormField>
                <FormField label="Password">
                  <input
                    type="password"
                    class="input"
                    value={state.forms.registerPassword}
                    onInput={(event) => {
                      state.forms.registerPassword = event.currentTarget.value;
                    }}
                  />
                </FormField>
                <FormField label="Additional profile fields (JSON)">
                  <textarea
                    class="input min-h-32 font-mono text-[0.78125rem]"
                    value={state.forms.registerExtra}
                    onInput={(event) => {
                      state.forms.registerExtra = event.currentTarget.value;
                    }}
                  />
                </FormField>
                <div class="flex flex-wrap gap-2">
                  <Button variant="primary" onClick={() => void register()}>
                    Create account
                  </Button>
                </div>
              </div>
            </Card>
          </Show>
        </section>
      </main>
    </div>
  );
}

function Sidebar() {
  const resources = createMemo(() => state.manifest?.resources ?? []);

  const navClass = (active: boolean) =>
    [
      "flex w-full items-center gap-3 rounded-xl px-3 py-2.5 text-left text-sm transition-colors",
      active ? "bg-accent-soft/70 text-ink ring-1 ring-accent-line" : "text-muted hover:bg-surface-2 hover:text-ink",
    ].join(" ");

  return (
    <aside class="w-80 shrink-0 border-r border-line bg-surface/45">
      <div class="h-full overflow-y-auto px-3 py-4">
        <Card class="mb-5 border-line-strong/70 bg-surface/80">
          <div class="space-y-4 px-4 py-4">
            <div>
              <p class="text-[0.6875rem] font-semibold uppercase tracking-[0.16em] text-faint">Workspace</p>
              <p class="mt-2 text-sm font-semibold text-ink">{state.manifest?.app_name ?? "apiplant app"}</p>
              <p class="mt-1 text-xs leading-6 text-muted">{currentUserLabel()}</p>
            </div>
            <Show when={state.auth.organizations.length}>
              <FormField label="Current organization">
                <select class="input" value={state.auth.selectedOrgId} onChange={(event) => void setActiveOrganization(event.currentTarget.value)}>
                  <option value="">{state.auth.organizations.length > 1 ? "Choose an organization" : "No organization selected"}</option>
                  <For each={state.auth.organizations}>
                    {(organization) => {
                      const id = String(organization.id ?? "");
                      const label = String(organization.name ?? organization.slug ?? id);
                      return <option value={id}>{label}</option>;
                    }}
                  </For>
                </select>
              </FormField>
            </Show>
            <div class="flex flex-wrap gap-2">
              <Button size="sm" onClick={() => navigate("organization")}>
                Org tools
              </Button>
              <Show when={state.manifest?.docs_url}>
                {(docsUrl) => (
                  <a class="inline-flex" href={docsUrl()} target="_blank" rel="noreferrer">
                    <Button size="sm" variant="ghost">
                      API docs
                    </Button>
                  </a>
                )}
              </Show>
            </div>
          </div>
        </Card>

        <div class="space-y-1">
          <button type="button" class={navClass(state.page.kind === "dashboard")} onClick={() => navigate("dashboard")}>
            <span class="font-semibold">Dashboard</span>
          </button>
          <button type="button" class={navClass(state.page.kind === "organization")} onClick={() => navigate("organization")}>
            <span class="font-semibold">Organizations & members</span>
          </button>
        </div>

        <div class="mt-5">
          <p class="px-3 pb-2 text-[0.6875rem] font-semibold uppercase tracking-[0.08em] text-faint">Resources</p>
          <div class="space-y-1">
            <Show
              when={resources().length}
              fallback={<p class="px-3 py-2 text-xs text-faint">No resources are available.</p>}
            >
              <For each={resources()}>
                {(resource) => (
                  <button
                    type="button"
                    class={navClass(state.page.kind === "resource" && state.page.name === resource.name)}
                    onClick={() => navigate("resource", resource.name)}
                  >
                    <span>{humanize(resource.name)}</span>
                    <span class="ml-auto text-[0.6875rem] text-faint">{resource.scope === "organization" ? "Org" : "Global"}</span>
                  </button>
                )}
              </For>
            </Show>
          </div>
        </div>

        <div class="mt-5">
          <p class="px-3 pb-2 text-[0.6875rem] font-semibold uppercase tracking-[0.08em] text-faint">Actions</p>
          <div class="space-y-1">
            <Show
              when={(state.manifest?.functions.length ?? 0) > 0}
              fallback={<p class="px-3 py-2 text-xs text-faint">No manual actions are available in this app.</p>}
            >
              <For each={state.manifest?.functions ?? []}>
                {(fn) => (
                  <button
                    type="button"
                    class={navClass(state.page.kind === "function" && state.page.name === fn.name)}
                    onClick={() => navigate("function", fn.name)}
                  >
                    <span>{humanize(fn.name)}</span>
                    <span class="ml-auto text-[0.6875rem] text-faint">{fn.method}</span>
                  </button>
                )}
              </For>
            </Show>
          </div>
        </div>
      </div>
    </aside>
  );
}

function DashboardPage() {
  const sections = createMemo(() => {
    const resources = state.manifest?.resources ?? [];
    return {
      content: resources.filter((resource) => !resource.builtin),
      spotlight: resources.filter((resource) => !resource.builtin).slice(0, 5),
      actions: (state.manifest?.functions ?? []).slice(0, 5),
    };
  });
  const org = createMemo(() => currentOrganization());

  return (
    <>
      <PageHeader
        eyebrow="Operator overview"
        title="Dashboard"
        copy="Work from one organization at a time, keep members up to date, and manage the content your application already exposes."
        badges={[
          { tone: "accent", label: currentUserLabel() },
          { label: `${state.auth.organizations.length} organization${state.auth.organizations.length === 1 ? "" : "s"}` },
          {
            label: sections().content.length
              ? `${sections().content.length} content type${sections().content.length === 1 ? "" : "s"}`
              : "built-ins only",
          },
          { label: `${state.manifest?.functions.length ?? 0} actions` },
          ...(state.auth.role ? [{ tone: "accent" as const, label: `Current role: ${state.auth.role}` }] : []),
        ]}
      />

      <div class="space-y-3">
        <Show when={organizationNeedsSelection()}>
          <Notice kind="warn" message="Choose an organization before opening organization-scoped content or role-gated actions." />
        </Show>
        <Show when={!state.auth.organizations.length}>
          <Notice
            kind="warn"
            message="You are signed in, but you do not belong to any organizations yet. Create one below or wait for an invitation."
          />
        </Show>
      </div>

      <Card class="mt-4 overflow-hidden border-line-strong/70 bg-[linear-gradient(135deg,color-mix(in_srgb,var(--color-accent)_12%,transparent),transparent_45%),linear-gradient(180deg,var(--color-surface),color-mix(in_srgb,var(--color-surface)_88%,transparent))]">
        <div class="grid gap-5 px-5 py-5 xl:grid-cols-[1.15fr_0.85fr] xl:px-6 xl:py-6">
          <div class="space-y-4">
            <div class="flex flex-wrap items-center gap-2">
              <Badge tone="accent">{state.manifest?.app_name ?? "apiplant app"}</Badge>
              <Badge>{org() ? "Organization selected" : "No active organization"}</Badge>
            </div>
            <h2 class="text-xl font-semibold tracking-tight text-ink">Quick actions</h2>
            <div class="flex flex-wrap gap-2">
              <Button variant="primary" onClick={() => navigate("organization")}>
                Open organization tools
              </Button>
              <Button variant="ghost" onClick={() => void refreshSession()}>
                Refresh session
              </Button>
            </div>
          </div>
          <div class="grid gap-3 sm:grid-cols-2">
            <MetricCard
              label="Workspace"
              value={org() ? String(org()?.name ?? org()?.slug ?? org()?.id ?? "") : "Not selected"}
              hint="Global resources still work without a selected organization."
              tone="accent"
            />
            <MetricCard
              label="API base"
              value={state.manifest?.api_base_url.replace(/^https?:\/\//, "") ?? "—"}
              hint="Every request from this panel talks directly to your deployed API."
            />
            <MetricCard label="Resources" value={String(sections().content.length)} hint="Custom content types available in this application." />
            <MetricCard label="Actions" value={String(state.manifest?.functions.length ?? 0)} hint="Manually callable functions exposed to operators." />
          </div>
        </div>
      </Card>

      <div class="mt-4 grid gap-4 xl:grid-cols-3">
        <Card>
          <CardHeader title="Current workspace" hint="Switch when you belong to more than one organization." />
          <div class="space-y-4 px-4 py-4">
            <FormField label="Organization">
              <select class="input" value={state.auth.selectedOrgId} onChange={(event) => void setActiveOrganization(event.currentTarget.value)}>
                <option value="">{state.auth.organizations.length > 1 ? "Choose an organization" : "No organization selected"}</option>
                <For each={state.auth.organizations}>
                  {(organization) => {
                    const id = String(organization.id ?? "");
                    const label = String(organization.name ?? organization.slug ?? id);
                    return <option value={id}>{label}</option>;
                  }}
                </For>
              </select>
            </FormField>
            <Notice
              kind="info"
              message={
                org()
                  ? `Active now: ${String(org()?.name ?? org()?.slug ?? org()?.id ?? "")}`
                  : "Pick the organization you want to work in. Global resources still work without one."
              }
            />
            <div class="flex flex-wrap gap-2">
              <Button onClick={() => navigate("organization")}>Open organization tools</Button>
              <Button variant="ghost" onClick={() => void refreshSession()}>
                Refresh
              </Button>
            </div>
          </div>
        </Card>

        <Card>
          <CardHeader title="Content" hint="Jump straight into the records people care about." />
          <div class="space-y-2 px-4 py-4">
            <Show when={sections().spotlight.length} fallback={<p class="text-xs text-faint">No custom content resources are configured yet.</p>}>
              <For each={sections().spotlight}>
                {(resource) => (
                  <button
                    type="button"
                    class="flex w-full items-center gap-3 rounded-xl border border-line bg-surface-2/40 px-3 py-2 text-left text-sm text-muted transition-colors hover:border-line-strong hover:text-ink"
                    onClick={() => navigate("resource", resource.name)}
                  >
                    <span class="font-medium text-ink">{humanize(resource.name)}</span>
                    <span class="ml-auto text-[0.6875rem] text-faint">
                      {resource.scope === "organization" ? "Org content" : "Shared content"}
                    </span>
                  </button>
                )}
              </For>
            </Show>
          </div>
        </Card>

        <Card>
          <CardHeader title="Actions" hint="Manual one-off operations exposed by the application." />
          <div class="space-y-2 px-4 py-4">
            <Show when={sections().actions.length} fallback={<p class="text-xs text-faint">No administrator-facing actions are available.</p>}>
              <For each={sections().actions}>
                {(fn) => (
                  <button
                    type="button"
                    class="flex w-full items-center gap-3 rounded-xl border border-line bg-surface-2/40 px-3 py-2 text-left text-sm text-muted transition-colors hover:border-line-strong hover:text-ink"
                    onClick={() => navigate("function", fn.name)}
                  >
                    <span class="font-medium text-ink">{humanize(fn.name)}</span>
                    <span class="ml-auto text-[0.6875rem] text-faint">{fn.method}</span>
                  </button>
                )}
              </For>
            </Show>
          </div>
        </Card>
      </div>
    </>
  );
}

function OrganizationPage() {
  const currentOrg = createMemo(() => currentOrganization());
  return (
    <>
      <PageHeader
        eyebrow="People and tenancy"
        title="Organizations & members"
        copy="Switch between organizations, keep your current workspace details up to date, and manage who belongs to it."
      />

      <div class="grid gap-4 xl:grid-cols-4">
        <MetricCard label="Organizations" value={String(state.auth.organizations.length)} hint="Organizations you can operate inside right now." tone="accent" />
        <MetricCard label="Members" value={state.auth.selectedOrgId ? String(state.organizations.members.length) : "—"} hint="People visible in the active workspace." />
        <MetricCard label="Role" value={state.auth.role ?? "Unassigned"} hint="Your role inside the current organization." />
        <MetricCard label="Current org" value={currentOrg() ? String(currentOrg()?.name ?? currentOrg()?.slug ?? currentOrg()?.id ?? "") : "None"} hint="The organization that receives org-scoped requests." />
      </div>

      <div class="mt-4 grid gap-4 xl:grid-cols-2">
        <Card>
          <CardHeader title="Your organizations" hint="If someone adds you to another organization, it appears here after you refresh." />
          <div class="space-y-4 px-4 py-4">
            <FormField label="Active organization">
              <select class="input" value={state.auth.selectedOrgId} onChange={(event) => void setActiveOrganization(event.currentTarget.value)}>
                <option value="">{state.auth.organizations.length > 1 ? "Choose an organization" : "No organization selected"}</option>
                <For each={state.auth.organizations}>
                  {(organization) => {
                    const id = String(organization.id ?? "");
                    const label = String(organization.name ?? organization.slug ?? id);
                    return <option value={id}>{label}</option>;
                  }}
                </For>
              </select>
            </FormField>

            <div class="space-y-2">
              <Show when={state.auth.organizations.length} fallback={<p class="text-xs text-faint">No organizations yet. Create one below to start working.</p>}>
                <For each={state.auth.organizations}>
                  {(organization) => {
                    const id = String(organization.id ?? "");
                    return (
                      <button
                        type="button"
                        class={[
                          "flex w-full flex-col items-start rounded-xl border px-3 py-3 text-left transition-colors",
                          state.auth.selectedOrgId === id
                            ? "border-accent-line bg-accent-soft/40"
                            : "border-line bg-surface-2/30 hover:border-line-strong",
                        ].join(" ")}
                        onClick={() => void setActiveOrganization(id)}
                      >
                        <span class="text-sm font-medium text-ink">{String(organization.name ?? organization.slug ?? id)}</span>
                        <span class="mt-1 text-xs text-faint">{String(organization.slug ?? id)}</span>
                      </button>
                    );
                  }}
                </For>
              </Show>
            </div>

            <div class="flex flex-wrap gap-2">
              <Button onClick={() => void refreshSession()}>Refresh organizations</Button>
            </div>
          </div>
        </Card>

        <Card>
          <CardHeader title="Create an organization" hint="The creator becomes an admin automatically." />
          <div class="space-y-4 px-4 py-4">
            <FormField label="Name">
              <input
                class="input"
                value={state.forms.createOrgName}
                onInput={(event) => {
                  state.forms.createOrgName = event.currentTarget.value;
                }}
              />
            </FormField>
            <FormField label="Slug">
              <input
                class="input"
                value={state.forms.createOrgSlug}
                onInput={(event) => {
                  state.forms.createOrgSlug = event.currentTarget.value;
                }}
              />
            </FormField>
            <div class="flex flex-wrap gap-2">
              <Button variant="primary" onClick={() => void createOrganization()}>
                Create organization
              </Button>
            </div>
          </div>
        </Card>
      </div>

      <div class="mt-4 grid gap-4 xl:grid-cols-2">
        <Card>
          <CardHeader title="Current organization" hint="Update the selected organization's basic details when you have admin access." />
          <div class="space-y-4 px-4 py-4">
            <Show
              when={currentOrg()}
              fallback={<p class="text-xs text-faint">Choose an organization to edit it.</p>}
            >
              <>
                <FormField label="Name">
                  <input
                    class="input"
                    value={state.forms.editOrgName}
                    onInput={(event) => {
                      state.forms.editOrgName = event.currentTarget.value;
                    }}
                  />
                </FormField>
                <FormField label="Slug">
                  <input
                    class="input"
                    value={state.forms.editOrgSlug}
                    onInput={(event) => {
                      state.forms.editOrgSlug = event.currentTarget.value;
                    }}
                  />
                </FormField>
                <div class="flex flex-wrap gap-2">
                  <Button onClick={() => void updateCurrentOrganization()}>Save organization details</Button>
                </div>
              </>
            </Show>
          </div>
        </Card>

        <Card>
          <CardHeader
            title="Invite a member"
            hint={`Look up a user by ${state.manifest?.auth.identity_field ?? "identity"} or paste their user id directly.`}
          />
          <div class="space-y-4 px-4 py-4">
            <FormField label={state.manifest?.auth.identity_field ?? "Identity"}>
              <input
                class="input"
                value={state.forms.inviteIdentity}
                onInput={(event) => {
                  state.forms.inviteIdentity = event.currentTarget.value;
                }}
              />
            </FormField>
            <FormField label="User id (optional)">
              <input
                class="input font-mono text-[0.78125rem]"
                value={state.forms.inviteUserId}
                onInput={(event) => {
                  state.forms.inviteUserId = event.currentTarget.value;
                }}
              />
            </FormField>
            <FormField label="Role">
              <input
                class="input"
                value={state.forms.inviteRole}
                onInput={(event) => {
                  state.forms.inviteRole = event.currentTarget.value;
                }}
              />
            </FormField>
            <div class="flex flex-wrap gap-2">
              <Button onClick={() => void lookupInvitee()}>{state.organizations.inviteLookupLoading ? "Looking up…" : "Find user"}</Button>
              <Button variant="primary" disabled={!state.auth.selectedOrgId} onClick={() => void inviteMember()}>
                Add to organization
              </Button>
            </div>
            <Show
              when={state.organizations.inviteLookup}
              fallback={
                <Show
                  when={state.organizations.inviteLookupError}
                  fallback={
                    <Notice
                      kind="info"
                      message="People can join organizations once an admin creates their membership here. There is no separate self-join flow in apiplant."
                    />
                  }
                >
                  <Notice kind="error" message={state.organizations.inviteLookupError ?? ""} />
                </Show>
              }
            >
              <Notice kind="success" message={`Ready to invite: ${state.organizations.inviteLookup?.label ?? ""}`} />
            </Show>
          </div>
        </Card>
      </div>

      <Card class="mt-4">
        <CardHeader title="Members in the active organization" hint="Change roles or remove access for the selected workspace." />
        <div class="px-4 py-4">
          <Switch>
            <Match when={!state.auth.selectedOrgId}>
              <p class="text-xs text-faint">Pick an organization to see its members.</p>
            </Match>
            <Match when={state.organizations.loadingMembers}>
              <p class="text-xs text-faint">Loading members…</p>
            </Match>
            <Match when={state.organizations.membersError}>
              <Notice kind="error" message={state.organizations.membersError ?? ""} />
            </Match>
            <Match when={state.organizations.members.length > 0}>
              <div class="overflow-x-auto rounded-xl border border-line">
                <table class="min-w-full divide-y divide-line text-sm">
                  <thead class="bg-surface-2/60 text-left text-[0.6875rem] uppercase tracking-[0.08em] text-faint">
                    <tr>
                      <th class="px-3 py-2">Person</th>
                      <th class="px-3 py-2">User id</th>
                      <th class="px-3 py-2">Role</th>
                      <th class="px-3 py-2">Actions</th>
                    </tr>
                  </thead>
                  <tbody class="divide-y divide-line">
                    <For each={state.organizations.members}>
                      {(member) => {
                        const membershipId = String(member.id ?? "");
                        const user = asRecord(member.user);
                        const person =
                          (state.manifest && typeof user?.[state.manifest.auth.identity_field] === "string"
                            ? String(user[state.manifest.auth.identity_field])
                            : typeof user?.display_name === "string"
                              ? user.display_name
                              : String(member.user_id ?? "Member"));
                        return (
                          <tr class="align-top">
                            <td class="px-3 py-3 text-ink">{person}</td>
                            <td class="px-3 py-3">
                              <Mono>{String(member.user_id ?? "—")}</Mono>
                            </td>
                            <td class="px-3 py-3">
                              <input
                                class="input"
                                value={state.organizations.memberRoleDrafts[membershipId] ?? ""}
                                onInput={(event) => {
                                  state.organizations.memberRoleDrafts[membershipId] = event.currentTarget.value;
                                }}
                              />
                            </td>
                            <td class="px-3 py-3">
                              <div class="flex flex-wrap gap-2">
                                <Button onClick={() => void saveMembershipRole(membershipId)}>Save</Button>
                                <Button variant="danger" onClick={() => void removeMembership(membershipId)}>
                                  Remove
                                </Button>
                              </div>
                            </td>
                          </tr>
                        );
                      }}
                    </For>
                  </tbody>
                </table>
              </div>
            </Match>
            <Match when={true}>
              <p class="text-xs text-faint">No members found for this organization yet.</p>
            </Match>
          </Switch>
        </div>
      </Card>
    </>
  );
}

function ResourcePage(props: { resource: ResourceManifest }) {
  const entry = () => ensureResourceState(props.resource);
  const visibleFieldsMemo = createMemo(() => visibleResourceFields(props.resource));
  const columnsMemo = createMemo(() => ["id", ...visibleFieldsMemo().map((field) => field.name)].slice(0, 5));
  const currentOrgMissing = createMemo(() => props.resource.scope === "organization" && !state.auth.selectedOrgId);

  return (
    <>
      <PageHeader
        eyebrow="Content workspace"
        title={humanize(props.resource.name)}
        copy={props.resource.permission_summary}
        badges={[
          {
            tone: props.resource.scope === "global" ? "accent" : "neutral",
            label: props.resource.scope === "global" ? "Shared across all organizations" : "Works inside the active organization",
          },
          { label: `${visibleFieldsMemo().length} visible field${visibleFieldsMemo().length === 1 ? "" : "s"}` },
          { label: props.resource.endpoint_summary },
        ]}
      />

      <Show when={currentOrgMissing()}>
        <Notice kind="warn" message={`Choose an organization before managing ${humanize(props.resource.name).toLowerCase()}.`} />
      </Show>

      <div class="mt-4 grid gap-4 xl:grid-cols-3">
        <MetricCard label="Collection" value={String(entry().rows.length)} hint="Records currently loaded into the browser." />
        <MetricCard label="Selection" value={entry().selectedId ? "1 record" : "None"} hint="Pick an item from the table to edit it here." tone="accent" />
        <MetricCard label="Writable fields" value={String(editableFields(props.resource).length)} hint="Fields exposed in the form after hidden and stamped values are removed." />
      </div>

      <div class="mt-4 grid gap-4 xl:grid-cols-[1.15fr_0.85fr]">
        <Card>
          <CardHeader title="Browse records" hint="Load a page of records and pick one to edit." />
          <div class="space-y-4 px-4 py-4">
            <div class="grid gap-4 md:grid-cols-2">
              <FormField label="Filter field">
                <select
                  class="input"
                  value={entry().filterField}
                  onChange={(event) => {
                    entry().filterField = event.currentTarget.value;
                  }}
                >
                  <option value="">No filter</option>
                  <For each={filterableFields(props.resource)}>{(field) => <option value={field.name}>{humanize(field.name)}</option>}</For>
                </select>
              </FormField>
              <FormField label="Filter value">
                <input
                  class="input"
                  value={entry().filterValue}
                  onInput={(event) => {
                    entry().filterValue = event.currentTarget.value;
                  }}
                />
              </FormField>
            </div>

            <div class="grid gap-4 md:grid-cols-2">
              <FormField label="Page size">
                <input
                  type="number"
                  class="input"
                  value={entry().limit}
                  onInput={(event) => {
                    entry().limit = event.currentTarget.value;
                  }}
                />
              </FormField>
              <FormField label="Offset">
                <input
                  type="number"
                  class="input"
                  value={entry().offset}
                  onInput={(event) => {
                    entry().offset = event.currentTarget.value;
                  }}
                />
              </FormField>
            </div>

            <div class="flex flex-wrap gap-2">
              <Button variant="primary" disabled={currentOrgMissing() || entry().loading} onClick={() => void loadResourceCollection(props.resource)}>
                {entry().loading ? "Loading…" : "Load records"}
              </Button>
              <Button onClick={() => startNewRecord(props.resource)}>New record</Button>
            </div>

            <Show
              when={entry().rows.length}
              fallback={<p class="text-xs text-faint">No records loaded yet.</p>}
            >
              <div class="overflow-x-auto rounded-xl border border-line">
                <table class="min-w-full divide-y divide-line text-sm">
                  <thead class="bg-surface-2/60 text-left text-[0.6875rem] uppercase tracking-[0.08em] text-faint">
                    <tr>
                      <For each={columnsMemo()}>{(column) => <th class="px-3 py-2">{humanize(column)}</th>}</For>
                    </tr>
                  </thead>
                  <tbody class="divide-y divide-line">
                    <For each={entry().rows}>
                      {(row) => {
                        const id = String(row.id ?? "");
                        return (
                          <tr class={entry().selectedId === id ? "bg-accent-soft/20" : ""}>
                            <For each={columnsMemo()}>
                              {(column, index) => (
                                <td class="px-3 py-3 align-top text-muted">
                                  <Show
                                    when={index() === 0}
                                    fallback={<span>{compactValue(row[column])}</span>}
                                  >
                                    <button
                                      type="button"
                                      class="font-mono text-[0.78125rem] text-accent hover:underline"
                                      onClick={() => {
                                        selectResourceRecord(props.resource, row);
                                      }}
                                    >
                                      {compactValue(row[column])}
                                    </button>
                                  </Show>
                                </td>
                              )}
                            </For>
                          </tr>
                        );
                      }}
                    </For>
                  </tbody>
                </table>
              </div>
            </Show>

            <Show when={entry().error}>
              <Notice kind="error" message={entry().error ?? ""} />
            </Show>
          </div>
        </Card>

        <Card>
          <CardHeader title={entry().selectedId ? "Edit record" : "Create a record"} hint="Use the form below instead of editing raw JSON." />
          <div class="space-y-4 px-4 py-4">
            <Show
              when={editableFields(props.resource).length}
              fallback={<p class="text-xs text-faint">This resource has no writable fields exposed to the admin panel.</p>}
            >
              <div class="grid gap-4 md:grid-cols-2">
                <For each={editableFields(props.resource)}>{(field) => <ResourceFieldEditor resource={props.resource} field={field} entry={entry()} />}</For>
              </div>
            </Show>

            <div class="flex flex-wrap gap-2">
              <Button
                variant="primary"
                disabled={entry().saving || currentOrgMissing()}
                onClick={() => void saveResourceRecord(props.resource, "create")}
              >
                Create
              </Button>
              <Button
                disabled={!entry().selectedId || entry().saving || currentOrgMissing()}
                onClick={() => void saveResourceRecord(props.resource, "update")}
              >
                Save changes
              </Button>
              <Button
                variant="danger"
                disabled={!entry().selectedId || entry().saving || currentOrgMissing()}
                onClick={() => void deleteResourceRecord(props.resource)}
              >
                Delete
              </Button>
            </div>

            <div class="border-t border-line pt-4">
              <h3 class="text-sm font-semibold tracking-tight text-ink">Selected record preview</h3>
              <Show
                when={entry().selectedRecord}
                fallback={<p class="mt-2 text-xs text-faint">Select a record from the table, or start a new one above.</p>}
              >
                <pre class="mt-3 overflow-x-auto rounded-xl border border-line bg-surface-2/50 p-3 font-mono text-[0.78125rem] leading-7 text-muted">
                  {prettyJson(entry().selectedRecord)}
                </pre>
              </Show>
            </div>
          </div>
        </Card>
      </div>
    </>
  );
}

function FunctionPage(props: { fn: FunctionManifest }) {
  const entry = () => ensureFunctionState(props.fn);

  return (
    <>
      <PageHeader
        eyebrow="Manual operation"
        title={humanize(props.fn.name)}
        copy={props.fn.description || "Manual action exposed by the application."}
        badges={[
          { tone: "accent", label: props.fn.method },
          { label: props.fn.visibility_label },
        ]}
      />

      <div class="mb-4 grid gap-4 xl:grid-cols-3">
        <MetricCard label="Method" value={props.fn.method} hint="HTTP method used to invoke this action." tone="accent" />
        <MetricCard label="Visibility" value={props.fn.visibility_label} hint="Authentication or organization role required to run it." />
        <MetricCard label="Payload" value={props.fn.method === "GET" ? "No body" : "JSON"} hint="Input format expected by this action." />
      </div>

      <div class="grid gap-4 xl:grid-cols-2">
        <Card>
          <CardHeader title="Run action" hint={props.fn.note} />
          <div class="space-y-4 px-4 py-4">
            <Notice
              kind="info"
              message={props.fn.method === "GET" ? "This action does not send a request body." : "Provide the action parameters as JSON."}
            />
            <Show when={props.fn.method !== "GET"}>
              <FormField label="Parameters">
                <textarea
                  class="input min-h-40 font-mono text-[0.78125rem]"
                  value={entry().input}
                  onInput={(event) => {
                    entry().input = event.currentTarget.value;
                  }}
                />
              </FormField>
            </Show>
            <div class="flex flex-wrap gap-2">
              <Button variant="primary" onClick={() => void invokeFunction(props.fn)}>
                {entry().loading ? "Running…" : "Run action"}
              </Button>
            </div>
            <Show when={entry().error}>
              <Notice kind="error" message={entry().error ?? ""} />
            </Show>
          </div>
        </Card>

        <Card>
          <CardHeader title="Result" hint="The raw response returned by the application." />
          <div class="px-4 py-4">
            <Show
              when={entry().output !== null}
              fallback={<p class="text-xs text-faint">Run the action to see its result.</p>}
            >
              <pre class="overflow-x-auto rounded-xl border border-line bg-surface-2/50 p-3 font-mono text-[0.78125rem] leading-7 text-muted">
                {prettyJson(entry().output)}
              </pre>
            </Show>
          </div>
        </Card>
      </div>
    </>
  );
}

export function App() {
  onMount(() => {
    readStoredAuth();
    void (async () => {
      const response = await fetch(MANIFEST_URL);
      if (!response.ok) throw new Error(`Failed to load ${MANIFEST_URL}`);
      state.manifest = (await response.json()) as AdminManifest;
      state.forms.loginIdentity = state.forms.loginIdentity || "";
      state.forms.registerIdentity = state.forms.registerIdentity || "";
      if (isAuthenticated()) {
        try {
          await refreshSession();
          state.page = { kind: "dashboard", name: null };
        } catch (error) {
          signOut();
          setNotice("error", error instanceof Error ? error.message : String(error));
        }
      }
    })().catch((error) => {
      setNotice("error", error instanceof Error ? error.message : String(error));
    });
  });

  return (
    <Show
      when={state.manifest}
      fallback={
        <div class="relative z-10 flex min-h-screen items-center justify-center p-6">
          <EmptyState title="Loading the admin panel" description="Fetching the baked application manifest and interface assets." />
        </div>
      }
    >
      <Show when={isAuthenticated()} fallback={<AuthPage />}>
        <div class="relative z-10 flex min-h-screen flex-col">
          <header class="flex min-h-16 items-center gap-3 border-b border-line bg-surface/70 px-4 py-3 backdrop-blur-md">
            <button type="button" class="group flex items-center gap-2" onClick={() => navigate("dashboard")} title="Dashboard">
              <HeadMark class="h-7 text-accent transition-opacity group-hover:opacity-80" />
              <span class="text-sm font-semibold tracking-tight">
                apiplant <span class="text-accent">admin</span>
              </span>
            </button>
            <Badge>{state.manifest?.app_name ?? ""}</Badge>
            <div class="flex-1" />
            <Show when={state.manifest?.docs_url}>
              {(docsUrl) => (
                <a href={docsUrl()} target="_blank" rel="noreferrer" class="hidden lg:inline-flex">
                  <Button size="sm" variant="ghost">
                    Open docs
                  </Button>
                </a>
              )}
            </Show>
            <Show when={currentOrganization()}>
              {(org) => <span class="inline-flex rounded-full border border-line bg-surface px-3 py-1 font-mono text-[0.6875rem] text-muted">{String(org().name ?? org().slug ?? org().id ?? "")}</span>}
            </Show>
            <span class="inline-flex rounded-full border border-line bg-surface px-3 py-1 font-mono text-[0.6875rem] text-muted">
              {currentUserLabel()}
            </span>
            <AdminThemeToggle />
            <Button variant="ghost" onClick={signOut}>
              Sign out
            </Button>
          </header>

          <div class="flex min-h-0 flex-1">
            <Sidebar />

            <main class="min-w-0 flex-1 overflow-y-auto">
              <div class="mx-auto max-w-[96rem] px-5 py-5">
                <div class="space-y-3">
                  <Show when={state.notice}>
                    {(notice) => <Notice kind={notice().kind} message={notice().message} />}
                  </Show>
                  <Show when={state.auth.refreshing}>
                    <Notice kind="info" message="Refreshing your session…" />
                  </Show>
                </div>

                <div class="mt-4">
                  <Switch>
                    <Match when={state.page.kind === "organization"}>
                      <OrganizationPage />
                    </Match>
                    <Match when={state.page.kind === "resource" && resourceByName(state.page.name)}>
                      <ResourcePage resource={resourceByName(state.page.name)!} />
                    </Match>
                    <Match when={state.page.kind === "function" && functionByName(state.page.name)}>
                      <FunctionPage fn={functionByName(state.page.name)!} />
                    </Match>
                    <Match when={state.page.kind === "resource"}>
                      <EmptyState title="That resource is gone" description="It was deleted or renamed. Pick another from the sidebar." />
                    </Match>
                    <Match when={state.page.kind === "function"}>
                      <EmptyState title="That action is gone" description="It is no longer available. Pick another from the sidebar." />
                    </Match>
                    <Match when={true}>
                      <DashboardPage />
                    </Match>
                  </Switch>
                </div>
              </div>
            </main>
          </div>
        </div>
      </Show>
    </Show>
  );
}
