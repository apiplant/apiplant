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
    /// Named functions to run around each CRUD operation.
    #[serde(default)]
    pub hooks: Hooks,
    /// Optional auth configuration; only meaningful on the `user` resource.
    #[serde(default)]
    pub auth: Option<AuthSpec>,
    /// How (and whether) this resource appears in the generated admin dashboard.
    #[serde(default)]
    pub admin: ResourceAdmin,
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

    /// The function bound to a lifecycle event, if any.
    pub fn hook(&self, event: HookEvent) -> Option<&str> {
        self.hooks.get(event)
    }

    /// Validate internal consistency (called after loading).
    pub fn validate(&self) -> crate::Result<()> {
        for (event, function) in self.hooks.iter() {
            if function.trim().is_empty() {
                return Err(crate::Error::Schema {
                    resource: self.meta.name.clone(),
                    message: format!("hook `{}` names an empty function", event.as_str()),
                });
            }
        }
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
            if field.admin.format != ContentFormat::Plain
                && !matches!(field.ty, FieldType::Text | FieldType::String)
            {
                return Err(crate::Error::Schema {
                    resource: self.meta.name.clone(),
                    message: format!(
                        "field `{fname}` sets [admin] format = \"{}\" but is not a text field",
                        field.admin.format.as_str()
                    ),
                });
            }
        }
        // `[admin]` is presentation, so a typo here is silent rather than
        // dangerous — but it is still a typo, and naming a column that does not
        // exist is never what anyone meant.
        for column in &self.admin.columns {
            if !self.fields.contains_key(column) && column != "id" {
                return Err(crate::Error::Schema {
                    resource: self.meta.name.clone(),
                    message: format!("[admin] columns names unknown field `{column}`"),
                });
            }
        }
        for (key, declared) in [
            ("display_field", &self.admin.display_field),
            ("search_field", &self.admin.search_field),
        ] {
            if let Some(field) = declared {
                if !self.fields.contains_key(field) {
                    return Err(crate::Error::Schema {
                        resource: self.meta.name.clone(),
                        message: format!("[admin] {key} names unknown field `{field}`"),
                    });
                }
            }
        }
        Ok(())
    }

    /// Human label for a single record, e.g. `"Purchase order"`.
    pub fn admin_label(&self) -> String {
        self.admin
            .label
            .clone()
            .unwrap_or_else(|| titleize(&self.meta.name))
    }

    /// Human label for the collection, e.g. `"Purchase orders"`.
    pub fn admin_plural(&self) -> String {
        self.admin
            .plural
            .clone()
            .unwrap_or_else(|| pluralize(&self.admin_label()))
    }

    /// The field whose value names a record in tables, pickers and headings.
    ///
    /// An explicit `display_field` wins; otherwise the first conventionally
    /// named field (`name`, `title`, …), then the first plain string field, and
    /// finally `None` — at which point the dashboard falls back to the id.
    pub fn admin_display_field(&self) -> Option<String> {
        if let Some(declared) = &self.admin.display_field {
            if self.fields.contains_key(declared) {
                return Some(declared.clone());
            }
        }
        const PREFERRED: [&str; 7] = ["name", "title", "label", "slug", "code", "number", "email"];
        for candidate in PREFERRED {
            if let Some(field) = self.fields.get(candidate) {
                if !field.hidden && matches!(field.ty, FieldType::String | FieldType::Text) {
                    return Some(candidate.to_string());
                }
            }
        }
        self.fields
            .iter()
            .find(|(_, field)| !field.hidden && field.ty == FieldType::String)
            .map(|(name, _)| name.clone())
    }

    /// The field the dashboard's list search box filters on.
    pub fn admin_search_field(&self) -> Option<String> {
        match &self.admin.search_field {
            Some(declared) if self.fields.contains_key(declared) => Some(declared.clone()),
            Some(_) | None => self.admin_display_field(),
        }
    }

    /// The columns the list table shows, in order.
    ///
    /// Declared `columns` win. Otherwise: the display field first, then up to
    /// four more dashboard-visible fields, skipping `json`/`text` blobs (which
    /// never read well in a cell) and the tenancy column.
    pub fn admin_columns(&self) -> Vec<String> {
        let declared: Vec<String> = self
            .admin
            .columns
            .iter()
            .filter(|name| self.fields.contains_key(*name))
            .cloned()
            .collect();
        if !declared.is_empty() {
            return declared;
        }

        let display = self.admin_display_field();
        let mut columns: Vec<String> = display.iter().cloned().collect();
        for (name, field) in &self.fields {
            if columns.len() >= 5 {
                break;
            }
            let skip = field.hidden
                || !field.admin.visible
                || Some(name) == display.as_ref()
                || name == "organization_id"
                || matches!(field.ty, FieldType::Json | FieldType::Text);
            if !skip {
                columns.push(name.clone());
            }
        }
        columns
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
        self.references()
            .into_iter()
            .find(|r| r.relation == relation)
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
        // Model files get the same `$VAR` expansion `main.toml` does — a
        // resource can name an environment variable anywhere it takes a string.
        let source = path.file_name().unwrap_or_default().to_string_lossy();
        let resource: Resource =
            crate::env::parse_toml(&text, &source).map_err(|e| crate::Error::Toml {
                path: path.to_path_buf(),
                source: e,
            })?;
        resource.validate()?;
        Ok(resource)
    }
}

/// The `[admin]` section of a resource: how it is presented — and to whom — in
/// the generated dashboard.
///
/// This is **presentation only**. Hiding a resource from the dashboard does not
/// make its API endpoints any less reachable; that is what
/// [`Permissions`] is for. The two are deliberately separate: `[permissions]`
/// decides what the API allows, `[admin]` decides what an operator is shown.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ResourceAdmin {
    /// Show this resource in the dashboard. Unset means "decide from the
    /// resource" — see [`ResourceAdmin::is_visible`].
    pub visible: Option<bool>,
    /// Organisation roles that may see it. Empty means "anyone who can list it"
    /// — the API remains the authority either way.
    pub roles: Vec<String>,
    /// Human label for one record ("Product"). Defaults to the resource name.
    pub label: Option<String>,
    /// Human label for the collection ("Products"). Defaults to `label` + "s".
    pub plural: Option<String>,
    /// Sidebar group heading ("Catalogue"). Ungrouped resources sort last.
    pub group: Option<String>,
    /// Which field to render when a record is named — in tables, in reference
    /// pickers, in breadcrumbs. Defaults to the first sensible string field.
    pub display_field: Option<String>,
    /// Columns to show in the list table, in order. Empty means "pick for me".
    pub columns: Vec<String>,
    /// Field the list search box filters on. Defaults to `display_field`.
    pub search_field: Option<String>,
    /// Sort key within the sidebar group; lower comes first.
    pub order: i64,
}

impl ResourceAdmin {
    /// Whether the named resource appears in the dashboard's resource
    /// navigation.
    ///
    /// An explicit `visible` always wins. Otherwise the auth resources default
    /// to hidden — they are managed through purpose-built screens (account,
    /// team, organisation, API keys), and a raw table of `membership` rows is
    /// exactly the developer-facing surface the dashboard is meant to avoid.
    pub fn is_visible(&self, resource_name: &str) -> bool {
        self.visible.unwrap_or(!is_auth_resource(resource_name))
    }
}

/// Whether a resource is one of the built-in auth/tenancy resources, which the
/// dashboard manages through dedicated screens instead of generic CRUD.
pub fn is_auth_resource(name: &str) -> bool {
    matches!(
        name,
        "user" | "organization" | "membership" | "membership_role" | "api_key" | "oauth_connection"
    )
}

/// Per-field dashboard presentation, from `[fields.<name>.admin]`.
#[derive(Debug, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct FieldAdmin {
    /// Show this field in the dashboard at all. Hidden fields are still part of
    /// the API (unlike [`Field::hidden`]).
    pub visible: bool,
    /// Show the field but refuse edits.
    pub readonly: bool,
    /// Human label. Defaults to a title-cased field name.
    pub label: Option<String>,
    /// One line of guidance shown under the input.
    pub help: Option<String>,
    /// Input to render. `auto` picks from the field's type.
    pub widget: Widget,
    /// Allowed values, turning the input into a dropdown. Each entry may be
    /// `"value"` or `"value|Label"`.
    pub options: Vec<String>,
    /// Placeholder text for free-text inputs.
    pub placeholder: Option<String>,
    /// What the stored text *is*, for `text`/`string` fields. Purely a
    /// presentation hint: the dashboard highlights and previews the markup,
    /// and the API stores and returns the same characters either way.
    pub format: ContentFormat,
}

impl Default for FieldAdmin {
    fn default() -> Self {
        FieldAdmin {
            visible: true,
            readonly: false,
            label: None,
            help: None,
            widget: Widget::Auto,
            options: Vec::new(),
            placeholder: None,
            format: ContentFormat::Plain,
        }
    }
}

/// What kind of content a free-text field holds.
///
/// Nothing about storage changes — this only tells the dashboard whether to
/// give the editor markup highlighting and a live preview.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ContentFormat {
    /// Ordinary text, edited in a plain textarea (the default).
    #[default]
    Plain,
    Markdown,
    Html,
}

impl ContentFormat {
    pub fn as_str(self) -> &'static str {
        match self {
            ContentFormat::Plain => "plain",
            ContentFormat::Markdown => "markdown",
            ContentFormat::Html => "html",
        }
    }
}

/// The input a field is edited with in the dashboard.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Widget {
    /// Derive the input from the field's type (the default).
    Auto,
    Text,
    Textarea,
    Select,
    Email,
    Url,
    Password,
    Color,
    Date,
    DateTime,
    Json,
    Switch,
}

impl Widget {
    pub fn as_str(self) -> &'static str {
        match self {
            Widget::Auto => "auto",
            Widget::Text => "text",
            Widget::Textarea => "textarea",
            Widget::Select => "select",
            Widget::Email => "email",
            Widget::Url => "url",
            Widget::Password => "password",
            Widget::Color => "color",
            Widget::Date => "date",
            Widget::DateTime => "date_time",
            Widget::Json => "json",
            Widget::Switch => "switch",
        }
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
    /// Dashboard presentation for this field, from `[fields.<name>.admin]`.
    #[serde(default)]
    pub admin: FieldAdmin,
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

/// `"purchase_order"` → `"Purchase order"`. Sentence case, not title case: a
/// dashboard full of Capitalised Nouns reads like a form, not an application.
pub fn titleize(name: &str) -> String {
    let spaced = name.trim_end_matches("_id").replace('_', " ");
    let mut chars = spaced.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
        None => spaced,
    }
}

/// A deliberately small English pluraliser — enough for the labels apiplant
/// generates, and overridable per resource with `[admin] plural`.
pub fn pluralize(label: &str) -> String {
    let lower = label.to_lowercase();
    if lower.ends_with('s')
        || lower.ends_with("x")
        || lower.ends_with("ch")
        || lower.ends_with("sh")
    {
        format!("{label}es")
    } else if lower.ends_with('y')
        && !lower.ends_with("ay")
        && !lower.ends_with("ey")
        && !lower.ends_with("oy")
        && !lower.ends_with("uy")
    {
        format!("{}ies", &label[..label.len() - 1])
    } else {
        format!("{label}s")
    }
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

/// One point in a resource's request lifecycle at which a function may run.
///
/// `before_*` hooks run after the permission check but before the database is
/// touched, so they can validate, rewrite the submitted payload, or abort the
/// request. `after_*` hooks run once the operation succeeded and can rewrite the
/// response body.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum HookEvent {
    BeforeList,
    AfterList,
    BeforeRead,
    AfterRead,
    BeforeCreate,
    AfterCreate,
    BeforeUpdate,
    AfterUpdate,
    BeforeDelete,
    AfterDelete,
}

impl HookEvent {
    /// Every event, in lifecycle order.
    pub const ALL: [HookEvent; 10] = [
        HookEvent::BeforeList,
        HookEvent::AfterList,
        HookEvent::BeforeRead,
        HookEvent::AfterRead,
        HookEvent::BeforeCreate,
        HookEvent::AfterCreate,
        HookEvent::BeforeUpdate,
        HookEvent::AfterUpdate,
        HookEvent::BeforeDelete,
        HookEvent::AfterDelete,
    ];

    /// The TOML key / wire name, e.g. `"before_create"`.
    pub fn as_str(self) -> &'static str {
        match self {
            HookEvent::BeforeList => "before_list",
            HookEvent::AfterList => "after_list",
            HookEvent::BeforeRead => "before_read",
            HookEvent::AfterRead => "after_read",
            HookEvent::BeforeCreate => "before_create",
            HookEvent::AfterCreate => "after_create",
            HookEvent::BeforeUpdate => "before_update",
            HookEvent::AfterUpdate => "after_update",
            HookEvent::BeforeDelete => "before_delete",
            HookEvent::AfterDelete => "after_delete",
        }
    }

    /// The operation this event belongs to (`"create"`, `"list"`, …).
    pub fn action(self) -> &'static str {
        match self {
            HookEvent::BeforeList | HookEvent::AfterList => "list",
            HookEvent::BeforeRead | HookEvent::AfterRead => "read",
            HookEvent::BeforeCreate | HookEvent::AfterCreate => "create",
            HookEvent::BeforeUpdate | HookEvent::AfterUpdate => "update",
            HookEvent::BeforeDelete | HookEvent::AfterDelete => "delete",
        }
    }

    /// `"before"` or `"after"`.
    pub fn phase(self) -> &'static str {
        if self.is_before() {
            "before"
        } else {
            "after"
        }
    }

    /// Whether this event fires ahead of the database operation.
    pub fn is_before(self) -> bool {
        matches!(
            self,
            HookEvent::BeforeList
                | HookEvent::BeforeRead
                | HookEvent::BeforeCreate
                | HookEvent::BeforeUpdate
                | HookEvent::BeforeDelete
        )
    }
}

/// The `[hooks]` section of a resource: a function name per lifecycle event.
///
/// Unknown keys are rejected so a typo (`befor_create`) fails at load time
/// instead of silently never firing.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Hooks {
    pub before_list: Option<String>,
    pub after_list: Option<String>,
    pub before_read: Option<String>,
    pub after_read: Option<String>,
    pub before_create: Option<String>,
    pub after_create: Option<String>,
    pub before_update: Option<String>,
    pub after_update: Option<String>,
    pub before_delete: Option<String>,
    pub after_delete: Option<String>,
}

impl Hooks {
    /// The function bound to an event, if any.
    pub fn get(&self, event: HookEvent) -> Option<&str> {
        let slot = match event {
            HookEvent::BeforeList => &self.before_list,
            HookEvent::AfterList => &self.after_list,
            HookEvent::BeforeRead => &self.before_read,
            HookEvent::AfterRead => &self.after_read,
            HookEvent::BeforeCreate => &self.before_create,
            HookEvent::AfterCreate => &self.after_create,
            HookEvent::BeforeUpdate => &self.before_update,
            HookEvent::AfterUpdate => &self.after_update,
            HookEvent::BeforeDelete => &self.before_delete,
            HookEvent::AfterDelete => &self.after_delete,
        };
        slot.as_deref()
    }

    /// Every declared `(event, function)` pair, in lifecycle order.
    pub fn iter(&self) -> impl Iterator<Item = (HookEvent, &str)> {
        HookEvent::ALL
            .into_iter()
            .filter_map(|event| self.get(event).map(|name| (event, name)))
    }

    /// Whether any hook at all is declared.
    pub fn is_empty(&self) -> bool {
        self.iter().next().is_none()
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

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_resource(src: &str) -> Resource {
        let resource: Resource = toml::from_str(src).unwrap();
        resource.validate().unwrap();
        resource
    }

    #[test]
    fn access_parser_and_permissions_defaults_match_org_membership_model() {
        assert_eq!(Access::parse("public"), Access::Public);
        assert_eq!(Access::parse("authenticated"), Access::Authenticated);
        assert_eq!(Access::parse("member"), Access::Member);
        assert_eq!(Access::parse("owner"), Access::Owner);
        assert_eq!(Access::parse("role:admin"), Access::Role("admin".into()));
        assert_eq!(Access::parse("wat"), Access::Private);

        let defaults = Permissions::default();
        assert_eq!(defaults.list, Access::Member);
        assert_eq!(defaults.read, Access::Member);
        assert_eq!(defaults.create, Access::Member);
        assert_eq!(defaults.update, Access::Member);
        assert_eq!(defaults.delete, Access::Member);
    }

    #[test]
    fn validate_rejects_reserved_id_and_dangling_reference_definition() {
        let reserved_id: Resource = toml::from_str(
            r#"
[resource]
name = "bad"

[fields.id]
type = "string"
"#,
        )
        .unwrap();
        assert!(reserved_id.validate().is_err());

        let missing_target: Resource = toml::from_str(
            r#"
[resource]
name = "bad_ref"

[fields.owner_id]
type = "reference"
"#,
        )
        .unwrap();
        assert!(missing_target.validate().is_err());
    }

    #[test]
    fn content_format_is_only_allowed_on_text_fields() {
        let good: Resource = toml::from_str(
            r#"
[resource]
name = "article"

[fields.body]
type = "text"

[fields.body.admin]
format = "markdown"
"#,
        )
        .unwrap();
        good.validate().unwrap();
        assert_eq!(good.fields["body"].admin.format, ContentFormat::Markdown);

        let bad: Resource = toml::from_str(
            r#"
[resource]
name = "article"

[fields.published]
type = "boolean"

[fields.published.admin]
format = "html"
"#,
        )
        .unwrap();
        assert!(bad.validate().is_err());
    }

    #[test]
    fn references_derive_relation_names_and_default_on_delete() {
        let resource = parse_resource(
            r#"
[resource]
name = "comment"

[fields.post_id]
type = "reference"
references = "post"
required = true

[fields.author_id]
type = "reference"
references = "user"
on_delete = "cascade"
"#,
        );

        assert_eq!(relation_name("owner_id"), "owner");
        assert_eq!(relation_name("slug"), "slug");

        let refs = resource.references();
        assert_eq!(refs.len(), 2);
        let post = refs.iter().find(|rf| rf.field == "post_id").unwrap();
        assert_eq!(post.target, "post");
        assert_eq!(post.relation, "post");
        assert_eq!(post.on_delete, OnDelete::Restrict);
        assert!(post.required);

        let author = resource.reference_by_relation("author").unwrap();
        assert_eq!(author.field, "author_id");
        assert_eq!(author.on_delete, OnDelete::Cascade);
    }

    #[test]
    fn hooks_parse_per_event_and_iterate_in_lifecycle_order() {
        let resource = parse_resource(
            r#"
[resource]
name = "post"

[fields.title]
type = "string"

[hooks]
before_create = "validate_post"
after_create = "notify"
after_list = "redact"
"#,
        );

        assert_eq!(
            resource.hook(HookEvent::BeforeCreate),
            Some("validate_post")
        );
        assert_eq!(resource.hook(HookEvent::AfterCreate), Some("notify"));
        assert_eq!(resource.hook(HookEvent::AfterList), Some("redact"));
        assert_eq!(resource.hook(HookEvent::BeforeUpdate), None);
        assert!(!resource.hooks.is_empty());

        let declared: Vec<_> = resource.hooks.iter().collect();
        assert_eq!(
            declared,
            vec![
                (HookEvent::AfterList, "redact"),
                (HookEvent::BeforeCreate, "validate_post"),
                (HookEvent::AfterCreate, "notify"),
            ]
        );
    }

    #[test]
    fn hook_events_expose_wire_names_actions_and_phases() {
        assert_eq!(HookEvent::BeforeCreate.as_str(), "before_create");
        assert_eq!(HookEvent::BeforeCreate.action(), "create");
        assert_eq!(HookEvent::BeforeCreate.phase(), "before");
        assert!(HookEvent::BeforeCreate.is_before());

        assert_eq!(HookEvent::AfterList.as_str(), "after_list");
        assert_eq!(HookEvent::AfterList.action(), "list");
        assert_eq!(HookEvent::AfterList.phase(), "after");
        assert!(!HookEvent::AfterList.is_before());

        // Every event round-trips through the `[hooks]` section under its own key.
        for event in HookEvent::ALL {
            let resource = parse_resource(&format!(
                "[resource]\nname = \"post\"\n\n[hooks]\n{} = \"h\"\n",
                event.as_str()
            ));
            assert_eq!(resource.hook(event), Some("h"), "{}", event.as_str());
            assert_eq!(resource.hooks.iter().count(), 1);
        }
    }

    #[test]
    fn hooks_reject_typos_and_empty_function_names() {
        let typo = toml::from_str::<Resource>(
            r#"
[resource]
name = "post"

[hooks]
befor_create = "oops"
"#,
        );
        assert!(typo.is_err(), "unknown hook keys must not be ignored");

        let empty: Resource = toml::from_str(
            r#"
[resource]
name = "post"

[hooks]
after_delete = "  "
"#,
        )
        .unwrap();
        assert!(empty.validate().is_err());
    }

    #[test]
    fn resources_have_no_hooks_by_default() {
        let resource = parse_resource("[resource]\nname = \"post\"\n");
        assert!(resource.hooks.is_empty());
        assert!(HookEvent::ALL
            .into_iter()
            .all(|event| resource.hook(event).is_none()));
    }

    #[test]
    fn admin_visibility_defaults_hide_auth_resources_but_nothing_else() {
        let post = parse_resource("[resource]\nname = \"post\"\n");
        assert!(post.admin.is_visible("post"));

        let user = parse_resource(crate::defaults::USER_TOML);
        assert!(!user.admin.is_visible("user"));
        assert!(!parse_resource(crate::defaults::MEMBERSHIP_TOML)
            .admin
            .is_visible("membership"));

        // An app that replaces a built-in still gets the dedicated screen…
        let replaced = parse_resource(
            "[resource]\nname = \"user\"\nscope = \"global\"\n\n[fields.email]\ntype = \"string\"\n",
        );
        assert!(!replaced.admin.is_visible("user"));

        // …unless it asks for the table back.
        let opted_in = parse_resource(
            "[resource]\nname = \"user\"\nscope = \"global\"\n\n[admin]\nvisible = true\n",
        );
        assert!(opted_in.admin.is_visible("user"));
    }

    #[test]
    fn admin_labels_are_inferred_and_overridable() {
        let inferred = parse_resource("[resource]\nname = \"purchase_order\"\n");
        assert_eq!(inferred.admin_label(), "Purchase order");
        assert_eq!(inferred.admin_plural(), "Purchase orders");

        let overridden = parse_resource(
            "[resource]\nname = \"person\"\n\n[admin]\nlabel = \"Person\"\nplural = \"People\"\n",
        );
        assert_eq!(overridden.admin_plural(), "People");

        assert_eq!(titleize("owner_id"), "Owner");
        assert_eq!(titleize("total_cents"), "Total cents");
        assert_eq!(pluralize("Category"), "Categories");
        assert_eq!(pluralize("Address"), "Addresses");
        assert_eq!(pluralize("Day"), "Days");
        assert_eq!(pluralize("Product"), "Products");
    }

    #[test]
    fn display_field_prefers_conventional_names_then_any_string() {
        let conventional = parse_resource(
            r#"
[resource]
name = "product"

[fields.sku]
type = "string"

[fields.name]
type = "string"
"#,
        );
        assert_eq!(conventional.admin_display_field().as_deref(), Some("name"));

        let only_odd_names =
            parse_resource("[resource]\nname = \"blob\"\n\n[fields.zzz]\ntype = \"string\"\n");
        assert_eq!(only_odd_names.admin_display_field().as_deref(), Some("zzz"));

        let nothing_stringy =
            parse_resource("[resource]\nname = \"tick\"\n\n[fields.count]\ntype = \"integer\"\n");
        assert_eq!(nothing_stringy.admin_display_field(), None);

        // An explicit choice always wins, conventional or not.
        let declared = parse_resource(
            r#"
[resource]
name = "product"

[admin]
display_field = "sku"

[fields.sku]
type = "string"

[fields.name]
type = "string"
"#,
        );
        assert_eq!(declared.admin_display_field().as_deref(), Some("sku"));
        // `search_field` falls back to whatever names the record.
        assert_eq!(declared.admin_search_field().as_deref(), Some("sku"));
    }

    #[test]
    fn inferred_columns_skip_blobs_and_dashboard_hidden_fields() {
        let resource = parse_resource(
            r#"
[resource]
name = "product"

[fields.name]
type = "string"

[fields.status]
type = "string"

[fields.description]
type = "text"

[fields.attributes]
type = "json"

[fields.secret_ratio]
type = "float"

[fields.secret_ratio.admin]
visible = false
"#,
        );
        assert_eq!(resource.admin_columns(), vec!["name", "status"]);

        let declared = parse_resource(
            r#"
[resource]
name = "product"

[admin]
columns = ["status", "name"]

[fields.name]
type = "string"

[fields.status]
type = "string"
"#,
        );
        assert_eq!(declared.admin_columns(), vec!["status", "name"]);
    }

    #[test]
    fn admin_section_rejects_columns_naming_fields_that_do_not_exist() {
        let bad_column: Resource = toml::from_str(
            "[resource]\nname = \"post\"\n\n[admin]\ncolumns = [\"nope\"]\n\n[fields.title]\ntype = \"string\"\n",
        )
        .unwrap();
        assert!(bad_column.validate().is_err());

        let bad_display: Resource = toml::from_str(
            "[resource]\nname = \"post\"\n\n[admin]\ndisplay_field = \"nope\"\n\n[fields.title]\ntype = \"string\"\n",
        )
        .unwrap();
        assert!(bad_display.validate().is_err());

        // A typo in a key is caught too, rather than silently ignored.
        assert!(toml::from_str::<Resource>(
            "[resource]\nname = \"post\"\n\n[admin]\nvisibel = true\n"
        )
        .is_err());
    }

    #[test]
    fn field_admin_carries_widget_options_and_visibility() {
        let resource = parse_resource(
            r#"
[resource]
name = "product"

[fields.status]
type = "string"

[fields.status.admin]
label = "Lifecycle"
widget = "select"
options = ["draft", "active|Live"]
readonly = true
"#,
        );
        let status = &resource.fields["status"];
        assert_eq!(status.admin.label.as_deref(), Some("Lifecycle"));
        assert_eq!(status.admin.widget, Widget::Select);
        assert_eq!(status.admin.widget.as_str(), "select");
        assert_eq!(status.admin.options, vec!["draft", "active|Live"]);
        assert!(status.admin.readonly);
        // Defaults stay out of the way when `[fields.x.admin]` says nothing.
        assert!(status.admin.visible);
        assert_eq!(resource.fields["status"].admin.help, None);
    }

    #[test]
    fn org_column_reflects_resource_scope() {
        let org_scoped = parse_resource(
            r#"
[resource]
name = "post"

[fields.title]
type = "string"
"#,
        );
        let global = parse_resource(
            r#"
[resource]
name = "plan"
scope = "global"

[fields.name]
type = "string"
"#,
        );
        let organization = parse_resource(crate::defaults::ORGANIZATION_TOML);

        assert_eq!(org_scoped.org_column(), Some("organization_id"));
        assert_eq!(global.org_column(), None);
        assert_eq!(organization.org_column(), Some("id"));
    }
}
