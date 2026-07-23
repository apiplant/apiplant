//! The declarative resource model.
//!
//! A *resource* is one `models/<name>.toml` file. It declares fields and a
//! per-action permission policy; the framework turns it into a Postgres table
//! and a set of RESTful CRUD endpoints. Users, roles and api-keys are ordinary
//! resources that ship with built-in defaults (see [`crate::defaults`]).

use serde::Deserialize;
use std::collections::BTreeMap;
use std::path::Path;

/// A fully-parsed resource definition.
#[derive(Debug, Clone, Deserialize)]
pub struct Resource {
    #[serde(rename = "resource")]
    pub meta: ResourceMeta,
    /// Ordered map so generated columns/endpoints are deterministic.
    #[serde(default)]
    pub fields: BTreeMap<String, Field>,
    #[serde(default)]
    pub permissions: Permissions,
    /// Optional auth configuration; only meaningful on the `user` resource.
    #[serde(default)]
    pub auth: Option<AuthSpec>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ResourceMeta {
    pub name: String,
    /// Physical table name; defaults to `apiplant_<name>` when omitted.
    pub table: Option<String>,
    /// Add `created_at` / `updated_at` columns (default true).
    #[serde(default = "yes")]
    pub timestamps: bool,
    /// Column used for `owner` permission checks (default `owner_id`).
    #[serde(default = "default_owner_field")]
    pub owner_field: String,
    /// Tenancy: `organization` (default — rows belong to an org and are isolated)
    /// or `global` (shared across the whole deployment).
    #[serde(default = "default_scope")]
    pub scope: Scope,
}

fn yes() -> bool {
    true
}
fn default_owner_field() -> String {
    "owner_id".to_string()
}
fn default_scope() -> Scope {
    Scope::Organization
}

impl Resource {
    /// Physical table name.
    pub fn table_name(&self) -> String {
        self.meta
            .table
            .clone()
            .unwrap_or_else(|| format!("apiplant_{}", self.meta.name))
    }

    /// Validate internal consistency (called after loading).
    pub fn validate(&self) -> crate::Result<()> {
        for (fname, field) in &self.fields {
            if fname == "id" {
                return Err(crate::Error::Schema {
                    resource: self.meta.name.clone(),
                    message: "`id` is reserved and added automatically".into(),
                });
            }
            if field.ty == FieldType::Reference && field.references.is_none() {
                return Err(crate::Error::Schema {
                    resource: self.meta.name.clone(),
                    message: format!("field `{fname}` is a reference without `references`"),
                });
            }
        }
        Ok(())
    }

    /// All `belongs_to` references declared by this resource's fields.
    pub fn references(&self) -> Vec<Reference> {
        self.fields
            .iter()
            .filter_map(|(name, field)| {
                if field.ty != FieldType::Reference {
                    return None;
                }
                let target = field.references.clone()?;
                Some(Reference {
                    field: name.clone(),
                    target,
                    relation: relation_name(name).to_string(),
                    on_delete: field.on_delete.unwrap_or(OnDelete::Restrict),
                    required: field.required,
                })
            })
            .collect()
    }

    /// Find the reference exposed under a given relation name (`"owner"`).
    pub fn reference_by_relation(&self, relation: &str) -> Option<Reference> {
        self.references().into_iter().find(|r| r.relation == relation)
    }

    /// Whether this resource is isolated per organisation (the default).
    pub fn is_org_scoped(&self) -> bool {
        self.meta.scope == Scope::Organization
    }

    /// The column that carries this resource's organisation, if any:
    /// `organization_id` for org-scoped resources, `id` for the `organization`
    /// resource itself (its rows *are* organisations), else `None`.
    pub fn org_column(&self) -> Option<&'static str> {
        if self.is_org_scoped() {
            Some("organization_id")
        } else if self.meta.name == "organization" {
            Some("id")
        } else {
            None
        }
    }

    /// Load and validate a single resource file.
    pub fn load(path: &Path) -> crate::Result<Self> {
        let text = std::fs::read_to_string(path).map_err(|e| crate::Error::Io {
            path: path.to_path_buf(),
            source: e,
        })?;
        let resource: Resource = toml::from_str(&text).map_err(|e| crate::Error::Toml {
            path: path.to_path_buf(),
            source: e,
        })?;
        resource.validate()?;
        Ok(resource)
    }
}

/// One column in a resource.
#[derive(Debug, Clone, Deserialize)]
pub struct Field {
    #[serde(rename = "type")]
    pub ty: FieldType,
    /// Target resource name when `ty == Reference`.
    #[serde(default)]
    pub references: Option<String>,
    #[serde(default)]
    pub required: bool,
    #[serde(default)]
    pub unique: bool,
    /// Exclude from API responses (e.g. password hashes).
    #[serde(default)]
    pub hidden: bool,
    /// Optional default rendered as a SQL literal.
    #[serde(default)]
    pub default: Option<serde_json::Value>,
    pub max_length: Option<u32>,
    /// For `reference` fields: what happens to this row when the referenced row
    /// is deleted. Defaults to [`OnDelete::Restrict`] (safe: blocks orphaning).
    #[serde(default)]
    pub on_delete: Option<OnDelete>,
}

/// Supported column types.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FieldType {
    String,
    Text,
    Integer,
    BigInt,
    Float,
    Boolean,
    Uuid,
    Timestamp,
    Json,
    /// Foreign key; see [`Field::references`].
    Reference,
}

/// Referential action applied by a foreign key when the parent row is deleted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OnDelete {
    /// Block the parent delete while children exist (default).
    Restrict,
    /// Null out this reference (requires a nullable column).
    SetNull,
    /// Delete this row too.
    Cascade,
    /// No referential action.
    NoAction,
}

impl OnDelete {
    pub fn to_sql(self) -> &'static str {
        match self {
            OnDelete::Restrict => "RESTRICT",
            OnDelete::SetNull => "SET NULL",
            OnDelete::Cascade => "CASCADE",
            OnDelete::NoAction => "NO ACTION",
        }
    }
}

/// A resolved `belongs_to` edge: the referencing field, its target resource, and
/// the JSON key it expands under (`owner_id` → `owner`).
#[derive(Debug, Clone)]
pub struct Reference {
    pub field: String,
    pub target: String,
    pub relation: String,
    pub on_delete: OnDelete,
    pub required: bool,
}

/// The relation name a reference field expands under: `owner_id` → `owner`,
/// otherwise the field name unchanged.
pub fn relation_name(field: &str) -> &str {
    field.strip_suffix("_id").unwrap_or(field)
}

/// Whether a resource is isolated per organisation (the default) or shared
/// across the whole deployment.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Scope {
    /// Rows belong to an organisation; every request is scoped to the caller's
    /// active organisation and `organization_id` is enforced automatically.
    Organization,
    /// Not tenant-scoped — shared by everyone, governed only by `[permissions]`.
    Global,
}

/// Access policy for a single action on a resource.
///
/// On an organisation-scoped resource, org membership is always required and
/// queries are already filtered to the caller's active organisation; these
/// levels then decide *who among the members* may act. `Role` is an
/// **organisation** role (from the caller's membership), not a global one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Access {
    /// No auth required. Only meaningful on `global` resources; on an
    /// org-scoped resource it is treated like `member`.
    Public,
    /// Any authenticated principal (⇒ any member, on an org-scoped resource).
    Authenticated,
    /// Any member of the (active) organisation.
    Member,
    /// A member holding the named role **within the organisation**.
    Role(String),
    /// The principal must own the row (owner_field == principal id).
    Owner,
    /// Never exposed.
    Private,
}

impl Access {
    /// Parse the string form used in TOML (`"public"`, `"member"`, `"role:admin"`, …).
    pub fn parse(s: &str) -> Access {
        match s {
            "public" => Access::Public,
            "authenticated" => Access::Authenticated,
            "member" => Access::Member,
            "owner" => Access::Owner,
            "private" => Access::Private,
            other => other
                .strip_prefix("role:")
                .map(|role| Access::Role(role.to_string()))
                .unwrap_or(Access::Private),
        }
    }
}

/// Per-action permissions. Strings in TOML are parsed via [`Access::parse`].
#[derive(Debug, Clone, Deserialize)]
#[serde(from = "PermissionsRaw")]
pub struct Permissions {
    pub list: Access,
    pub read: Access,
    pub create: Access,
    pub update: Access,
    pub delete: Access,
}

impl Default for Permissions {
    fn default() -> Self {
        // Multitenant-by-default: every action is limited to members of the
        // caller's organisation. On a `global` resource, `member` behaves like
        // `authenticated` (there is no org to belong to).
        Permissions {
            list: Access::Member,
            read: Access::Member,
            create: Access::Member,
            update: Access::Member,
            delete: Access::Member,
        }
    }
}

#[derive(Deserialize)]
struct PermissionsRaw {
    list: Option<String>,
    read: Option<String>,
    create: Option<String>,
    update: Option<String>,
    delete: Option<String>,
}

impl From<PermissionsRaw> for Permissions {
    fn from(r: PermissionsRaw) -> Self {
        let d = Permissions::default();
        Permissions {
            list: r.list.map(|s| Access::parse(&s)).unwrap_or(d.list),
            read: r.read.map(|s| Access::parse(&s)).unwrap_or(d.read),
            create: r.create.map(|s| Access::parse(&s)).unwrap_or(d.create),
            update: r.update.map(|s| Access::parse(&s)).unwrap_or(d.update),
            delete: r.delete.map(|s| Access::parse(&s)).unwrap_or(d.delete),
        }
    }
}

/// Auth configuration carried in a `[auth]` section on the `user` resource.
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct AuthSpec {
    /// Field used as the login identifier.
    pub identity_field: String,
    /// Field holding the password hash.
    pub password_field: String,
    /// Enabled OAuth providers, e.g. `["google", "facebook"]`.
    pub oauth_providers: Vec<String>,
}

impl Default for AuthSpec {
    fn default() -> Self {
        AuthSpec {
            identity_field: "email".to_string(),
            password_field: "password_hash".to_string(),
            oauth_providers: Vec::new(),
        }
    }
}
