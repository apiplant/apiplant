//! The declarative agent model.
//!
//! An *agent* is one `agents/<name>.toml` file. It names a configured AI chat
//! surface — prompt, access policy and whether its history is persisted. When
//! storage is enabled the app gets two generated resources, one for threads and
//! one for messages, so migrations and read-only history browsing use the same
//! machinery as every other table.

use crate::config::AiConfig;
use crate::schema::{titleize, Access, Resource, Scope};
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::path::Path;

/// One configured chat agent.
#[derive(Debug, Clone, Deserialize)]
pub struct Agent {
    #[serde(rename = "agent")]
    pub meta: AgentMeta,
    /// Optional per-agent AI provider overrides. When absent, the app's
    /// global `[ai]` configuration answers this agent too.
    #[serde(default)]
    pub ai: Option<AgentAiOverride>,
    #[serde(default)]
    pub tools: Vec<AgentTool>,
    #[serde(default)]
    pub permissions: AgentPermissions,
}

/// The `[agent]` section.
#[derive(Debug, Clone, Deserialize)]
pub struct AgentMeta {
    pub name: String,
    /// Human description, surfaced in docs and discovery responses.
    #[serde(default)]
    pub description: String,
    /// System prompt this agent always answers under.
    #[serde(default)]
    pub system: String,
    /// Legacy per-agent model override.
    ///
    /// Kept for compatibility with older agent files. New overrides belong in
    /// the top-level `[ai]` table of the agent file, which can also override
    /// the provider, endpoint, key and timeout.
    #[serde(default)]
    pub model: Option<String>,
    /// Legacy per-agent temperature override.
    #[serde(default)]
    pub temperature: Option<f32>,
    /// Legacy per-agent max_tokens override.
    #[serde(default)]
    pub max_tokens: Option<u32>,
    /// Whether persisted threads/messages belong to an organisation or are
    /// shared deployment-wide. Only meaningful when storage is enabled.
    #[serde(default = "default_scope")]
    pub scope: Scope,
    #[serde(default)]
    pub storage: AgentStorage,
}

fn default_scope() -> Scope {
    Scope::Global
}

/// Optional persisted history for an agent.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct AgentStorage {
    /// Keep threads and messages in generated resources.
    pub enabled: bool,
    /// Refresh the rolling summary once the unsummarised tail crosses this
    /// many characters.
    pub summary_after_characters: Option<u32>,
}

impl AgentStorage {
    pub fn summary_after_characters(&self, default: usize) -> usize {
        self.summary_after_characters
            .unwrap_or(default as u32)
            .max(1) as usize
    }
}

/// Optional per-agent AI client overrides, from a top-level `[ai]` table in
/// the agent file.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct AgentAiOverride {
    pub provider: Option<String>,
    pub endpoint: Option<String>,
    pub model: Option<String>,
    pub api_key: Option<String>,
    pub system: Option<String>,
    pub max_tokens: Option<u32>,
    pub temperature: Option<f32>,
    pub reasoning: Option<bool>,
    pub thinking: Option<bool>,
    pub timeout_secs: Option<u64>,
}

/// One function-backed tool an agent may ask the model to call.
#[derive(Debug, Clone, Deserialize)]
pub struct AgentTool {
    /// Name exposed to the model.
    pub name: String,
    /// Human description passed to the model.
    #[serde(default)]
    pub description: String,
    /// JSON Schema for arguments the model must provide.
    #[serde(default = "default_tool_input_schema")]
    pub input_schema: Value,
    /// JSON Schema the function returns. Stored for docs/UI and validation by
    /// convention; the function itself is still the authority at runtime.
    #[serde(default = "default_tool_output_schema")]
    pub output_schema: Value,
    /// Loaded function name to invoke when this tool is called.
    pub function: String,
}

fn default_tool_input_schema() -> Value {
    json!({ "type": "object", "properties": {} })
}

fn default_tool_output_schema() -> Value {
    json!({})
}

/// Per-agent access policy.
#[derive(Debug, Clone, Deserialize)]
#[serde(from = "AgentPermissionsRaw")]
pub struct AgentPermissions {
    /// Who may chat with this agent.
    pub chat: Access,
    /// Who may read stored history, through the generated resources.
    pub history: Access,
    /// Who may delete stored threads from history.
    pub delete_history: Access,
}

impl Default for AgentPermissions {
    fn default() -> Self {
        AgentPermissions {
            chat: Access::Authenticated,
            history: Access::Owner,
            delete_history: Access::Owner,
        }
    }
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
struct AgentPermissionsRaw {
    chat: Option<String>,
    history: Option<String>,
    delete_history: Option<String>,
}

impl From<AgentPermissionsRaw> for AgentPermissions {
    fn from(raw: AgentPermissionsRaw) -> Self {
        let default = AgentPermissions::default();
        AgentPermissions {
            chat: raw
                .chat
                .map(|value| Access::parse(&value))
                .unwrap_or(default.chat),
            history: raw
                .history
                .map(|value| Access::parse(&value))
                .unwrap_or(default.history.clone()),
            delete_history: raw
                .delete_history
                .map(|value| Access::parse(&value))
                .unwrap_or(default.history),
        }
    }
}

impl Agent {
    /// Load and validate a single agent file.
    pub fn load(path: &Path) -> crate::Result<Self> {
        let text = std::fs::read_to_string(path).map_err(|e| crate::Error::Io {
            path: path.to_path_buf(),
            source: e,
        })?;
        let source = path.file_name().unwrap_or_default().to_string_lossy();
        let agent: Agent =
            crate::env::parse_toml(&text, &source).map_err(|e| crate::Error::Toml {
                path: path.to_path_buf(),
                source: e,
            })?;
        agent.validate()?;
        Ok(agent)
    }

    pub fn validate(&self) -> crate::Result<()> {
        if self.meta.name.trim().is_empty() {
            return Err(crate::Error::Schema {
                resource: "agent".to_string(),
                message: "[agent] name cannot be empty".to_string(),
            });
        }
        if matches!(self.permissions.chat, Access::Owner) {
            return Err(crate::Error::Schema {
                resource: self.meta.name.clone(),
                message: "[permissions] chat = \"owner\" is not valid for an agent".to_string(),
            });
        }
        if self.meta.storage.enabled && matches!(self.permissions.chat, Access::Public) {
            return Err(crate::Error::Schema {
                resource: self.meta.name.clone(),
                message:
                    "a stored agent cannot be public because persisted history needs an authenticated owner"
                        .to_string(),
            });
        }
        if self.meta.storage.enabled && matches!(self.permissions.delete_history, Access::Public) {
            return Err(crate::Error::Schema {
                resource: self.meta.name.clone(),
                message: "a stored agent cannot allow public history deletion".to_string(),
            });
        }
        if self.meta.storage.enabled
            && self.meta.scope == Scope::Global
            && matches!(self.permissions.chat, Access::Member | Access::Role(_))
        {
            return Err(crate::Error::Schema {
                resource: self.meta.name.clone(),
                message:
                    "a stored global agent cannot use `member` or `role:` chat access; use scope = \"organization\""
                        .to_string(),
            });
        }
        if self.meta.storage.enabled
            && self.meta.scope == Scope::Global
            && matches!(self.permissions.history, Access::Member | Access::Role(_))
        {
            return Err(crate::Error::Schema {
                resource: self.meta.name.clone(),
                message:
                    "a stored global agent cannot use `member` or `role:` history access; use scope = \"organization\""
                        .to_string(),
            });
        }
        if self.meta.storage.enabled
            && self.meta.scope == Scope::Global
            && matches!(
                self.permissions.delete_history,
                Access::Member | Access::Role(_)
            )
        {
            return Err(crate::Error::Schema {
                resource: self.meta.name.clone(),
                message:
                    "a stored global agent cannot use `member` or `role:` delete_history access; use scope = \"organization\""
                        .to_string(),
            });
        }
        for tool in &self.tools {
            if tool.name.trim().is_empty() {
                return Err(crate::Error::Schema {
                    resource: self.meta.name.clone(),
                    message: "agent tools must have a non-empty name".to_string(),
                });
            }
            if !tool
                .name
                .chars()
                .all(|ch| ch.is_ascii_alphanumeric() || ch == '_' || ch == '-')
            {
                return Err(crate::Error::Schema {
                    resource: self.meta.name.clone(),
                    message: format!(
                        "agent tool `{}` may only contain letters, digits, `_` or `-`",
                        tool.name
                    ),
                });
            }
            if tool.function.trim().is_empty() {
                return Err(crate::Error::Schema {
                    resource: self.meta.name.clone(),
                    message: format!("agent tool `{}` names an empty function", tool.name),
                });
            }
            if !tool.input_schema.is_object() {
                return Err(crate::Error::Schema {
                    resource: self.meta.name.clone(),
                    message: format!(
                        "agent tool `{}` input_schema must be a JSON object",
                        tool.name
                    ),
                });
            }
            if !tool.output_schema.is_object() {
                return Err(crate::Error::Schema {
                    resource: self.meta.name.clone(),
                    message: format!(
                        "agent tool `{}` output_schema must be a JSON object",
                        tool.name
                    ),
                });
            }
        }
        Ok(())
    }

    /// The app's `[ai]` configuration, with this agent's own overrides applied.
    ///
    /// The legacy `[agent] model|temperature|max_tokens` keys still work and
    /// win over the fallback values they historically replaced.
    pub fn merged_ai_config(&self, base: &AiConfig) -> AiConfig {
        let mut merged = base.clone();
        if let Some(ai) = &self.ai {
            if let Some(provider) = &ai.provider {
                merged.provider = provider.clone();
            }
            if let Some(endpoint) = &ai.endpoint {
                merged.endpoint = endpoint.clone();
            }
            if let Some(model) = &ai.model {
                merged.model = model.clone();
            }
            if let Some(api_key) = &ai.api_key {
                merged.api_key = api_key.clone();
            }
            if let Some(system) = &ai.system {
                merged.system = system.clone();
            }
            if let Some(max_tokens) = ai.max_tokens {
                merged.max_tokens = max_tokens;
            }
            if let Some(temperature) = ai.temperature {
                merged.temperature = temperature;
            }
            if let Some(reasoning) = ai.reasoning {
                merged.reasoning = reasoning;
            }
            if let Some(thinking) = ai.thinking {
                merged.thinking = Some(thinking);
            }
            if let Some(timeout_secs) = ai.timeout_secs {
                merged.timeout_secs = timeout_secs;
            }
        }
        if let Some(model) = &self.meta.model {
            merged.model = model.clone();
        }
        if let Some(temperature) = self.meta.temperature {
            merged.temperature = temperature;
        }
        if let Some(max_tokens) = self.meta.max_tokens {
            merged.max_tokens = max_tokens;
        }
        merged
    }

    /// Human label for a configured agent.
    pub fn label(&self) -> String {
        titleize(&self.meta.name)
    }

    /// The generated resource that stores conversation threads, if history is on.
    pub fn thread_resource_name(&self) -> String {
        format!("ai_{}_thread", self.meta.name)
    }

    /// The generated resource that stores persisted messages, if history is on.
    pub fn message_resource_name(&self) -> String {
        format!("ai_{}_message", self.meta.name)
    }

    /// The two generated resources backing persisted history.
    pub fn storage_resources(&self) -> crate::Result<BTreeMap<String, Resource>> {
        let mut resources = BTreeMap::new();
        if !self.meta.storage.enabled {
            return Ok(resources);
        }

        let scope = match self.meta.scope {
            Scope::Global => "global",
            Scope::Organization => "organization",
        };
        let history = self.permissions.history.as_string();
        let delete_history = self.permissions.delete_history.as_string();
        let label = self.label();
        let thread_name = self.thread_resource_name();
        let message_name = self.message_resource_name();

        let thread = format!(
            r#"
[resource]
name = "{thread_name}"
scope = "{scope}"
timestamps = true

[admin]
label = "{label} thread"
plural = "{label} threads"
visible = false

[permissions]
list   = "{history}"
read   = "{history}"
create = "private"
update = "private"
delete = "{delete_history}"

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
"#
        );
        let message = format!(
            r#"
[resource]
name = "{message_name}"
scope = "{scope}"
timestamps = true

[admin]
label = "{label} message"
plural = "{label} messages"
visible = false

[permissions]
list   = "{history}"
read   = "{history}"
create = "private"
update = "private"
delete = "private"

[fields.thread_id]
type = "reference"
references = "{thread_name}"
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
"#
        );

        for src in [thread, message] {
            let resource: Resource = toml::from_str(&src).map_err(|source| crate::Error::Toml {
                path: Path::new("<generated agent resource>").to_path_buf(),
                source,
            })?;
            resource.validate()?;
            resources.insert(resource.meta.name.clone(), resource);
        }

        Ok(resources)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(src: &str) -> Agent {
        let agent: Agent = toml::from_str(src).unwrap();
        agent.validate().unwrap();
        agent
    }

    #[test]
    fn stored_agents_generate_thread_and_message_resources() {
        let agent = parse(
            r#"
[agent]
name = "coach"
storage.enabled = true

[permissions]
chat = "authenticated"
history = "owner"
"#,
        );

        let resources = agent.storage_resources().unwrap();
        assert!(resources.contains_key("ai_coach_thread"));
        assert!(resources.contains_key("ai_coach_message"));
        assert!(resources["ai_coach_thread"].fields["summary"].hidden);
        assert!(resources["ai_coach_thread"].fields["summary_updated_at"].hidden);
        assert_eq!(
            resources["ai_coach_thread"].permissions.delete.as_string(),
            "owner"
        );
    }

    #[test]
    fn stored_agents_may_override_history_deletion_access() {
        let agent = parse(
            r#"
[agent]
name = "coach"
scope = "organization"
storage.enabled = true

[permissions]
chat = "authenticated"
history = "owner"
delete_history = "role:admin"
"#,
        );

        let resources = agent.storage_resources().unwrap();
        assert_eq!(
            resources["ai_coach_thread"].permissions.delete.as_string(),
            "role:admin"
        );
    }

    #[test]
    fn stored_agents_may_override_summary_thresholds() {
        let agent = parse(
            r#"
[agent]
name = "coach"
storage.enabled = true
storage.summary_after_characters = 1200
"#,
        );

        assert_eq!(agent.meta.storage.summary_after_characters(12_000), 1200);
    }

    #[test]
    fn a_stored_agent_cannot_be_public() {
        let agent: Agent = toml::from_str(
            r#"
[agent]
name = "coach"
storage.enabled = true

[permissions]
chat = "public"
"#,
        )
        .unwrap();
        assert!(agent.validate().is_err());
    }

    #[test]
    fn an_agent_can_override_the_app_ai_config() {
        let agent = parse(
            r#"
[agent]
name = "coach"

[ai]
provider = "custom"
endpoint = "http://localhost:8080"
api_key = ""
model = "local"
temperature = 0.2
timeout_secs = 15
reasoning = true
thinking = false
"#,
        );

        let base = AiConfig {
            provider: "openai".to_string(),
            endpoint: String::new(),
            model: "gpt-4o-mini".to_string(),
            api_key: "$OPENAI_API_KEY".to_string(),
            system: "base".to_string(),
            max_tokens: 2048,
            temperature: -1.0,
            access: "authenticated".to_string(),
            reasoning: false,
            thinking: None,
            timeout_secs: 300,
        };

        let merged = agent.merged_ai_config(&base);
        assert_eq!(merged.provider, "custom");
        assert_eq!(merged.endpoint, "http://localhost:8080");
        assert_eq!(merged.model, "local");
        assert_eq!(merged.api_key, "");
        assert_eq!(merged.timeout_secs, 15);
        assert_eq!(merged.access, "authenticated");
        assert!(merged.reasoning);
        // Showing the thinking and asking for it are separate decisions.
        assert_eq!(merged.thinking, Some(false));
    }
}
