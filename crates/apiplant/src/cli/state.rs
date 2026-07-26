//! What the console is showing, and what each key does to it.
//!
//! One state machine, driven by key presses and holding every request it makes,
//! so the drawing code in [`super::ui`] is a pure function of this and never
//! has to decide anything.

use std::collections::BTreeSet;
use std::path::PathBuf;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use serde_json::{Map, Value};
use tokio::sync::oneshot;

use super::api::{self, Client, FunctionManifest, Manifest, ResourceManifest};
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
    Function(usize),
    Session,
}

#[derive(Debug, Clone)]
pub struct NavItem {
    pub label: String,
    pub group: String,
    pub kind: NavKind,
}

/// Build the sidebar from the manifest.
///
/// Only what the operator can actually reach: a resource the server will not
/// list for anyone, or a function with no endpoint, is a dead entry that only
/// teaches people the console is broken.
fn navigation(manifest: &Manifest) -> Vec<NavItem> {
    let mut resources: Vec<(usize, &ResourceManifest)> = manifest
        .resources
        .iter()
        .enumerate()
        .filter(|(_, resource)| resource.visible && resource.permissions.list.possible())
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

    for (index, function) in manifest.functions.iter().enumerate() {
        if !function.visible {
            continue;
        }
        items.push(NavItem {
            label: function.label.clone(),
            group: function.group.clone().unwrap_or_else(|| "Actions".into()),
            kind: NavKind::Function(index),
        });
    }

    items.push(NavItem {
        label: "Session".into(),
        group: "Console".into(),
        kind: NavKind::Session,
    });
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FormKind {
    SignInPassword,
    SignInKey,
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
    /// The same request as `Create` on `organization`, but it resolves the
    /// onboarding modal rather than returning to a list.
    FoundOrganization {
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

    /// A form for creating one record of `resource`.
    fn create(index: usize, resource: &ResourceManifest) -> Form {
        let fields = resource
            .fields
            .iter()
            .filter(|field| field.editable())
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
            .collect();
        Form::new(
            format!("New {}", resource.label.to_lowercase()),
            "Create",
            FormKind::Create { resource: index },
            fields,
        )
    }

    /// The onboarding form: the organisation's own fields, whatever the app has
    /// made those, with wording for someone who has nowhere to work yet.
    fn found_organization(index: usize, resource: &ResourceManifest) -> Form {
        let mut form = Form::create(index, resource);
        form.title = "Create your organization".into();
        form.submit = "Create organization".into();
        form.subtitle = Some(
            "Almost everything here belongs to an organization, and you are in none yet. \
             Create one and you become its admin."
                .into(),
        );
        form.kind = FormKind::FoundOrganization { resource: index };
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
}

#[derive(Debug, Clone)]
pub struct Detail {
    pub resource: usize,
    pub record: Value,
    pub scroll: u16,
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
    Delete { resource: usize, id: String },
    Run { function: usize, body: Value },
    SignOut,
}

#[derive(Debug, Clone)]
pub struct Confirm {
    pub prompt: String,
    pub action: ConfirmAction,
}

/// The first thing after signing in, when the account belongs to no
/// organisation.
///
/// Nearly every resource is scoped to one, so a session without one lists
/// nothing and fails every write — which reads as a broken console rather than
/// as an account that is not finished. Which of the two forms this takes is the
/// app's decision, not ours: an app whose `organization` resource lets an
/// authenticated caller create one gets a form, and an app that provisions
/// tenants itself gets the only useful answer, which is who to ask.
#[derive(Debug, Clone)]
pub enum Onboarding {
    Create(Form),
    AskAnAdmin,
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

pub const SIGN_IN_OPTIONS: [(&str, &str); 3] = [
    (
        "Open the dashboard in a browser",
        "Sign in there and it sends a key straight back to this console.",
    ),
    (
        "Sign in with an email and password",
        "The console mints and saves a key for you.",
    ),
    ("Paste an API key", "For a key you already have."),
];

// --- the console ------------------------------------------------------------

pub struct Cli {
    pub client: Client,
    pub manifest: Manifest,
    pub store: Store,
    pub dir: PathBuf,

    pub nav: Vec<NavItem>,
    pub nav_index: usize,
    pub focus: Focus,
    pub main: Main,

    pub sign_in: Option<SignIn>,
    pub picker: Option<Picker>,
    pub confirm: Option<Confirm>,
    pub onboarding: Option<Onboarding>,
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
    pub status: String,
    pub error: Option<String>,
    pub quit: bool,
    /// Printed once the screen has been given back.
    pub farewell: Option<String>,
}

impl Cli {
    pub fn new(client: Client, manifest: Manifest, store: Store, dir: PathBuf) -> Cli {
        let nav = navigation(&manifest);
        Cli {
            client,
            manifest,
            store,
            dir,
            nav,
            nav_index: 0,
            focus: Focus::Nav,
            main: Main::Empty("Pick something from the sidebar.".into()),
            sign_in: None,
            picker: None,
            confirm: None,
            onboarding: None,
            help: false,
            organizations: Vec::new(),
            organizations_known: false,
            identity: None,
            identity_id: None,
            identity_note: None,
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
        self.status = format!("Connected to {}", self.client.origin);

        // Decide about the organisation *before* opening anything. Loading a
        // list first leaves an error on screen about work the modal is already
        // explaining you cannot do yet.
        self.check_organization();
        if self.onboarding.is_none() {
            self.open_selected().await;
        }
    }

    /// Put the onboarding modal up if there is nowhere to work.
    fn check_organization(&mut self) {
        // Asking someone to create an organisation because a request failed is
        // worse than not asking at all: they already have one, and now they are
        // being pushed towards a second.
        if !self.organizations.is_empty() || !self.organizations_known {
            self.onboarding = None;
            return;
        }
        // A `role:` policy can never be satisfied by someone with no
        // organisation, so anything narrower than "authenticated" means the
        // only way in is for an admin to add them.
        let founding = self
            .manifest
            .resources
            .iter()
            .enumerate()
            .find(|(_, resource)| resource.name == "organization")
            .filter(|(_, resource)| {
                matches!(
                    resource.permissions.create.value.as_str(),
                    "public" | "authenticated"
                )
            })
            .map(|(index, resource)| Form::found_organization(index, resource));

        self.onboarding = Some(match founding {
            Some(form) => Onboarding::Create(form),
            None => Onboarding::AskAnAdmin,
        });
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

    pub fn resource(&self, index: usize) -> Option<&ResourceManifest> {
        self.manifest.resources.get(index)
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
                });
                self.reload().await;
            }
            NavKind::Function(index) => {
                let Some(function) = self.function(index) else {
                    return;
                };
                self.main = Main::Form(Form::run(index, function));
            }
            NavKind::Session => self.main = Main::Session,
        }
    }

    /// Re-run the current list query.
    pub async fn reload(&mut self) {
        let Main::List(list) = &self.main else { return };
        let (resource_index, page, search) = (list.resource, list.page, list.search.clone());
        let Some(resource) = self.resource(resource_index).cloned() else {
            return;
        };

        let mut query: Vec<(&str, String)> = vec![
            ("limit", PAGE.to_string()),
            ("offset", (page * PAGE).to_string()),
        ];
        // The API filters by exact field value; the manifest names the one field
        // the dashboard's search box uses, so the console searches the same one.
        let search_field = resource.search_field.clone();
        if let (Some(field), false) = (search_field.as_deref(), search.trim().is_empty()) {
            query.push((field, search.trim().to_string()));
        }

        self.status = format!("Loading {}…", resource.plural.to_lowercase());
        match self.client.list(&resource.name, &query).await {
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
        let record = match row.get("id").and_then(Value::as_str) {
            Some(id) if resource.permissions.read.possible() => {
                match self.client.read(&resource.name, id).await {
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
        if !resource.permissions.create.possible() {
            self.fail(format!("{} cannot be created here", resource.plural));
            return;
        }
        self.main = Main::Form(Form::create(index, &resource));
    }

    fn start_edit(&mut self) {
        let Some((index, resource)) = self.current_resource() else {
            return;
        };
        if !resource.permissions.update.possible() {
            self.fail(format!("{} cannot be edited here", resource.plural));
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
        if !resource.permissions.delete.possible() {
            self.fail(format!("{} cannot be deleted here", resource.plural));
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
        // The onboarding modal owns the screen while it is up, so its form is
        // the one being submitted whatever the pane underneath holds.
        if let Some(Onboarding::Create(form)) = self.onboarding.clone() {
            return self.found_organization(form).await;
        }
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
            FormKind::SignInPassword | FormKind::SignInKey => self.submit_sign_in(form).await,
            // Only reachable from the onboarding modal, which is handled above.
            FormKind::FoundOrganization { .. } => {}
        }
    }

    /// Create the caller's first organisation and let them in.
    async fn found_organization(&mut self, form: Form) {
        let FormKind::FoundOrganization { resource } = form.kind else {
            return;
        };
        let Some(resource) = self.resource(resource).cloned() else {
            return;
        };
        let body = match Self::body_of(&form, false) {
            Ok(body) => body,
            Err(problem) => return self.fail(problem),
        };

        self.status = "Creating your organization…".into();
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
                self.onboarding = None;
                self.say(format!("{label} is ready. You are its admin."));
                self.open_selected().await;
            }
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
            FormKind::SignInPassword => {
                let identity = form.fields[0].value.trim().to_string();
                let password = form.fields[1].value.clone();
                let field = self.manifest.auth.identity_field.clone();
                self.status = "Signing in…".into();
                match self.client.login(&field, &identity, &password).await {
                    Ok(token) => {
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
        self.onboarding = None;
        self.main = Main::Empty(String::new());
        self.sign_in = Some(SignIn::Menu { index: 0 });
        self.say("Signed out. The saved key for this server was removed.");
    }

    /// Mint another key and show it — for pasting into a script or another
    /// machine, which is the whole reason to want one from here.
    async fn issue_key(&mut self) {
        self.status = "Issuing a key…".into();
        match self.client.create_api_key(&key_label()).await {
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
                self.say(format!("Now working in {label}."));
                // Every org-scoped list on screen is now the wrong list.
                if matches!(self.main, Main::List(_)) {
                    self.reload().await;
                }
            }
            PickerKind::Reference { field } => {
                if let Main::Form(form) = &mut self.main {
                    if let Some(field) = form.fields.get_mut(field) {
                        field.set(value);
                    }
                }
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

        let field = match (&mut self.onboarding, &mut self.sign_in, &mut self.main) {
            (Some(Onboarding::Create(form)), _, _) => form.current(),
            (_, Some(SignIn::Form(form)), _) => form.current(),
            (_, _, Main::Form(form)) => form.current(),
            (_, _, Main::List(list)) if list.searching => {
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
        if self.onboarding.is_some() {
            return self.onboarding_key(key).await;
        }
        if self.sign_in.is_some() {
            return self.sign_in_key(key).await;
        }
        self.main_key(key).await;
    }

    async fn onboarding_key(&mut self, key: KeyEvent) {
        // Esc puts it away. A terminal has no second window to escape to, and
        // the global resources — your account, your keys — work perfectly well
        // without an organisation; the Session screen can bring it back.
        let editing = matches!(
            self.onboarding.as_ref(),
            Some(Onboarding::Create(form)) if form.editing
        );
        if matches!(key.code, KeyCode::Esc) && !editing {
            self.onboarding = None;
            self.say("Carrying on without an organization — press N on the Session screen to come back to this.");
            return self.open_selected().await;
        }

        match self.onboarding.as_mut() {
            Some(Onboarding::Create(_)) => {
                let submitted = {
                    let Some(Onboarding::Create(form)) = self.onboarding.as_mut() else {
                        return;
                    };
                    form_key(form, key)
                };
                if submitted {
                    self.submit().await;
                }
            }
            Some(Onboarding::AskAnAdmin) => match key.code {
                // An admin may have added them while this was on screen.
                KeyCode::Char('r') => {
                    self.status = "Checking…".into();
                    self.load_organizations().await;
                    self.check_organization();
                    if self.onboarding.is_some() {
                        self.say("Still no organization on your account.");
                    } else {
                        self.open_selected().await;
                    }
                }
                KeyCode::Char('x') => {
                    self.onboarding = None;
                    self.sign_out();
                }
                KeyCode::Char('q') => self.quit = true,
                _ => {}
            },
            None => {}
        }
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
        match self.sign_in.as_mut() {
            Some(SignIn::Menu { index }) => match key.code {
                KeyCode::Down | KeyCode::Char('j') => {
                    *index = (*index + 1).min(SIGN_IN_OPTIONS.len() - 1)
                }
                KeyCode::Up | KeyCode::Char('k') => *index = index.saturating_sub(1),
                KeyCode::Char('q') => self.quit = true,
                KeyCode::Char('?') => self.help = true,
                KeyCode::Enter => match *index {
                    0 => self.start_browser_sign_in().await,
                    1 => {
                        let form = Form::sign_in_password(&self.manifest);
                        self.sign_in = Some(SignIn::Form(form));
                    }
                    _ => self.sign_in = Some(SignIn::Form(Form::sign_in_key())),
                },
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
            return self.open_selected().await;
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

    async fn session_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Char('g') => self.issue_key().await,
            KeyCode::Char('N') => {
                self.load_organizations().await;
                self.check_organization();
                if self.onboarding.is_none() {
                    self.say("You already belong to an organization.");
                }
            }
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

/// What is visibly wrong with an API key, if anything.
///
/// Every key the server issues is `apik_` followed by 64 hex characters, so a
/// string that is not that was mistyped, truncated, or lost characters on the
/// way through the terminal — which is invisible when the box shows dots. This
/// never returns the key itself.
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

    /// A console that has asked the server about organisations and been told
    /// the answer — which is what makes an empty list mean anything.
    fn console(resources: Vec<ResourceManifest>) -> Cli {
        let manifest = Manifest {
            resources,
            ..Default::default()
        };
        let client = Client::new("http://x:1".into(), "/api".into(), "/admin".into()).unwrap();
        let mut cli = Cli::new(client, manifest, Store::default(), PathBuf::from("."));
        cli.organizations_known = true;
        cli
    }

    #[test]
    fn an_account_with_no_organization_is_offered_one_to_create() {
        let mut cli = console(vec![organization("authenticated")]);
        cli.check_organization();

        let Some(Onboarding::Create(form)) = &cli.onboarding else {
            panic!("expected a form, got {:?}", cli.onboarding);
        };
        assert!(matches!(form.kind, FormKind::FoundOrganization { .. }));
        // The app's own organisation fields, not a hard-coded pair.
        let names: Vec<&str> = form.fields.iter().map(|f| f.name.as_str()).collect();
        assert_eq!(names, vec!["name", "slug"]);

        // Once they belong somewhere, it goes away.
        cli.organizations = vec![("id".into(), "Acme".into())];
        cli.check_organization();
        assert!(cli.onboarding.is_none());
    }

    #[test]
    fn a_failed_lookup_never_asks_someone_to_create_a_second_organization() {
        // "The request did not answer" and "you belong to nothing" are different
        // facts. Treating the first as the second is how someone who already has
        // an organisation gets pushed towards making another one.
        let mut cli = console(vec![organization("authenticated")]);
        cli.organizations_known = false;
        cli.check_organization();
        assert!(cli.onboarding.is_none());
    }

    #[test]
    fn an_app_that_provisions_tenants_itself_says_who_to_ask() {
        // A `role:` policy can never be satisfied by someone in no organisation,
        // so offering the form would only produce a 403.
        for policy in ["role:admin", "none", "member"] {
            let mut cli = console(vec![organization(policy)]);
            cli.check_organization();
            assert!(
                matches!(cli.onboarding, Some(Onboarding::AskAnAdmin)),
                "{policy} should not offer a form"
            );
        }
    }

    #[test]
    fn the_onboarding_form_sends_the_fields_the_app_declared() {
        let organization = organization("authenticated");
        let mut form = Form::found_organization(0, &organization);
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
        forbidden.permissions.list.value = "none".into();

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

        let nav = navigation(&manifest);
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
