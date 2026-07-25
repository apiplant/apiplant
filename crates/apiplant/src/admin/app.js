const STORAGE_KEY = "apiplant-admin-session";
const THEME_KEY = "apiplant-admin-theme";
const MANIFEST_URL = "./apiplant-admin.json";

const state = {
  manifest: null,
  page: { kind: "overview", name: null },
  auth: {
    mode: "bearer",
    bearerToken: "",
    apiKey: "",
    organizations: [],
    selectedOrgId: "",
    role: null,
    userId: null,
  },
  forms: {
    loginIdentity: "",
    loginPassword: "",
    registerIdentity: "",
    registerPassword: "",
    registerExtra: "{}",
    manualBearerToken: "",
    manualApiKey: "",
  },
  notice: null,
  resources: {},
  functions: {},
};

const root = document.getElementById("app");
if (!root) throw new Error("#app is missing");

function escapeHtml(value) {
  return String(value)
    .replaceAll("&", "&amp;")
    .replaceAll("<", "&lt;")
    .replaceAll(">", "&gt;")
    .replaceAll('"', "&quot;")
    .replaceAll("'", "&#39;");
}

function prettyJson(value) {
  return JSON.stringify(value, null, 2);
}

function readStoredAuth() {
  try {
    const raw = localStorage.getItem(STORAGE_KEY);
    if (!raw) return;
    const parsed = JSON.parse(raw);
    if (parsed && typeof parsed === "object") {
      state.auth.mode = parsed.mode === "apiKey" ? "apiKey" : "bearer";
      state.auth.bearerToken = typeof parsed.bearerToken === "string" ? parsed.bearerToken : "";
      state.auth.apiKey = typeof parsed.apiKey === "string" ? parsed.apiKey : "";
      state.auth.selectedOrgId = typeof parsed.selectedOrgId === "string" ? parsed.selectedOrgId : "";
      state.forms.manualBearerToken = state.auth.bearerToken;
      state.forms.manualApiKey = state.auth.apiKey;
    }
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

function currentTheme() {
  const stored = localStorage.getItem(THEME_KEY);
  if (stored === "light" || stored === "dark") return stored;
  return window.matchMedia?.("(prefers-color-scheme: light)").matches ? "light" : "dark";
}

function applyTheme(theme) {
  document.documentElement.classList.toggle("light", theme === "light");
  document.documentElement.style.colorScheme = theme;
  localStorage.setItem(THEME_KEY, theme);
}

function toggleTheme() {
  applyTheme(currentTheme() === "dark" ? "light" : "dark");
}

function humanizeField(name) {
  return name.replaceAll("_", " ");
}

function authHeader() {
  if (state.auth.mode === "apiKey" && state.auth.apiKey) return { "X-Api-Key": state.auth.apiKey };
  if (state.auth.bearerToken) return { Authorization: `Bearer ${state.auth.bearerToken}` };
  return {};
}

function isAuthenticated() {
  return Boolean(state.auth.apiKey || state.auth.bearerToken);
}

function isOrgScopedResource(resource) {
  return resource.scope === "organization";
}

function needsOrgContextForFunction(fn) {
  return fn.visibility === "role";
}

function activeOrgRequired(noteSource) {
  return Boolean(noteSource && noteSource.requires_org);
}

function setNotice(kind, message) {
  state.notice = { kind, message };
  render();
}

function clearNotice() {
  state.notice = null;
}

function decodeJwtUserId(token) {
  try {
    const parts = token.split(".");
    if (parts.length !== 3) return null;
    const payload = JSON.parse(
      atob(parts[1].replaceAll("-", "+").replaceAll("_", "/").padEnd(Math.ceil(parts[1].length / 4) * 4, "=")),
    );
    return typeof payload.sub === "string" ? payload.sub : null;
  } catch {
    return null;
  }
}

async function apiRequest(path, options = {}) {
  if (!state.manifest) throw new Error("manifest not loaded");
  const headers = { Accept: "application/json", ...authHeader(), ...(options.headers || {}) };
  if (options.body !== undefined) headers["Content-Type"] = "application/json";
  if (options.requiresOrg && !state.auth.selectedOrgId && state.auth.organizations.length > 1) {
    throw new Error("Select an organization before calling that endpoint.");
  }
  if (options.requiresOrg && state.auth.selectedOrgId) headers["X-Organization"] = state.auth.selectedOrgId;

  const response = await fetch(`${state.manifest.api_base_url}${path}`, {
    method: options.method || "GET",
    headers,
    body: options.body === undefined ? undefined : JSON.stringify(options.body),
  });

  if (response.status === 204) return null;
  const text = await response.text();
  const payload = text ? safeJson(text) : null;
  if (!response.ok) {
    const message =
      payload && typeof payload === "object" && !Array.isArray(payload) && typeof payload.error === "string"
        ? payload.error
        : `Request failed with ${response.status}`;
    const error = new Error(message);
    error.status = response.status;
    error.payload = payload;
    throw error;
  }
  return payload;
}

function safeJson(text) {
  try {
    return JSON.parse(text);
  } catch {
    return text;
  }
}

function userResource() {
  return state.manifest?.resources.find((resource) => resource.name === "user") ?? null;
}

function ensureResourceState(name) {
  if (!state.resources[name]) {
    const resource = resourceByName(name);
    state.resources[name] = {
      loading: false,
      error: null,
      rows: [],
      selectedId: "",
      selectedRecord: null,
      payload: resource ? templateForResource(resource, null) : "{}",
      filters: "{}",
      readId: "",
      limit: "50",
      offset: "0",
      lastResponse: null,
    };
  }
  return state.resources[name];
}

function ensureFunctionState(name) {
  if (!state.functions[name]) {
    const fn = functionByName(name);
    state.functions[name] = {
      input: fn && fn.method !== "GET" ? "{}" : "",
      loading: false,
      error: null,
      output: null,
    };
  }
  return state.functions[name];
}

function normalizeApiValue(value, field) {
  if (value === undefined) return undefined;
  if (value === null) return null;
  switch (field.type) {
    case "integer":
    case "big_int":
    case "float":
      return value === "" ? null : Number(value);
    case "boolean":
      return Boolean(value);
    default:
      return value;
  }
}

function templateForResource(resource, record) {
  const data = {};
  for (const field of resource.fields) {
    if (!field.writable) continue;
    if (record && Object.prototype.hasOwnProperty.call(record, field.name)) {
      data[field.name] = record[field.name];
      continue;
    }
    if (field.default_value !== null) {
      data[field.name] = field.default_value;
      continue;
    }
    if (!field.required) continue;
    switch (field.type) {
      case "boolean":
        data[field.name] = false;
        break;
      case "integer":
      case "big_int":
      case "float":
        data[field.name] = 0;
        break;
      case "json":
        data[field.name] = {};
        break;
      default:
        data[field.name] = "";
    }
  }
  return prettyJson(data);
}

function compactValue(value) {
  if (value === null || value === undefined) return "—";
  if (typeof value === "object") {
    const text = JSON.stringify(value);
    return text.length > 92 ? `${text.slice(0, 89)}…` : text;
  }
  const text = String(value);
  return text.length > 92 ? `${text.slice(0, 89)}…` : text;
}

function currentCredentialLabel() {
  if (state.auth.mode === "apiKey" && state.auth.apiKey) return "API key";
  if (state.auth.bearerToken) return "session token";
  return "signed out";
}

function resourceByName(name) {
  return state.manifest?.resources.find((resource) => resource.name === name) ?? null;
}

function functionByName(name) {
  return state.manifest?.functions.find((fn) => fn.name === name) ?? null;
}

function canTryPermission(permission, resource) {
  if (permission.value === "private") return false;
  if (!isAuthenticated()) return permission.value === "public" && resource.scope === "global";
  return true;
}

function navigationTo(kind, name = null) {
  state.page = { kind, name };
  clearNotice();
  render();
  if (kind === "resource" && name) {
    const resource = resourceByName(name);
    const entry = ensureResourceState(name);
    if (resource && !entry.rows.length && resource.permissions.list.value !== "private") {
      void loadResourceCollection(resource);
    }
  }
}

async function authenticateFromForms(mode) {
  try {
    clearNotice();
    state.auth.mode = mode;
    if (mode === "bearer") {
      state.auth.bearerToken = state.forms.manualBearerToken.trim();
      state.auth.apiKey = "";
    } else {
      state.auth.apiKey = state.forms.manualApiKey.trim();
      state.auth.bearerToken = "";
    }
    state.auth.userId = decodeJwtUserId(state.auth.bearerToken);
    persistAuth();
    await refreshAuthContext();
    setNotice("success", `Using ${mode === "apiKey" ? "an API key" : "a session token"} for API requests.`);
  } catch (error) {
    setNotice("error", error.message || String(error));
  }
}

async function login() {
  const manifest = state.manifest;
  if (!manifest) return;
  try {
    clearNotice();
    const identity = state.forms.loginIdentity.trim();
    const password = state.forms.loginPassword;
    const payload = { [manifest.auth.identity_field]: identity, password };
    const response = await apiRequest("/auth/login", { method: "POST", body: payload });
    state.auth.mode = "bearer";
    state.auth.bearerToken = response.token;
    state.auth.apiKey = "";
    state.forms.manualBearerToken = response.token;
    state.forms.loginPassword = "";
    state.auth.userId = decodeJwtUserId(response.token);
    persistAuth();
    await refreshAuthContext();
    setNotice("success", "Signed in.");
  } catch (error) {
    setNotice("error", error.message || String(error));
  }
}

async function register() {
  const manifest = state.manifest;
  if (!manifest) return;
  try {
    clearNotice();
    const identity = state.forms.registerIdentity.trim();
    const password = state.forms.registerPassword;
    const extra = parseJsonDraft(state.forms.registerExtra, "registration extras");
    const payload = { ...extra, [manifest.auth.identity_field]: identity, password };
    const response = await apiRequest("/auth/register", { method: "POST", body: payload });
    state.auth.mode = "bearer";
    state.auth.bearerToken = response.token;
    state.auth.apiKey = "";
    state.forms.manualBearerToken = response.token;
    state.forms.registerPassword = "";
    state.forms.registerExtra = "{}";
    state.auth.userId = decodeJwtUserId(response.token);
    persistAuth();
    await refreshAuthContext();
    setNotice("success", "Account created and signed in.");
  } catch (error) {
    setNotice("error", error.message || String(error));
  }
}

function signOut() {
  state.auth.mode = "bearer";
  state.auth.bearerToken = "";
  state.auth.apiKey = "";
  state.auth.organizations = [];
  state.auth.selectedOrgId = "";
  state.auth.role = null;
  state.auth.userId = null;
  state.forms.manualBearerToken = "";
  state.forms.manualApiKey = "";
  persistAuth();
  render();
}

async function refreshAuthContext() {
  state.auth.organizations = [];
  state.auth.role = null;
  if (!isAuthenticated()) {
    render();
    return;
  }
  const organizations = await apiRequest("/organization");
  state.auth.organizations = Array.isArray(organizations) ? organizations : [];
  const onlyOrg = state.auth.organizations.length === 1 ? String(state.auth.organizations[0].id ?? "") : "";
  if (onlyOrg && !state.auth.selectedOrgId) state.auth.selectedOrgId = onlyOrg;
  if (
    state.auth.selectedOrgId &&
    !state.auth.organizations.some((organization) => String(organization.id) === state.auth.selectedOrgId)
  ) {
    state.auth.selectedOrgId = onlyOrg;
  }
  persistAuth();
  await refreshRole();
  render();
}

async function refreshRole() {
  state.auth.role = null;
  if (!state.auth.selectedOrgId || !state.auth.userId) return;
  try {
    const rows = await apiRequest(`/membership?user_id=${encodeURIComponent(state.auth.userId)}&limit=1`, {
      requiresOrg: true,
    });
    if (Array.isArray(rows) && rows[0] && typeof rows[0].role === "string") state.auth.role = rows[0].role;
  } catch {
    state.auth.role = null;
  }
}

function parseJsonDraft(text, label) {
  try {
    const value = JSON.parse(text || "{}");
    if (!value || typeof value !== "object" || Array.isArray(value)) {
      throw new Error(`${label} must be a JSON object.`);
    }
    return value;
  } catch (error) {
    throw new Error(`Invalid ${label}: ${error.message || error}`);
  }
}

async function loadResourceCollection(resource) {
  const entry = ensureResourceState(resource.name);
  entry.loading = true;
  entry.error = null;
  render();
  try {
    const filters = parseJsonDraft(entry.filters || "{}", `${resource.name} filters`);
    const params = new URLSearchParams();
    const limit = Number(entry.limit || 50);
    const offset = Number(entry.offset || 0);
    params.set("limit", Number.isFinite(limit) ? String(limit) : "50");
    params.set("offset", Number.isFinite(offset) ? String(offset) : "0");
    for (const [key, value] of Object.entries(filters)) {
      if (value === "" || value === undefined) continue;
      params.set(key, String(value));
    }
    if (resource.relations.length) {
      params.set(
        "expand",
        resource.relations.map((relation) => relation.relation).join(","),
      );
    }
    const rows = await apiRequest(`/${resource.name}?${params.toString()}`, {
      requiresOrg: activeOrgRequired(resource.permissions.list),
    });
    entry.rows = Array.isArray(rows) ? rows : [];
    entry.selectedId = entry.rows[0]?.id ? String(entry.rows[0].id) : "";
    entry.selectedRecord = entry.rows[0] ?? null;
    entry.payload = templateForResource(resource, entry.selectedRecord);
    entry.lastResponse = rows;
  } catch (error) {
    entry.error = error.message || String(error);
  } finally {
    entry.loading = false;
    render();
  }
}

async function loadResourceById(resource) {
  const entry = ensureResourceState(resource.name);
  const id = entry.readId.trim();
  if (!id) {
    setNotice("error", "Enter a record id first.");
    return;
  }
  entry.loading = true;
  entry.error = null;
  render();
  try {
    const params = resource.relations.length
      ? `?expand=${encodeURIComponent(resource.relations.map((relation) => relation.relation).join(","))}`
      : "";
    const row = await apiRequest(`/${resource.name}/${encodeURIComponent(id)}${params}`, {
      requiresOrg: activeOrgRequired(resource.permissions.read),
    });
    entry.rows = [row];
    entry.selectedId = String(row.id ?? id);
    entry.selectedRecord = row;
    entry.payload = templateForResource(resource, row);
    entry.lastResponse = row;
  } catch (error) {
    entry.error = error.message || String(error);
  } finally {
    entry.loading = false;
    render();
  }
}

async function createResourceRecord(resource) {
  const entry = ensureResourceState(resource.name);
  try {
    clearNotice();
    const payload = parseJsonDraft(entry.payload, `${resource.name} payload`);
    entry.loading = true;
    render();
    const row = await apiRequest(`/${resource.name}`, {
      method: "POST",
      body: payload,
      requiresOrg: activeOrgRequired(resource.permissions.create),
    });
    entry.selectedId = String(row.id ?? "");
    entry.selectedRecord = row;
    entry.payload = templateForResource(resource, row);
    setNotice("success", `Created ${resource.name}.`);
    await loadResourceCollection(resource);
  } catch (error) {
    entry.loading = false;
    setNotice("error", error.message || String(error));
    render();
  }
}

async function updateResourceRecord(resource) {
  const entry = ensureResourceState(resource.name);
  if (!entry.selectedId) {
    setNotice("error", "Select or load a record first.");
    return;
  }
  try {
    clearNotice();
    const payload = parseJsonDraft(entry.payload, `${resource.name} payload`);
    entry.loading = true;
    render();
    const row = await apiRequest(`/${resource.name}/${encodeURIComponent(entry.selectedId)}`, {
      method: "PATCH",
      body: payload,
      requiresOrg: activeOrgRequired(resource.permissions.update),
    });
    entry.selectedRecord = row;
    entry.payload = templateForResource(resource, row);
    setNotice("success", `Updated ${resource.name}.`);
    await loadResourceCollection(resource);
  } catch (error) {
    entry.loading = false;
    setNotice("error", error.message || String(error));
    render();
  }
}

async function deleteResourceRecord(resource) {
  const entry = ensureResourceState(resource.name);
  if (!entry.selectedId) {
    setNotice("error", "Select or load a record first.");
    return;
  }
  if (!window.confirm(`Delete ${resource.name} ${entry.selectedId}?`)) return;
  try {
    clearNotice();
    entry.loading = true;
    render();
    await apiRequest(`/${resource.name}/${encodeURIComponent(entry.selectedId)}`, {
      method: "DELETE",
      requiresOrg: activeOrgRequired(resource.permissions.delete),
    });
    entry.selectedId = "";
    entry.selectedRecord = null;
    entry.payload = templateForResource(resource, null);
    setNotice("success", `Deleted ${resource.name}.`);
    await loadResourceCollection(resource);
  } catch (error) {
    entry.loading = false;
    setNotice("error", error.message || String(error));
    render();
  }
}

async function invokeFunction(fn) {
  const entry = ensureFunctionState(fn.name);
  entry.loading = true;
  entry.error = null;
  entry.output = null;
  render();
  try {
    const body =
      fn.method === "GET"
        ? undefined
        : fn.method === "DELETE" && !entry.input.trim()
          ? {}
          : parseJsonDraft(entry.input || "{}", `${fn.name} input`);
    const response = await apiRequest(`/functions/${fn.name}`, {
      method: fn.method,
      body,
      requiresOrg: needsOrgContextForFunction(fn),
    });
    entry.output = response;
  } catch (error) {
    entry.error = error.message || String(error);
  } finally {
    entry.loading = false;
    render();
  }
}

function permissionBadges(resource) {
  return ["list", "read", "create", "update", "delete"]
    .map((action) => {
      const permission = resource.permissions[action];
      return `<span class="ap-badge ${permission.value === "private" ? "warn" : permission.value.startsWith("role:") ? "accent" : ""}" title="${escapeHtml(permission.note)}">${escapeHtml(action)}: ${escapeHtml(permission.value)}</span>`;
    })
    .join("");
}

function renderOverview() {
  const manifest = state.manifest;
  if (!manifest) return "";
  const signedIn = isAuthenticated();
  const orgOptions = state.auth.organizations
    .map((organization) => {
      const id = String(organization.id ?? "");
      const label = organization.name || organization.slug || id;
      return `<option value="${escapeHtml(id)}"${state.auth.selectedOrgId === id ? " selected" : ""}>${escapeHtml(label)}</option>`;
    })
    .join("");
  const docsLink = manifest.docs_url
    ? `<a class="ap-anchor ap-mono" href="${escapeHtml(manifest.docs_url)}" target="_blank" rel="noreferrer">${escapeHtml(manifest.docs_url)}</a>`
    : `<span class="ap-subtle">Swagger UI is disabled for this app.</span>`;

  return `
    <div class="ap-page-header">
      <h1 class="ap-page-title">${escapeHtml(manifest.title)}</h1>
      <p class="ap-page-copy">
        Static admin panel baked from this app directory. It knows the resources, auth model, function endpoints,
        and permission rules up front, then talks directly to <span class="ap-mono">${escapeHtml(manifest.api_base_url)}</span>.
      </p>
      <div class="ap-meta-row">
        <span class="ap-badge accent">${manifest.resources.length} resources</span>
        <span class="ap-badge">${manifest.functions.length} callable functions</span>
        <span class="ap-badge">${escapeHtml(signedIn ? `authenticated via ${currentCredentialLabel()}` : "signed out")}</span>
      </div>
    </div>

    <div class="ap-grid cols-3">
      <section class="ap-card">
        <div class="ap-card-head">
          <div>
            <h2 class="ap-card-title">Overview</h2>
            <p class="ap-card-hint">What this build can manage without reading the app directory at runtime.</p>
          </div>
        </div>
        <div class="ap-stat-grid">
          <div class="ap-stat">
            <p class="ap-stat-label">Resources</p>
            <p class="ap-stat-value">${manifest.resources.length}</p>
            <p class="ap-stat-note">CRUD surfaces from the baked schema</p>
          </div>
          <div class="ap-stat">
            <p class="ap-stat-label">Functions</p>
            <p class="ap-stat-value">${manifest.functions.length}</p>
            <p class="ap-stat-note">Loaded non-private function endpoints</p>
          </div>
          <div class="ap-stat">
            <p class="ap-stat-label">Auth</p>
            <p class="ap-stat-value">${escapeHtml(manifest.auth.identity_field)}</p>
            <p class="ap-stat-note">Identity field for login/register</p>
          </div>
          <div class="ap-stat">
            <p class="ap-stat-label">Docs</p>
            <p class="ap-stat-value">${manifest.docs_url ? "on" : "off"}</p>
            <p class="ap-stat-note">OpenAPI / Swagger availability</p>
          </div>
        </div>
      </section>

      <section class="ap-card">
        <div class="ap-card-head">
          <div>
            <h2 class="ap-card-title">Authentication</h2>
            <p class="ap-card-hint">Sign in with a session token, or paste an existing token or API key.</p>
          </div>
        </div>
        <div class="ap-card-body ap-stack">
          ${
            signedIn
              ? `
                <div class="ap-auth-status">
                  <span class="ap-badge accent">Signed in</span>
                  <span class="ap-badge">${escapeHtml(currentCredentialLabel())}</span>
                  ${state.auth.role ? `<span class="ap-badge accent">role: ${escapeHtml(state.auth.role)}</span>` : ""}
                </div>
                <div class="ap-field">
                  <label class="ap-label" for="org-select">Active organization</label>
                  <select id="org-select" class="ap-select" data-field="selected-org">
                    <option value="">${state.auth.organizations.length > 1 ? "Choose an organization" : "No organization selected"}</option>
                    ${orgOptions}
                  </select>
                </div>
                <div class="ap-actions">
                  <button class="ap-btn" type="button" data-action="refresh-auth">Refresh memberships</button>
                  <button class="ap-btn ap-btn-danger" type="button" data-action="sign-out">Sign out</button>
                </div>
              `
              : `
                <div class="ap-auth-grid">
                  <div>
                    <div class="ap-field">
                      <label class="ap-label" for="login-identity">${escapeHtml(manifest.auth.identity_field)}</label>
                      <input id="login-identity" class="input" data-field="login-identity" value="${escapeHtml(state.forms.loginIdentity)}" />
                    </div>
                    <div class="ap-field">
                      <label class="ap-label" for="login-password">password</label>
                      <input id="login-password" type="password" class="input" data-field="login-password" value="${escapeHtml(state.forms.loginPassword)}" />
                    </div>
                    <div class="ap-actions">
                      <button class="ap-btn ap-btn-primary" type="button" data-action="login">Sign in</button>
                    </div>
                  </div>
                  <div>
                    <div class="ap-field">
                      <label class="ap-label" for="register-identity">${escapeHtml(manifest.auth.identity_field)}</label>
                      <input id="register-identity" class="input" data-field="register-identity" value="${escapeHtml(state.forms.registerIdentity)}" />
                    </div>
                    <div class="ap-field">
                      <label class="ap-label" for="register-password">password</label>
                      <input id="register-password" type="password" class="input" data-field="register-password" value="${escapeHtml(state.forms.registerPassword)}" />
                    </div>
                    <div class="ap-field">
                      <label class="ap-label" for="register-extra">extra user fields (JSON object)</label>
                      <textarea id="register-extra" class="ap-textarea ap-mono" data-field="register-extra">${escapeHtml(state.forms.registerExtra)}</textarea>
                    </div>
                    <div class="ap-actions">
                      <button class="ap-btn ap-btn-primary" type="button" data-action="register"${manifest.auth.allow_registration ? "" : " disabled"}>Register</button>
                    </div>
                  </div>
                </div>
                ${
                  !manifest.auth.allow_registration
                    ? `<div class="ap-notice warn">Registration is disabled for this app, so create users elsewhere and sign in here.</div>`
                    : ""
                }
              `
          }

          <div class="ap-divider"></div>
          <p class="ap-auth-separator">Existing credentials</p>
          <div class="ap-auth-grid">
            <div>
              <div class="ap-field">
                <label class="ap-label" for="manual-bearer">session token</label>
                <textarea id="manual-bearer" class="ap-textarea ap-mono" data-field="manual-bearer">${escapeHtml(state.forms.manualBearerToken)}</textarea>
              </div>
              <div class="ap-actions">
                <button class="ap-btn" type="button" data-action="use-bearer">Use token</button>
              </div>
            </div>
            <div>
              <div class="ap-field">
                <label class="ap-label" for="manual-api-key">API key</label>
                <textarea id="manual-api-key" class="ap-textarea ap-mono" data-field="manual-api-key">${escapeHtml(state.forms.manualApiKey)}</textarea>
              </div>
              <div class="ap-actions">
                <button class="ap-btn" type="button" data-action="use-api-key">Use API key</button>
              </div>
            </div>
          </div>
        </div>
      </section>

      <section class="ap-card">
        <div class="ap-card-head">
          <div>
            <h2 class="ap-card-title">Runtime notes</h2>
            <p class="ap-card-hint">Permissions still come from the live API, especially owner- and role-scoped operations.</p>
          </div>
        </div>
        <div class="ap-card-body">
          <ul class="ap-help-list">
            <li>Org-scoped resources use the selected <span class="ap-mono">X-Organization</span> header.</li>
            <li>Role-gated function calls and resource writes are still enforced server-side.</li>
            <li>Function request bodies are raw JSON. GET functions are invoked without a body because browsers forbid GET payloads.</li>
            <li>This static app should be served over HTTP(S); opening it as <span class="ap-mono">file://</span> will block the manifest fetch in most browsers.</li>
          </ul>
          <div class="ap-divider"></div>
          <p class="ap-subtle">OpenAPI:</p>
          <div>${docsLink}</div>
        </div>
      </section>
    </div>
  `;
}

function renderResourcePage(resource) {
  if (!resource) {
    return `<div class="ap-empty">That resource is no longer in the baked manifest.</div>`;
  }
  const entry = ensureResourceState(resource.name);
  const selected = entry.selectedRecord;
  const rows = entry.rows;
  const columns = ["id", ...resource.fields.filter((field) => !field.hidden).map((field) => field.name)].slice(0, 7);
  const rowsHtml = rows.length
    ? `
      <div class="ap-table-wrap">
        <table class="ap-table">
          <thead>
            <tr>${columns.map((column) => `<th>${escapeHtml(column)}</th>`).join("")}</tr>
          </thead>
          <tbody>
            ${rows
              .map((row) => {
                const id = String(row.id ?? "");
                return `<tr class="${entry.selectedId === id ? "is-selected" : ""}">
                  ${columns
                    .map((column, index) => {
                      const cell = compactValue(row[column]);
                      return `<td>${
                        index === 0
                          ? `<button class="ap-link-btn ap-mono" type="button" data-action="select-row" data-resource="${escapeHtml(resource.name)}" data-id="${escapeHtml(id)}">${escapeHtml(cell)}</button>`
                          : escapeHtml(cell)
                      }</td>`;
                    })
                    .join("")}
                </tr>`;
              })
              .join("")}
          </tbody>
        </table>
      </div>
    `
    : `<div class="ap-empty">No records loaded yet.</div>`;

  return `
    <div class="ap-page-header">
      <h1 class="ap-page-title ap-mono">${escapeHtml(resource.name)}</h1>
      <p class="ap-page-copy">
        ${escapeHtml(resource.permission_summary)}
      </p>
      <div class="ap-meta-row">
        <span class="ap-badge ${resource.scope === "global" ? "accent" : ""}">${escapeHtml(resource.scope === "global" ? "global resource" : "org-scoped resource")}</span>
        <span class="ap-badge">${resource.fields.length} declared fields</span>
        ${permissionBadges(resource)}
      </div>
    </div>

    <div class="ap-grid cols-2">
      <section class="ap-card">
        <div class="ap-card-head">
          <div>
            <h2 class="ap-card-title">Load records</h2>
            <p class="ap-card-hint">Exact-match filters follow apiplant's list API. Empty filters load the newest page.</p>
          </div>
        </div>
        <div class="ap-card-body ap-stack">
          <div class="ap-field-inline">
            <div class="ap-field">
              <label class="ap-label" for="limit-${escapeHtml(resource.name)}">limit</label>
              <input id="limit-${escapeHtml(resource.name)}" class="input ap-mono" data-field="resource-limit" data-resource="${escapeHtml(resource.name)}" value="${escapeHtml(entry.limit)}" />
            </div>
            <div class="ap-field">
              <label class="ap-label" for="offset-${escapeHtml(resource.name)}">offset</label>
              <input id="offset-${escapeHtml(resource.name)}" class="input ap-mono" data-field="resource-offset" data-resource="${escapeHtml(resource.name)}" value="${escapeHtml(entry.offset)}" />
            </div>
          </div>
          <div class="ap-field">
            <label class="ap-label" for="filters-${escapeHtml(resource.name)}">filters (JSON object)</label>
            <textarea id="filters-${escapeHtml(resource.name)}" class="ap-textarea ap-mono" data-field="resource-filters" data-resource="${escapeHtml(resource.name)}">${escapeHtml(entry.filters)}</textarea>
          </div>
          <div class="ap-actions">
            <button class="ap-btn ap-btn-primary" type="button" data-action="load-collection" data-resource="${escapeHtml(resource.name)}"${resource.permissions.list.value === "private" ? " disabled" : ""}>Load collection</button>
            <button class="ap-btn" type="button" data-action="new-payload" data-resource="${escapeHtml(resource.name)}">New payload</button>
          </div>
          <div class="ap-divider"></div>
          <div class="ap-field">
            <label class="ap-label" for="read-id-${escapeHtml(resource.name)}">load one record by id</label>
            <input id="read-id-${escapeHtml(resource.name)}" class="input ap-mono" data-field="resource-read-id" data-resource="${escapeHtml(resource.name)}" value="${escapeHtml(entry.readId)}" />
          </div>
          <div class="ap-actions">
            <button class="ap-btn" type="button" data-action="load-by-id" data-resource="${escapeHtml(resource.name)}"${resource.permissions.read.value === "private" ? " disabled" : ""}>Load record</button>
          </div>
          ${
            entry.error
              ? `<div class="ap-notice error">${escapeHtml(entry.error)}</div>`
              : ""
          }
        </div>
      </section>

      <section class="ap-card">
        <div class="ap-card-head">
          <div>
            <h2 class="ap-card-title">Selection & payload</h2>
            <p class="ap-card-hint">Edit raw JSON for create or update. Writable fields follow the baked schema, not the live OpenAPI.</p>
          </div>
        </div>
        <div class="ap-card-body ap-stack">
          <dl class="ap-kv">
            <dt>Selected record</dt>
            <dd>${selected ? `<span class="ap-mono">${escapeHtml(String(selected.id ?? entry.selectedId))}</span>` : "none"}</dd>
            <dt>Owner field</dt>
            <dd class="ap-mono">${escapeHtml(resource.owner_field)}</dd>
            <dt>References</dt>
            <dd>${resource.relations.length ? resource.relations.map((relation) => `${relation.field} → ${relation.target}`).join(", ") : "none"}</dd>
          </dl>
          <div class="ap-field">
            <label class="ap-label" for="payload-${escapeHtml(resource.name)}">request payload</label>
            <textarea id="payload-${escapeHtml(resource.name)}" class="ap-textarea ap-mono" data-field="resource-payload" data-resource="${escapeHtml(resource.name)}">${escapeHtml(entry.payload)}</textarea>
          </div>
          <div class="ap-actions">
            <button class="ap-btn ap-btn-primary" type="button" data-action="create-record" data-resource="${escapeHtml(resource.name)}"${!canTryPermission(resource.permissions.create, resource) ? " disabled" : ""}>Create</button>
            <button class="ap-btn" type="button" data-action="update-record" data-resource="${escapeHtml(resource.name)}"${!entry.selectedId || resource.permissions.update.value === "private" ? " disabled" : ""}>Update</button>
            <button class="ap-btn ap-btn-danger" type="button" data-action="delete-record" data-resource="${escapeHtml(resource.name)}"${!entry.selectedId || resource.permissions.delete.value === "private" ? " disabled" : ""}>Delete</button>
          </div>
          <div class="ap-notice">
            ${escapeHtml(
              selected
                ? "Update uses PATCH with the JSON above. Owner, organization and hidden fields are omitted from generated templates."
                : "Create starts from a baked writable-field template. Add or remove keys as needed before submitting.",
            )}
          </div>
        </div>
      </section>
    </div>

    <div class="ap-grid cols-2" style="margin-top: 0.9rem;">
      <section class="ap-card">
        <div class="ap-card-head">
          <div>
            <h2 class="ap-card-title">Loaded records</h2>
            <p class="ap-card-hint">${escapeHtml(resource.endpoint_summary)}</p>
          </div>
        </div>
        <div class="ap-card-body">
          ${rowsHtml}
        </div>
      </section>

      <section class="ap-card">
        <div class="ap-card-head">
          <div>
            <h2 class="ap-card-title">Record JSON</h2>
            <p class="ap-card-hint">Expanded relations are shown when the resource declares references.</p>
          </div>
        </div>
        <div class="ap-card-body">
          ${
            selected
              ? `<pre class="ap-json">${escapeHtml(prettyJson(selected))}</pre>`
              : `<div class="ap-empty">Select or load a record to inspect its JSON response.</div>`
          }
        </div>
      </section>
    </div>
  `;
}

function renderFunctionPage(fn) {
  if (!fn) {
    return `<div class="ap-empty">That function is no longer in the baked manifest.</div>`;
  }
  const entry = ensureFunctionState(fn.name);
  return `
    <div class="ap-page-header">
      <h1 class="ap-page-title ap-mono">${escapeHtml(fn.name)}</h1>
      <p class="ap-page-copy">${escapeHtml(fn.description || "Compiled function endpoint.")}</p>
      <div class="ap-meta-row">
        <span class="ap-badge accent">${escapeHtml(fn.method)}</span>
        <span class="ap-badge ${fn.visibility === "role" ? "accent" : fn.visibility === "private" ? "warn" : ""}">${escapeHtml(fn.visibility_label)}</span>
      </div>
    </div>

    <div class="ap-grid cols-2">
      <section class="ap-card">
        <div class="ap-card-head">
          <div>
            <h2 class="ap-card-title">Invoke</h2>
            <p class="ap-card-hint">${escapeHtml(fn.note)}</p>
          </div>
        </div>
        <div class="ap-card-body ap-stack">
          <dl class="ap-kv">
            <dt>Endpoint</dt>
            <dd class="ap-mono">${escapeHtml(`${state.manifest.api_base_url}/functions/${fn.name}`)}</dd>
            <dt>Method</dt>
            <dd>${escapeHtml(fn.method)}</dd>
            <dt>Role</dt>
            <dd>${fn.role ? escapeHtml(fn.role) : "—"}</dd>
          </dl>
          <div class="ap-field">
            <label class="ap-label" for="function-input-${escapeHtml(fn.name)}">request body</label>
            <textarea id="function-input-${escapeHtml(fn.name)}" class="ap-textarea ap-mono" data-field="function-input" data-function="${escapeHtml(fn.name)}"${fn.method === "GET" ? " disabled" : ""}>${escapeHtml(entry.input)}</textarea>
          </div>
          ${
            fn.method === "GET"
              ? `<div class="ap-notice warn">GET functions are invoked without a request body because the browser fetch API rejects GET payloads.</div>`
              : ""
          }
          <div class="ap-actions">
            <button class="ap-btn ap-btn-primary" type="button" data-action="invoke-function" data-function="${escapeHtml(fn.name)}">Invoke</button>
          </div>
          ${
            entry.error
              ? `<div class="ap-notice error">${escapeHtml(entry.error)}</div>`
              : ""
          }
        </div>
      </section>

      <section class="ap-card">
        <div class="ap-card-head">
          <div>
            <h2 class="ap-card-title">Response</h2>
            <p class="ap-card-hint">Raw JSON returned by the function endpoint.</p>
          </div>
        </div>
        <div class="ap-card-body">
          ${
            entry.output !== null
              ? `<pre class="ap-json">${escapeHtml(prettyJson(entry.output))}</pre>`
              : `<div class="ap-empty">Run the function to inspect its response.</div>`
          }
        </div>
      </section>
    </div>
  `;
}

function renderSidebar() {
  const manifest = state.manifest;
  if (!manifest) return "";
  return `
    <aside class="ap-sidebar">
      <div class="ap-sidebar-inner">
        <div class="ap-sidebar-section">
          <button class="ap-nav-btn ap-nav-primary ${state.page.kind === "overview" ? "is-active" : ""}" type="button" data-nav-kind="overview">Overview</button>
        </div>
        <div class="ap-sidebar-section">
          <p class="ap-sidebar-label">Resources</p>
          ${manifest.resources
            .map(
              (resource) => `
                <button
                  class="ap-nav-btn ap-nav-mono ${state.page.kind === "resource" && state.page.name === resource.name ? "is-active" : ""}"
                  type="button"
                  data-nav-kind="resource"
                  data-nav-name="${escapeHtml(resource.name)}"
                >
                  <span>${escapeHtml(resource.name)}</span>
                </button>
              `,
            )
            .join("")}
        </div>
        <div class="ap-sidebar-section">
          <p class="ap-sidebar-label">Functions</p>
          ${
            manifest.functions.length
              ? manifest.functions
                  .map(
                    (fn) => `
                      <button
                        class="ap-nav-btn ap-nav-mono ${state.page.kind === "function" && state.page.name === fn.name ? "is-active" : ""}"
                        type="button"
                        data-nav-kind="function"
                        data-nav-name="${escapeHtml(fn.name)}"
                      >
                        <span>${escapeHtml(fn.name)}</span>
                      </button>
                    `,
                  )
                  .join("")
              : `<div class="ap-empty">No callable functions were loaded when this panel was built.</div>`
          }
        </div>
      </div>
    </aside>
  `;
}

function renderNotice() {
  if (!state.notice) return "";
  return `<div class="ap-notice ${escapeHtml(state.notice.kind)}">${escapeHtml(state.notice.message)}</div>`;
}

function render() {
  if (!state.manifest) {
    root.innerHTML = `
      <div class="ap-admin-shell">
        <main class="ap-nav-main">
          <div class="ap-main-inner">
            <div class="ap-empty">Loading the baked admin manifest…</div>
          </div>
        </main>
      </div>
    `;
    return;
  }

  const page =
    state.page.kind === "resource" && state.page.name
      ? renderResourcePage(resourceByName(state.page.name))
      : state.page.kind === "function" && state.page.name
        ? renderFunctionPage(functionByName(state.page.name))
        : renderOverview();

  root.innerHTML = `
    <div class="ap-admin-shell">
      <header class="ap-admin-header">
        <button class="ap-admin-brand" type="button" data-nav-kind="overview" title="Overview">
          <span class="logo-head ap-logo" role="img" aria-label="apiplant"></span>
          <span class="ap-admin-title">apiplant <span class="ap-accent">admin</span></span>
        </button>
        <span class="ap-badge">${escapeHtml(state.manifest.app_name)}</span>
        <div class="ap-header-spacer"></div>
        <span class="ap-api-pill">${escapeHtml(state.manifest.api_base_url)}</span>
        <button class="ap-theme-toggle" type="button" data-action="toggle-theme" title="Toggle theme" aria-label="Toggle theme">
          ${
            currentTheme() === "dark"
              ? `<svg width="16" height="16" viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.4"><circle cx="8" cy="8" r="3.1"></circle><path d="M8 1.4v1.5M8 13.1v1.5M14.6 8h-1.5M2.9 8H1.4M12.67 3.33l-1.06 1.06M4.39 11.61l-1.06 1.06M12.67 12.67l-1.06-1.06M4.39 4.39L3.33 3.33" stroke-linecap="round"></path></svg>`
              : `<svg width="16" height="16" viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.4"><path d="M13.2 9.6A5.4 5.4 0 0 1 6.4 2.8a5.6 5.6 0 1 0 6.8 6.8z" stroke-linejoin="round"></path></svg>`
          }
        </button>
      </header>
      <div class="ap-admin-body">
        ${renderSidebar()}
        <main class="ap-nav-main">
          <div class="ap-main-inner">
            ${renderNotice()}
            ${page}
          </div>
        </main>
      </div>
    </div>
  `;
}

root.addEventListener("click", (event) => {
  const target = event.target.closest("[data-action],[data-nav-kind]");
  if (!target) return;

  const navKind = target.getAttribute("data-nav-kind");
  if (navKind) {
    navigationTo(navKind, target.getAttribute("data-nav-name"));
    return;
  }

  const action = target.getAttribute("data-action");
  if (!action) return;

  switch (action) {
    case "toggle-theme":
      toggleTheme();
      render();
      break;
    case "login":
      void login();
      break;
    case "register":
      void register();
      break;
    case "use-bearer":
      void authenticateFromForms("bearer");
      break;
    case "use-api-key":
      void authenticateFromForms("apiKey");
      break;
    case "sign-out":
      signOut();
      break;
    case "refresh-auth":
      void refreshAuthContext();
      break;
    case "load-collection": {
      const resource = resourceByName(target.getAttribute("data-resource"));
      if (resource) void loadResourceCollection(resource);
      break;
    }
    case "load-by-id": {
      const resource = resourceByName(target.getAttribute("data-resource"));
      if (resource) void loadResourceById(resource);
      break;
    }
    case "new-payload": {
      const resource = resourceByName(target.getAttribute("data-resource"));
      if (resource) {
        const entry = ensureResourceState(resource.name);
        entry.selectedId = "";
        entry.selectedRecord = null;
        entry.payload = templateForResource(resource, null);
        render();
      }
      break;
    }
    case "select-row": {
      const resource = resourceByName(target.getAttribute("data-resource"));
      const id = target.getAttribute("data-id");
      if (resource && id) {
        const entry = ensureResourceState(resource.name);
        entry.selectedId = id;
        entry.selectedRecord = entry.rows.find((row) => String(row.id) === id) ?? null;
        entry.payload = templateForResource(resource, entry.selectedRecord);
        render();
      }
      break;
    }
    case "create-record": {
      const resource = resourceByName(target.getAttribute("data-resource"));
      if (resource) void createResourceRecord(resource);
      break;
    }
    case "update-record": {
      const resource = resourceByName(target.getAttribute("data-resource"));
      if (resource) void updateResourceRecord(resource);
      break;
    }
    case "delete-record": {
      const resource = resourceByName(target.getAttribute("data-resource"));
      if (resource) void deleteResourceRecord(resource);
      break;
    }
    case "invoke-function": {
      const fn = functionByName(target.getAttribute("data-function"));
      if (fn) void invokeFunction(fn);
      break;
    }
    default:
      break;
  }
});

function handleFieldInput(event) {
  const target = event.target;
  if (!(target instanceof HTMLInputElement || target instanceof HTMLTextAreaElement || target instanceof HTMLSelectElement)) {
    return;
  }
  const field = target.getAttribute("data-field");
  if (!field) return;
  switch (field) {
    case "login-identity":
      state.forms.loginIdentity = target.value;
      break;
    case "login-password":
      state.forms.loginPassword = target.value;
      break;
    case "register-identity":
      state.forms.registerIdentity = target.value;
      break;
    case "register-password":
      state.forms.registerPassword = target.value;
      break;
    case "register-extra":
      state.forms.registerExtra = target.value;
      break;
    case "manual-bearer":
      state.forms.manualBearerToken = target.value;
      break;
    case "manual-api-key":
      state.forms.manualApiKey = target.value;
      break;
    case "selected-org":
      state.auth.selectedOrgId = target.value;
      persistAuth();
      void refreshRole().then(() => render());
      break;
    case "resource-filters": {
      const entry = ensureResourceState(target.getAttribute("data-resource"));
      entry.filters = target.value;
      break;
    }
    case "resource-payload": {
      const entry = ensureResourceState(target.getAttribute("data-resource"));
      entry.payload = target.value;
      break;
    }
    case "resource-read-id": {
      const entry = ensureResourceState(target.getAttribute("data-resource"));
      entry.readId = target.value;
      break;
    }
    case "resource-limit": {
      const entry = ensureResourceState(target.getAttribute("data-resource"));
      entry.limit = target.value;
      break;
    }
    case "resource-offset": {
      const entry = ensureResourceState(target.getAttribute("data-resource"));
      entry.offset = target.value;
      break;
    }
    case "function-input": {
      const entry = ensureFunctionState(target.getAttribute("data-function"));
      entry.input = target.value;
      break;
    }
    default:
      break;
  }
}

root.addEventListener("input", handleFieldInput);
root.addEventListener("change", handleFieldInput);

async function boot() {
  try {
    readStoredAuth();
    const response = await fetch(MANIFEST_URL);
    if (!response.ok) throw new Error(`failed to load ${MANIFEST_URL} (${response.status})`);
    state.manifest = await response.json();
    state.page = { kind: "overview", name: null };
    if (isAuthenticated()) {
      state.auth.userId = decodeJwtUserId(state.auth.bearerToken);
      await refreshAuthContext();
    }
    render();
  } catch (error) {
    root.innerHTML = `
      <div class="ap-admin-shell">
        <main class="ap-nav-main">
          <div class="ap-main-inner">
            <div class="ap-notice error">
              ${escapeHtml(error.message || String(error))}
            </div>
          </div>
        </main>
      </div>
    `;
  }
}

boot();
