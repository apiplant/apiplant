//! What the console is showing, and what each key does to it.
//!
//! One state machine, driven by key presses and holding every request it makes,
//! so the drawing code in [`super::ui`] is a pure function of this and never
//! has to decide anything.

use std::collections::BTreeSet;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use serde_json::{json, Map, Value};
use tokio::sync::oneshot;

use super::api::{self, AgentManifest, Client, FunctionManifest, Manifest, ResourceManifest};
use super::link::{self, Handoff};
use super::store::{Saved, Store};

/// Rows per page. Large enough that paging is rare on a real screen, small
/// enough that a mistyped filter does not pull a table into memory.
pub const PAGE: usize = 50;

// --- navigation ------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Focus {
    Nav,
    Main,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NavKind {
    Resource(usize),
    Agent(usize),
    Function(usize),
    Account,
    Team,
    Session,
}

#[derive(Debug, Clone)]
pub struct NavItem {
    pub label: String,
    pub group: String,
    pub kind: NavKind,
}

/// What the console knows about the caller when it decides what to show them.
///
/// The dashboard makes the same decision from the same three facts. The fourth
/// — whether the roles are *known* — exists because this console has to work
/// them out from the API rather than being handed them: an app that will not
/// let you list your own memberships leaves us unable to say, and "unable to
/// say" must not read as "you hold none".
#[derive(Debug, Clone, Copy)]
pub struct Reach<'a> {
    pub signed_in: bool,
    /// Whether there is an active organisation to work in.
    pub organization: bool,
    pub roles: &'a [String],
    pub roles_known: bool,
}

impl Reach<'_> {
    /// Nobody, before we know who is asking.
    pub fn unknown() -> Reach<'static> {
        Reach {
            signed_in: false,
            organization: false,
            roles: &[],
            roles_known: false,
        }
    }

    /// Whether to put this action in front of the caller.
    ///
    /// Hiding is a claim, so it is only made from knowledge: a `role:` policy
    /// we cannot check is left alone, and the server refuses it if we were
    /// wrong. Everything else the manifest settles — `private` is nobody,
    /// `authenticated` needs a session, and organisation-scoped work needs an
    /// organisation to do it in.
    pub fn may(&self, global: bool, permission: &api::ActionPermission) -> bool {
        // A policy this console does not recognise — an older build against a
        // newer server — is not grounds for hiding anything.
        if permission.value.is_empty() {
            return true;
        }
        if permission.role.is_some() && !self.roles_known {
            // A role is held *in* an organisation, so a session in none holds
            // none and this is decidable after all. Inside one it is not, and
            // a door that might open beats one that is missing.
            return permission.possible() && self.signed_in && self.organization;
        }
        permission.allowed(self.signed_in, global || self.organization, self.roles)
    }

    /// The same question for a function, whose permission is a bare string.
    pub fn may_run(&self, function: &FunctionManifest) -> bool {
        let permission = api::ActionPermission {
            role: function
                .permission
                .strip_prefix("role:")
                .map(str::to_string),
            value: function.permission.clone(),
            ..Default::default()
        };
        self.may(!function.requires_org, &permission)
    }
}

/// Build the sidebar from the manifest.
///
/// Only what the operator can actually reach: a resource the server will not
/// list for anyone, or a function with no endpoint, is a dead entry that only
/// teaches people the console is broken.
fn navigation(manifest: &Manifest, reach: &Reach) -> Vec<NavItem> {
    let reachable =
        |permission: &api::ActionPermission, global: bool| reach.may(global, permission);

    let mut resources: Vec<(usize, &ResourceManifest)> = manifest
        .resources
        .iter()
        .enumerate()
        .filter(|(_, resource)| {
            resource.visible && reachable(&resource.permissions.list, resource.scope == "global")
        })
        .collect();
    resources.sort_by(|(_, a), (_, b)| {
        a.group
            .cmp(&b.group)
            .then(a.order.cmp(&b.order))
            .then(a.plural.cmp(&b.plural))
    });

    let mut items: Vec<NavItem> = resources
        .into_iter()
        .map(|(index, resource)| NavItem {
            label: resource.plural.clone(),
            group: resource.group.clone().unwrap_or_else(|| "Data".into()),
            kind: NavKind::Resource(index),
        })
        .collect();

    for (index, agent) in manifest.agents.iter().enumerate() {
        if !reach.may(agent.scope == "global", &agent.chat) {
            continue;
        }
        items.push(NavItem {
            label: agent.label.clone(),
            group: "Agents".into(),
            kind: NavKind::Agent(index),
        });
    }

    for (index, function) in manifest.functions.iter().enumerate() {
        // An action needing a role you do not hold, or an organisation you have
        // not got, is a button that can only answer 403.
        if !function.visible || !reach.may_run(function) {
            continue;
        }
        items.push(NavItem {
            label: function.label.clone(),
            group: function.group.clone().unwrap_or_else(|| "Actions".into()),
            kind: NavKind::Function(index),
        });
    }

    // The auth resources are hidden from the list above — `user`, `membership`
    // and the rest are how tenancy is *stored*, and a table of membership rows
    // with a `user_id` column is a developer's view of a team. The dashboard
    // gives them purpose-built screens instead, and so does this: an account,
    // a team, the organizations you belong to, your keys.
    let listable = |name: &str| {
        manifest
            .resources
            .iter()
            .position(|resource| resource.name == name)
            .filter(|index| {
                let resource = &manifest.resources[*index];
                reachable(&resource.permissions.list, resource.scope == "global")
            })
    };

    let mut console = Vec::new();
    // Your own account is only a screen once there is a session to own it.
    if reach.signed_in && !manifest.auth.profile_fields.is_empty() {
        console.push(NavItem {
            label: "Account".into(),
            group: "Console".into(),
            kind: NavKind::Account,
        });
    }
    if listable("membership").is_some() {
        console.push(NavItem {
            label: "Team".into(),
            group: "Console".into(),
            kind: NavKind::Team,
        });
    }
    // These two are ordinary tables underneath, so they reuse the list screen
    // rather than getting a bespoke one — the same records, the same keys.
    for name in ["organization", "api_key"] {
        if let Some(index) = listable(name) {
            // An app that deliberately turned the generic table back on has it
            // in the sidebar already; a second entry for the same rows is only
            // confusing.
            if manifest.resources[index].visible {
                continue;
            }
            console.push(NavItem {
                // Whatever the app calls them, so a renamed `organization`
                // reads the same here as everywhere else.
                label: manifest.resources[index].plural.clone(),
                group: "Console".into(),
                kind: NavKind::Resource(index),
            });
        }
    }
    console.push(NavItem {
        label: "Session".into(),
        group: "Console".into(),
        kind: NavKind::Session,
    });

    items.extend(console);
    items
}

// --- forms -----------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct FormField {
    pub name: String,
    pub label: String,
    pub ty: String,
    pub value: String,
    pub cursor: usize,
    pub help: Option<String>,
    pub required: bool,
    pub secret: bool,
    /// A closed set of values, cycled with left/right instead of typed.
    pub options: Vec<String>,
    /// The resource a reference points at, so Enter can offer a picker rather
    /// than asking someone to type a UUID from memory.
    pub references: Option<String>,
    /// What the record held before editing, so an update sends only changes.
    pub was: Option<String>,
}

impl FormField {
    fn text(name: &str, label: &str) -> FormField {
        FormField {
            name: name.into(),
            label: label.into(),
            ty: "string".into(),
            value: String::new(),
            cursor: 0,
            help: None,
            required: false,
            secret: false,
            options: Vec::new(),
            references: None,
            was: None,
        }
    }

    fn set(&mut self, value: String) {
        self.cursor = value.chars().count();
        self.value = value;
    }

    pub fn changed(&self) -> bool {
        match &self.was {
            Some(was) => was != &self.value,
            None => !self.value.is_empty(),
        }
    }

    /// A boolean has no empty state, so it is toggled rather than typed.
    pub fn toggleable(&self) -> bool {
        self.ty == "boolean"
    }
}

/// The second password box on a sign-up form.
///
/// Named rather than positional because it has to be found again on submit —
/// to compare it, and to keep it out of the body, since it is a check and not a
/// field on anybody's `user` row.
pub const CONFIRM_PASSWORD: &str = "password_confirmation";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FormKind {
    SignInPassword,
    SignInKey,
    /// Creating an account, when the app allows it.
    SignUp,
    /// Asking for a password reset link, when the app can send one.
    ForgotPassword,
    /// Editing your own user record — what the dashboard calls Your account.
    Profile,
    /// Naming a key before it is minted. Keys are not created through the
    /// `api_key` resource: the plaintext exists once, in the reply from
    /// `/auth/apikeys`, and never again.
    NewApiKey,
    /// Adding somebody to the active organisation by their identity.
    AddMember,
    Create {
        resource: usize,
    },
    Update {
        resource: usize,
        id: String,
    },
    Run {
        function: usize,
    },
    /// The same request as `Create` on `organization`, but it switches into
    /// what it created rather than returning to a list.
    NewOrganization {
        resource: usize,
    },
}

#[derive(Debug, Clone)]
pub struct Form {
    pub title: String,
    pub subtitle: Option<String>,
    pub fields: Vec<FormField>,
    /// `fields.len()` selects the submit button, which is why it is an index
    /// into the form and not into the fields.
    pub index: usize,
    pub editing: bool,
    pub submit: String,
    pub kind: FormKind,
    /// The whole form is one JSON document — a function with no input schema
    /// takes whatever the caller sends, and guessing a shape would be worse
    /// than showing the document.
    pub raw_body: bool,
}

impl Form {
    fn new(
        title: impl Into<String>,
        submit: impl Into<String>,
        kind: FormKind,
        fields: Vec<FormField>,
    ) -> Form {
        Form {
            title: title.into(),
            subtitle: None,
            fields,
            index: 0,
            editing: false,
            submit: submit.into(),
            kind,
            raw_body: false,
        }
    }

    pub fn on_submit(&self) -> bool {
        self.index >= self.fields.len()
    }

    pub fn current(&mut self) -> Option<&mut FormField> {
        self.fields.get_mut(self.index)
    }

    fn sign_in_password(manifest: &Manifest) -> Form {
        let mut identity =
            FormField::text(&manifest.auth.identity_field, &manifest.auth.identity_label);
        identity.required = true;
        let mut password = FormField::text("password", "Password");
        password.required = true;
        password.secret = true;
        let mut form = Form::new(
            "Sign in",
            "Sign in",
            FormKind::SignInPassword,
            vec![identity, password],
        );
        form.subtitle = Some(
            "The console signs in, then mints an API key so the next run starts where you left off."
                .into(),
        );
        form
    }

    fn sign_in_key() -> Form {
        let mut key = FormField::text("api_key", "API key");
        key.required = true;
        key.secret = true;
        let mut form = Form::new("Use an API key", "Connect", FormKind::SignInKey, vec![key]);
        form.subtitle = Some("Paste a key from the dashboard's API keys screen.".into());
        form
    }

    /// Sign-up, when the app allows it: the identity, a password, and whatever
    /// else the app says an account needs.
    fn sign_up(manifest: &Manifest) -> Form {
        let mut identity =
            FormField::text(&manifest.auth.identity_field, &manifest.auth.identity_label);
        identity.required = true;
        let mut password = FormField::text("password", "Password");
        password.required = true;
        password.secret = true;
        // Typed twice, because it cannot be read back. A typo in a password
        // nobody can see is discovered at the *next* sign-in, and the way back
        // from there is a reset the app may not even offer.
        let mut confirmation = FormField::text(CONFIRM_PASSWORD, "Confirm password");
        confirmation.required = true;
        confirmation.secret = true;

        let mut fields = vec![identity, password, confirmation];
        // Whatever the app declared as asked at sign-up — the same list the
        // dashboard's register form shows.
        fields.extend(
            inputs(&manifest.auth.signup_fields)
                .into_iter()
                .filter(|field| field.name != manifest.auth.identity_field),
        );

        let mut form = Form::new(
            "Create an account",
            "Create account",
            FormKind::SignUp,
            fields,
        );
        form.subtitle = Some(if manifest.auth.require_email_verification {
            "This app lets anyone register. You will confirm your address by email \
             before you can sign in."
                .into()
        } else {
            "This app lets anyone register. You will be signed in straight away.".to_string()
        });
        form
    }

    /// Asking for a reset link. One box: the address, which is all the endpoint
    /// takes — the rest of the flow happens in a browser, from the mailbox.
    fn forgot_password(manifest: &Manifest) -> Form {
        let mut identity =
            FormField::text(&manifest.auth.identity_field, &manifest.auth.identity_label);
        identity.required = true;
        let mut form = Form::new(
            "Reset your password",
            "Send reset link",
            FormKind::ForgotPassword,
            vec![identity],
        );
        form.subtitle = Some(
            "We will email a link. Open it in a browser to choose a new password — \
             your current one keeps working until you do."
                .into(),
        );
        form
    }

    /// Your own details, from the fields the app says are yours to change.
    fn profile(manifest: &Manifest, record: Option<&Value>) -> Form {
        let mut fields = inputs(&manifest.auth.profile_fields);
        for field in &mut fields {
            let current = record
                .and_then(|record| record.get(&field.name))
                .map(to_text)
                .unwrap_or_default();
            field.set(current.clone());
            field.was = Some(current);
        }
        let mut form = Form::new("Your account", "Save changes", FormKind::Profile, fields);
        form.subtitle = Some("The details other people see, and how you sign in.".into());
        form
    }

    /// Naming a key before it is issued.
    fn new_api_key() -> Form {
        let mut name = FormField::text("name", "Name");
        name.help = Some(
            "Name it after whatever will use it, so you know what you are \
                          revoking later."
                .into(),
        );
        let mut form = Form::new("New API key", "Create key", FormKind::NewApiKey, vec![name]);
        form.subtitle = Some(
            "A key acts as you, with everything you can do. The key itself is shown \
                  once and never again."
                .into(),
        );
        form
    }

    /// Adding someone to the organisation, by the identity they signed up with.
    ///
    /// Looking the account up first would not work: you may only read users you
    /// already share an organisation with, which by definition this person is
    /// not. The server resolves the identity, and says so when nobody has it.
    ///
    /// With invitations available the wording changes and so does the endpoint
    /// — see [`App::add_member`] — because the constraint it explains no longer
    /// holds: the person does not need an account first.
    fn add_member(manifest: &Manifest, roles: &[String]) -> Form {
        let mut identity =
            FormField::text(&manifest.auth.identity_field, &manifest.auth.identity_label);
        identity.required = true;
        let mut role = FormField::text("role", "Role");
        role.options = roles.to_vec();
        role.help = Some("Their starting role. You can give them more afterwards.".into());
        role.set(
            roles
                .iter()
                .find(|role| *role == "member")
                .cloned()
                .or_else(|| roles.first().cloned())
                .unwrap_or_default(),
        );
        let inviting = manifest.auth.invitations_enabled;
        let mut form = Form::new(
            if inviting {
                "Invite someone to this organization"
            } else {
                "Add someone to this organization"
            },
            if inviting {
                "Send invitation"
            } else {
                "Add to organization"
            },
            FormKind::AddMember,
            vec![identity, role],
        );
        form.subtitle = Some(if inviting {
            "We will email them a link. They can accept it whether or not they already \
             have an account."
                .into()
        } else {
            "They need an account already — adding them here is what gives it access.".to_string()
        });
        form
    }

    /// A form for creating one record of `resource`.
    fn create(index: usize, resource: &ResourceManifest) -> Form {
        let editable: Vec<_> = resource
            .fields
            .iter()
            .filter(|field| field.editable())
            .cloned()
            .collect();
        let fields = inputs(&editable);
        Form::new(
            format!("New {}", resource.label.to_lowercase()),
            "Create",
            FormKind::Create { resource: index },
            fields,
        )
    }

    /// Starting another organisation: the organisation's own fields, whatever
    /// the app has made those, and you land inside it once it exists.
    fn new_organization(index: usize, resource: &ResourceManifest) -> Form {
        let mut form = Form::create(index, resource);
        form.title = "Create an organization".into();
        form.submit = "Create organization".into();
        form.subtitle = Some(
            "A separate workspace for its own records. You become its admin, and the \
             console switches into it."
                .into(),
        );
        form.kind = FormKind::NewOrganization { resource: index };
        form
    }

    /// The same form, filled in from a record.
    fn edit(index: usize, resource: &ResourceManifest, record: &Value) -> Form {
        let mut form = Form::create(index, resource);
        for field in &mut form.fields {
            let current = record.get(&field.name).map(to_text).unwrap_or_default();
            field.set(current.clone());
            field.was = Some(current);
        }
        let id = record
            .get("id")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        form.title = format!("Edit {}", resource.title_of(record));
        form.submit = "Save".into();
        form.kind = FormKind::Update {
            resource: index,
            id,
        };
        form
    }

    /// A form for one function, built from the JSON Schema of its input.
    fn run(index: usize, function: &FunctionManifest) -> Form {
        let mut fields = Vec::new();
        let mut raw_body = false;

        match function.input_schema.as_ref().and_then(schema_fields) {
            Some(schema_fields) if !schema_fields.is_empty() => fields = schema_fields,
            _ => {
                raw_body = true;
                let mut body = FormField::text("body", "Request body");
                body.ty = "json".into();
                body.help = Some("JSON sent as the whole request body.".into());
                body.set("{}".into());
                fields.push(body);
            }
        }

        let submit = if function.run_label.is_empty() {
            "Run".to_string()
        } else {
            function.run_label.clone()
        };
        let mut form = Form::new(
            function.label.clone(),
            submit,
            FormKind::Run { function: index },
            fields,
        );
        form.raw_body = raw_body;
        form.subtitle = (!function.description.is_empty()).then(|| function.description.clone());
        form
    }
}

/// Turn manifest fields into the boxes an operator types into.
fn inputs(fields: &[api::FieldManifest]) -> Vec<FormField> {
    fields
        .iter()
        .map(|field| {
            let mut entry = FormField::text(&field.name, &field.label);
            entry.ty = field.ty.clone();
            entry.help = field.help.clone();
            entry.required = field.required;
            entry.secret = field.widget == "password";
            entry.options = field.options.iter().map(|o| o.value.clone()).collect();
            entry.references = field.references.clone();
            if let Some(default) = &field.default_value {
                entry.set(to_text(default));
            } else if field.ty == "boolean" {
                entry.set("false".into());
            }
            entry
        })
        .collect()
}

/// Read a function's input schema into form fields.
fn schema_fields(schema: &Value) -> Option<Vec<FormField>> {
    let properties = schema.get("properties")?.as_object()?;
    let required: BTreeSet<&str> = schema
        .get("required")
        .and_then(Value::as_array)
        .map(|names| names.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default();

    Some(
        properties
            .iter()
            .map(|(name, spec)| {
                let ty = spec
                    .get("type")
                    .and_then(Value::as_str)
                    // A union like `["string", "null"]` is still typed enough to
                    // pick an input from its first member.
                    .or_else(|| {
                        spec.get("type")?
                            .as_array()?
                            .iter()
                            .filter_map(Value::as_str)
                            .find(|ty| *ty != "null")
                    })
                    .unwrap_or("string");
                let mut field = FormField::text(name, &titleize(name));
                field.ty = ty.to_string();
                field.required = required.contains(name.as_str());
                field.help = spec
                    .get("description")
                    .and_then(Value::as_str)
                    .map(str::to_string);
                field.options = spec
                    .get("enum")
                    .and_then(Value::as_array)
                    .map(|values| values.iter().map(to_text).collect())
                    .unwrap_or_default();
                if let Some(default) = spec.get("default") {
                    field.set(to_text(default));
                } else if ty == "boolean" {
                    field.set("false".into());
                } else if matches!(ty, "object" | "array") {
                    field.set(if ty == "array" {
                        "[]".into()
                    } else {
                        "{}".into()
                    });
                }
                field
            })
            .collect(),
    )
}

fn titleize(name: &str) -> String {
    // Only the first word is capitalised: "Dry run" reads as a label, "Dry Run"
    // reads as a product name.
    let words: Vec<&str> = name.split(['_', '-']).filter(|s| !s.is_empty()).collect();
    let mut out = words.join(" ");
    if let Some(first) = out.chars().next() {
        out.replace_range(..first.len_utf8(), &first.to_uppercase().to_string());
    }
    if out.is_empty() {
        name.to_string()
    } else {
        out
    }
}

/// A JSON value as the text an operator edits.
pub fn to_text(value: &Value) -> String {
    match value {
        Value::Null => String::new(),
        Value::String(text) => text.clone(),
        Value::Bool(flag) => flag.to_string(),
        other => other.to_string(),
    }
}

// --- what the main pane is showing ------------------------------------------

#[derive(Debug, Clone)]
pub struct List {
    pub resource: usize,
    pub rows: Vec<Value>,
    pub index: usize,
    pub page: usize,
    pub search: String,
    pub searching: bool,
    pub cursor: usize,
    /// A `(field, value)` the list is pinned to — how the records belonging to
    /// one parent are shown, as `field=value` on the query.
    pub filter: Option<(String, String)>,
    /// What that parent is called, for the title.
    pub filter_label: Option<String>,
}

#[derive(Debug, Clone)]
pub struct Detail {
    pub resource: usize,
    pub record: Value,
    pub scroll: u16,
}

/// One person in the active organisation, with every role they hold.
///
/// Roles arrive from two tables — the membership's own `role` column and its
/// `membership_role` rows — and this is where they are stitched back together,
/// the same way the server does it when it checks a permission.
#[derive(Debug, Clone)]
pub struct Member {
    pub membership_id: String,
    pub name: String,
    /// The primary role, the one the membership itself carries.
    pub primary: Option<String>,
    /// The granted roles, as `(grant id, role)` — the id is what revoking one
    /// deletes.
    pub grants: Vec<(String, String)>,
    pub is_me: bool,
}

impl Member {
    /// Every role, primary first, without repeats.
    pub fn roles(&self) -> Vec<String> {
        let mut roles: Vec<String> = self.primary.iter().cloned().collect();
        for (_, role) in &self.grants {
            if !roles.contains(role) {
                roles.push(role.clone());
            }
        }
        roles
    }

    /// Whether this role may be taken away from this person *here*.
    ///
    /// Nobody may remove their own `admin`. An organisation can only lose its
    /// last administrator if that administrator removes themselves, so refusing
    /// it is what keeps every organisation administrable. Another admin still
    /// can. The server refuses it too; this is so the console does not offer
    /// something it knows will come back 403.
    pub fn may_revoke(&self, role: &str) -> bool {
        !(self.is_me && role == ADMIN_ROLE)
    }
}

/// The role an app gives whoever creates an organisation, and the one that
/// implies every other.
pub const ADMIN_ROLE: &str = "admin";

#[derive(Debug, Clone)]
pub struct Team {
    pub members: Vec<Member>,
    pub index: usize,
    /// Whether this caller may hand roles out at all.
    pub manage: bool,
}

#[derive(Debug, Clone)]
pub struct AgentThread {
    pub id: String,
    pub title: String,
    pub updated_at: Option<String>,
}

#[derive(Debug, Clone)]
pub struct AgentMessage {
    pub role: String,
    pub content: String,
    pub meta: Option<String>,
}

#[derive(Debug, Clone)]
pub struct AgentChat {
    pub agent: usize,
    pub threads: Vec<AgentThread>,
    pub thread_index: usize,
    pub thread_id: Option<String>,
    pub messages: Vec<AgentMessage>,
    pub draft: String,
    pub cursor: usize,
    pub editing: bool,
    pub browse_note: Option<String>,
}

#[derive(Debug, Clone)]
pub enum Main {
    /// Nothing chosen yet, or nothing to show.
    Empty(String),
    List(List),
    Detail(Detail),
    Form(Form),
    Output {
        title: String,
        body: String,
        scroll: u16,
    },
    Agent(AgentChat),
    Team(Team),
    Session,
}

// --- overlays ---------------------------------------------------------------

#[derive(Debug, Clone)]
pub enum PickerKind {
    Organization,
    /// Choosing a value for a reference field on the form underneath.
    Reference {
        field: usize,
    },
    /// Opening the records that point at the one on screen.
    Children {
        /// `id` of the record they belong to, and what it is called.
        parent: String,
        label: String,
    },
    /// Giving the highlighted member another role.
    GrantRole {
        member: usize,
    },
    /// Taking one of the highlighted member's roles away.
    RevokeRole {
        member: usize,
    },
}

#[derive(Debug, Clone)]
pub struct Picker {
    pub title: String,
    /// `(value, label)` — the value is what gets sent, the label what is read.
    pub items: Vec<(String, String)>,
    pub index: usize,
    pub kind: PickerKind,
}

#[derive(Debug, Clone)]
pub enum ConfirmAction {
    Delete {
        resource: usize,
        id: String,
    },
    /// Taking away somebody's access to the active organisation.
    RemoveMember {
        id: String,
        name: String,
    },
    Run {
        function: usize,
        body: Value,
    },
    SignOut,
}

#[derive(Debug, Clone)]
pub struct Confirm {
    pub prompt: String,
    pub action: ConfirmAction,
}

/// The sign-in screen, which owns the whole display until it succeeds.
pub enum SignIn {
    Menu {
        index: usize,
    },
    Form(Form),
    /// Waiting for the dashboard in the browser to hand a key back.
    Browser {
        url: String,
        opened: bool,
        key: oneshot::Receiver<Result<String, String>>,
    },
}

/// The doors on the first screen, in the order they are offered.
///
/// Registering is only one of them where the app actually allows it: an option
/// that always answers "registration is closed" is worse than no option.
pub fn sign_in_options(manifest: &Manifest) -> Vec<(&'static str, &'static str)> {
    let mut options = vec![
        (
            BROWSER_DOOR,
            "Sign in there and it sends a key straight back to this console.",
        ),
        (PASSWORD_DOOR, "The console mints and saves a key for you."),
        (API_KEY_DOOR, "For a key you already have."),
    ];
    if manifest.auth.allow_registration {
        options.push(("Create an account", "This app is open to new accounts."));
    }
    // Only where the server can actually send the link. Offering it otherwise
    // is the same mistake as offering registration on an app that has closed
    // it: a door that answers 404.
    if manifest.auth.password_reset_enabled {
        options.push((
            FORGOT_PASSWORD_DOOR,
            "We will email you a link to choose a new one.",
        ));
    }
    options
}

/// Labels the sign-in menu dispatches on.
///
/// By label rather than by index: the list is conditional — registration and
/// password reset each come and go with the app's configuration — so a `match`
/// on position would silently open the wrong door the moment one of them was
/// absent.
const BROWSER_DOOR: &str = "Open the dashboard in a browser";
const PASSWORD_DOOR: &str = "Sign in with an email and password";
const API_KEY_DOOR: &str = "Paste an API key";
const REGISTER_DOOR: &str = "Create an account";
const FORGOT_PASSWORD_DOOR: &str = "Forgot your password?";

// --- the console ------------------------------------------------------------

pub struct Cli {
    pub client: Client,
    pub manifest: Manifest,
    pub store: Store,
    pub target: super::Target,

    pub nav: Vec<NavItem>,
    pub nav_index: usize,
    pub focus: Focus,
    pub main: Main,

    pub sign_in: Option<SignIn>,
    pub picker: Option<Picker>,
    pub confirm: Option<Confirm>,
    pub help: bool,

    pub organizations: Vec<(String, String)>,
    /// Whether the list above is a fact. False when the lookup failed, which is
    /// not the same as belonging to none.
    pub organizations_known: bool,
    /// The signed-in user's record, when it could be read, and their id.
    pub identity: Option<Value>,
    pub identity_id: Option<String>,
    /// Why the account could not be named, when it could not be.
    pub identity_note: Option<String>,
    /// Every role the caller holds in the active organisation, primary first.
    pub roles: Vec<String>,
    /// Whether that list is a fact. False when the app will not tell us —
    /// which is not the same as holding none, and must never be read as it.
    pub roles_known: bool,
    pub status: String,
    pub error: Option<String>,
    pub quit: bool,
    /// Printed once the screen has been given back.
    pub farewell: Option<String>,
}

impl Cli {
    pub fn new(client: Client, manifest: Manifest, store: Store, target: super::Target) -> Cli {
        let nav = navigation(&manifest, &Reach::unknown());
        Cli {
            client,
            manifest,
            store,
            target,
            nav,
            nav_index: 0,
            focus: Focus::Nav,
            main: Main::Empty("Pick something from the sidebar.".into()),
            sign_in: None,
            picker: None,
            confirm: None,
            help: false,
            organizations: Vec::new(),
            organizations_known: false,
            identity: None,
            identity_id: None,
            identity_note: None,
            roles: Vec::new(),
            roles_known: false,
            status: String::new(),
            error: None,
            quit: false,
            farewell: None,
        }
    }

    /// Decide what the first screen is.
    pub async fn start(&mut self) {
        if self.client.credentials.is_empty() {
            self.sign_in = Some(SignIn::Menu { index: 0 });
            return;
        }
        self.after_sign_in().await;
    }

    /// Everything that has to happen once we hold credentials.
    async fn after_sign_in(&mut self) {
        self.sign_in = None;
        self.load_identity().await;
        self.load_organizations().await;
        self.load_roles().await;
        self.rebuild_nav();
        self.status = format!("Connected to {}", self.client.origin);
        self.open_selected().await;
    }

    /// Poll anything happening off to the side. Called between frames.
    pub async fn tick(&mut self) {
        let arrived = match &mut self.sign_in {
            Some(SignIn::Browser { key, .. }) => match key.try_recv() {
                Ok(result) => Some(result),
                Err(oneshot::error::TryRecvError::Empty) => None,
                Err(oneshot::error::TryRecvError::Closed) => {
                    Some(Err("the browser handoff stopped listening".into()))
                }
            },
            _ => None,
        };
        match arrived {
            Some(Ok(key)) => {
                self.client.credentials.api_key = Some(key);
                self.save_credentials();
                self.after_sign_in().await;
            }
            Some(Err(message)) => {
                self.error = Some(message);
                self.sign_in = Some(SignIn::Menu { index: 0 });
            }
            None => {}
        }
    }

    // --- helpers ----------------------------------------------------------

    /// The ways in this app offers.
    pub fn doors(&self) -> Vec<(&'static str, &'static str)> {
        sign_in_options(&self.manifest)
    }

    pub fn resource(&self, index: usize) -> Option<&ResourceManifest> {
        self.manifest.resources.get(index)
    }

    pub fn agent(&self, index: usize) -> Option<&AgentManifest> {
        self.manifest.agents.get(index)
    }

    pub fn function(&self, index: usize) -> Option<&FunctionManifest> {
        self.manifest.functions.get(index)
    }

    pub fn organization_label(&self) -> String {
        match &self.client.organization {
            Some(id) => self
                .organizations
                .iter()
                .find(|(value, _)| value == id)
                .map(|(_, label)| label.clone())
                .unwrap_or_else(|| id.clone()),
            None => "no organization".into(),
        }
    }

    fn say(&mut self, message: impl Into<String>) {
        self.status = message.into();
        self.error = None;
    }

    fn fail(&mut self, error: impl std::fmt::Display) {
        self.error = Some(error.to_string());
    }

    fn save_credentials(&mut self) {
        let saved = Saved {
            api_key: self.client.credentials.api_key.clone(),
            organization: self.client.organization.clone(),
        };
        // Nothing to save is not a failure: signing in with a password when the
        // app has no `api_key` resource still gives a working session.
        if saved.api_key.is_none() {
            return;
        }
        let origin = self.client.origin.clone();
        if let Err(problem) = self.store.remember(&origin, saved) {
            self.status = format!("Signed in, but the key could not be saved: {problem}");
        }
    }

    /// Load the organisations the caller belongs to.
    ///
    /// A request that *fails* is not the same fact as a caller who belongs to
    /// nothing, and conflating them is how someone with an organisation gets
    /// asked to create one. So a failure is reported and remembered as "we do
    /// not know", not quietly recorded as "none".
    async fn load_organizations(&mut self) {
        self.organizations_known = false;

        // An app whose `organization` resource nobody may list is an app that
        // does not put organisations in front of operators; there is nothing to
        // load, and nothing to complain about either.
        let listable = self
            .manifest
            .resources
            .iter()
            .find(|resource| resource.name == "organization")
            .is_some_and(|resource| resource.permissions.list.possible());
        if !listable {
            self.organizations = Vec::new();
            return;
        }

        match self.client.organizations().await {
            Ok(organizations) => {
                // A remembered organisation the caller no longer belongs to
                // would silently scope every list to nothing.
                let known = self
                    .client
                    .organization
                    .as_ref()
                    .is_some_and(|current| organizations.iter().any(|(id, _)| id == current));
                if !known {
                    self.client.organization = organizations.first().map(|(id, _)| id.clone());
                }
                self.organizations = organizations;
                self.organizations_known = true;
            }
            Err(error) => {
                self.organizations = Vec::new();
                self.fail(format!("could not list your organizations: {error}"));
            }
        }
    }

    /// Work out who the credentials belong to, for the Session screen.
    ///
    /// There is no "who am I" endpoint, so this takes the two routes the API
    /// does offer: a session token carries the user id in its subject claim,
    /// and an API key can find its own row — `api_key` is listed by owner, so
    /// whatever comes back is the caller's.
    async fn load_identity(&mut self) {
        self.identity = None;
        self.identity_id = None;
        self.identity_note = None;

        let mut id = self
            .client
            .credentials
            .token
            .as_deref()
            .and_then(jwt_subject);

        if id.is_none() {
            if let Ok(rows) = self.client.list("api_key", &[("limit", "1".into())]).await {
                id = rows
                    .first()
                    .and_then(|row| row.get("owner_id"))
                    .and_then(Value::as_str)
                    .map(str::to_string);
            }
        }

        let Some(id) = id else { return };
        // Reading the row is a bonus, not a requirement: `user` is readable by
        // people you share an organisation with, so someone in none cannot read
        // even themselves. The id alone still beats showing nothing.
        self.identity = self.client.read("user", &id).await.ok();
        self.identity_id = Some(id);
    }

    /// What the console was pointed at — a server address, or the directory
    /// whose `main.toml` supplied one.
    pub fn target_label(&self) -> String {
        match &self.target {
            super::Target::Server(_) => self.client.origin.clone(),
            super::Target::Dir(dir) => format!("{} ({})", self.client.origin, dir.display()),
        }
    }

    /// How to name whoever is signed in.
    pub fn identity_label(&self) -> String {
        if let Some(record) = &self.identity {
            let field = &self.manifest.auth.identity_field;
            for key in [field.as_str(), "email", "display_name", "name"] {
                if let Some(text) = record.get(key).map(api::scalar) {
                    if !text.is_empty() {
                        return text;
                    }
                }
            }
        }
        match &self.identity_id {
            Some(id) => id.clone(),
            None if self.client.credentials.api_key.is_some() => "an API key".into(),
            None => "unknown".into(),
        }
    }

    // --- roles ------------------------------------------------------------

    /// Rebuild the sidebar for what the caller turns out to be able to reach.
    ///
    /// Roles are per organisation, so this is not a one-off at startup: it runs
    /// again whenever they change or the active organisation does.
    fn rebuild_nav(&mut self) {
        let here = self.nav.get(self.nav_index).map(|item| item.kind);
        self.nav = navigation(&self.manifest, &self.reach());
        // Keep the cursor on whatever it was on, when that still exists.
        self.nav_index = here
            .and_then(|kind| self.nav.iter().position(|item| item.kind == kind))
            .unwrap_or(0)
            .min(self.nav.len().saturating_sub(1));
    }

    /// What is known about the caller right now.
    pub fn reach(&self) -> Reach<'_> {
        Reach {
            signed_in: !self.client.credentials.is_empty(),
            organization: self.client.organization.is_some(),
            roles: &self.roles,
            roles_known: self.roles_known,
        }
    }

    /// Whether the caller may take this action, as far as the manifest can say.
    ///
    /// The API is still the authority — `owner` narrows to your own rows, and
    /// only the server knows which those are. This decides what to put in front
    /// of somebody, which is what the dashboard uses the same rule for.
    pub fn may(&self, resource: &ResourceManifest, action: &api::ActionPermission) -> bool {
        self.reach().may(resource.scope == "global", action)
    }

    /// Whether a resource is in the manifest and listable at all.
    fn listable(&self, name: &str) -> bool {
        self.manifest
            .resources
            .iter()
            .any(|resource| resource.name == name && resource.permissions.list.possible())
    }

    /// The caller's own roles in the active organisation.
    ///
    /// Deliberately narrow: this asks for one membership rather than the whole
    /// team, because it runs on every sign-in and every organisation switch,
    /// and an app may well let you see your own membership and nobody else's.
    async fn load_roles(&mut self) {
        self.roles = Vec::new();
        self.roles_known = false;

        let Some(me) = self.identity_id.clone() else {
            return;
        };
        if self.client.organization.is_none() || !self.listable("membership") {
            return;
        }
        let query = [("user_id", me), ("limit", "1".into())];
        let Ok(rows) = self.client.list("membership", &query).await else {
            return;
        };
        // No membership row is a fact too: you hold no roles here.
        let Some(row) = rows.first() else {
            self.roles_known = true;
            return;
        };
        let Some(id) = row.get("id").map(api::scalar).filter(|id| !id.is_empty()) else {
            return;
        };
        let Some(grants) = self.grants_for(&id).await else {
            // The primary role alone is worth showing, but it is not the whole
            // list, so nothing may be hidden on the strength of it.
            self.roles = role_of(row).into_iter().collect();
            return;
        };
        let member = Member {
            membership_id: id.clone(),
            name: String::new(),
            primary: role_of(row),
            grants,
            is_me: true,
        };
        self.roles = member.roles();
        self.roles_known = true;
    }

    /// The `membership_role` rows belonging to one membership, or `None` when
    /// this caller cannot see them.
    ///
    /// An app is free to drop that resource, in which case there are no grants
    /// and the primary role is the whole list — a known answer. An app that has
    /// it but will not let an operator list it is the other case: the grants
    /// exist and we cannot read them, so the answer is "we do not know".
    async fn grants_for(&self, membership_id: &str) -> Option<Vec<(String, String)>> {
        let declared = self
            .manifest
            .resources
            .iter()
            .any(|resource| resource.name == "membership_role");
        if !declared {
            return Some(Vec::new());
        }
        if !self.listable("membership_role") {
            return None;
        }
        let query = [
            ("membership_id", membership_id.to_string()),
            ("limit", "100".into()),
        ];
        let rows = self.client.list("membership_role", &query).await.ok()?;
        Some(rows.iter().filter_map(grant_of).collect())
    }

    /// Everyone in the active organisation, with their roles.
    async fn fetch_members(&mut self) -> Result<Vec<Member>, String> {
        let full = [("limit", "200".into()), ("expand", "user".into())];
        let rows = match self.client.list("membership", &full).await {
            Ok(rows) => rows,
            // `user` is readable by the people you share an organisation with,
            // and an app may narrow even that. A refused expansion is a reason
            // to show ids instead of names, not to show no team at all.
            Err(_) => self
                .client
                .list("membership", &[("limit", "200".into())])
                .await
                .map_err(|error| error.to_string())?,
        };

        // One request for every grant in the organisation, not one per member:
        // a team of forty should not be forty round trips. A failure here is
        // reported rather than swallowed — a Team screen quietly missing half
        // of somebody's roles is worse than one that says it could not load.
        let grants = if self.listable("membership_role") {
            self.client
                .list("membership_role", &[("limit", "500".into())])
                .await
                .map_err(|error| format!("the roles could not be loaded: {error}"))?
        } else {
            Vec::new()
        };

        Ok(rows
            .iter()
            .filter_map(|row| {
                let membership_id = row.get("id").map(api::scalar).filter(|id| !id.is_empty())?;
                let user_id = row.get("user_id").map(api::scalar).unwrap_or_default();
                Some(Member {
                    is_me: self
                        .identity_id
                        .as_ref()
                        .is_some_and(|me| *me == user_id && !user_id.is_empty()),
                    name: self.member_name(row, &user_id),
                    grants: grants
                        .iter()
                        .filter(|grant| {
                            grant.get("membership_id").map(api::scalar).as_deref()
                                == Some(membership_id.as_str())
                        })
                        .filter_map(grant_of)
                        .collect(),
                    primary: role_of(row),
                    membership_id,
                })
            })
            .collect())
    }

    /// How to name a member: their identity field, then whatever the `user`
    /// resource calls its records, then the bare id.
    fn member_name(&self, row: &Value, user_id: &str) -> String {
        if let Some(user) = row.get("user").filter(|value| value.is_object()) {
            let field = self.manifest.auth.identity_field.clone();
            for key in [field.as_str(), "email", "display_name", "name"] {
                if let Some(text) = user.get(key).map(api::scalar).filter(|t| !t.is_empty()) {
                    return text;
                }
            }
            if let Some(resource) = self.manifest.resources.iter().find(|r| r.name == "user") {
                let title = resource.title_of(user);
                if !title.is_empty() {
                    return title;
                }
            }
        }
        user_id.to_string()
    }

    /// Load the Team screen, keeping the cursor where it was.
    async fn load_team(&mut self) {
        if self.client.organization.is_none() {
            self.main = Main::Empty(
                "Roles belong to an organization, and this session has none. Press O to pick one."
                    .into(),
            );
            return;
        }
        let index = match &self.main {
            Main::Team(team) => team.index,
            _ => 0,
        };
        self.status = "Loading the team…".into();
        match self.fetch_members().await {
            Ok(members) => {
                // Whatever the team says about *us* is fresher than what we
                // knew, and it is the same fact — read from the same two tables
                // in one go, so it is a known one.
                if let Some(me) = members.iter().find(|member| member.is_me) {
                    self.roles = me.roles();
                    self.roles_known = true;
                    self.rebuild_nav();
                }
                let count = members.len();
                self.main = Main::Team(Team {
                    index: index.min(count.saturating_sub(1)),
                    manage: self
                        .manifest
                        .resources
                        .iter()
                        .find(|resource| resource.name == "membership_role")
                        .is_some_and(|resource| self.may(resource, &resource.permissions.create)),
                    members,
                });
                self.say(format!("{count} in {}", self.organization_label()));
            }
            Err(error) => {
                self.main = Main::Team(Team {
                    members: Vec::new(),
                    index: 0,
                    manage: false,
                });
                self.fail(error);
            }
        }
    }

    /// The roles this app names, for the pickers.
    ///
    /// The manifest collects every role mentioned in a permission, a function
    /// or a field's options. An app that names none still has the two the
    /// framework itself creates memberships with.
    fn known_roles(&self) -> Vec<String> {
        let known = &self.manifest.auth.known_roles;
        if known.is_empty() {
            vec!["member".into(), ADMIN_ROLE.into()]
        } else {
            known.clone()
        }
    }

    /// Offer the records that point at the one on screen.
    ///
    /// The dashboard draws these underneath a record; a terminal has no room
    /// for that, so they are a list you open — the ordinary list screen, pinned
    /// to this parent.
    fn open_children_picker(&mut self) {
        let Some((_, resource)) = self.current_resource() else {
            return;
        };
        let Some(record) = self.current_record() else {
            return;
        };
        let Some(id) = record
            .get("id")
            .map(api::scalar)
            .filter(|id| !id.is_empty())
        else {
            return self.say("That record has no id to look anything up by.");
        };

        let items: Vec<(String, String)> = resource
            .children
            .iter()
            .filter(|child| {
                self.manifest.resources.iter().any(|target| {
                    target.name == child.resource && target.permissions.list.possible()
                })
            })
            .map(|child| {
                (
                    format!("{}\u{1f}{}", child.resource, child.field),
                    child.label.clone(),
                )
            })
            .collect();
        if items.is_empty() {
            return self.say(format!(
                "Nothing else in this app points at a {}.",
                resource.label.to_lowercase()
            ));
        }
        self.picker = Some(Picker {
            title: format!("Belonging to {}", resource.title_of(&record)),
            items,
            index: 0,
            kind: PickerKind::Children {
                parent: id,
                label: resource.title_of(&record),
            },
        });
    }

    fn start_add_member(&mut self) {
        if self.client.organization.is_none() {
            return self.say("Pick an organization first — a team belongs to one.");
        }
        let may = self
            .manifest
            .resources
            .iter()
            .find(|resource| resource.name == "membership")
            .is_some_and(|resource| self.may(resource, &resource.permissions.create));
        if !may {
            return self.say("You may not add people to this organization.");
        }
        let roles = self.known_roles();
        self.main = Main::Form(Form::add_member(&self.manifest, &roles));
    }

    fn ask_remove_member(&mut self) {
        let Main::Team(team) = &self.main else { return };
        let Some(member) = team.members.get(team.index) else {
            return;
        };
        // Removing your own membership is the other way to leave an
        // organisation without an admin, and the server refuses it for the same
        // reason it refuses dropping your own admin role.
        if member.is_me && member.roles().iter().any(|role| role == ADMIN_ROLE) {
            return self.say(
                "You cannot remove your own access while you are an admin here — another admin \
                 can do it for you.",
            );
        }
        self.confirm = Some(Confirm {
            prompt: format!(
                "Remove {} from {}? They lose access to everything in it; their account itself \
                 is not deleted.",
                member.name,
                self.organization_label()
            ),
            action: ConfirmAction::RemoveMember {
                id: member.membership_id.clone(),
                name: member.name.clone(),
            },
        });
    }

    fn open_grant_picker(&mut self) {
        let Main::Team(team) = &self.main else { return };
        if !team.manage {
            return self.say("You may not hand out roles in this organization.");
        }
        let index = team.index;
        let Some(member) = team.members.get(index) else {
            return;
        };
        let held = member.roles();
        let name = member.name.clone();
        let items: Vec<(String, String)> = self
            .known_roles()
            .into_iter()
            // Never offer a role someone already holds: the server refuses a
            // second copy, and a second copy would make revoking the first look
            // like it did nothing.
            .filter(|role| !held.contains(role))
            .map(|role| (role.clone(), role))
            .collect();
        if items.is_empty() {
            return self.say(format!("{name} already holds every role this app names."));
        }
        self.picker = Some(Picker {
            title: format!("Give {name} a role"),
            items,
            index: 0,
            kind: PickerKind::GrantRole { member: index },
        });
    }

    fn open_revoke_picker(&mut self) {
        let Main::Team(team) = &self.main else { return };
        let index = team.index;
        let Some(member) = team.members.get(index) else {
            return;
        };
        let name = member.name.clone();
        let mine = member.is_me;
        // The value is the grant row to delete; the primary role has none, so
        // it is revoked by clearing the column instead and carries no id.
        let items: Vec<(String, String)> = member
            .roles()
            .into_iter()
            .filter(|role| member.may_revoke(role))
            .map(|role| {
                let id = member
                    .grants
                    .iter()
                    .find(|(_, held)| *held == role)
                    .map(|(id, _)| id.clone())
                    .unwrap_or_default();
                (format!("{id}\u{1f}{role}"), role)
            })
            .collect();
        if items.is_empty() {
            return self.say(if mine {
                "You cannot remove your own admin role — another admin can do it for you."
                    .to_string()
            } else {
                format!("{name} holds no role you can take away.")
            });
        }
        self.picker = Some(Picker {
            title: format!("Take a role from {name}"),
            items,
            index: 0,
            kind: PickerKind::RevokeRole { member: index },
        });
    }

    async fn grant_role(&mut self, member: usize, role: String) {
        let Main::Team(team) = &self.main else { return };
        let Some(member) = team.members.get(member) else {
            return;
        };
        let (id, name, mine) = (
            member.membership_id.clone(),
            member.name.clone(),
            member.is_me,
        );
        let body = serde_json::json!({ "membership_id": id, "role": role });
        match self.client.create("membership_role", body).await {
            Ok(_) => {
                self.say(format!("{name} is also {role}."));
                self.after_role_change(mine).await;
            }
            Err(error) => self.fail(error),
        }
    }

    async fn revoke_role(&mut self, member: usize, grant_id: String, role: String) {
        let Main::Team(team) = &self.main else { return };
        let Some(member) = team.members.get(member) else {
            return;
        };
        let (id, name, mine) = (
            member.membership_id.clone(),
            member.name.clone(),
            member.is_me,
        );
        let done = if grant_id.is_empty() {
            // The primary role has no row to delete; clearing the column is how
            // it goes away.
            self.client
                .update("membership", &id, serde_json::json!({ "role": null }))
                .await
                .map(|_| ())
        } else {
            self.client.delete("membership_role", &grant_id).await
        };
        match done {
            Ok(()) => {
                self.say(format!("{name} is no longer {role}."));
                self.after_role_change(mine).await;
            }
            Err(error) => self.fail(error),
        }
    }

    /// Re-read the screen after a role moved — and our own permissions too,
    /// when the role that moved was ours.
    async fn after_role_change(&mut self, was_me: bool) {
        let said = self.status.clone();
        self.load_team().await;
        if was_me {
            self.load_roles().await;
            self.rebuild_nav();
        }
        self.status = said;
    }

    // --- opening things ---------------------------------------------------

    /// Show whatever the sidebar selection points at.
    pub async fn open_selected(&mut self) {
        let Some(item) = self.nav.get(self.nav_index).cloned() else {
            return;
        };
        match item.kind {
            NavKind::Resource(index) => {
                self.main = Main::List(List {
                    resource: index,
                    rows: Vec::new(),
                    index: 0,
                    page: 0,
                    search: String::new(),
                    searching: false,
                    cursor: 0,
                    filter: None,
                    filter_label: None,
                });
                self.reload().await;
            }
            NavKind::Agent(index) => {
                self.main = Main::Agent(AgentChat {
                    agent: index,
                    threads: Vec::new(),
                    thread_index: 0,
                    thread_id: None,
                    messages: Vec::new(),
                    draft: String::new(),
                    cursor: 0,
                    editing: false,
                    browse_note: None,
                });
                self.reload_agent().await;
            }
            NavKind::Function(index) => {
                let Some(function) = self.function(index) else {
                    return;
                };
                self.main = Main::Form(Form::run(index, function));
            }
            NavKind::Account => {
                // Read the record fresh: what we loaded at sign-in was for a
                // name in the header, and this is a form somebody is about to
                // save over.
                if let Some(id) = self.identity_id.clone() {
                    if let Ok(record) = self.client.read("user", &id).await {
                        self.identity = Some(record);
                    }
                }
                self.main = Main::Form(Form::profile(&self.manifest, self.identity.as_ref()));
                if self.identity_id.is_none() {
                    self.fail(
                        "the console could not work out which account these credentials belong \
                         to, so there is nothing to edit here",
                    );
                }
            }
            NavKind::Team => {
                self.main = Main::Team(Team {
                    members: Vec::new(),
                    index: 0,
                    manage: false,
                });
                self.load_team().await;
            }
            NavKind::Session => self.main = Main::Session,
        }
    }

    async fn reload_agent(&mut self) {
        let (agent_index, current_thread) = match &self.main {
            Main::Agent(chat) => (chat.agent, chat.thread_id.clone()),
            _ => return,
        };
        let Some(agent) = self.agent(agent_index).cloned() else {
            return;
        };

        let mut threads = Vec::new();
        let mut browse_note = None;
        if agent.storage {
            if let Some(resource) = agent.thread_resource.as_deref() {
                match self.client.list(resource, &[("limit", "100".into())]).await {
                    Ok(rows) => {
                        threads = rows
                            .iter()
                            .filter_map(|row| {
                                let id =
                                    row.get("id").map(api::scalar).filter(|id| !id.is_empty())?;
                                let title = row
                                    .get("title")
                                    .map(api::scalar)
                                    .filter(|title| !title.is_empty())
                                    .unwrap_or_else(|| id.clone());
                                Some(AgentThread {
                                    id,
                                    title,
                                    updated_at: row
                                        .get("updated_at")
                                        .and_then(Value::as_str)
                                        .map(str::to_string),
                                })
                            })
                            .collect();
                    }
                    Err(error) => {
                        browse_note = Some(format!("Conversations cannot be listed here: {error}"));
                    }
                }
            }
        }

        let chosen = current_thread.or_else(|| threads.first().map(|thread| thread.id.clone()));
        let selected = chosen
            .as_ref()
            .and_then(|id| threads.iter().position(|thread| &thread.id == id))
            .unwrap_or(0);
        let message_result = self.load_agent_messages(&agent, chosen.as_deref()).await;
        let message_note = message_result.as_ref().err().map(|error| error.to_string());
        let messages = message_result.unwrap_or_default();

        if let Main::Agent(chat) = &mut self.main {
            if chat.agent != agent_index {
                return;
            }
            chat.threads = threads;
            chat.thread_index = selected;
            chat.thread_id = chosen;
            chat.messages = messages;
            chat.browse_note = browse_note.or(message_note);
        }
    }

    async fn load_agent_messages(
        &self,
        agent: &AgentManifest,
        thread_id: Option<&str>,
    ) -> Result<Vec<AgentMessage>, anyhow::Error> {
        let Some(thread_id) = thread_id else {
            return Ok(Vec::new());
        };
        let Some(resource) = agent.message_resource.as_deref() else {
            return Ok(Vec::new());
        };
        let rows = self
            .client
            .list(
                resource,
                &[
                    ("thread_id", thread_id.to_string()),
                    ("limit", "500".into()),
                ],
            )
            .await?;
        Ok(rows
            .into_iter()
            .rev()
            .map(|row| {
                let provider = row
                    .get("provider")
                    .map(api::scalar)
                    .filter(|text| !text.is_empty());
                let model = row
                    .get("model")
                    .map(api::scalar)
                    .filter(|text| !text.is_empty());
                let finish = row
                    .get("finish_reason")
                    .map(api::scalar)
                    .filter(|text| !text.is_empty());
                let tool = row
                    .get("tool_name")
                    .map(api::scalar)
                    .filter(|text| !text.is_empty())
                    .map(|name| format!("tool: {name}"));
                let call = row
                    .get("tool_call_id")
                    .map(api::scalar)
                    .filter(|text| !text.is_empty())
                    .map(|id| format!("call: {id}"));
                let meta = [tool, call, provider, model, finish]
                    .into_iter()
                    .flatten()
                    .collect::<Vec<_>>()
                    .join(" · ");
                AgentMessage {
                    role: row
                        .get("role")
                        .map(api::scalar)
                        .filter(|text| !text.is_empty())
                        .unwrap_or_else(|| "user".into()),
                    content: row.get("content").map(api::scalar).unwrap_or_default(),
                    meta: (!meta.is_empty()).then_some(meta),
                }
            })
            .collect())
    }

    async fn open_agent_thread(&mut self, thread_id: String) {
        let Some((agent_index, agent_name)) = (match &self.main {
            Main::Agent(chat) => self
                .agent(chat.agent)
                .map(|agent| (chat.agent, agent.label.clone())),
            _ => None,
        }) else {
            return;
        };
        let Some(agent) = self.agent(agent_index).cloned() else {
            return;
        };
        match self.load_agent_messages(&agent, Some(&thread_id)).await {
            Ok(messages) => {
                if let Main::Agent(chat) = &mut self.main {
                    chat.thread_id = Some(thread_id.clone());
                    chat.messages = messages;
                    if let Some(index) = chat
                        .threads
                        .iter()
                        .position(|thread| thread.id == thread_id)
                    {
                        chat.thread_index = index;
                    }
                }
                self.say(format!(
                    "Opened a {} conversation.",
                    agent_name.to_lowercase()
                ));
            }
            Err(error) => self.fail(error),
        }
    }

    /// The relations worth asking the API to inline: the ones whose target this
    /// caller may read at all.
    fn expandable(&self, resource: &ResourceManifest) -> Vec<String> {
        resource
            .relations
            .iter()
            .filter(|relation| {
                self.manifest
                    .resources
                    .iter()
                    .find(|target| target.name == relation.target)
                    .is_some_and(|target| target.permissions.read.possible())
            })
            .map(|relation| relation.relation.clone())
            .collect()
    }

    /// Re-run the current list query.
    pub async fn reload(&mut self) {
        let Main::List(list) = &self.main else { return };
        let (resource_index, page, search) = (list.resource, list.page, list.search.clone());
        let filter = list.filter.clone();
        let Some(resource) = self.resource(resource_index).cloned() else {
            return;
        };

        let mut query: Vec<(&str, String)> = vec![
            ("limit", PAGE.to_string()),
            ("offset", (page * PAGE).to_string()),
        ];
        // The manifest names the one field the dashboard's search box uses, so
        // the console searches the same one — through the API's `field~`
        // substring match, which is what typing half a name means here too.
        let search_field = resource
            .search_field
            .as_deref()
            .map(|field| format!("{field}~"));
        if let (Some(field), false) = (search_field.as_deref(), search.trim().is_empty()) {
            query.push((field, search.trim().to_string()));
        }
        // A child list is the same list with the parent pinned on it.
        if let Some((field, value)) = &filter {
            query.push((field.as_str(), value.clone()));
        }

        // Ask the API to inline what each row points at, so a column holding a
        // uuid can show "Acme Ltd" instead. A relation whose target this caller
        // may not read makes the whole request fail, and a table nobody can see
        // is worse than a table of ids — so that answer is retried plain.
        let relations = self.expandable(&resource);
        self.status = format!("Loading {}…", resource.plural.to_lowercase());
        let mut expanded = query.clone();
        expanded.extend(api::expand_query(&relations));
        let answer = match self.client.list(&resource.name, &expanded).await {
            Err(error) if !relations.is_empty() => self
                .client
                .list(&resource.name, &query)
                .await
                .map_err(|_| error),
            other => other,
        };
        match answer {
            Ok(rows) => {
                let count = rows.len();
                if let Main::List(list) = &mut self.main {
                    list.index = list.index.min(count.saturating_sub(1));
                    list.rows = rows;
                }
                self.say(format!(
                    "{count} {} on page {}",
                    resource.plural.to_lowercase(),
                    page + 1
                ));
            }
            Err(error) => {
                if let Main::List(list) = &mut self.main {
                    list.rows = Vec::new();
                }
                self.fail(error);
            }
        }
    }

    /// Open the highlighted row.
    async fn open_row(&mut self) {
        let Main::List(list) = &self.main else { return };
        let Some(row) = list.rows.get(list.index).cloned() else {
            return;
        };
        let resource_index = list.resource;
        let Some(resource) = self.resource(resource_index).cloned() else {
            return;
        };

        // The list may have been trimmed by the server; re-reading the record
        // is what makes the detail screen show everything it is allowed to.
        let relations = self.expandable(&resource);
        let record = match row.get("id").and_then(Value::as_str) {
            Some(id) if resource.permissions.read.possible() => {
                match self
                    .client
                    .read_expanding(&resource.name, id, &relations)
                    .await
                {
                    Ok(record) => record,
                    Err(error) => {
                        self.fail(error);
                        row
                    }
                }
            }
            _ => row,
        };
        self.main = Main::Detail(Detail {
            resource: resource_index,
            record,
            scroll: 0,
        });
    }

    fn start_create(&mut self) {
        let Some((index, resource)) = self.current_resource() else {
            return;
        };
        if !self.may(&resource, &resource.permissions.create) {
            let note = resource.permissions.create.note.clone();
            self.fail(format!(
                "{} cannot be created here. {note}",
                resource.plural
            ));
            return;
        }
        // A key is not made by writing a row: the plaintext exists once, in the
        // reply from `/auth/apikeys`, and the table only ever holds its hash.
        if resource.name == "api_key" {
            self.main = Main::Form(Form::new_api_key());
            return;
        }
        self.main = Main::Form(Form::create(index, &resource));
    }

    /// Open the form for starting another organisation, from wherever the
    /// console offers it.
    fn start_new_organization(&mut self) {
        let found = self
            .manifest
            .resources
            .iter()
            .enumerate()
            .find(|(_, resource)| resource.name == "organization")
            .map(|(index, resource)| (index, resource.clone()));
        let Some((index, resource)) = found else {
            return self.fail("this app has no organization resource".to_string());
        };
        if !self.may(&resource, &resource.permissions.create) {
            let note = resource.permissions.create.note.clone();
            return self.fail(format!(
                "{} cannot be created here. {note}",
                resource.plural
            ));
        }
        self.main = Main::Form(Form::new_organization(index, &resource));
    }

    fn start_edit(&mut self) {
        let Some((index, resource)) = self.current_resource() else {
            return;
        };
        if !self.may(&resource, &resource.permissions.update) {
            let note = resource.permissions.update.note.clone();
            self.fail(format!("{} cannot be edited here. {note}", resource.plural));
            return;
        }
        let Some(record) = self.current_record() else {
            return;
        };
        self.main = Main::Form(Form::edit(index, &resource, &record));
    }

    fn ask_delete(&mut self) {
        let Some((index, resource)) = self.current_resource() else {
            return;
        };
        if !self.may(&resource, &resource.permissions.delete) {
            let note = resource.permissions.delete.note.clone();
            self.fail(format!(
                "{} cannot be deleted here. {note}",
                resource.plural
            ));
            return;
        }
        let Some(record) = self.current_record() else {
            return;
        };
        let Some(id) = record.get("id").and_then(Value::as_str) else {
            self.fail("that record has no id to delete by");
            return;
        };
        self.confirm = Some(Confirm {
            prompt: format!(
                "Delete {} \"{}\"? This cannot be undone.",
                resource.label.to_lowercase(),
                resource.title_of(&record)
            ),
            action: ConfirmAction::Delete {
                resource: index,
                id: id.to_string(),
            },
        });
    }

    /// The resource behind whatever the main pane is showing.
    fn current_resource(&self) -> Option<(usize, ResourceManifest)> {
        let index = match &self.main {
            Main::List(list) => list.resource,
            Main::Detail(detail) => detail.resource,
            _ => return None,
        };
        Some((index, self.resource(index)?.clone()))
    }

    /// The record the cursor is on, from a list row or the detail screen.
    fn current_record(&self) -> Option<Value> {
        match &self.main {
            Main::List(list) => list.rows.get(list.index).cloned(),
            Main::Detail(detail) => Some(detail.record.clone()),
            _ => None,
        }
    }

    // --- submitting -------------------------------------------------------

    /// Collect a form into the JSON body it describes.
    fn body_of(form: &Form, only_changes: bool) -> Result<Value, String> {
        if form.raw_body {
            let field = &form.fields[0];
            return serde_json::from_str(field.value.trim())
                .map_err(|e| format!("the request body is not valid JSON: {e}"));
        }
        let mut body = Map::new();
        for field in &form.fields {
            if only_changes && !field.changed() {
                continue;
            }
            // The second password box is a check, not a column: it is compared
            // by `confirm_passwords` and never sent anywhere.
            if field.name == CONFIRM_PASSWORD {
                continue;
            }
            // An empty box means "no opinion", which for a create is the
            // difference between sending `""` and letting the default apply.
            if field.value.trim().is_empty() && !field.toggleable() {
                if field.required && !only_changes {
                    return Err(format!("`{}` is required", field.label));
                }
                continue;
            }
            let value = api::parse_typed(&field.ty, &field.label, &field.value)
                .map_err(|e| e.to_string())?;
            body.insert(field.name.clone(), value);
        }
        Ok(Value::Object(body))
    }

    async fn submit(&mut self) {
        let Main::Form(form) = self.main.clone() else {
            let Some(SignIn::Form(form)) = &self.sign_in else {
                return;
            };
            let form = form.clone();
            self.submit_sign_in(form).await;
            return;
        };

        match form.kind.clone() {
            FormKind::Create { resource } => {
                let Some(resource) = self.resource(resource).cloned() else {
                    return;
                };
                let body = match Self::body_of(&form, false) {
                    Ok(body) => body,
                    Err(problem) => return self.fail(problem),
                };
                self.status = format!("Creating {}…", resource.label.to_lowercase());
                match self.client.create(&resource.name, body).await {
                    Ok(record) => {
                        self.say(format!("Created {}.", resource.title_of(&record)));
                        self.back_to_list().await;
                    }
                    Err(error) => self.fail(error),
                }
            }
            FormKind::Update { resource, id } => {
                let Some(resource) = self.resource(resource).cloned() else {
                    return;
                };
                let body = match Self::body_of(&form, true) {
                    Ok(body) => body,
                    Err(problem) => return self.fail(problem),
                };
                if body.as_object().is_some_and(Map::is_empty) {
                    self.say("Nothing changed.");
                    return self.back_to_list().await;
                }
                self.status = "Saving…".into();
                match self.client.update(&resource.name, &id, body).await {
                    Ok(record) => {
                        self.say(format!("Saved {}.", resource.title_of(&record)));
                        self.back_to_list().await;
                    }
                    Err(error) => self.fail(error),
                }
            }
            FormKind::Run { function } => {
                let Some(function) = self.function(function).cloned() else {
                    return;
                };
                let body = match Self::body_of(&form, false) {
                    Ok(body) => body,
                    Err(problem) => return self.fail(problem),
                };
                // A function that declared a confirmation wants one every time,
                // including from here.
                if let Some(prompt) = function.confirm.clone() {
                    let index = self
                        .manifest
                        .functions
                        .iter()
                        .position(|f| f.name == function.name)
                        .unwrap_or_default();
                    self.confirm = Some(Confirm {
                        prompt,
                        action: ConfirmAction::Run {
                            function: index,
                            body,
                        },
                    });
                    return;
                }
                self.run_function(&function, body).await;
            }
            FormKind::Profile => self.save_profile(form).await,
            FormKind::NewApiKey => {
                let name = form.fields[0].value.trim().to_string();
                let name = if name.is_empty() { key_label() } else { name };
                self.issue_key(&name).await;
            }
            FormKind::AddMember => self.add_member(form).await,
            FormKind::SignInPassword
            | FormKind::SignInKey
            | FormKind::SignUp
            | FormKind::ForgotPassword => self.submit_sign_in(form).await,
            FormKind::NewOrganization { .. } => self.create_organization(form).await,
        }
    }

    /// Create an organisation and switch the console into it.
    async fn create_organization(&mut self, form: Form) {
        let FormKind::NewOrganization { resource } = form.kind else {
            return;
        };
        let Some(resource) = self.resource(resource).cloned() else {
            return;
        };
        let body = match Self::body_of(&form, false) {
            Ok(body) => body,
            Err(problem) => return self.fail(problem),
        };

        self.status = "Creating the organization…".into();
        match self.client.create(&resource.name, body).await {
            Ok(created) => {
                let label = resource.title_of(&created);
                // The server makes the creator an admin member, so the session
                // has to be re-read before anything will look right.
                self.load_organizations().await;
                if let Some(id) = created.get("id").and_then(Value::as_str) {
                    self.client.organization = Some(id.to_string());
                }
                self.save_credentials();
                self.load_roles().await;
                // Everything scoped to an organisation just became reachable.
                self.rebuild_nav();
                self.say(format!("{label} is ready. You are its admin."));
                self.open_selected().await;
            }
            Err(error) => self.fail(error),
        }
    }

    /// Save your own details.
    async fn save_profile(&mut self, form: Form) {
        let Some(id) = self.identity_id.clone() else {
            return self.fail(
                "there is no account to save — sign in with a password or a key \
                              whose owner this console can read",
            );
        };
        let body = match Self::body_of(&form, true) {
            Ok(body) => body,
            Err(problem) => return self.fail(problem),
        };
        if body.as_object().is_some_and(Map::is_empty) {
            return self.say("Nothing changed.");
        }
        self.status = "Saving…".into();
        match self.client.update("user", &id, body).await {
            Ok(record) => {
                self.identity = Some(record);
                self.say("Your details are saved.");
                self.main = Main::Form(Form::profile(&self.manifest, self.identity.as_ref()));
            }
            Err(error) => self.fail(error),
        }
    }

    /// Add somebody to the active organisation, by whichever door this
    /// deployment has.
    ///
    /// With email configured the address is *invited* — which is the version
    /// that works for somebody who has never been here. Without it, the only
    /// thing an organisation can do is admit an account that already exists,
    /// and the 404 that comes back for an address nobody holds is worth saying
    /// in words.
    async fn add_member(&mut self, form: Form) {
        let who = form.fields[0].value.trim().to_string();
        if who.is_empty() {
            return self.fail(format!(
                "`{}` is required",
                self.manifest.auth.identity_label
            ));
        }

        if self.manifest.auth.invitations_enabled {
            let role = form
                .fields
                .iter()
                .find(|field| field.name == "role")
                .map(|field| field.value.trim().to_string())
                .unwrap_or_default();
            self.status = format!("Inviting {who}…");
            return match self.client.invite(&who, &role).await {
                Ok(()) => {
                    self.say(format!(
                        "Invitation sent to {who}. They can accept it whether or not they \
                         already have an account."
                    ));
                    self.load_team().await;
                }
                Err(error) => self.fail(error),
            };
        }

        let body = match Self::body_of(&form, false) {
            Ok(body) => body,
            Err(problem) => return self.fail(problem),
        };
        self.status = format!("Adding {who}…");
        match self.client.create("membership", body).await {
            Ok(_) => {
                self.say(format!(
                    "{who} can now work in {}.",
                    self.organization_label()
                ));
                self.load_team().await;
            }
            // The server answers 404 when nobody is registered with that
            // identity, which is worth saying in words rather than as a code.
            Err(error) if error.to_string().contains("404") => self.fail(format!(
                "nobody is registered as `{who}`. They need an account before they can be \
                 added.\n{error}"
            )),
            Err(error) => self.fail(error),
        }
    }

    async fn run_function(&mut self, function: &FunctionManifest, body: Value) {
        self.status = format!("Running {}…", function.label);
        match self.client.invoke(function, body).await {
            Ok(output) => {
                self.say(format!("{} finished.", function.label));
                self.main = Main::Output {
                    title: format!("{} — result", function.label),
                    body: serde_json::to_string_pretty(&output)
                        .unwrap_or_else(|_| output.to_string()),
                    scroll: 0,
                };
            }
            Err(error) => self.fail(error),
        }
    }

    /// Leave a form and show the list it came from again.
    async fn back_to_list(&mut self) {
        self.open_selected().await;
    }

    // --- signing in -------------------------------------------------------

    async fn submit_sign_in(&mut self, form: Form) {
        match form.kind {
            FormKind::SignInKey => {
                let key = form.fields[0].value.trim().to_string();
                if key.is_empty() {
                    return self.fail("paste a key, or pick another way to sign in");
                }
                self.client.credentials.api_key = Some(key.clone());
                // A key that is not accepted should fail here, not on the first
                // list three screens later — so the check has to be a request
                // that *needs* the key. `/_health` is public: it answers 200 to
                // a key made of nonsense.
                //
                // Only a rejection is disqualifying. Any other failure says
                // something about the app — a locked-down `api_key` resource,
                // say — and not about the credential, so it is reported and the
                // key is kept.
                match self.client.list("api_key", &[("limit", "1".into())]).await {
                    Ok(_) => {
                        self.save_credentials();
                        self.after_sign_in().await;
                    }
                    Err(error) => {
                        let rejected = error.to_string().contains("401");
                        if rejected {
                            self.client.credentials.api_key = None;
                            // A rejected key is usually a damaged one, and the
                            // damage is invisible behind the mask. Say what is
                            // wrong with its shape without echoing the secret.
                            let detail = match key_shape(&key) {
                                Some(problem) => format!("that key was not accepted — {problem}"),
                                None => "that key was not accepted. It is well-formed, so it has \
                                         probably been revoked, or it belongs to another server."
                                    .to_string(),
                            };
                            return self.fail(format!("{detail}\n{error}"));
                        }
                        self.save_credentials();
                        self.after_sign_in().await;
                        self.fail(format!(
                            "signed in, but the key could not be verified.\n{error}"
                        ));
                    }
                }
            }
            FormKind::ForgotPassword => {
                let identity = form.fields[0].value.trim().to_string();
                if identity.is_empty() {
                    return self.fail("enter the address to send the link to");
                }
                self.status = "Asking for a reset link…".into();
                match self.client.forgot_password(&identity).await {
                    // Deliberately hedged: the endpoint answers the same way
                    // for an address with no account, so claiming a message was
                    // sent would be a claim the console cannot make.
                    Ok(()) => {
                        self.say(format!(
                            "If {identity} has an account, a reset link is on its way. \
                             Open it in a browser, then sign in here."
                        ));
                        self.main = Main::Form(Form::sign_in_password(&self.manifest));
                    }
                    Err(error) => self.fail(error),
                }
            }
            FormKind::SignInPassword | FormKind::SignUp => {
                let registering = form.kind == FormKind::SignUp;
                let identity = form.fields[0].value.trim().to_string();
                let field = self.manifest.auth.identity_field.clone();

                if registering {
                    if let Err(problem) = confirm_passwords(&form) {
                        return self.fail(problem);
                    }
                }

                self.status = if registering {
                    "Creating your account…".into()
                } else {
                    "Signing in…".to_string()
                };
                let door = if registering {
                    // Registration collects whatever else the app declared, so
                    // the form is the body rather than a fixed pair.
                    match Self::body_of(&form, false) {
                        Ok(body) => self.client.register(body).await,
                        Err(problem) => return self.fail(problem),
                    }
                } else {
                    let password = form.fields[1].value.clone();
                    self.client
                        .login(&field, &identity, &password)
                        .await
                        .map(Some)
                };
                match door {
                    // Registering into an app that confirms addresses produces
                    // an account and no session, on purpose. The console cannot
                    // open a mailbox, so it says where to go next.
                    Ok(None) => {
                        self.say(format!(
                            "Account created. Open the link sent to {identity} to confirm \
                             your address, then sign in here."
                        ));
                        self.main = Main::Form(Form::sign_in_password(&self.manifest));
                    }
                    Ok(Some(token)) => {
                        self.client.credentials.token = Some(token);
                        // Trade the session for a key so the next run is silent.
                        match self.client.create_api_key(&key_label()).await {
                            Ok(key) => {
                                self.client.credentials.api_key = Some(key);
                                self.client.credentials.token = None;
                                self.save_credentials();
                            }
                            Err(error) => {
                                self.status = format!(
                                    "Signed in for this session only — no key could be issued: {error}"
                                );
                            }
                        }
                        self.after_sign_in().await;
                    }
                    Err(error) => self.fail(error),
                }
            }
            _ => {}
        }
    }

    /// Hand the whole business to the dashboard in a browser.
    async fn start_browser_sign_in(&mut self) {
        let handoff = match Handoff::start(&self.client.admin_url(), &key_label()).await {
            Ok(handoff) => handoff,
            Err(error) => return self.fail(error),
        };
        let url = handoff.url.clone();
        let (sender, receiver) = oneshot::channel();
        tokio::spawn(async move {
            let _ = sender.send(handoff.wait().await.map_err(|e| e.to_string()));
        });

        let opened = link::open_browser(&url);
        self.sign_in = Some(SignIn::Browser {
            url,
            opened,
            key: receiver,
        });
        self.error = None;
    }

    fn sign_out(&mut self) {
        self.client.credentials = Default::default();
        let origin = self.client.origin.clone();
        let _ = self.store.forget(&origin);
        self.organizations.clear();
        self.organizations_known = false;
        self.identity = None;
        self.identity_id = None;
        self.identity_note = None;
        self.roles.clear();
        self.roles_known = false;
        self.rebuild_nav();
        self.main = Main::Empty(String::new());
        self.sign_in = Some(SignIn::Menu { index: 0 });
        self.say("Signed out. The saved key for this server was removed.");
    }

    /// Mint another key and show it — for pasting into a script or another
    /// machine, which is the whole reason to want one from here.
    async fn issue_key(&mut self, name: &str) {
        self.status = "Issuing a key…".into();
        match self.client.create_api_key(name).await {
            Ok(key) => {
                self.say("Copy this now — the server will not show it again.");
                self.main = Main::Output {
                    title: "New API key".into(),
                    body: key,
                    scroll: 0,
                };
            }
            Err(error) => self.fail(error),
        }
    }

    // --- overlays ---------------------------------------------------------

    fn open_organization_picker(&mut self) {
        if self.organizations.is_empty() {
            return self.say("This app has no organizations to switch between.");
        }
        let index = self
            .client
            .organization
            .as_ref()
            .and_then(|current| self.organizations.iter().position(|(id, _)| id == current))
            .unwrap_or(0);
        self.picker = Some(Picker {
            title: "Active organization".into(),
            items: self.organizations.clone(),
            index,
            kind: PickerKind::Organization,
        });
    }

    /// Offer the records a reference field could point at.
    async fn open_reference_picker(&mut self) {
        let Main::Form(form) = &self.main else { return };
        let index = form.index;
        let Some(field) = form.fields.get(index) else {
            return;
        };
        let Some(target) = field.references.clone() else {
            return;
        };
        let Some(resource) = self
            .manifest
            .resources
            .iter()
            .find(|resource| resource.name == target)
            .cloned()
        else {
            return self.fail(format!("this app does not expose `{target}`"));
        };

        self.status = format!("Loading {}…", resource.plural.to_lowercase());
        match self
            .client
            .list(&resource.name, &[("limit", "100".into())])
            .await
        {
            Ok(rows) => {
                let items: Vec<(String, String)> = rows
                    .iter()
                    .filter_map(|row| {
                        let id = row.get("id").map(api::scalar).filter(|id| !id.is_empty())?;
                        let label = resource.title_of(row);
                        Some((id.clone(), if label.is_empty() { id } else { label }))
                    })
                    .collect();
                if items.is_empty() {
                    return self.say(format!(
                        "There are no {} to pick.",
                        resource.plural.to_lowercase()
                    ));
                }
                self.say(format!("Pick a {}.", resource.label.to_lowercase()));
                self.picker = Some(Picker {
                    title: resource.label.clone(),
                    items,
                    index: 0,
                    kind: PickerKind::Reference { field: index },
                });
            }
            Err(error) => self.fail(error),
        }
    }

    async fn choose(&mut self) {
        let Some(picker) = self.picker.take() else {
            return;
        };
        let Some((value, label)) = picker.items.get(picker.index).cloned() else {
            return;
        };
        match picker.kind {
            PickerKind::Organization => {
                self.client.organization = Some(value);
                self.save_credentials();
                // Roles are per organisation, so the ones we hold here are not
                // the ones we held there.
                self.load_roles().await;
                // What you may reach is a fact about the organisation you are
                // in, so the sidebar is one of the things that changes.
                self.rebuild_nav();
                self.say(format!("Now working in {label}."));
                // Every org-scoped list on screen is now the wrong list.
                match self.main {
                    Main::List(_) => self.reload().await,
                    Main::Team(_) => self.load_team().await,
                    _ => {}
                }
            }
            PickerKind::Reference { field } => {
                if let Main::Form(form) = &mut self.main {
                    if let Some(field) = form.fields.get_mut(field) {
                        field.set(value);
                    }
                }
            }
            PickerKind::Children { parent, label } => {
                let Some((resource, field)) = value.split_once('\u{1f}') else {
                    return;
                };
                let Some(index) = self
                    .manifest
                    .resources
                    .iter()
                    .position(|entry| entry.name == resource)
                else {
                    return;
                };
                self.main = Main::List(List {
                    resource: index,
                    rows: Vec::new(),
                    index: 0,
                    page: 0,
                    search: String::new(),
                    searching: false,
                    cursor: 0,
                    filter: Some((field.to_string(), parent)),
                    filter_label: Some(label),
                });
                self.reload().await;
            }
            PickerKind::GrantRole { member } => self.grant_role(member, value).await,
            PickerKind::RevokeRole { member } => {
                let (grant_id, role) = value.split_once('\u{1f}').unwrap_or(("", value.as_str()));
                let (grant_id, role) = (grant_id.to_string(), role.to_string());
                self.revoke_role(member, grant_id, role).await;
            }
        }
    }

    async fn resolve_confirm(&mut self, agreed: bool) {
        let Some(confirm) = self.confirm.take() else {
            return;
        };
        if !agreed {
            return self.say("Cancelled.");
        }
        match confirm.action {
            ConfirmAction::Delete { resource, id } => {
                let Some(resource) = self.resource(resource).cloned() else {
                    return;
                };
                match self.client.delete(&resource.name, &id).await {
                    Ok(()) => {
                        self.say(format!("Deleted one {}.", resource.label.to_lowercase()));
                        // A detail screen for a record that no longer exists is
                        // not something to leave someone looking at.
                        self.open_selected().await;
                    }
                    Err(error) => self.fail(error),
                }
            }
            ConfirmAction::Run { function, body } => {
                let Some(function) = self.function(function).cloned() else {
                    return;
                };
                self.run_function(&function, body).await;
            }
            ConfirmAction::RemoveMember { id, name } => {
                match self.client.delete("membership", &id).await {
                    Ok(()) => {
                        self.say(format!("{name} no longer has access."));
                        self.load_team().await;
                    }
                    Err(error) => self.fail(error),
                }
            }
            ConfirmAction::SignOut => self.sign_out(),
        }
    }

    // --- keys -------------------------------------------------------------

    /// Insert pasted text into whichever box has the cursor.
    ///
    /// A paste is unambiguous: nobody pastes a secret at a form hoping to
    /// trigger its shortcuts. So it goes into the selected field whether or not
    /// that field was already open for editing, and control characters are
    /// dropped rather than turned into keystrokes.
    pub fn on_paste(&mut self, text: &str) {
        let text: String = text.chars().filter(|c| !c.is_control()).collect();
        if text.is_empty() {
            return;
        }

        let field = match (&mut self.sign_in, &mut self.main) {
            (Some(SignIn::Form(form)), _) => form.current(),
            (_, Main::Agent(chat)) if chat.editing => {
                let mut cursor = chat.cursor;
                insert(&mut chat.draft, &mut cursor, &text);
                chat.cursor = cursor;
                return;
            }
            (_, Main::Form(form)) => form.current(),
            (_, Main::List(list)) if list.searching => {
                let mut cursor = list.cursor;
                insert(&mut list.search, &mut cursor, &text);
                list.cursor = cursor;
                return;
            }
            _ => None,
        };
        let Some(field) = field else { return };
        let mut cursor = field.cursor;
        insert(&mut field.value, &mut cursor, &text);
        field.cursor = cursor;
    }

    pub async fn on_key(&mut self, key: KeyEvent) {
        // Ctrl-C always means stop, wherever the cursor is.
        if key.modifiers.contains(KeyModifiers::CONTROL) && matches!(key.code, KeyCode::Char('c')) {
            self.quit = true;
            return;
        }
        if self.help {
            self.help = false;
            return;
        }
        if self.confirm.is_some() {
            match key.code {
                KeyCode::Char('y') | KeyCode::Char('Y') | KeyCode::Enter => {
                    self.resolve_confirm(true).await
                }
                KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => {
                    self.resolve_confirm(false).await
                }
                _ => {}
            }
            return;
        }
        if self.picker.is_some() {
            return self.picker_key(key).await;
        }
        if self.sign_in.is_some() {
            return self.sign_in_key(key).await;
        }
        self.main_key(key).await;
    }

    async fn picker_key(&mut self, key: KeyEvent) {
        let Some(picker) = &mut self.picker else {
            return;
        };
        let last = picker.items.len().saturating_sub(1);
        match key.code {
            KeyCode::Down | KeyCode::Char('j') => picker.index = (picker.index + 1).min(last),
            KeyCode::Up | KeyCode::Char('k') => picker.index = picker.index.saturating_sub(1),
            KeyCode::Home | KeyCode::Char('g') => picker.index = 0,
            KeyCode::End | KeyCode::Char('G') => picker.index = last,
            KeyCode::Enter => self.choose().await,
            KeyCode::Esc | KeyCode::Char('q') => {
                self.picker = None;
            }
            _ => {}
        }
    }

    async fn sign_in_key(&mut self, key: KeyEvent) {
        // Resolved up front: the menu below borrows `self` mutably, and asking
        // the manifest which doors exist needs it immutably.
        let doors = self.doors();
        let last_door = doors.len().saturating_sub(1);
        match self.sign_in.as_mut() {
            Some(SignIn::Menu { index }) => match key.code {
                KeyCode::Down | KeyCode::Char('j') => *index = (*index + 1).min(last_door),
                KeyCode::Up | KeyCode::Char('k') => *index = index.saturating_sub(1),
                KeyCode::Char('q') => self.quit = true,
                KeyCode::Char('?') => self.help = true,
                KeyCode::Enter => {
                    let chosen = doors.get(*index).map(|(label, _)| *label);
                    match chosen {
                        Some(BROWSER_DOOR) => self.start_browser_sign_in().await,
                        Some(API_KEY_DOOR) => {
                            self.sign_in = Some(SignIn::Form(Form::sign_in_key()))
                        }
                        Some(REGISTER_DOOR) => {
                            let form = Form::sign_up(&self.manifest);
                            self.sign_in = Some(SignIn::Form(form));
                        }
                        Some(FORGOT_PASSWORD_DOOR) => {
                            let form = Form::forgot_password(&self.manifest);
                            self.sign_in = Some(SignIn::Form(form));
                        }
                        // Password sign-in, and the fallback for a menu that
                        // somehow has no entry at this index.
                        _ => {
                            let form = Form::sign_in_password(&self.manifest);
                            self.sign_in = Some(SignIn::Form(form));
                        }
                    }
                }
                _ => {}
            },
            Some(SignIn::Form(_)) => {
                if matches!(key.code, KeyCode::Esc) {
                    let editing = matches!(
                        self.sign_in.as_ref(),
                        Some(SignIn::Form(form)) if form.editing
                    );
                    if !editing {
                        self.sign_in = Some(SignIn::Menu { index: 0 });
                        return;
                    }
                }
                let submitted = {
                    let Some(SignIn::Form(form)) = self.sign_in.as_mut() else {
                        return;
                    };
                    form_key(form, key)
                };
                if submitted {
                    self.submit().await;
                }
            }
            Some(SignIn::Browser { url, .. }) => match key.code {
                KeyCode::Esc => self.sign_in = Some(SignIn::Menu { index: 0 }),
                KeyCode::Char('o') => {
                    let url = url.clone();
                    if !link::open_browser(&url) {
                        self.fail("no browser could be opened — copy the address instead");
                    }
                }
                KeyCode::Char('q') => self.quit = true,
                _ => {}
            },
            None => {}
        }
    }

    async fn main_key(&mut self, key: KeyEvent) {
        // While a text box has the cursor, ordinary letters are text.
        let typing = match &self.main {
            Main::Form(form) => form.editing,
            Main::List(list) => list.searching,
            Main::Agent(chat) => chat.editing,
            _ => false,
        };

        if !typing {
            match key.code {
                KeyCode::Char('q') => {
                    self.quit = true;
                    self.farewell = Some(format!("Disconnected from {}.", self.client.origin));
                    return;
                }
                KeyCode::Char('?') => {
                    self.help = true;
                    return;
                }
                KeyCode::Tab => {
                    self.focus = match self.focus {
                        Focus::Nav => Focus::Main,
                        Focus::Main => Focus::Nav,
                    };
                    return;
                }
                KeyCode::Char('O') => {
                    self.open_organization_picker();
                    return;
                }
                _ => {}
            }
        }

        if self.focus == Focus::Nav && !typing {
            return self.nav_key(key).await;
        }

        match &mut self.main {
            Main::List(_) => self.list_key(key).await,
            Main::Detail(_) => self.detail_key(key).await,
            Main::Form(_) => self.form_key(key).await,
            Main::Output { .. } => self.output_key(key),
            Main::Agent(_) => self.agent_key(key).await,
            Main::Team(_) => self.team_key(key).await,
            Main::Session => self.session_key(key).await,
            Main::Empty(_) => self.focus = Focus::Nav,
        }
    }

    async fn nav_key(&mut self, key: KeyEvent) {
        let last = self.nav.len().saturating_sub(1);
        match key.code {
            KeyCode::Down | KeyCode::Char('j') => {
                self.nav_index = (self.nav_index + 1).min(last);
                self.open_selected().await;
            }
            KeyCode::Up | KeyCode::Char('k') => {
                self.nav_index = self.nav_index.saturating_sub(1);
                self.open_selected().await;
            }
            KeyCode::Home | KeyCode::Char('g') => {
                self.nav_index = 0;
                self.open_selected().await;
            }
            KeyCode::End | KeyCode::Char('G') => {
                self.nav_index = last;
                self.open_selected().await;
            }
            KeyCode::Enter | KeyCode::Right | KeyCode::Char('l') => {
                self.open_selected().await;
                self.focus = Focus::Main;
            }
            KeyCode::Char('r') => self.open_selected().await,
            _ => {}
        }
    }

    async fn list_key(&mut self, key: KeyEvent) {
        // The search box, while it has the cursor.
        let searching = matches!(&self.main, Main::List(list) if list.searching);
        if searching {
            let Main::List(list) = &mut self.main else {
                return;
            };
            match key.code {
                KeyCode::Enter => {
                    list.searching = false;
                    list.page = 0;
                    self.reload().await;
                }
                KeyCode::Esc => {
                    list.searching = false;
                    list.search.clear();
                    list.cursor = 0;
                    self.reload().await;
                }
                _ => {
                    let mut cursor = list.cursor;
                    edit_text(&mut list.search, &mut cursor, key);
                    list.cursor = cursor;
                }
            }
            return;
        }

        let last = match &self.main {
            Main::List(list) => list.rows.len().saturating_sub(1),
            _ => 0,
        };
        match key.code {
            KeyCode::Down | KeyCode::Char('j') => {
                if let Main::List(list) = &mut self.main {
                    list.index = (list.index + 1).min(last);
                }
            }
            KeyCode::Up | KeyCode::Char('k') => {
                if let Main::List(list) = &mut self.main {
                    list.index = list.index.saturating_sub(1);
                }
            }
            KeyCode::Home | KeyCode::Char('g') => {
                if let Main::List(list) = &mut self.main {
                    list.index = 0;
                }
            }
            KeyCode::End | KeyCode::Char('G') => {
                if let Main::List(list) = &mut self.main {
                    list.index = last;
                }
            }
            KeyCode::Char(']') | KeyCode::PageDown => {
                let full = matches!(&self.main, Main::List(list) if list.rows.len() == PAGE);
                // Without a total from the server, a short page is the only
                // signal that there is nothing after it.
                if full {
                    if let Main::List(list) = &mut self.main {
                        list.page += 1;
                        list.index = 0;
                    }
                    self.reload().await;
                } else {
                    self.say("That is the last page.");
                }
            }
            KeyCode::Char('[') | KeyCode::PageUp => {
                let first = matches!(&self.main, Main::List(list) if list.page == 0);
                if !first {
                    if let Main::List(list) = &mut self.main {
                        list.page -= 1;
                        list.index = 0;
                    }
                    self.reload().await;
                }
            }
            KeyCode::Char('/') => {
                let searchable = matches!(
                    self.current_resource(),
                    Some((_, resource)) if resource.search_field.is_some()
                );
                if searchable {
                    if let Main::List(list) = &mut self.main {
                        list.searching = true;
                    }
                } else {
                    self.say("This resource has no field to search on.");
                }
            }
            KeyCode::Char('r') => self.reload().await,
            KeyCode::Enter | KeyCode::Right | KeyCode::Char('l') => self.open_row().await,
            KeyCode::Char('n') => self.start_create(),
            KeyCode::Char('e') => self.start_edit(),
            KeyCode::Char('d') => self.ask_delete(),
            KeyCode::Esc | KeyCode::Left | KeyCode::Char('h') => self.focus = Focus::Nav,
            _ => {}
        }
    }

    async fn detail_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Down | KeyCode::Char('j') => {
                if let Main::Detail(detail) = &mut self.main {
                    detail.scroll = detail.scroll.saturating_add(1);
                }
            }
            KeyCode::Up | KeyCode::Char('k') => {
                if let Main::Detail(detail) = &mut self.main {
                    detail.scroll = detail.scroll.saturating_sub(1);
                }
            }
            KeyCode::Char('e') => self.start_edit(),
            KeyCode::Char('d') => self.ask_delete(),
            KeyCode::Char('c') => self.open_children_picker(),
            KeyCode::Char('r') => {
                // Re-read, in case someone else changed it.
                let record = self.current_record();
                if let (Some((_, resource)), Some(record)) = (self.current_resource(), record) {
                    if let Some(id) = record.get("id").and_then(Value::as_str) {
                        match self.client.read(&resource.name, id).await {
                            Ok(fresh) => {
                                if let Main::Detail(detail) = &mut self.main {
                                    detail.record = fresh;
                                }
                                self.say("Reloaded.");
                            }
                            Err(error) => self.fail(error),
                        }
                    }
                }
            }
            KeyCode::Esc | KeyCode::Left | KeyCode::Char('h') => self.open_selected().await,
            _ => {}
        }
    }

    async fn form_key(&mut self, key: KeyEvent) {
        // Enter on a reference field opens a picker rather than a text box —
        // nobody remembers a UUID.
        let wants_picker = matches!(&self.main, Main::Form(form)
            if !form.editing
                && matches!(key.code, KeyCode::Enter)
                && form.fields.get(form.index).is_some_and(|f| f.references.is_some()));
        if wants_picker {
            return self.open_reference_picker().await;
        }

        let leaving = matches!(&self.main, Main::Form(form) if !form.editing)
            && matches!(key.code, KeyCode::Esc);
        if leaving {
            self.open_selected().await;
            // A form that *is* the screen — your account, an action, naming a
            // key — has no list underneath to go back to. Redrawing it would
            // read as esc doing nothing, so the cursor goes to the sidebar.
            if matches!(self.main, Main::Form(_)) {
                self.focus = Focus::Nav;
            }
            return;
        }

        let submitted = {
            let Main::Form(form) = &mut self.main else {
                return;
            };
            form_key(form, key)
        };
        if submitted {
            self.submit().await;
        }
    }

    fn output_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Down | KeyCode::Char('j') => {
                if let Main::Output { scroll, .. } = &mut self.main {
                    *scroll = scroll.saturating_add(1);
                }
            }
            KeyCode::Up | KeyCode::Char('k') => {
                if let Main::Output { scroll, .. } = &mut self.main {
                    *scroll = scroll.saturating_sub(1);
                }
            }
            KeyCode::Esc | KeyCode::Left | KeyCode::Char('h') => self.focus = Focus::Nav,
            _ => {}
        }
    }

    async fn agent_key(&mut self, key: KeyEvent) {
        let editing = matches!(&self.main, Main::Agent(chat) if chat.editing);
        if editing {
            let submitted = {
                let Main::Agent(chat) = &mut self.main else {
                    return;
                };
                match key.code {
                    KeyCode::Enter => true,
                    KeyCode::Esc => {
                        chat.editing = false;
                        false
                    }
                    _ => {
                        edit_text(&mut chat.draft, &mut chat.cursor, key);
                        false
                    }
                }
            };
            if submitted {
                self.send_agent_message().await;
            }
            return;
        }

        match key.code {
            KeyCode::Down | KeyCode::Char('j') => {
                if let Main::Agent(chat) = &mut self.main {
                    let last = chat.threads.len().saturating_sub(1);
                    chat.thread_index = (chat.thread_index + 1).min(last);
                }
            }
            KeyCode::Up | KeyCode::Char('k') => {
                if let Main::Agent(chat) = &mut self.main {
                    chat.thread_index = chat.thread_index.saturating_sub(1);
                }
            }
            KeyCode::Enter | KeyCode::Right | KeyCode::Char('l') => {
                let thread_id = match &self.main {
                    Main::Agent(chat) => chat
                        .threads
                        .get(chat.thread_index)
                        .map(|thread| thread.id.clone()),
                    _ => None,
                };
                if let Some(thread_id) = thread_id {
                    self.open_agent_thread(thread_id).await;
                }
            }
            KeyCode::Char('i') => {
                if let Main::Agent(chat) = &mut self.main {
                    chat.editing = true;
                }
            }
            KeyCode::Char('n') => {
                if let Main::Agent(chat) = &mut self.main {
                    chat.thread_id = None;
                    chat.messages.clear();
                    chat.draft.clear();
                    chat.cursor = 0;
                    chat.editing = true;
                }
                self.say("Starting a new conversation.");
            }
            KeyCode::Char('r') => self.reload_agent().await,
            KeyCode::Esc | KeyCode::Left | KeyCode::Char('h') => self.focus = Focus::Nav,
            _ => {}
        }
    }

    async fn send_agent_message(&mut self) {
        let (agent_index, message, thread_id, history_visible) = match &self.main {
            Main::Agent(chat) => (
                chat.agent,
                chat.draft.trim().to_string(),
                chat.thread_id.clone(),
                !chat.threads.is_empty() || chat.browse_note.is_none(),
            ),
            _ => return,
        };
        if message.is_empty() {
            return self.say("Type a message first.");
        }
        let Some(agent) = self.agent(agent_index).cloned() else {
            return;
        };

        if let Main::Agent(chat) = &mut self.main {
            chat.editing = false;
            chat.messages.push(AgentMessage {
                role: "user".into(),
                content: message.clone(),
                meta: None,
            });
            chat.draft.clear();
            chat.cursor = 0;
        }

        self.status = format!("Asking {}…", agent.label);
        let response = self
            .client
            .request(
                reqwest::Method::POST,
                &format!("/ai/agents/{}/chat", api::encode(&agent.name)),
                Some(json!({
                    "message": message,
                    "thread_id": thread_id,
                    "stream": false,
                })),
            )
            .await;

        match response {
            Ok(reply) => {
                let text = reply.get("text").map(api::scalar).unwrap_or_default();
                let next_thread = reply
                    .get("thread_id")
                    .and_then(Value::as_str)
                    .map(str::to_string)
                    .or(thread_id);
                let meta = {
                    let provider = reply
                        .get("provider")
                        .map(api::scalar)
                        .filter(|text| !text.is_empty());
                    let model = reply
                        .get("model")
                        .map(api::scalar)
                        .filter(|text| !text.is_empty());
                    let finish = reply
                        .get("finish_reason")
                        .map(api::scalar)
                        .filter(|text| !text.is_empty());
                    let joined = [provider, model, finish]
                        .into_iter()
                        .flatten()
                        .collect::<Vec<_>>()
                        .join(" · ");
                    (!joined.is_empty()).then_some(joined)
                };

                if let Main::Agent(chat) = &mut self.main {
                    chat.thread_id = next_thread.clone();
                    chat.messages.push(AgentMessage {
                        role: "assistant".into(),
                        content: text,
                        meta,
                    });
                }
                self.say(format!("{} replied.", agent.label));
                if agent.storage && history_visible {
                    self.reload_agent().await;
                }
            }
            Err(error) => self.fail(error),
        }
    }

    async fn team_key(&mut self, key: KeyEvent) {
        let last = match &self.main {
            Main::Team(team) => team.members.len().saturating_sub(1),
            _ => 0,
        };
        let move_to = |cli: &mut Cli, index: usize| {
            if let Main::Team(team) = &mut cli.main {
                team.index = index.min(last);
            }
        };
        match key.code {
            KeyCode::Down | KeyCode::Char('j') => {
                let next = match &self.main {
                    Main::Team(team) => team.index + 1,
                    _ => 0,
                };
                move_to(self, next);
            }
            KeyCode::Up | KeyCode::Char('k') => {
                let previous = match &self.main {
                    Main::Team(team) => team.index.saturating_sub(1),
                    _ => 0,
                };
                move_to(self, previous);
            }
            KeyCode::Home => move_to(self, 0),
            KeyCode::End | KeyCode::Char('G') => move_to(self, last),
            KeyCode::Char('g') => self.open_grant_picker(),
            KeyCode::Char('t') => self.open_revoke_picker(),
            // `n` and `d` mean the same here as on any list: make one, remove
            // one. What they make and remove is a person's access.
            KeyCode::Char('n') => self.start_add_member(),
            KeyCode::Char('d') => self.ask_remove_member(),
            KeyCode::Char('r') => self.load_team().await,
            KeyCode::Esc | KeyCode::Left | KeyCode::Char('h') => self.focus = Focus::Nav,
            _ => {}
        }
    }

    async fn session_key(&mut self, key: KeyEvent) {
        match key.code {
            // Named, like the dashboard's: an unnamed key is one nobody dares
            // revoke a year later.
            KeyCode::Char('g') => self.main = Main::Form(Form::new_api_key()),
            // Another workspace alongside the personal one every account is
            // given. An app that provisions tenants itself has narrowed
            // `create`, and a `role:` policy is answered by the usual refusal
            // rather than by a form nobody can submit.
            KeyCode::Char('N') => self.start_new_organization(),
            KeyCode::Char('x') => {
                self.confirm = Some(Confirm {
                    prompt: format!(
                        "Sign out of {} and forget its saved key?",
                        self.client.origin
                    ),
                    action: ConfirmAction::SignOut,
                });
            }
            KeyCode::Esc | KeyCode::Left | KeyCode::Char('h') => self.focus = Focus::Nav,
            _ => {}
        }
    }
}

/// What a reference field should read as: the name of the record it points at
/// when the API inlined it, and nothing when it did not.
///
/// A column of uuids says only that these rows differ. This is the whole reason
/// the console asks for `?expand=`.
pub fn related_label(
    manifest: &Manifest,
    resource: &ResourceManifest,
    record: &Value,
    field: &str,
) -> Option<String> {
    let relation = resource
        .relations
        .iter()
        .find(|relation| relation.field == field)?;
    let related = record
        .get(&relation.relation)
        .filter(|value| value.is_object())?;
    let target = manifest
        .resources
        .iter()
        .find(|target| target.name == relation.target)?;
    let label = target.title_of(related);
    (!label.is_empty()).then_some(label)
}

/// The keys holding an inlined record rather than a value of this resource's
/// own — shown through the field that points at them, not as raw JSON.
pub fn relation_keys(resource: &ResourceManifest) -> Vec<&str> {
    resource
        .relations
        .iter()
        .map(|relation| relation.relation.as_str())
        .collect()
}

/// A membership's primary role, if it has one. An empty column is no role.
fn role_of(row: &Value) -> Option<String> {
    row.get("role").map(api::scalar).filter(|r| !r.is_empty())
}

/// A `membership_role` row as `(id, role)`.
fn grant_of(row: &Value) -> Option<(String, String)> {
    let id = row.get("id").map(api::scalar).filter(|id| !id.is_empty())?;
    let role = row.get("role").map(api::scalar).filter(|r| !r.is_empty())?;
    Some((id, role))
}

/// What is visibly wrong with an API key, if anything.
///
/// Every key the server issues is `apik_` followed by 64 hex characters, so a
/// string that is not that was mistyped, truncated, or lost characters on the
/// way through the terminal — which is invisible when the box shows dots. This
/// never returns the key itself.
/// Check that the two password boxes on a sign-up form agree.
///
/// A form without a confirmation box passes — there is nothing to disagree
/// with, which is the case for every app whose console predates this.
fn confirm_passwords(form: &Form) -> Result<(), String> {
    let value = |name: &str| {
        form.fields
            .iter()
            .find(|field| field.name == name)
            .map(|field| field.value.as_str())
    };
    match (value("password"), value(CONFIRM_PASSWORD)) {
        (Some(password), Some(confirmation)) if password != confirmation => {
            Err("the two passwords do not match".to_string())
        }
        _ => Ok(()),
    }
}

fn key_shape(key: &str) -> Option<String> {
    const PREFIX: &str = "apik_";
    const LENGTH: usize = PREFIX.len() + 64;

    let count = key.chars().count();
    if !key.starts_with(PREFIX) {
        return Some(format!(
            "it does not start with `{PREFIX}`, which every key issued by this server does"
        ));
    }
    if count != LENGTH {
        return Some(format!(
            "it is {count} characters and a key is {LENGTH}. \
             {} — check the paste arrived whole",
            if count < LENGTH {
                "Characters were lost"
            } else {
                "There are extra characters"
            }
        ));
    }
    if !key[PREFIX.len()..].chars().all(|c| c.is_ascii_hexdigit()) {
        return Some("it has characters after `apik_` that are not hexadecimal".into());
    }
    None
}

/// The `sub` claim of a session token — the user id it was issued for.
///
/// Reading it needs no verification and no key: the server checks the signature
/// on every request, and all this is for is a name on the Session screen. A
/// token we misread simply shows nothing.
fn jwt_subject(token: &str) -> Option<String> {
    let payload = token.split('.').nth(1)?;
    let claims: Value = serde_json::from_slice(&base64url(payload)?).ok()?;
    claims
        .get("sub")
        .and_then(Value::as_str)
        .map(str::to_string)
}

/// Decode unpadded base64url. Small enough not to be worth a dependency.
fn base64url(text: &str) -> Option<Vec<u8>> {
    const ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut out = Vec::with_capacity(text.len() * 3 / 4);
    let mut buffer: u32 = 0;
    let mut bits = 0;
    for byte in text.bytes() {
        if byte == b'=' {
            break;
        }
        let value = ALPHABET.iter().position(|c| *c == byte)? as u32;
        buffer = (buffer << 6) | value;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((buffer >> bits) as u8);
        }
    }
    Some(out)
}

/// The name a key created from here is given, so it is recognisable in the
/// dashboard's list months later.
fn key_label() -> String {
    let host = std::env::var("HOSTNAME")
        .ok()
        .filter(|host| !host.is_empty())
        .unwrap_or_else(|| "terminal".into());
    format!("apiplant cli ({host})")
}

/// Move around a form and edit its boxes. Returns whether it was submitted.
fn form_key(form: &mut Form, key: KeyEvent) -> bool {
    if form.editing {
        match key.code {
            KeyCode::Enter | KeyCode::Esc => form.editing = false,
            _ => {
                if let Some(field) = form.current() {
                    let mut cursor = field.cursor;
                    edit_text(&mut field.value, &mut cursor, key);
                    field.cursor = cursor;
                }
            }
        }
        return false;
    }

    let last = form.fields.len(); // the submit button
    match key.code {
        KeyCode::Down | KeyCode::Char('j') | KeyCode::Tab => {
            form.index = (form.index + 1).min(last)
        }
        KeyCode::Up | KeyCode::Char('k') | KeyCode::BackTab => {
            form.index = form.index.saturating_sub(1)
        }
        KeyCode::Enter => {
            if form.on_submit() {
                return true;
            }
            match form.current() {
                // A boolean and a closed list of options have nothing to type.
                Some(field) if field.toggleable() => {
                    let flipped = (field.value != "true").to_string();
                    field.set(flipped);
                }
                Some(field) if !field.options.is_empty() => {
                    let next = field
                        .options
                        .iter()
                        .position(|option| option == &field.value)
                        .map(|at| (at + 1) % field.options.len())
                        .unwrap_or(0);
                    let value = field.options[next].clone();
                    field.set(value);
                }
                Some(_) => form.editing = true,
                None => {}
            }
        }
        KeyCode::Char(' ') => {
            if let Some(field) = form.current() {
                if field.toggleable() {
                    let flipped = (field.value != "true").to_string();
                    field.set(flipped);
                }
            }
        }
        // `i` opens the box under the cursor, for hands that expect vi.
        KeyCode::Char('i') if !form.on_submit() => form.editing = true,
        KeyCode::Char('D') => {
            if let Some(field) = form.current() {
                field.set(String::new());
            }
        }
        _ => {}
    }
    false
}

/// Insert a whole string at the cursor, in one go.
fn insert(text: &mut String, cursor: &mut usize, addition: &str) {
    let mut chars: Vec<char> = text.chars().collect();
    *cursor = (*cursor).min(chars.len());
    for c in addition.chars() {
        chars.insert(*cursor, c);
        *cursor += 1;
    }
    *text = chars.into_iter().collect();
}

/// The one text-editing implementation, shared by every box in the console.
fn edit_text(text: &mut String, cursor: &mut usize, key: KeyEvent) {
    let mut chars: Vec<char> = text.chars().collect();
    *cursor = (*cursor).min(chars.len());
    match key.code {
        KeyCode::Char(c) => {
            chars.insert(*cursor, c);
            *cursor += 1;
        }
        KeyCode::Backspace => {
            if *cursor > 0 {
                chars.remove(*cursor - 1);
                *cursor -= 1;
            }
        }
        KeyCode::Delete => {
            if *cursor < chars.len() {
                chars.remove(*cursor);
            }
        }
        KeyCode::Left => *cursor = cursor.saturating_sub(1),
        KeyCode::Right => *cursor = (*cursor + 1).min(chars.len()),
        KeyCode::Home => *cursor = 0,
        KeyCode::End => *cursor = chars.len(),
        _ => return,
    }
    *text = chars.into_iter().collect();
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::api::{ActionPermission, ActionPermissions, FieldManifest};
    use serde_json::json;

    fn press(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn field(name: &str, ty: &str) -> FieldManifest {
        FieldManifest {
            name: name.into(),
            label: name.into(),
            ty: ty.into(),
            widget: "text".into(),
            admin_visible: true,
            writable: true,
            ..Default::default()
        }
    }

    fn resource() -> ResourceManifest {
        ResourceManifest {
            name: "product".into(),
            label: "Product".into(),
            plural: "Products".into(),
            visible: true,
            display_field: Some("name".into()),
            fields: vec![
                field("name", "string"),
                field("stock", "integer"),
                field("live", "boolean"),
            ],
            permissions: ActionPermissions {
                list: ActionPermission {
                    value: "public".into(),
                    ..Default::default()
                },
                ..Default::default()
            },
            ..Default::default()
        }
    }

    /// The built-in `organization`, with whatever `create` policy the test wants.
    fn organization(create: &str) -> ResourceManifest {
        let mut name = field("name", "string");
        name.required = true;
        ResourceManifest {
            name: "organization".into(),
            label: "Organization".into(),
            plural: "Organizations".into(),
            display_field: Some("name".into()),
            fields: vec![name, field("slug", "string")],
            permissions: ActionPermissions {
                create: ActionPermission {
                    value: create.into(),
                    role: (create == "role:admin").then(|| "admin".to_string()),
                    ..Default::default()
                },
                ..Default::default()
            },
            ..Default::default()
        }
    }

    /// Somebody signed in, working in an organisation, whose roles are known.
    fn signed_in(roles: &[String]) -> Reach<'_> {
        Reach {
            signed_in: true,
            organization: true,
            roles,
            roles_known: true,
        }
    }

    /// A console that has asked the server about organisations and been told
    /// the answer — which is what makes an empty list mean anything.
    fn console(resources: Vec<ResourceManifest>) -> Cli {
        let manifest = Manifest {
            resources,
            ..Default::default()
        };
        let client = Client::new("http://x:1".into(), "/api".into(), "/admin".into()).unwrap();
        let mut cli = Cli::new(
            client,
            manifest,
            Store::default(),
            crate::cli::Target::Dir(".".into()),
        );
        cli.organizations_known = true;
        // Signed in, somewhere to work, and roles we could actually read —
        // which is what the sidebar is built from.
        cli.client.credentials.api_key = Some("apik_test".into());
        cli.client.organization = Some("org-1".into());
        cli.roles_known = true;
        cli.rebuild_nav();
        cli
    }

    /// A `membership` resource anyone in the organisation may list, plus the
    /// `membership_role` join table whose rows are the grants.
    fn membership_resources(create: &str) -> Vec<ResourceManifest> {
        let listable = ActionPermission {
            value: "member".into(),
            ..Default::default()
        };
        vec![
            ResourceManifest {
                name: "membership".into(),
                label: "Membership".into(),
                plural: "Memberships".into(),
                permissions: ActionPermissions {
                    list: listable.clone(),
                    ..Default::default()
                },
                ..Default::default()
            },
            ResourceManifest {
                name: "membership_role".into(),
                label: "Role".into(),
                plural: "Roles".into(),
                permissions: ActionPermissions {
                    list: listable,
                    create: ActionPermission {
                        value: create.into(),
                        role: create.strip_prefix("role:").map(str::to_string),
                        ..Default::default()
                    },
                    ..Default::default()
                },
                ..Default::default()
            },
        ]
    }

    fn member(name: &str, primary: Option<&str>, grants: &[&str], is_me: bool) -> Member {
        Member {
            membership_id: format!("m-{name}"),
            name: name.into(),
            primary: primary.map(str::to_string),
            grants: grants
                .iter()
                .map(|role| (format!("g-{name}-{role}"), role.to_string()))
                .collect(),
            is_me,
        }
    }

    /// The built-in auth resources, as the manifest describes them: hidden from
    /// the resource navigation, listable by whoever they belong to.
    fn auth_resources() -> Vec<ResourceManifest> {
        let mut resources = membership_resources("role:admin");
        for (name, label, plural) in [
            ("organization", "Organization", "Organizations"),
            ("api_key", "API key", "API keys"),
            ("user", "User", "Users"),
        ] {
            resources.push(ResourceManifest {
                name: name.into(),
                label: label.into(),
                plural: plural.into(),
                // What the server sends for an auth resource.
                visible: false,
                auth_resource: true,
                scope: "global".into(),
                permissions: ActionPermissions {
                    list: ActionPermission {
                        value: "authenticated".into(),
                        ..Default::default()
                    },
                    ..Default::default()
                },
                ..Default::default()
            });
        }
        resources
    }

    #[test]
    fn tenancy_tables_stay_out_of_the_resource_list_and_get_their_own_screens() {
        let mut manifest = Manifest {
            resources: auth_resources(),
            ..Default::default()
        };
        manifest.resources.push(resource());
        manifest.auth.profile_fields = vec![field("display_name", "string")];

        let nav = navigation(&manifest, &signed_in(&[]));
        let data: Vec<&str> = nav
            .iter()
            .filter(|item| item.group == "Data")
            .map(|item| item.label.as_str())
            .collect();
        // `membership`, `membership_role` and `user` are how tenancy is stored,
        // not things to browse as tables.
        assert_eq!(data, vec!["Products"]);

        let console: Vec<&str> = nav
            .iter()
            .filter(|item| item.group == "Console")
            .map(|item| item.label.as_str())
            .collect();
        assert_eq!(
            console,
            vec!["Account", "Team", "Organizations", "API keys", "Session"]
        );
    }

    #[test]
    fn a_resource_an_app_deliberately_shows_is_not_listed_twice() {
        let mut manifest = Manifest {
            resources: auth_resources(),
            ..Default::default()
        };
        // `[admin] visible = true` on `organization` puts it in the resource
        // navigation; a second Console entry for the same rows is clutter.
        let index = manifest
            .resources
            .iter()
            .position(|resource| resource.name == "organization")
            .unwrap();
        manifest.resources[index].visible = true;

        let nav = navigation(&manifest, &signed_in(&[]));
        let labels: Vec<&str> = nav.iter().map(|item| item.label.as_str()).collect();
        assert_eq!(labels.iter().filter(|l| **l == "Organizations").count(), 1);
    }

    #[test]
    fn a_screen_only_one_role_may_reach_leaves_the_sidebar_when_it_is_not_yours() {
        let mut locked = resource();
        locked.plural = "Invoices".into();
        locked.permissions.list = ActionPermission {
            value: "role:billing".into(),
            role: Some("billing".into()),
            ..Default::default()
        };
        let manifest = Manifest {
            resources: vec![locked],
            ..Default::default()
        };

        let labels = |roles: &[&str]| -> Vec<String> {
            let roles: Vec<String> = roles.iter().map(|r| r.to_string()).collect();
            navigation(&manifest, &signed_in(&roles))
                .iter()
                .map(|item| item.label.clone())
                .collect()
        };
        let unknowable = || -> Vec<String> {
            // An app that will not let you list your own memberships.
            let reach = Reach {
                signed_in: true,
                organization: true,
                roles: &[],
                roles_known: false,
            };
            navigation(&manifest, &reach)
                .iter()
                .map(|item| item.label.clone())
                .collect()
        };

        assert!(labels(&["member"]).iter().all(|l| l != "Invoices"));
        assert!(labels(&["billing"]).iter().any(|l| l == "Invoices"));
        // An admin holds every role, so it is theirs too.
        assert!(labels(&["admin"]).iter().any(|l| l == "Invoices"));
        // Holding no roles is a fact, and it hides it.
        assert!(labels(&[]).iter().all(|l| l != "Invoices"));
        // Being unable to find out is not, and hides nothing: the server
        // refuses it if we were wrong, which is better than a door that is
        // missing for someone who has the key.
        assert!(unknowable().iter().any(|l| l == "Invoices"));
    }

    #[test]
    fn a_session_with_no_organization_is_not_shown_the_work_that_needs_one() {
        let mut scoped = resource();
        scoped.scope = "organization".into();
        scoped.permissions.list.value = "member".into();
        let mut global = resource();
        global.name = "article".into();
        global.plural = "Articles".into();
        global.scope = "global".into();
        global.permissions.list.value = "authenticated".into();

        let manifest = Manifest {
            resources: vec![scoped, global],
            functions: vec![
                FunctionManifest {
                    name: "reconcile".into(),
                    label: "Reconcile".into(),
                    visible: true,
                    permission: "member".into(),
                    requires_org: true,
                    ..Default::default()
                },
                FunctionManifest {
                    name: "ping".into(),
                    label: "Ping".into(),
                    visible: true,
                    permission: "authenticated".into(),
                    ..Default::default()
                },
            ],
            ..Default::default()
        };

        let labels = |reach: &Reach| -> Vec<String> {
            navigation(&manifest, reach)
                .iter()
                .map(|item| item.label.clone())
                .collect()
        };

        // Signed in, but nowhere to work: the org-scoped table would list
        // nothing and the org-scoped action could only answer 403.
        let homeless = Reach {
            signed_in: true,
            organization: false,
            roles: &[],
            roles_known: true,
        };
        let seen = labels(&homeless);
        assert!(seen.iter().all(|l| l != "Products"), "{seen:?}");
        assert!(seen.iter().all(|l| l != "Reconcile"), "{seen:?}");
        // What does not need one is still there.
        assert!(seen.iter().any(|l| l == "Articles"));
        assert!(seen.iter().any(|l| l == "Ping"));

        let settled = labels(&signed_in(&[]));
        assert!(settled.iter().any(|l| l == "Products"));
        assert!(settled.iter().any(|l| l == "Reconcile"));
    }

    #[test]
    fn an_action_gated_on_a_role_leaves_the_sidebar_the_same_way_a_table_does() {
        let manifest = Manifest {
            functions: vec![FunctionManifest {
                name: "settle".into(),
                label: "Settle accounts".into(),
                visible: true,
                permission: "role:billing".into(),
                requires_org: true,
                ..Default::default()
            }],
            ..Default::default()
        };
        let labels = |roles: &[&str]| -> Vec<String> {
            let roles: Vec<String> = roles.iter().map(|r| r.to_string()).collect();
            navigation(&manifest, &signed_in(&roles))
                .iter()
                .map(|item| item.label.clone())
                .collect()
        };
        assert!(labels(&["member"]).iter().all(|l| l != "Settle accounts"));
        assert!(labels(&["billing"]).iter().any(|l| l == "Settle accounts"));
        assert!(labels(&["admin"]).iter().any(|l| l == "Settle accounts"));
    }

    #[test]
    fn nothing_is_offered_before_the_console_knows_who_is_asking() {
        let mut manifest = Manifest {
            resources: auth_resources(),
            ..Default::default()
        };
        manifest.resources.push(resource());
        manifest.auth.profile_fields = vec![field("display_name", "string")];

        // The sign-in screen owns the display at this point. What matters is
        // that nothing needing a session is drawn for one that does not exist
        // yet — Products is `public`, and public means public.
        let labels: Vec<String> = navigation(&manifest, &Reach::unknown())
            .iter()
            .map(|item| item.label.clone())
            .collect();
        assert_eq!(labels, vec!["Products", "Session"]);
    }

    #[test]
    fn private_means_nobody_which_is_not_the_same_as_needing_a_role() {
        let private = ActionPermission {
            value: "private".into(),
            ..Default::default()
        };
        assert!(!private.possible());
        assert!(!private.allowed(true, true, &["admin".into()]));

        let role = ActionPermission {
            value: "role:editor".into(),
            role: Some("editor".into()),
            ..Default::default()
        };
        assert!(role.possible());
        assert!(!role.allowed(true, true, &["member".into()]));
        assert!(role.allowed(true, true, &["editor".into()]));
        // `admin` satisfies every role check, here as everywhere else.
        assert!(role.allowed(true, true, &["admin".into()]));

        let member = ActionPermission {
            value: "member".into(),
            ..Default::default()
        };
        assert!(!member.allowed(true, false, &[]), "needs an organization");
        assert!(member.allowed(true, true, &[]));
        assert!(!member.allowed(false, true, &[]));
    }

    #[tokio::test]
    async fn an_api_key_is_named_and_minted_rather_than_written_as_a_row() {
        let mut keys = ResourceManifest {
            name: "api_key".into(),
            label: "API key".into(),
            plural: "API keys".into(),
            scope: "global".into(),
            ..Default::default()
        };
        keys.permissions.create.value = "authenticated".into();
        // `update` on `api_key` is private: the row holds a hash, and there is
        // nothing about it to edit.
        keys.permissions.update.value = "private".into();

        let mut cli = console(vec![keys]);
        cli.client.credentials.api_key = Some("apik_test".into());
        cli.main = Main::List(List {
            resource: 0,
            rows: vec![json!({ "id": "k1", "name": "Nightly import" })],
            index: 0,
            page: 0,
            search: String::new(),
            searching: false,
            cursor: 0,
            filter: None,
            filter_label: None,
        });

        cli.start_create();
        let Main::Form(form) = &cli.main else {
            panic!("expected the naming form, got {:?}", cli.main);
        };
        assert_eq!(form.kind, FormKind::NewApiKey);

        // And editing one is not offered at all.
        cli.main = Main::List(List {
            resource: 0,
            rows: vec![json!({ "id": "k1" })],
            index: 0,
            page: 0,
            search: String::new(),
            searching: false,
            cursor: 0,
            filter: None,
            filter_label: None,
        });
        cli.start_edit();
        assert!(matches!(cli.main, Main::List(_)));
        assert!(cli.error.is_some());
    }

    #[test]
    fn registering_is_offered_only_where_the_app_allows_it() {
        let mut manifest = Manifest::default();
        assert_eq!(sign_in_options(&manifest).len(), 3);

        manifest.auth.allow_registration = true;
        let doors = sign_in_options(&manifest);
        assert_eq!(doors.len(), 4);
        assert_eq!(doors[3].0, "Create an account");
    }

    #[test]
    fn resetting_a_password_is_offered_only_where_the_server_can_send_the_link() {
        let mut manifest = Manifest::default();
        let labels = |manifest: &Manifest| -> Vec<&'static str> {
            sign_in_options(manifest)
                .into_iter()
                .map(|(l, _)| l)
                .collect()
        };
        assert!(!labels(&manifest).contains(&FORGOT_PASSWORD_DOOR));

        manifest.auth.password_reset_enabled = true;
        assert!(labels(&manifest).contains(&FORGOT_PASSWORD_DOOR));

        // The menu is conditional at both ends, so the doors are dispatched by
        // label; nothing here may depend on where one happens to land.
        manifest.auth.allow_registration = false;
        let doors = labels(&manifest);
        assert!(!doors.contains(&REGISTER_DOOR));
        assert!(doors.contains(&FORGOT_PASSWORD_DOOR));
    }

    #[test]
    fn signing_up_asks_for_the_password_twice_and_never_sends_the_second_box() {
        let mut manifest = Manifest::default();
        manifest.auth.allow_registration = true;
        let mut form = Form::sign_up(&manifest);

        let set = |form: &mut Form, name: &str, value: &str| {
            let field = form
                .fields
                .iter_mut()
                .find(|field| field.name == name)
                .unwrap_or_else(|| panic!("no `{name}` box"));
            field.set(value.to_string());
        };
        set(&mut form, "email", "ann@example.test");
        set(&mut form, "password", "hunter2");
        set(&mut form, CONFIRM_PASSWORD, "hunter3");

        // A typo in a box nobody can read is caught here, not at the next
        // sign-in.
        assert!(confirm_passwords(&form).is_err());

        set(&mut form, CONFIRM_PASSWORD, "hunter2");
        assert!(confirm_passwords(&form).is_ok());

        // The confirmation is a check, not a column: the server never sees it.
        let body = Cli::body_of(&form, false).unwrap();
        assert_eq!(body["password"], "hunter2");
        assert_eq!(body["email"], "ann@example.test");
        assert!(body.get(CONFIRM_PASSWORD).is_none());
    }

    #[test]
    fn adding_a_teammate_says_what_it_will_actually_do() {
        let mut manifest = Manifest::default();
        let roles = vec!["member".to_string(), "admin".to_string()];

        // With no mailer, the only door is an account that already exists —
        // and the form says so, because it is a real constraint on the reader.
        let form = Form::add_member(&manifest, &roles);
        assert_eq!(form.submit, "Add to organization");
        assert!(form
            .subtitle
            .as_deref()
            .unwrap()
            .contains("account already"));

        manifest.auth.invitations_enabled = true;
        let form = Form::add_member(&manifest, &roles);
        assert_eq!(form.submit, "Send invitation");
        assert!(form.subtitle.as_deref().unwrap().contains("whether or not"));
    }

    #[test]
    fn a_reference_reads_as_the_name_of_what_it_points_at() {
        let mut order = resource();
        order.name = "order".into();
        order.relations = vec![api::RelationManifest {
            field: "customer_id".into(),
            relation: "customer".into(),
            target: "customer".into(),
            label: "Customer".into(),
        }];
        let mut customer = resource();
        customer.name = "customer".into();
        customer.display_field = Some("name".into());

        let manifest = Manifest {
            resources: vec![order.clone(), customer],
            ..Default::default()
        };

        let expanded = json!({
            "customer_id": "c-1",
            "customer": { "id": "c-1", "name": "Acme Ltd" },
        });
        assert_eq!(
            related_label(&manifest, &order, &expanded, "customer_id").as_deref(),
            Some("Acme Ltd")
        );
        // Not expanded — because the server would not, or was not asked — is a
        // uuid, and the caller falls back to printing it.
        let plain = json!({ "customer_id": "c-1" });
        assert_eq!(
            related_label(&manifest, &order, &plain, "customer_id"),
            None
        );
        assert_eq!(relation_keys(&order), vec!["customer"]);
    }

    #[tokio::test]
    async fn a_record_offers_the_records_that_belong_to_it() {
        let mut customer = resource();
        customer.name = "customer".into();
        customer.label = "Customer".into();
        customer.display_field = Some("name".into());
        customer.children = vec![
            api::ChildManifest {
                resource: "order".into(),
                field: "customer_id".into(),
                label: "Orders".into(),
            },
            // A child nobody may list is not a door.
            api::ChildManifest {
                resource: "audit".into(),
                field: "customer_id".into(),
                label: "Audit entries".into(),
            },
        ];
        let mut order = resource();
        order.name = "order".into();
        order.plural = "Orders".into();
        let mut audit = resource();
        audit.name = "audit".into();
        audit.permissions.list.value = "private".into();

        let mut cli = console(vec![customer, order, audit]);
        cli.main = Main::Detail(Detail {
            resource: 0,
            record: json!({ "id": "c-1", "name": "Beta Foods" }),
            scroll: 0,
        });

        cli.open_children_picker();
        let picker = cli.picker.clone().expect("a picker of children");
        let offered: Vec<&str> = picker.items.iter().map(|(_, l)| l.as_str()).collect();
        assert_eq!(offered, vec!["Orders"]);

        cli.choose().await;
        let Main::List(list) = &cli.main else {
            panic!("expected the child list, got {:?}", cli.main);
        };
        // The ordinary list screen, pinned to the record it came from.
        assert_eq!(list.resource, 1);
        assert_eq!(
            list.filter,
            Some(("customer_id".to_string(), "c-1".to_string()))
        );
        assert_eq!(list.filter_label.as_deref(), Some("Beta Foods"));
    }

    #[test]
    fn adding_someone_asks_for_the_identity_they_signed_up_with() {
        let mut manifest = Manifest::default();
        manifest.auth.identity_field = "email".into();
        manifest.auth.identity_label = "Email".into();

        let form = Form::add_member(&manifest, &["member".into(), "admin".into()]);
        let names: Vec<&str> = form.fields.iter().map(|f| f.name.as_str()).collect();
        assert_eq!(names, vec!["email", "role"]);
        assert!(form.fields[0].required);
        // The least surprising starting role, not the first one in the list.
        assert_eq!(form.fields[1].value, "member");

        let mut form = form;
        form.fields[0].set("sam@example.test".into());
        assert_eq!(
            Cli::body_of(&form, false).unwrap(),
            json!({ "email": "sam@example.test", "role": "member" })
        );
    }

    #[test]
    fn a_members_roles_are_the_primary_one_and_their_grants_without_repeats() {
        let sam = member("sam", Some("member"), &["editor", "member"], false);
        // The primary role comes first — it is the one the server reports as
        // *the* role — and a grant that repeats it is not a second role.
        assert_eq!(sam.roles(), vec!["member", "editor"]);

        // Someone with no role at all is a real state: a membership row whose
        // column was cleared.
        assert!(member("kim", None, &[], false).roles().is_empty());
    }

    #[test]
    fn the_team_screen_appears_only_where_memberships_can_be_listed() {
        let with = console(membership_resources("role:admin"));
        assert!(with.nav.iter().any(|item| item.kind == NavKind::Team));

        // An app whose memberships nobody may list has no team to show, and a
        // screen that can only ever say "forbidden" is worse than no screen.
        let private = console(vec![ResourceManifest {
            name: "membership".into(),
            permissions: ActionPermissions {
                list: ActionPermission {
                    value: "private".into(),
                    ..Default::default()
                },
                ..Default::default()
            },
            ..Default::default()
        }]);
        assert!(!private.nav.iter().any(|item| item.kind == NavKind::Team));
    }

    #[tokio::test]
    async fn an_admin_may_take_roles_from_others_but_never_their_own_admin() {
        let mut cli = console(membership_resources("role:admin"));
        cli.main = Main::Team(Team {
            members: vec![
                member("me", Some("admin"), &["editor"], true),
                member("sam", Some("member"), &["editor"], false),
            ],
            index: 0,
            manage: true,
        });

        // Our own admin is not on offer. An organisation can only lose its last
        // administrator if that administrator removes themselves, so this is
        // what keeps every organisation administrable.
        cli.open_revoke_picker();
        let picker = cli.picker.take().expect("a picker for our other role");
        let offered: Vec<&str> = picker.items.iter().map(|(_, role)| role.as_str()).collect();
        assert_eq!(offered, vec!["editor"]);

        // Someone else's roles are all fair game, admin included.
        if let Main::Team(team) = &mut cli.main {
            team.index = 1;
        }
        cli.open_revoke_picker();
        let picker = cli.picker.take().expect("a picker");
        let offered: Vec<&str> = picker.items.iter().map(|(_, role)| role.as_str()).collect();
        assert_eq!(offered, vec!["member", "editor"]);
        // The primary role carries no grant row: it is revoked by clearing the
        // column, and the picker says so by having nothing to delete.
        assert_eq!(picker.items[0].0, "\u{1f}member");
        assert_eq!(picker.items[1].0, "g-sam-editor\u{1f}editor");
    }

    #[tokio::test]
    async fn only_the_last_admin_of_an_organization_is_told_why_they_cannot_resign() {
        let mut cli = console(membership_resources("role:admin"));
        cli.main = Main::Team(Team {
            members: vec![member("me", Some("admin"), &[], true)],
            index: 0,
            manage: true,
        });
        cli.open_revoke_picker();
        assert!(cli.picker.is_none());
        assert!(
            cli.status.contains("another admin can do it for you"),
            "got {:?}",
            cli.status
        );
    }

    #[tokio::test]
    async fn a_role_someone_already_holds_is_never_offered_again() {
        let mut cli = console(membership_resources("role:admin"));
        cli.manifest.auth.known_roles = vec!["member".into(), "editor".into(), "admin".into()];
        cli.main = Main::Team(Team {
            members: vec![member("sam", Some("member"), &["editor"], false)],
            index: 0,
            manage: true,
        });

        // The server refuses a duplicate, and a second copy would make revoking
        // the first look like it did nothing.
        cli.open_grant_picker();
        let picker = cli.picker.take().expect("a picker");
        let offered: Vec<&str> = picker.items.iter().map(|(_, role)| role.as_str()).collect();
        assert_eq!(offered, vec!["admin"]);
    }

    #[tokio::test]
    async fn a_console_that_may_not_grant_roles_says_so_instead_of_offering_a_picker() {
        let mut cli = console(membership_resources("private"));
        cli.main = Main::Team(Team {
            members: vec![member("sam", Some("member"), &[], false)],
            index: 0,
            // What the manifest said about `membership_role.create`.
            manage: false,
        });
        cli.open_grant_picker();
        assert!(cli.picker.is_none());
        assert!(cli.status.contains("may not hand out roles"));
    }

    #[test]
    fn an_app_that_names_no_roles_still_offers_the_two_the_framework_uses() {
        let cli = console(membership_resources("role:admin"));
        assert_eq!(cli.known_roles(), vec!["member", "admin"]);
    }

    #[test]
    fn starting_another_organization_offers_the_fields_the_app_declared() {
        let mut cli = console(vec![organization("authenticated")]);
        cli.start_new_organization();

        let Main::Form(form) = &cli.main else {
            panic!("expected a form, got {:?}", cli.main);
        };
        assert!(matches!(form.kind, FormKind::NewOrganization { .. }));
        // The app's own organisation fields, not a hard-coded pair.
        let names: Vec<&str> = form.fields.iter().map(|f| f.name.as_str()).collect();
        assert_eq!(names, vec!["name", "slug"]);
    }

    #[test]
    fn an_app_that_provisions_tenants_itself_refuses_rather_than_offering_a_form() {
        // A policy the caller cannot satisfy would only produce a 403, so say so
        // instead of putting up a form that cannot be submitted.
        for policy in ["role:admin", "private"] {
            let mut cli = console(vec![organization(policy)]);
            cli.start_new_organization();
            assert!(
                !matches!(cli.main, Main::Form(_)),
                "{policy} should not offer a form"
            );
        }
    }

    #[test]
    fn the_new_organization_form_sends_the_fields_the_app_declared() {
        let organization = organization("authenticated");
        let mut form = Form::new_organization(0, &organization);
        assert_eq!(form.submit, "Create organization");

        // Required means required here too.
        assert!(Cli::body_of(&form, false).is_err());

        form.fields[0].set("Acme Logistics".into());
        assert_eq!(
            Cli::body_of(&form, false).unwrap(),
            json!({ "name": "Acme Logistics" })
        );

        // An untouched optional field is left out, not sent as an empty string
        // that would occupy the unique slug.
        form.fields[1].set("acme".into());
        assert_eq!(
            Cli::body_of(&form, false).unwrap(),
            json!({ "name": "Acme Logistics", "slug": "acme" })
        );
    }

    #[test]
    fn the_sidebar_leaves_out_what_cannot_be_reached() {
        let mut hidden = resource();
        hidden.name = "secret".into();
        hidden.visible = false;
        let mut forbidden = resource();
        forbidden.name = "locked".into();
        forbidden.permissions.list.value = "private".into();

        let manifest = Manifest {
            resources: vec![resource(), hidden, forbidden],
            functions: vec![
                FunctionManifest {
                    name: "sync".into(),
                    label: "Sync".into(),
                    visible: true,
                    ..Default::default()
                },
                FunctionManifest {
                    name: "internal".into(),
                    visible: false,
                    ..Default::default()
                },
            ],
            ..Default::default()
        };

        let nav = navigation(&manifest, &signed_in(&[]));
        let labels: Vec<&str> = nav.iter().map(|item| item.label.as_str()).collect();
        // Products, the one visible action, and the session screen — nothing else.
        assert_eq!(labels, vec!["Products", "Sync", "Session"]);
    }

    #[test]
    fn an_edit_form_starts_from_the_record_and_submits_only_what_changed() {
        let resource = resource();
        let record = json!({ "id": "abc", "name": "Chair", "stock": 4, "live": true });
        let mut form = Form::edit(0, &resource, &record);

        assert_eq!(form.title, "Edit Chair");
        assert!(matches!(&form.kind, FormKind::Update { id, .. } if id == "abc"));
        assert_eq!(form.fields[0].value, "Chair");
        assert_eq!(form.fields[1].value, "4");
        assert_eq!(form.fields[2].value, "true");

        // Nothing touched: nothing to send, so no request that logs a no-op
        // update against every record someone merely looked at.
        let body = Cli::body_of(&form, true).unwrap();
        assert_eq!(body, json!({}));

        form.fields[1].set("9".into());
        let body = Cli::body_of(&form, true).unwrap();
        assert_eq!(body, json!({ "stock": 9 }));
    }

    #[test]
    fn a_create_form_refuses_to_submit_without_a_required_field() {
        let mut resource = resource();
        resource.fields[0].required = true;
        let form = Form::create(0, &resource);
        let error = Cli::body_of(&form, false).unwrap_err();
        assert!(error.contains("required"), "{error}");
    }

    #[test]
    fn a_function_without_a_schema_is_edited_as_one_json_document() {
        let function = FunctionManifest {
            name: "reindex".into(),
            label: "Reindex".into(),
            method: "POST".into(),
            ..Default::default()
        };
        let mut form = Form::run(0, &function);
        assert!(form.raw_body);
        assert_eq!(form.fields.len(), 1);

        form.fields[0].set(r#"{"full": true}"#.into());
        assert_eq!(Cli::body_of(&form, false).unwrap(), json!({ "full": true }));

        form.fields[0].set("not json".into());
        assert!(Cli::body_of(&form, false).is_err());
    }

    #[test]
    fn a_function_with_a_schema_gets_one_box_per_property() {
        let function = FunctionManifest {
            name: "invite".into(),
            label: "Invite".into(),
            input_schema: Some(json!({
                "type": "object",
                "required": ["email"],
                "properties": {
                    "email": { "type": "string", "description": "Who to invite" },
                    "role": { "type": "string", "enum": ["member", "admin"], "default": "member" },
                    "notify": { "type": "boolean" },
                },
            })),
            ..Default::default()
        };
        let mut form = Form::run(0, &function);
        assert!(!form.raw_body);
        assert_eq!(form.fields.len(), 3);

        let email = form.fields.iter().find(|f| f.name == "email").unwrap();
        assert!(email.required);
        assert_eq!(email.help.as_deref(), Some("Who to invite"));

        let role = form.fields.iter().position(|f| f.name == "role").unwrap();
        assert_eq!(form.fields[role].value, "member");
        assert_eq!(form.fields[role].options, vec!["member", "admin"]);
        // Enter cycles a closed set instead of opening a text box.
        form.index = role;
        form_key(&mut form, press(KeyCode::Enter));
        assert!(!form.editing);
        assert_eq!(form.fields[role].value, "admin");

        // An untouched boolean still has to be sent: `false` is an answer.
        let notify = form.fields.iter().position(|f| f.name == "notify").unwrap();
        form.index = notify;
        form_key(&mut form, press(KeyCode::Char(' ')));
        assert_eq!(form.fields[notify].value, "true");
    }

    #[test]
    fn enter_on_the_last_row_submits_and_nowhere_else_does() {
        let function = FunctionManifest {
            label: "Run".into(),
            ..Default::default()
        };
        let mut form = Form::run(0, &function);

        assert!(!form_key(&mut form, press(KeyCode::Enter))); // opens the box
        assert!(form.editing);
        assert!(!form_key(&mut form, press(KeyCode::Esc)));

        assert!(!form_key(&mut form, press(KeyCode::Down)));
        assert!(form.on_submit());
        assert!(form_key(&mut form, press(KeyCode::Enter)));
    }

    #[test]
    fn a_damaged_key_says_what_is_wrong_with_it_without_showing_it() {
        let good = format!("apik_{}", "a1b2c3d4".repeat(8));
        assert_eq!(good.len(), 69);
        assert!(key_shape(&good).is_none());

        // The case that started this: a paste that lost one character. The box
        // shows dots, so the only way anyone finds out is if we say so.
        let short = &good[..good.len() - 1];
        let problem = key_shape(short).unwrap();
        assert!(problem.contains("68 characters"), "{problem}");
        assert!(problem.contains("Characters were lost"), "{problem}");
        assert!(!problem.contains(short), "the key must never be echoed");

        assert!(key_shape("hunter2").unwrap().contains("apik_"));
        assert!(key_shape(&format!("{good}x")).unwrap().contains("extra"));
        assert!(key_shape(&format!("apik_{}", "z".repeat(64)))
            .unwrap()
            .contains("hexadecimal"));
    }

    #[test]
    fn a_paste_lands_whole_in_the_selected_field() {
        let resource = resource();
        let mut cli = console(vec![resource.clone()]);
        cli.main = Main::Form(Form::create(0, &resource));

        // Not in edit mode, and the text contains characters that are form
        // shortcuts — `i` opens a box, `D` clears one. A paste is not typing.
        cli.on_paste("apik_id");
        let Main::Form(form) = &cli.main else {
            panic!()
        };
        assert_eq!(form.fields[0].value, "apik_id");
        assert!(!form.editing);

        // Control characters are not keystrokes either.
        cli.on_paste("\r\n!");
        let Main::Form(form) = &cli.main else {
            panic!()
        };
        assert_eq!(form.fields[0].value, "apik_id!");
    }

    #[test]
    fn typing_edits_at_the_cursor() {
        let mut text = String::new();
        let mut cursor = 0;
        for c in "hello".chars() {
            edit_text(&mut text, &mut cursor, press(KeyCode::Char(c)));
        }
        assert_eq!((text.as_str(), cursor), ("hello", 5));

        edit_text(&mut text, &mut cursor, press(KeyCode::Home));
        edit_text(&mut text, &mut cursor, press(KeyCode::Char('s')));
        assert_eq!(text, "shello");

        edit_text(&mut text, &mut cursor, press(KeyCode::End));
        edit_text(&mut text, &mut cursor, press(KeyCode::Backspace));
        assert_eq!(text, "shell");

        // A key that is not text leaves it alone.
        edit_text(&mut text, &mut cursor, press(KeyCode::F(1)));
        assert_eq!(text, "shell");
    }

    #[test]
    fn a_record_is_named_by_its_display_field() {
        let resource = resource();
        assert_eq!(
            resource.title_of(&json!({ "name": "Chair", "id": "x" })),
            "Chair"
        );
        // Falling back to the id beats showing an empty heading.
        assert_eq!(resource.title_of(&json!({ "id": "x" })), "x");
    }

    #[test]
    fn schema_property_names_become_readable_labels() {
        assert_eq!(titleize("dry_run"), "Dry run");
        assert_eq!(titleize("email"), "Email");
        assert_eq!(titleize(""), "");
    }
}
