import { emitTable, parseResource, parseTable } from "./toml";
import type { AgentAiOverride, AgentEntry, AgentTool, Resource, Scope, TomlTable } from "./types";

function isTable(value: unknown): value is TomlTable {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function asString(value: unknown, fallback = ""): string {
  return typeof value === "string" ? value : fallback;
}

function asOptionalString(value: unknown): string | undefined {
  return typeof value === "string" ? value : undefined;
}

function asNumber(value: unknown): number | undefined {
  return typeof value === "number" && Number.isFinite(value) ? value : undefined;
}

function asBool(value: unknown, fallback = false): boolean {
  return typeof value === "boolean" ? value : fallback;
}

function asTable(value: unknown): TomlTable {
  return isTable(value) ? value : {};
}

function asScope(value: unknown): Scope {
  return value === "organization" ? "organization" : "global";
}

function fileStem(path: string): string {
  const base = path.slice(path.lastIndexOf("/") + 1);
  return base.endsWith(".toml") ? base.slice(0, -".toml".length) : base;
}

export function agentStorageBuiltinNames(agent: AgentEntry): string[] {
  if (!agent.storageEnabled) return [];
  return [`ai_${agent.name}_thread`, `ai_${agent.name}_message`];
}

export function isAgentStorageBuiltinName(name: string): boolean {
  return /^ai_.+_(thread|message)$/.test(name);
}

function generatedResourceSummary(agent: AgentEntry, kind: "thread" | "message"): string {
  return kind === "thread"
    ? `Generated persisted-history threads for the ${agent.name} agent. Create a resource file to extend the default fields or permissions.`
    : `Generated persisted-history messages for the ${agent.name} agent. Create a resource file to extend the default fields or permissions.`;
}

function generatedResourceToml(agent: AgentEntry, kind: "thread" | "message"): string {
  const scope = agent.scope;
  const history = agent.history;
  const threadName = `ai_${agent.name}_thread`;
  const messageName = `ai_${agent.name}_message`;

  if (kind === "thread") {
    return `
[resource]
name = "${threadName}"
scope = "${scope}"
timestamps = true

[admin]
visible = false

[permissions]
list   = "${history}"
read   = "${history}"
create = "private"
update = "private"
delete = "${history}"

[fields.owner_id]
type = "reference"
references = "user"
required = true
on_delete = "cascade"

[fields.title]
type = "string"
max_length = 200

[fields.summary]
type = "text"
hidden = true

[fields.summary_message_count]
type = "integer"
hidden = true

[fields.summary_characters]
type = "integer"
hidden = true

[fields.summary_updated_at]
type = "timestamp"
hidden = true
`;
  }

  return `
[resource]
name = "${messageName}"
scope = "${scope}"
timestamps = true

[admin]
visible = false

[permissions]
list   = "${history}"
read   = "${history}"
create = "private"
update = "private"
delete = "private"

[fields.thread_id]
type = "reference"
references = "${threadName}"
required = true
on_delete = "cascade"

[fields.owner_id]
type = "reference"
references = "user"
required = true
on_delete = "cascade"

[fields.role]
type = "string"
required = true

[fields.content]
type = "text"
required = true

[fields.reasoning]
type = "text"

[fields.tool_call_id]
type = "string"

[fields.tool_name]
type = "string"

[fields.tool_input]
type = "json"

[fields.tool_output]
type = "json"

[fields.provider]
type = "string"

[fields.model]
type = "string"

[fields.finish_reason]
type = "string"

[fields.input_tokens]
type = "integer"

[fields.output_tokens]
type = "integer"
`;
}

export function agentStorageBuiltinResource(agent: AgentEntry, kind: "thread" | "message"): Resource {
  return parseResource(generatedResourceToml(agent, kind));
}

export function agentStorageBuiltinEntries(agent: AgentEntry) {
  if (!agent.storageEnabled) return [];
  return (["thread", "message"] as const).map((kind) => {
    const resource = agentStorageBuiltinResource(agent, kind);
    return {
      name: resource.name,
      path: null,
      builtin: true,
      builtinSummary: generatedResourceSummary(agent, kind),
      resource,
    };
  });
}

export function parseAgent(text: string) {
  const table = parseTable(text);
  const meta = isTable(table.agent) ? table.agent : {};
  const storage = isTable(meta.storage) ? meta.storage : {};
  const ai = isTable(table.ai) ? table.ai : {};
  const permissions = isTable(table.permissions) ? table.permissions : {};
  const rawTools = Array.isArray(table.tools) ? table.tools : [];

  const name = asString(meta.name).trim();
  if (!name) throw new Error("[agent] name is required");

  const overrideFromTable: AgentAiOverride = {
    provider: asOptionalString(ai.provider),
    endpoint: asOptionalString(ai.endpoint),
    model: asOptionalString(ai.model),
    apiKey: asOptionalString(ai.api_key),
    system: asOptionalString(ai.system),
    temperature: asNumber(ai.temperature),
    maxTokens: asNumber(ai.max_tokens),
    timeoutSecs: asNumber(ai.timeout_secs),
    reasoning: typeof ai.reasoning === "boolean" ? ai.reasoning : undefined,
    thinking: typeof ai.thinking === "boolean" ? ai.thinking : undefined,
    reasoningFormat: asOptionalString(ai.reasoning_format),
  };
  const legacyModel = asString(meta.model).trim() || undefined;
  const legacyTemperature = asNumber(meta.temperature);
  const legacyMaxTokens = asNumber(meta.max_tokens);
  const aiOverride =
    Object.values(overrideFromTable).some((value) => value !== undefined) ||
    legacyModel !== undefined ||
    legacyTemperature !== undefined ||
    legacyMaxTokens !== undefined
      ? {
          ...overrideFromTable,
          model: overrideFromTable.model ?? legacyModel,
          temperature: overrideFromTable.temperature ?? legacyTemperature,
          maxTokens: overrideFromTable.maxTokens ?? legacyMaxTokens,
        }
      : null;

  return {
    name,
    description: asString(meta.description),
    system: asString(meta.system),
    scope: asScope(meta.scope),
    storageEnabled: asBool(storage.enabled),
    summaryAfterCharacters: asNumber(storage.summary_after_characters),
    chat: asString(permissions.chat, "authenticated"),
    history: asString(permissions.history, "owner"),
    aiOverride,
    tools: rawTools.filter(isTable).map((raw): AgentTool => ({
      name: asString(raw.name).trim(),
      description: asString(raw.description),
      inputSchema: asTable(raw.input_schema),
      outputSchema: asTable(raw.output_schema),
      function: asString(raw.function).trim(),
    })),
  };
}

export function summarizeAgent(path: string, text: string): AgentEntry {
  const parsed = parseAgent(text);
  return {
    path,
    name: parsed.name,
    description: parsed.description,
    system: parsed.system,
    scope: parsed.scope,
    storageEnabled: parsed.storageEnabled,
    summaryAfterCharacters: parsed.summaryAfterCharacters,
    chat: parsed.chat,
    history: parsed.history,
    aiOverride: parsed.aiOverride,
    tools: parsed.tools,
    fallbackName: fileStem(path),
  };
}

export function scaffoldAgent(name: string, storageEnabled: boolean): string {
  return emitAgent({
    path: `agents/${name}.toml`,
    name,
    fallbackName: name,
    description: "What this agent helps with.",
    system: "Set the standing instructions this agent should always follow.",
    scope: "global",
    storageEnabled,
    summaryAfterCharacters: undefined,
    chat: "authenticated",
    history: storageEnabled ? "owner" : "private",
    aiOverride: null,
    tools: [],
  });
}

export function fallbackAgentName(path: string): string {
  return fileStem(path);
}

export function emitAgent(agent: AgentEntry): string {
  const table: TomlTable = {
    agent: {
      name: agent.name,
      storage: {
        enabled: agent.storageEnabled,
      },
    },
    permissions: {
      chat: agent.chat,
      history: agent.history,
    },
  };

  const meta = table.agent as TomlTable;
  if (agent.description) meta.description = agent.description;
  if (agent.system) meta.system = agent.system;
  if (agent.scope !== "global") meta.scope = agent.scope;
  const storage = meta.storage as TomlTable;
  if (agent.summaryAfterCharacters !== undefined) {
    storage.summary_after_characters = agent.summaryAfterCharacters;
  }

  if (agent.aiOverride) {
    const ai: TomlTable = {};
    if (agent.aiOverride.provider !== undefined) ai.provider = agent.aiOverride.provider;
    if (agent.aiOverride.endpoint !== undefined) ai.endpoint = agent.aiOverride.endpoint;
    if (agent.aiOverride.model !== undefined) ai.model = agent.aiOverride.model;
    if (agent.aiOverride.apiKey !== undefined) ai.api_key = agent.aiOverride.apiKey;
    if (agent.aiOverride.system !== undefined) ai.system = agent.aiOverride.system;
    if (agent.aiOverride.temperature !== undefined) ai.temperature = agent.aiOverride.temperature;
    if (agent.aiOverride.maxTokens !== undefined) ai.max_tokens = agent.aiOverride.maxTokens;
    if (agent.aiOverride.timeoutSecs !== undefined) ai.timeout_secs = agent.aiOverride.timeoutSecs;
    if (agent.aiOverride.reasoning !== undefined) ai.reasoning = agent.aiOverride.reasoning;
    if (agent.aiOverride.thinking !== undefined) ai.thinking = agent.aiOverride.thinking;
    if (agent.aiOverride.reasoningFormat !== undefined) {
      ai.reasoning_format = agent.aiOverride.reasoningFormat;
    }
    table.ai = ai;
  }

  if (agent.tools.length) {
    table.tools = agent.tools.map((tool) => ({
      name: tool.name,
      description: tool.description,
      input_schema: tool.inputSchema,
      output_schema: tool.outputSchema,
      function: tool.function,
    }));
  }

  return emitTable(table);
}

function isRoleAccess(value: string): boolean {
  return value.startsWith("role:");
}

export function validateAgent(agent: AgentEntry, knownNames: string[]): string[] {
  const issues: string[] = [];
  if (!agent.name.trim()) issues.push("[agent] name is required.");
  if (knownNames.filter((name) => name === agent.name).length > 1) {
    issues.push(`Another agent already uses the name \`${agent.name}\`.`);
  }
  if (agent.chat === "owner") {
    issues.push('Chat access cannot be "owner" for an agent.');
  }
  if (agent.storageEnabled && agent.chat === "public") {
    issues.push("A stored agent cannot be public because persisted history needs an authenticated owner.");
  }
  if (agent.aiOverride?.provider === "custom" && !agent.aiOverride.endpoint?.trim()) {
    issues.push("A custom AI override needs an endpoint.");
  }
  if (agent.summaryAfterCharacters !== undefined && (!Number.isInteger(agent.summaryAfterCharacters) || agent.summaryAfterCharacters < 1)) {
    issues.push("storage.summary_after_characters must be a positive integer.");
  }
  if (agent.storageEnabled && agent.scope === "global" && (agent.chat === "member" || isRoleAccess(agent.chat))) {
    issues.push('A stored global agent cannot use "member" or "role:*" chat access; use organization scope instead.');
  }
  if (
    agent.storageEnabled &&
    agent.scope === "global" &&
    (agent.history === "member" || isRoleAccess(agent.history))
  ) {
    issues.push('A stored global agent cannot use "member" or "role:*" history access; use organization scope instead.');
  }
  for (const tool of agent.tools) {
    if (!tool.name.trim()) issues.push("Every agent tool needs a name.");
    if (tool.name && !/^[A-Za-z0-9_-]+$/.test(tool.name)) {
      issues.push(`Tool \`${tool.name}\` may only contain letters, digits, _ or -.`);
    }
    if (!tool.function.trim()) issues.push(`Tool \`${tool.name || "unnamed"}\` needs a function.`);
  }
  return issues;
}
