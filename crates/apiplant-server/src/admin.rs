//! The admin dashboard: its manifest, and baking a static copy of it.
//!
//! The dashboard is embedded in the binary and [served live](crate::run) for
//! every app, so the common case generates nothing. [`build`] writes the same
//! files out as a plain directory (`index.html`, `app.js`, `app.css` and a
//! manifest) for **hosting it somewhere other than the API** — a CDN, a bucket,
//! a different origin entirely. That copy is never read back by the server:
//! the running dashboard always describes the running app. Everything it needs
//! to know about the app — which resources exist, what to call them, which
//! fields to show, who may see what — is resolved *here*, at build time, and
//! written into `apiplant-admin.json`. The shipped JavaScript is the same for
//! every app.
//!
//! Two things are kept firmly apart, and it matters:
//!
//! * `[permissions]` / a function's `permission` decide what the **API**
//!   allows. They are enforced by the server on every request.
//! * `[admin]` decides what an **operator is shown**. It is presentation, and
//!   this generator treats it as such — hiding a resource here does not protect
//!   it, and the manifest never carries anything a signed-in caller could not
//!   already read from the API.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{anyhow, bail, Context, Result};
use apiplant_abi::{FunctionAccess, HttpMethod};
use apiplant_core::schema::{
    is_auth_resource, relation_name, titleize, Access, ContentFormat, Field, FieldType, OnDelete,
    Policy, Resource, Widget,
};
use apiplant_core::{Agent, App, ORG_CLASS_FIELD};
use serde::Serialize;
use serde_json::Value;

use crate::auth_routes::VERIFIED_AT_FIELD;
use crate::functions::FunctionRegistry;

/// Name of the manifest file the dashboard fetches on load.
pub const MANIFEST_FILE: &str = "apiplant-admin.json";

#[derive(Debug, Clone)]
pub struct Options {
    pub api: String,
    pub out: Option<PathBuf>,
}

#[derive(Debug, Serialize)]
struct AdminManifest {
    title: String,
    app_name: String,
    /// URL of the app's own mark, when it configured one.
    logo: Option<String>,
    /// Whether an account with no `avatar_url` may be drawn with its Gravatar,
    /// off unless `[admin] gravatar` turns it on: the dashboard should not
    /// hand addresses to a third party a deployment did not opt into.
    gravatar: bool,
    api_base_url: String,
    docs_url: Option<String>,
    /// Present only when the dashboard may call the app's AI endpoint to help
    /// fill text fields.
    #[serde(skip_serializing_if = "Option::is_none")]
    ai_assistance: Option<AdminAiAssistanceManifest>,
    auth: AuthManifest,
    resources: Vec<ResourceManifest>,
    functions: Vec<FunctionManifest>,
    agents: Vec<AgentManifest>,
    /// Present only in an app that takes money, so the dashboard shows its
    /// billing screens exactly where the `/billing` routes are mounted and the
    /// `billing_*` resources exist.
    #[serde(skip_serializing_if = "Option::is_none")]
    billing: Option<BillingManifest>,
    organization: OrganizationManifest,
}

/// Deployment-wide rules about the tenant itself.
#[derive(Debug, Serialize)]
struct OrganizationManifest {
    /// Who may write `organization.org_class` — the dashboard shows the class
    /// as an editable field only to them, and read-only to everyone else,
    /// rather than offering an input the server would silently ignore.
    org_class_editors: ActionPermissionManifest,
    /// Every class the app's own permissions mention, so the dashboard can
    /// offer them instead of asking an operator to remember the spelling.
    /// A class is a free string, so this is a suggestion, not a constraint.
    known_classes: Vec<String>,
}

/// What the dashboard needs to render billing.
#[derive(Debug, Serialize)]
struct BillingManifest {
    /// `stripe`.
    provider: String,
    /// Safe to put in a page: it is designed to be.
    publishable_key: String,
    /// The currency amounts are quoted in, for formatting a price list.
    currency: String,
    /// Whether the amounts shown are before tax — which is what a price list
    /// has to say out loud, because "€10" meaning "€12 at the till" is the
    /// single most complained-about thing in software pricing.
    automatic_tax: bool,
    /// Whether checkout asks the buyer for a VAT/GST number.
    tax_id_collection: bool,
    /// Whether deliveries can be verified. False means checkouts complete and
    /// nothing is ever recorded — worth saying on screen rather than leaving
    /// an operator to notice an empty table.
    webhooks_configured: bool,
}

#[derive(Debug, Serialize)]
struct AdminAiAssistanceManifest {
    prompt_placeholder: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    system: Option<String>,
}

#[derive(Debug, Serialize)]
struct AuthManifest {
    /// The field a person logs in with, and a label to put above the box.
    identity_field: String,
    identity_label: String,
    allow_registration: bool,
    /// Whether this deployment can send email at all. The three flags below are
    /// each already `false` without it — this one exists so an interface can
    /// explain *why* a button is missing ("no email provider is configured")
    /// rather than silently omitting it.
    email_enabled: bool,
    /// Whether a new account must confirm its address before it can sign in, so
    /// the register form can say what will happen instead of waiting for a
    /// login to be refused.
    require_email_verification: bool,
    /// Whether the team screen may invite somebody who has no account yet.
    invitations_enabled: bool,
    /// Whether to offer "forgot your password?".
    password_reset_enabled: bool,
    /// Extra fields the register form should collect, so nobody has to type
    /// JSON to create an account.
    signup_fields: Vec<FieldManifest>,
    /// Fields the account screen lets someone edit about themselves.
    profile_fields: Vec<FieldManifest>,
    /// Roles seen anywhere in the app's permissions, so role pickers can offer
    /// real choices instead of a free-text box.
    known_roles: Vec<String>,
    /// The third-party sign-ins this deployment offers, in the order they
    /// should be drawn: `{ provider, label, start_url }` each.
    ///
    /// Empty when `[oauth]` names nothing, which is what keeps a sign-in
    /// screen from offering a button that would land on a 404 — the same
    /// reason the three email flags above exist.
    oauth_providers: Vec<OAuthProviderManifest>,
}

#[derive(Debug, Serialize)]
struct OAuthProviderManifest {
    provider: String,
    /// What the button says.
    label: String,
    /// Where the button goes, **relative to `api_base_url`** — like every other
    /// path in this manifest, so a console pointed at a remote API builds the
    /// URL the same way it builds all the others.
    start_url: String,
    /// False for a provider that releases no address, so a screen can say why
    /// an account created through it has no email on file.
    provides_email: bool,
    /// A logo the app supplied for a provider apiplant does not draw itself —
    /// `[oauth.<name>] icon`, usually a file in `public/`. Empty for the four
    /// it does draw, and for one nobody gave an image for.
    icon: String,
}

#[derive(Debug, Serialize)]
struct ResourceManifest {
    name: String,
    /// Singular human label ("Purchase order").
    label: String,
    /// Collection human label ("Purchase orders").
    plural: String,
    /// Sidebar grouping, or `None` for the ungrouped tail.
    group: Option<String>,
    order: i64,
    builtin: bool,
    /// One of the auth/tenancy resources the dashboard manages with a dedicated
    /// screen rather than a generic table.
    auth_resource: bool,
    /// Whether it belongs in the resource navigation at all.
    visible: bool,
    /// Organisation roles that may see it; empty means "anyone who can list it".
    roles: Vec<String>,
    scope: &'static str,
    owner_field: String,
    /// Field whose value names a record in tables, pickers and headings.
    display_field: Option<String>,
    /// Field the list search box filters on.
    search_field: Option<String>,
    /// Every field `?search=` looks in — what the dashboard's search box
    /// actually covers, which may be more than one column.
    search_fields: Vec<String>,
    /// Columns for the list table, in order.
    columns: Vec<String>,
    fields: Vec<FieldManifest>,
    /// `belongs_to` edges out of this resource.
    relations: Vec<RelationManifest>,
    /// `has_many` edges into it — the record screen lists these inline.
    children: Vec<ChildManifest>,
    permissions: ActionPermissionsManifest,
}

#[derive(Debug, Serialize)]
struct ActionPermissionsManifest {
    list: ActionPermissionManifest,
    read: ActionPermissionManifest,
    create: ActionPermissionManifest,
    update: ActionPermissionManifest,
    delete: ActionPermissionManifest,
}

#[derive(Debug, Serialize)]
struct ActionPermissionManifest {
    value: String,
    /// The role name when `value` is `role:<name>`, so the UI needn't re-parse.
    role: Option<String>,
    /// The organisation class the policy is narrowed to, when it names one —
    /// the `@org_class=` half of the value, split out for the same reason.
    org_class: Option<String>,
    note: String,
    requires_org: bool,
}

#[derive(Debug, Serialize)]
struct FieldManifest {
    name: String,
    label: String,
    #[serde(rename = "type")]
    ty: &'static str,
    /// The input to render; `auto` lets the interface choose from `type`.
    widget: &'static str,
    help: Option<String>,
    placeholder: Option<String>,
    /// What the text is: `plain`, `markdown` or `html`. Presentation only —
    /// the dashboard highlights and previews the markup.
    format: &'static str,
    options: Vec<FieldOption>,
    required: bool,
    unique: bool,
    /// Stripped from API responses entirely (a password hash, say).
    hidden: bool,
    /// Present in the API but deliberately not shown in the dashboard.
    admin_visible: bool,
    readonly: bool,
    max_length: Option<u32>,
    references: Option<String>,
    relation: Option<String>,
    on_delete: Option<&'static str>,
    default_value: Option<Value>,
    /// Whether the dashboard may submit this field on create/update.
    writable: bool,
}

#[derive(Debug, Serialize)]
struct FieldOption {
    value: String,
    label: String,
}

#[derive(Debug, Serialize)]
struct RelationManifest {
    field: String,
    relation: String,
    target: String,
    /// Human label for the link ("Customer").
    label: String,
    required: bool,
}

/// A resource that points *at* this one — rendered as a related list on the
/// record screen, which is what turns a table of foreign keys into something a
/// non-technical operator can actually navigate.
#[derive(Debug, Serialize)]
struct ChildManifest {
    resource: String,
    /// The child's field that points here.
    field: String,
    label: String,
}

#[derive(Debug, Serialize)]
struct FunctionManifest {
    name: String,
    label: String,
    description: String,
    group: Option<String>,
    order: i64,
    method: &'static str,
    /// Effective access policy, in the shared `[permissions]` grammar.
    permission: String,
    role: Option<String>,
    permission_note: String,
    requires_org: bool,
    /// Whether it belongs in the dashboard's action list.
    visible: bool,
    roles: Vec<String>,
    /// Text for the confirmation step, or `None` to run without one.
    confirm: Option<String>,
    run_label: String,
    /// JSON Schema for the request body; the dashboard renders a form from it.
    input_schema: Option<Value>,
    output_schema: Option<Value>,
}

#[derive(Debug, Serialize)]
struct AgentManifest {
    name: String,
    label: String,
    description: String,
    scope: &'static str,
    storage: bool,
    thread_resource: Option<String>,
    message_resource: Option<String>,
    chat: ActionPermissionManifest,
    history: ActionPermissionManifest,
    delete_history: ActionPermissionManifest,
}

/// The `admin { … }` block of a function manifest, as carried over the ABI.
#[derive(Debug, Default, serde::Deserialize)]
#[serde(default)]
struct FunctionAdmin {
    visible: Option<bool>,
    roles: Vec<String>,
    label: Option<String>,
    group: Option<String>,
    description: Option<String>,
    confirm: Option<String>,
    run_label: Option<String>,
    order: Option<i64>,
}

pub fn build(app_dir: &Path, options: Options) -> Result<PathBuf> {
    let app = App::load(app_dir)?;
    let api_base_url = normalize_api_base(
        &options.api,
        &app.config.server.base_path,
        app.tls.is_some(),
    )?;
    let output_dir = options.out.unwrap_or_else(|| app_dir.join("admin"));
    let registry = FunctionRegistry::load(&app);
    // A baked copy has to make the same three claims the running server would,
    // so the mailer is built here too — for its verdict, not to send anything.
    let email_enabled = apiplant_email::Mailer::from_config(&app.config.email)
        .map(|mailer| mailer.is_some())
        .unwrap_or(false);
    let manifest = build_manifest(&app, &registry, api_base_url.clone(), email_enabled)?;

    fs::create_dir_all(&output_dir)
        .with_context(|| format!("failed to create {}", output_dir.display()))?;

    for (relative, _) in apiplant_assets::ADMIN {
        let path = output_dir.join(relative);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("failed to create {}", parent.display()))?;
        }
        let bytes = asset(relative).expect("listed asset");
        write_bytes(path, &bytes)?;
    }
    write_json(output_dir.join(MANIFEST_FILE), &manifest)?;

    Ok(output_dir)
}

/// One file of the embedded dashboard, ready to serve or write out.
///
/// The stylesheet is rewritten on the way past: Vite emits absolute
/// `url(/head.png)` references, and the dashboard is never at the site root —
/// it is under `/admin/`, or in a directory someone hosts wherever they like.
pub fn asset(path: &str) -> Option<std::borrow::Cow<'static, [u8]>> {
    use std::borrow::Cow;

    let bytes = apiplant_assets::find(apiplant_assets::ADMIN, path)?;
    if path.trim_matches('/') == "app.css" {
        let css = String::from_utf8_lossy(bytes)
            .replace("url(/head.png)", "url(./head.png)")
            .replace("url(/head-inverted.png)", "url(./head-inverted.png)");
        return Some(Cow::Owned(css.into_bytes()));
    }
    Some(Cow::Borrowed(bytes))
}

/// The manifest for an app already loaded by the server, as JSON.
///
/// `api_base_url` is the prefix the dashboard puts in front of every request;
/// served from the app's own origin that is just the API's `base_path`.
pub fn manifest_json(
    app: &App,
    functions: &FunctionRegistry,
    api_base_url: String,
    email_enabled: bool,
) -> Result<String> {
    let manifest = build_manifest(app, functions, api_base_url, email_enabled)?;
    Ok(serde_json::to_string(&manifest)?)
}

fn build_manifest(
    app: &App,
    functions: &FunctionRegistry,
    api_base_url: String,
    email_enabled: bool,
) -> Result<AdminManifest> {
    let app_name = app.display_name();
    let user = app.resources.get("user");
    let identity_field = user
        .and_then(|resource| resource.auth.as_ref())
        .map(|auth| auth.identity_field.clone())
        .unwrap_or_else(|| "email".to_string());
    let password_field = user
        .and_then(|resource| resource.auth.as_ref())
        .map(|auth| auth.password_field.clone())
        .unwrap_or_else(|| "password_hash".to_string());
    let docs_url = if app.config.docs.enabled {
        Some(format!("{}{}", api_base_url, app.config.docs.path))
    } else {
        None
    };

    // Functions bound to a resource's lifecycle are machinery, not operator
    // actions; they never appear as something to "run" even when their
    // permission would allow it.
    let hook_functions = app
        .resources
        .values()
        .flat_map(|resource| {
            resource
                .hooks
                .iter()
                .map(|(_, function)| function.to_string())
        })
        .collect::<BTreeSet<_>>();

    // Reverse index: which resources point at each resource, so a record screen
    // can offer its related lists.
    let mut children: BTreeMap<String, Vec<ChildManifest>> = BTreeMap::new();
    for child in app.resources.values() {
        let references = child.references();
        for reference in &references {
            // A tenancy column is plumbing — every org-scoped row has one, and
            // "this organization → all its products" is not a relationship
            // anyone wants to browse from the organisation screen.
            if reference.field == "organization_id" {
                continue;
            }
            if !app.resources.contains_key(&reference.target) {
                continue;
            }
            // When a child points at the same parent twice — an order's billing
            // *and* shipping address — the resource name alone names both
            // lists, so the relation has to disambiguate them.
            let ambiguous = references
                .iter()
                .filter(|other| other.target == reference.target)
                .count()
                > 1;
            let label = if ambiguous {
                format!(
                    "{} ({})",
                    child.admin_plural(),
                    titleize(&reference.relation).to_lowercase()
                )
            } else {
                child.admin_plural()
            };
            children
                .entry(reference.target.clone())
                .or_default()
                .push(ChildManifest {
                    resource: child.meta.name.clone(),
                    field: reference.field.clone(),
                    label,
                });
        }
    }

    let resources = app
        .resources
        .values()
        .map(|resource| {
            resource_manifest(
                resource,
                &password_field,
                children
                    .remove(resource.meta.name.as_str())
                    .unwrap_or_default(),
            )
        })
        .collect::<Vec<_>>();

    let mut loaded_functions = functions
        .iter()
        .filter(|entry| !hook_functions.contains(entry.manifest.name.as_str()))
        .map(|entry| function_manifest(&entry.manifest))
        // A private function has no endpoint at all, so there is nothing for an
        // operator to run and nothing to show.
        .filter(|manifest| manifest.permission != "private")
        .collect::<Vec<_>>();
    loaded_functions.sort_by(|left, right| {
        left.group
            .cmp(&right.group)
            .then(left.order.cmp(&right.order))
            .then(left.label.cmp(&right.label))
    });

    let mut agents = app
        .agents
        .values()
        .map(agent_manifest)
        .collect::<Vec<_>>();
    agents.sort_by(|left, right| {
        left.label
            .cmp(&right.label)
            .then(left.name.cmp(&right.name))
    });

    let signup_fields = user
        .map(|resource| {
            resource
                .fields
                .iter()
                .filter(|(name, field)| {
                    // The identity and password have their own inputs on the
                    // form. Of the rest, a field is asked for when the resource
                    // says so — `[fields.<name>.admin] signup`, which is how an
                    // app adds `name` and `surname` to the form without making
                    // them mandatory — and otherwise when it is `required`,
                    // since leaving one of those out simply fails the signup.
                    *name != &identity_field
                        && *name != &password_field
                        && field.admin.in_signup(field)
                        && !field.hidden
                        && field.admin.visible
                        && name.as_str() != "organization_id"
                        && name.as_str() != VERIFIED_AT_FIELD
                })
                .map(|(name, field)| field_manifest(name, field, resource))
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();

    let profile_fields = user
        .map(|resource| {
            resource
                .fields
                .iter()
                .filter(|(name, field)| {
                    !field.hidden && field.admin.visible && *name != &password_field
                })
                .map(|(name, field)| field_manifest(name, field, resource))
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();

    Ok(AdminManifest {
        title: format!("{app_name} admin"),
        app_name,
        logo: app.config.admin.logo.clone(),
        gravatar: app.config.admin.gravatar,
        api_base_url,
        docs_url,
        ai_assistance: admin_ai_assistance_manifest(app),
        auth: AuthManifest {
            identity_label: titleize(&identity_field),
            identity_field,
            allow_registration: app.config.auth.allow_registration,
            email_enabled,
            require_email_verification: app.config.auth.requires_email_verification(email_enabled),
            invitations_enabled: app.config.auth.invitations_enabled(email_enabled),
            password_reset_enabled: app.config.auth.password_reset_enabled(email_enabled),
            signup_fields,
            profile_fields,
            known_roles: known_roles(app, functions),
            oauth_providers: oauth_providers_manifest(app),
        },
        resources,
        functions: loaded_functions,
        agents,
        billing: billing_manifest(app),
        organization: OrganizationManifest {
            // `false` for `org_scoped`: the setting is answered against the
            // organisation the caller *selected*, like any global policy.
            org_class_editors: permission_manifest(
                &app.config.organization.org_class_policy(),
                false,
            ),
            known_classes: known_classes(app),
        },
    })
}

/// The sign-in buttons, built from config alone.
///
/// Deliberately not built from `apiplant_oauth::Providers`: the manifest is
/// generated by `apiplant admin` as well as by the running server, and a static
/// dashboard built on a laptop should describe the same buttons as the
/// deployment it is built for. Config is what both have.
fn oauth_providers_manifest(app: &App) -> Vec<OAuthProviderManifest> {
    app.config
        .oauth
        .active_providers()
        .into_iter()
        .map(|provider| {
            let configured = app.config.oauth.providers.get(provider);
            let builtin = apiplant_oauth::BUILTIN.iter().find(|b| b.key == provider);
            let label = configured
                .map(|c| c.label.trim())
                .filter(|label| !label.is_empty())
                .map(str::to_string)
                .or_else(|| builtin.map(|b| b.label.to_string()))
                .unwrap_or_else(|| titleize(provider));
            OAuthProviderManifest {
                start_url: format!("/auth/oauth/{provider}/start"),
                provides_email: builtin.map(|b| b.provides_email).unwrap_or(true),
                icon: configured
                    .map(|c| c.icon.trim())
                    .unwrap_or_default()
                    .to_string(),
                provider: provider.to_string(),
                label,
            }
        })
        .collect()
}

fn admin_ai_assistance_manifest(app: &App) -> Option<AdminAiAssistanceManifest> {
    let assistance = &app.config.admin.ai_assistance;
    (app.config.ai.enabled() && assistance.enabled).then(|| AdminAiAssistanceManifest {
        prompt_placeholder: assistance.prompt_placeholder.trim().to_string(),
        system: (!assistance.system.trim().is_empty())
            .then(|| assistance.system.trim().to_string()),
    })
}

/// Every role named anywhere in the app — resource permissions, function
/// permissions, `[admin] roles` — so the team screen can offer a dropdown
/// rather than asking someone to remember how "admin" is spelled.
/// The billing block, for an app that takes money.
///
/// Read from `[payments]` rather than from a built [`Payments`] client,
/// because this is also called by `apiplant admin` — which generates a
/// dashboard from a directory, offline, with no keys to connect anything
/// with. The two agree: a configured provider that cannot be built fails the
/// boot, so there is no app where one says yes and the other no.
///
/// [`Payments`]: apiplant_payments::Payments
fn billing_manifest(app: &App) -> Option<BillingManifest> {
    let payments = &app.config.payments;
    payments.enabled().then(|| BillingManifest {
        provider: payments.provider.trim().to_ascii_lowercase(),
        publishable_key: payments.publishable_key.trim().to_string(),
        currency: payments.default_currency(),
        automatic_tax: payments.automatic_tax,
        tax_id_collection: payments.collects_tax_ids(),
        webhooks_configured: payments.webhooks_enabled(),
    })
}

/// Every organisation class named by a permission anywhere in the app —
/// resources, agents, and the `org_class_editors` setting itself.
fn known_classes(app: &App) -> Vec<String> {
    let mut classes: BTreeSet<String> = BTreeSet::new();
    let mut note = |policy: &Policy| {
        if let Some(class) = &policy.org_class {
            classes.insert(class.clone());
        }
    };
    for resource in app.resources.values() {
        for policy in [
            &resource.permissions.list,
            &resource.permissions.read,
            &resource.permissions.create,
            &resource.permissions.update,
            &resource.permissions.delete,
        ] {
            note(policy);
        }
    }
    for agent in app.agents.values() {
        for policy in [
            &agent.permissions.chat,
            &agent.permissions.history,
            &agent.permissions.delete_history,
        ] {
            note(policy);
        }
    }
    note(&app.config.organization.org_class_policy());
    classes.into_iter().collect()
}

fn known_roles(app: &App, functions: &FunctionRegistry) -> Vec<String> {
    let mut roles: BTreeSet<String> = BTreeSet::new();
    // `member` is the role the built-in membership defaults describe, and every
    // app has one whether or not a permission names it.
    roles.insert("member".to_string());
    roles.insert("admin".to_string());

    for resource in app.resources.values() {
        for access in [
            &resource.permissions.list,
            &resource.permissions.read,
            &resource.permissions.create,
            &resource.permissions.update,
            &resource.permissions.delete,
        ] {
            if let Access::Role(role) = &access.level {
                roles.insert(role.clone());
            }
        }
        roles.extend(resource.admin.roles.iter().cloned());
    }
    for entry in functions.iter() {
        if let FunctionAccess::Role(role) = entry.manifest.access() {
            roles.insert(role);
        }
        roles.extend(parse_function_admin(&entry.manifest).roles);
    }
    for agent in app.agents.values() {
        for access in [
            &agent.permissions.chat,
            &agent.permissions.history,
            &agent.permissions.delete_history,
        ] {
            if let Access::Role(role) = &access.level {
                roles.insert(role.clone());
            }
        }
    }
    roles.into_iter().collect()
}

fn agent_manifest(agent: &Agent) -> AgentManifest {
    let org_scoped = agent.meta.scope == apiplant_core::Scope::Organization;
    AgentManifest {
        name: agent.meta.name.clone(),
        label: agent.label(),
        description: agent.meta.description.clone(),
        scope: if org_scoped { "organization" } else { "global" },
        storage: agent.meta.storage.enabled,
        thread_resource: agent
            .meta
            .storage
            .enabled
            .then(|| agent.thread_resource_name()),
        message_resource: agent
            .meta
            .storage
            .enabled
            .then(|| agent.message_resource_name()),
        chat: permission_manifest(&agent.permissions.chat, org_scoped),
        history: permission_manifest(&agent.permissions.history, org_scoped),
        delete_history: permission_manifest(&agent.permissions.delete_history, org_scoped),
    }
}

fn resource_manifest(
    resource: &Resource,
    password_field: &str,
    children: Vec<ChildManifest>,
) -> ResourceManifest {
    let fields = resource
        .fields
        .iter()
        .map(|(name, field)| field_manifest(name, field, resource))
        .collect::<Vec<_>>();
    let relations = resource
        .references()
        .into_iter()
        .filter(|reference| reference.field != "organization_id")
        .map(|reference| RelationManifest {
            label: titleize(&reference.relation),
            field: reference.field,
            relation: reference.relation,
            target: reference.target,
            required: reference.required,
        })
        .collect::<Vec<_>>();
    let org_scoped = resource.is_org_scoped();

    ResourceManifest {
        label: resource.admin_label(),
        plural: resource.admin_plural(),
        group: resource.admin.group.clone(),
        order: resource.admin.order,
        builtin: is_builtin_resource(&resource.meta.name),
        auth_resource: is_auth_resource(&resource.meta.name),
        visible: resource.admin.is_visible(&resource.meta.name),
        roles: resource.admin.roles.clone(),
        scope: if org_scoped { "organization" } else { "global" },
        owner_field: resource.meta.owner_field.clone(),
        display_field: resource.admin_display_field(),
        search_field: resource.admin_search_field(),
        search_fields: resource.admin_search_fields(),
        columns: resource
            .admin_columns()
            .into_iter()
            // A password column would never render usefully and, on a resource
            // that names one, is exactly the thing not to put in a table.
            .filter(|column| column != password_field || resource.meta.name != "user")
            .collect(),
        permissions: ActionPermissionsManifest {
            list: permission_manifest(&resource.permissions.list, org_scoped),
            read: permission_manifest(&resource.permissions.read, org_scoped),
            create: permission_manifest(&resource.permissions.create, org_scoped),
            update: permission_manifest(&resource.permissions.update, org_scoped),
            delete: permission_manifest(&resource.permissions.delete, org_scoped),
        },
        name: resource.meta.name.clone(),
        fields,
        relations,
        children,
    }
}

fn is_builtin_resource(name: &str) -> bool {
    is_auth_resource(name)
}

fn field_manifest(name: &str, field: &Field, resource: &Resource) -> FieldManifest {
    let references = field.references.clone();
    let relation = references.as_ref().map(|_| relation_name(name).to_string());
    // The framework stamps the owner and the tenant itself; offering either as
    // an input invites someone to fill in a value the server will overwrite.
    // …and `organization.org_class` is server-owned in the same way: only the
    // `org_class_editors` policy writes it, and the dashboard offers it on the
    // organisation screen, which knows whether this operator is named by it.
    // A generic record form here would be an input that silently does nothing.
    let stamped = name == resource.meta.owner_field
        || name == "organization_id"
        || (resource.meta.name == "organization" && name == ORG_CLASS_FIELD);

    FieldManifest {
        label: field
            .admin
            .label
            .clone()
            .unwrap_or_else(|| titleize(name))
            .to_string(),
        ty: field_type_name(field.ty),
        widget: resolve_widget(field),
        help: field.admin.help.clone(),
        placeholder: field.admin.placeholder.clone(),
        format: field.admin.format.as_str(),
        options: field
            .admin
            .options
            .iter()
            .map(|option| match option.split_once('|') {
                Some((value, label)) => FieldOption {
                    value: value.to_string(),
                    label: label.to_string(),
                },
                None => FieldOption {
                    value: option.clone(),
                    label: titleize(option),
                },
            })
            .collect(),
        required: field.required,
        unique: field.unique,
        hidden: field.hidden,
        admin_visible: field.admin.visible && !field.hidden,
        readonly: field.admin.readonly,
        max_length: field.max_length,
        references,
        relation,
        on_delete: field.on_delete.map(on_delete_name),
        default_value: field.default.clone(),
        writable: !field.hidden && !field.admin.readonly && !stamped,
        name: name.to_string(),
    }
}

/// Resolve `widget = "auto"` against the field's type, so the interface always
/// receives a concrete instruction and never has to duplicate this mapping.
fn resolve_widget(field: &Field) -> &'static str {
    if field.admin.widget != Widget::Auto {
        return field.admin.widget.as_str();
    }
    if !field.admin.options.is_empty() {
        return "select";
    }
    // Markup needs room and a preview beside it, whatever the column type.
    if field.admin.format != ContentFormat::Plain {
        return "textarea";
    }
    match field.ty {
        FieldType::File => "file",
        FieldType::Text => "textarea",
        FieldType::Boolean => "switch",
        FieldType::Json => "json",
        FieldType::Timestamp => "date_time",
        FieldType::Reference => "reference",
        FieldType::Integer | FieldType::BigInt | FieldType::Float => "number",
        FieldType::Uuid => "text",
        FieldType::String => "text",
    }
}

fn permission_manifest(policy: &Policy, org_scoped: bool) -> ActionPermissionManifest {
    let access = &policy.level;
    ActionPermissionManifest {
        value: policy.as_string(),
        role: match access {
            Access::Role(role) => Some(role.clone()),
            _ => None,
        },
        org_class: policy.org_class.clone(),
        note: access_note(policy, org_scoped),
        // A class qualifier is answered from the active organisation, so it
        // needs one selected even where the level alone would not.
        requires_org: org_scoped
            || policy.org_class.is_some()
            || matches!(access, Access::Role(_) | Access::Member),
    }
}

fn parse_function_admin(manifest: &apiplant_abi::FunctionManifest) -> FunctionAdmin {
    if manifest.admin.is_empty() {
        return FunctionAdmin::default();
    }
    serde_json::from_str(manifest.admin.as_str()).unwrap_or_default()
}

fn function_manifest(manifest: &apiplant_abi::FunctionManifest) -> FunctionManifest {
    let access = manifest.access();
    let admin = parse_function_admin(manifest);
    let name = manifest.name.to_string();
    let label = admin.label.unwrap_or_else(|| titleize(&name));

    FunctionManifest {
        label: label.clone(),
        description: admin
            .description
            .unwrap_or_else(|| manifest.description.to_string()),
        group: admin.group,
        order: admin.order.unwrap_or(0),
        method: method_name(manifest.method),
        permission: access.as_string(),
        role: match &access {
            FunctionAccess::Role(role) => Some(role.clone()),
            _ => None,
        },
        permission_note: function_access_note(&access),
        requires_org: matches!(access, FunctionAccess::Role(_) | FunctionAccess::Member),
        visible: admin.visible.unwrap_or(true),
        roles: admin.roles,
        confirm: admin.confirm,
        run_label: admin.run_label.unwrap_or(label),
        input_schema: parse_schema(manifest.input_schema.as_str()),
        output_schema: parse_schema(manifest.output_schema.as_str()),
        name,
    }
}

/// A manifest's schemas are optional and may be malformed (they come from
/// another language's library). An unreadable one simply means "no form", not a
/// failed build.
fn parse_schema(raw: &str) -> Option<Value> {
    if raw.trim().is_empty() {
        return None;
    }
    serde_json::from_str(raw).ok()
}

fn access_note(policy: &Policy, org_scoped: bool) -> String {
    let note = access_level_note(&policy.level, org_scoped);
    match &policy.org_class {
        Some(class) => format!("{note} Only in {class} organizations."),
        None => note,
    }
}

fn access_level_note(access: &Access, org_scoped: bool) -> String {
    if org_scoped {
        return match access {
            Access::Private => "Not available.".to_string(),
            Access::Owner => "Limited to records you created.".to_string(),
            Access::Role(role) => format!("Needs the {role} role."),
            _ => "Available to everyone in this organization.".to_string(),
        };
    }

    match access {
        Access::Public => "Available to anyone.".to_string(),
        Access::Authenticated | Access::Member => "Available once you sign in.".to_string(),
        Access::Owner => "Limited to records you created.".to_string(),
        Access::Role(role) => format!("Needs the {role} role."),
        Access::Private => "Not available.".to_string(),
    }
}

fn function_access_note(access: &FunctionAccess) -> String {
    match access {
        FunctionAccess::Public => "Anyone can run this.".to_string(),
        FunctionAccess::Authenticated => "Available once you sign in.".to_string(),
        FunctionAccess::Member => "Available to everyone in this organization.".to_string(),
        FunctionAccess::Role(role) => format!("Needs the {role} role."),
        FunctionAccess::Private => "Not available.".to_string(),
    }
}

fn field_type_name(ty: FieldType) -> &'static str {
    match ty {
        FieldType::String => "string",
        FieldType::Text => "text",
        FieldType::Integer => "integer",
        FieldType::BigInt => "big_int",
        FieldType::Float => "float",
        FieldType::Boolean => "boolean",
        FieldType::Uuid => "uuid",
        FieldType::Timestamp => "timestamp",
        FieldType::Json => "json",
        FieldType::File => "file",
        FieldType::Reference => "reference",
    }
}

fn on_delete_name(on_delete: OnDelete) -> &'static str {
    match on_delete {
        OnDelete::Restrict => "restrict",
        OnDelete::SetNull => "set_null",
        OnDelete::Cascade => "cascade",
        OnDelete::NoAction => "no_action",
    }
}

fn method_name(method: HttpMethod) -> &'static str {
    match method {
        HttpMethod::Get => "GET",
        HttpMethod::Post => "POST",
        HttpMethod::Put => "PUT",
        HttpMethod::Delete => "DELETE",
    }
}

fn normalize_api_base(raw: &str, base_path: &str, prefer_https: bool) -> Result<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        bail!("--api requires a domain or full API URL");
    }

    let mut url = if trimmed.contains("://") {
        trimmed.to_string()
    } else {
        format!(
            "{}://{}",
            if prefer_https { "https" } else { "http" },
            trimmed
        )
    };

    if !url.starts_with("http://") && !url.starts_with("https://") {
        bail!("--api must resolve to an http:// or https:// URL");
    }

    let scheme_end = url
        .find("://")
        .map(|index| index + 3)
        .ok_or_else(|| anyhow!("invalid API URL"))?;

    match url[scheme_end..].find('/') {
        None => {
            if !base_path.is_empty() {
                url.push_str(base_path);
            }
        }
        Some(relative_start) => {
            let path_start = scheme_end + relative_start;
            let path = &url[path_start..];
            if path == "/" {
                url.truncate(path_start);
                if !base_path.is_empty() {
                    url.push_str(base_path);
                }
            } else {
                while url.ends_with('/') {
                    url.pop();
                }
            }
        }
    }

    while url.ends_with('/') {
        url.pop();
    }

    Ok(url)
}

fn write_bytes(path: PathBuf, bytes: &[u8]) -> Result<()> {
    fs::write(&path, bytes).with_context(|| format!("failed to write {}", path.display()))
}

fn write_json(path: PathBuf, manifest: &AdminManifest) -> Result<()> {
    let bytes = serde_json::to_vec_pretty(manifest)?;
    fs::write(&path, bytes).with_context(|| format!("failed to write {}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_dir(label: &str) -> PathBuf {
        let mut dir = std::env::temp_dir();
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        dir.push(format!(
            "apiplant-admin-{label}-{}-{stamp}",
            std::process::id()
        ));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn build_manifest_for(resources: &[(&str, &str)]) -> Value {
        build_manifest_with_config(
            "[server]\nbase_path = \"/api\"\n\n[auth]\nallow_registration = true\n",
            resources,
        )
    }

    fn build_manifest_with_config(main_toml: &str, resources: &[(&str, &str)]) -> Value {
        let app_dir = temp_dir("app");
        let out_dir = temp_dir("out");
        fs::create_dir_all(app_dir.join("resources")).unwrap();
        fs::write(app_dir.join("main.toml"), main_toml).unwrap();
        for (name, src) in resources {
            fs::write(app_dir.join(format!("resources/{name}.toml")), src).unwrap();
        }

        build(
            &app_dir,
            Options {
                api: "https://example.com".to_string(),
                out: Some(out_dir.clone()),
            },
        )
        .unwrap();

        let manifest: Value =
            serde_json::from_slice(&fs::read(out_dir.join("apiplant-admin.json")).unwrap())
                .unwrap();
        fs::remove_dir_all(app_dir).unwrap();
        fs::remove_dir_all(out_dir).unwrap();
        manifest
    }

    fn resource<'a>(manifest: &'a Value, name: &str) -> &'a Value {
        manifest["resources"]
            .as_array()
            .unwrap()
            .iter()
            .find(|resource| resource["name"] == name)
            .unwrap_or_else(|| panic!("no `{name}` in manifest"))
    }

    /// The header an operator reads is the app's to choose; the directory it
    /// happens to live in is only the fallback.
    #[test]
    fn app_name_comes_from_config_and_falls_back_to_the_directory() {
        let named = build_manifest_with_config(
            "[app]\nname = \"Acme Logistics\"\n\n[server]\nbase_path = \"/api\"\n",
            &[],
        );
        assert_eq!(named["app_name"], "Acme Logistics");
        assert_eq!(named["title"], "Acme Logistics admin");

        // A blank name is not a name: it would render as a header with nothing
        // in it, so it falls back like an absent one.
        let blank = build_manifest_with_config(
            "[app]\nname = \"   \"\n\n[server]\nbase_path = \"/api\"\n",
            &[],
        );
        assert!(blank["app_name"]
            .as_str()
            .unwrap()
            .starts_with("apiplant-admin-app-"));

        let unnamed = build_manifest_for(&[]);
        assert!(unnamed["app_name"]
            .as_str()
            .unwrap()
            .starts_with("apiplant-admin-app-"));
    }

    #[test]
    fn api_base_uses_app_base_path_when_only_a_domain_is_given() {
        assert_eq!(
            normalize_api_base("admin.example.com", "/api", true).unwrap(),
            "https://admin.example.com/api"
        );
        assert_eq!(
            normalize_api_base("127.0.0.1:8099", "", false).unwrap(),
            "http://127.0.0.1:8099"
        );
    }

    #[test]
    fn explicit_api_paths_are_preserved() {
        assert_eq!(
            normalize_api_base("https://example.com/custom/", "/api", true).unwrap(),
            "https://example.com/custom"
        );
        assert_eq!(
            normalize_api_base("https://example.com/", "/api", true).unwrap(),
            "https://example.com/api"
        );
    }

    #[test]
    fn build_writes_static_admin_files_and_manifest() {
        let app_dir = temp_dir("files");
        let out_dir = temp_dir("files-out");
        fs::create_dir_all(app_dir.join("resources")).unwrap();
        fs::write(
            app_dir.join("main.toml"),
            "[server]\nbase_path = \"/api\"\n",
        )
        .unwrap();

        let written = build(
            &app_dir,
            Options {
                api: "https://example.com".to_string(),
                out: Some(out_dir.clone()),
            },
        )
        .unwrap();

        assert_eq!(written, out_dir);
        for file in [
            "index.html",
            "app.js",
            "app.css",
            "head.png",
            "head-inverted.png",
            "apiplant-admin.json",
        ] {
            assert!(out_dir.join(file).exists(), "{file} was not written");
        }

        fs::remove_dir_all(app_dir).unwrap();
        fs::remove_dir_all(out_dir).unwrap();
    }

    #[test]
    fn auth_resources_are_hidden_from_the_resource_navigation_by_default() {
        let manifest = build_manifest_for(&[(
            "post",
            "[resource]\nname = \"post\"\n\n[fields.title]\ntype = \"string\"\n",
        )]);

        for name in ["user", "organization", "membership", "api_key"] {
            let auth = resource(&manifest, name);
            assert_eq!(auth["visible"], false, "{name} should be hidden");
            assert_eq!(auth["auth_resource"], true);
        }
        assert_eq!(resource(&manifest, "post")["visible"], true);
        assert_eq!(resource(&manifest, "post")["auth_resource"], false);
    }

    #[test]
    fn admin_section_overrides_labels_columns_and_role_visibility() {
        let manifest = build_manifest_for(&[(
            "product",
            r#"
[resource]
name = "product"

[admin]
visible = true
roles = ["manager"]
label = "Item"
plural = "Catalogue items"
group = "Catalogue"
order = 3
display_field = "title"
columns = ["title", "status"]

[fields.title]
type = "string"
required = true

[fields.status]
type = "string"
default = "draft"

[fields.status.admin]
label = "Lifecycle"
widget = "select"
options = ["draft", "active|Live"]
help = "Only live items are sold."

[fields.internal_note]
type = "text"

[fields.internal_note.admin]
visible = false
"#,
        )]);

        let product = resource(&manifest, "product");
        assert_eq!(product["label"], "Item");
        assert_eq!(product["plural"], "Catalogue items");
        assert_eq!(product["group"], "Catalogue");
        assert_eq!(product["order"], 3);
        assert_eq!(product["roles"][0], "manager");
        assert_eq!(product["display_field"], "title");
        assert_eq!(product["columns"][0], "title");
        assert_eq!(product["columns"][1], "status");

        let field = |name: &str| {
            product["fields"]
                .as_array()
                .unwrap()
                .iter()
                .find(|field| field["name"] == name)
                .unwrap()
        };
        let status = field("status");
        assert_eq!(status["label"], "Lifecycle");
        assert_eq!(status["widget"], "select");
        assert_eq!(status["help"], "Only live items are sold.");
        assert_eq!(status["options"][0]["value"], "draft");
        assert_eq!(status["options"][0]["label"], "Draft");
        // `value|Label` splits into an explicit caption.
        assert_eq!(status["options"][1]["value"], "active");
        assert_eq!(status["options"][1]["label"], "Live");

        // Hidden in the dashboard, still part of the API.
        assert_eq!(field("internal_note")["admin_visible"], false);
        assert_eq!(field("internal_note")["hidden"], false);

        // The injected tenancy column is never an input.
        assert_eq!(field("organization_id")["writable"], false);
        assert_eq!(field("organization_id")["admin_visible"], false);
    }

    #[test]
    fn content_format_reaches_the_manifest_and_forces_a_textarea() {
        let manifest = build_manifest_for(&[(
            "article",
            r#"
[resource]
name = "article"

[fields.body]
type = "text"

[fields.body.admin]
format = "markdown"

[fields.summary]
type = "string"

[fields.summary.admin]
format = "html"

[fields.slug]
type = "string"
"#,
        )]);

        let article = resource(&manifest, "article");
        let field = |name: &str| {
            article["fields"]
                .as_array()
                .unwrap()
                .iter()
                .find(|field| field["name"] == name)
                .unwrap()
                .clone()
        };

        assert_eq!(field("body")["format"], "markdown");
        assert_eq!(field("body")["widget"], "textarea");
        // Markup needs the room even when the column is a plain string.
        assert_eq!(field("summary")["format"], "html");
        assert_eq!(field("summary")["widget"], "textarea");
        assert_eq!(field("slug")["format"], "plain");
        assert_eq!(field("slug")["widget"], "text");
    }

    #[test]
    fn admin_ai_assistance_appears_only_when_both_admin_and_ai_are_configured() {
        let enabled = build_manifest_with_config(
            r#"
[server]
base_path = "/api"

[ai]
provider = "openai"
api_key = "test"

[admin.ai_assistance]
enabled = true
system = "Return only the field value."
prompt_placeholder = "Prompt AI to fill this field"
"#,
            &[],
        );
        assert_eq!(
            enabled["ai_assistance"]["prompt_placeholder"],
            "Prompt AI to fill this field"
        );
        assert_eq!(
            enabled["ai_assistance"]["system"],
            "Return only the field value."
        );

        let no_ai = build_manifest_with_config(
            r#"
[server]
base_path = "/api"

[admin.ai_assistance]
enabled = true
"#,
            &[],
        );
        assert!(no_ai["ai_assistance"].is_null());

        let no_admin = build_manifest_with_config(
            r#"
[server]
base_path = "/api"

[ai]
provider = "openai"
api_key = "test"
"#,
            &[],
        );
        assert!(no_admin["ai_assistance"].is_null());
    }

    #[test]
    fn labels_and_columns_are_inferred_when_admin_says_nothing() {
        let manifest = build_manifest_for(&[(
            "purchase_order",
            r#"
[resource]
name = "purchase_order"

[fields.name]
type = "string"

[fields.notes]
type = "text"

[fields.settings]
type = "json"
"#,
        )]);

        let purchase_order = resource(&manifest, "purchase_order");
        assert_eq!(purchase_order["label"], "Purchase order");
        assert_eq!(purchase_order["plural"], "Purchase orders");
        assert_eq!(purchase_order["display_field"], "name");
        assert_eq!(purchase_order["search_field"], "name");

        // `text` and `json` never read well in a table cell, so they are left
        // out of the inferred column set.
        let columns: Vec<&str> = purchase_order["columns"]
            .as_array()
            .unwrap()
            .iter()
            .map(|column| column.as_str().unwrap())
            .collect();
        assert_eq!(columns, vec!["name"]);
    }

    #[test]
    fn related_lists_are_derived_from_incoming_references() {
        let manifest = build_manifest_for(&[
            (
                "order",
                "[resource]\nname = \"order\"\n\n[fields.number]\ntype = \"string\"\n",
            ),
            (
                "order_line",
                r#"
[resource]
name = "order_line"

[fields.order_id]
type = "reference"
references = "order"
required = true

[fields.quantity]
type = "integer"
"#,
            ),
        ]);

        let order = resource(&manifest, "order");
        let children = order["children"].as_array().unwrap();
        assert_eq!(children.len(), 1);
        assert_eq!(children[0]["resource"], "order_line");
        assert_eq!(children[0]["field"], "order_id");
        assert_eq!(children[0]["label"], "Order lines");

        // …and the child knows which way its own reference points.
        let line = resource(&manifest, "order_line");
        let relation = line["relations"]
            .as_array()
            .unwrap()
            .iter()
            .find(|relation| relation["field"] == "order_id")
            .unwrap();
        assert_eq!(relation["target"], "order");
        assert_eq!(relation["label"], "Order");
        assert_eq!(relation["required"], true);
    }

    #[test]
    fn known_roles_collect_every_role_the_app_names() {
        let manifest = build_manifest_for(&[(
            "product",
            r#"
[resource]
name = "product"

[permissions]
create = "role:buyer"
delete = "role:auditor"

[fields.name]
type = "string"
"#,
        )]);

        let roles: Vec<&str> = manifest["auth"]["known_roles"]
            .as_array()
            .unwrap()
            .iter()
            .map(|role| role.as_str().unwrap())
            .collect();
        assert!(roles.contains(&"buyer"));
        assert!(roles.contains(&"auditor"));
        // The two roles every app has, whether or not a permission names them.
        assert!(roles.contains(&"admin"));
        assert!(roles.contains(&"member"));
    }

    #[test]
    fn signup_collects_required_profile_fields_so_nobody_types_json() {
        let manifest = build_manifest_for(&[(
            "user",
            r#"
[resource]
name = "user"
scope = "global"

[auth]
identity_field = "email"
password_field = "password_hash"

[fields.email]
type = "string"
required = true
unique = true

[fields.password_hash]
type = "string"
hidden = true

[fields.full_name]
type = "string"
required = true

[fields.nickname]
type = "string"
"#,
        )]);

        let signup: Vec<&str> = manifest["auth"]["signup_fields"]
            .as_array()
            .unwrap()
            .iter()
            .map(|field| field["name"].as_str().unwrap())
            .collect();
        // Required extras only: the identity and password have their own inputs,
        // and an optional field would just be noise on a sign-up form.
        assert_eq!(signup, vec!["full_name"]);
        assert_eq!(manifest["auth"]["identity_label"], "Email");

        // The account screen offers everything editable, including optional
        // fields — but never the password hash.
        let profile: Vec<&str> = manifest["auth"]["profile_fields"]
            .as_array()
            .unwrap()
            .iter()
            .map(|field| field["name"].as_str().unwrap())
            .collect();
        assert!(profile.contains(&"nickname"));
        assert!(!profile.contains(&"password_hash"));
    }
}
