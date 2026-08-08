//! Auto-join: put a new account into the organisation that owns its email
//! domain.
//!
//! `resources/organization.toml` adds a `domain` column, and `resources/users.toml`
//! points the `user` resource's `after_create` event at the function below:
//!
//! ```toml
//! [hooks]
//! after_create = "user_after_create"
//! ```
//!
//! `POST /api/auth/register` is a create on `user`, so registering is what
//! normally fires this — but the same hook covers `POST /api/user`, because
//! both write the same row.

use apiplant_function::prelude::*;
use serde::Deserialize;
use serde_json::{json, Value};

/// Config from `functions/user_after_create.toml`.
#[derive(Deserialize)]
#[serde(default)]
struct Settings {
    /// The membership role granted to someone who joins this way. Deliberately
    /// not `admin`: matching a domain proves an address, not authority.
    role: String,
    /// Domains that must never be claimed by an organisation, however tempting.
    /// Without this, one `gmail.com` organisation would swallow the internet.
    public_domains: Vec<String>,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            role: "member".to_string(),
            public_domains: Vec::new(),
        }
    }
}

/// `after_create` on `user` — the stored row arrives as `input`, so the address
/// and the new id are both in hand. `proceed()` leaves the response untouched:
/// the caller still gets their token and their user, they simply arrive already
/// belonging somewhere.
fn user_after_create(ctx: &Context<Settings>, input: Value) -> Result<Value, String> {
    let hook = ctx.hook().ok_or("user_after_create is a lifecycle hook")?;
    let settings = ctx.config();

    let email = input["email"].as_str().unwrap_or_default();
    let user_id = input["id"].as_str().unwrap_or_default();
    // `record_id` is set on registration (the created account) and empty on a
    // plain `POST /api/user`, so the row is the reliable source for both.
    if user_id.is_empty() {
        ctx.warn(&format!(
            "{} created a user with no id; not auto-joining",
            hook.event
        ));
        return Ok(reply::proceed());
    }

    let Some(domain) = domain_of(email) else {
        return Ok(reply::proceed());
    };
    if settings
        .public_domains
        .iter()
        .any(|listed| listed.eq_ignore_ascii_case(&domain))
    {
        ctx.info(&format!("{domain} is a public mail domain; not auto-joining"));
        return Ok(reply::proceed());
    }

    // `domain` is unique on `organization`, so this matches at most one row.
    let org = ctx.query_one(
        "SELECT id::text AS id, name FROM apiplant_organization WHERE lower(domain) = $1",
        &[json!(domain)],
    )?;
    let Some(org) = org else {
        ctx.info(&format!("no organisation owns {domain}"));
        return Ok(reply::proceed());
    };
    let org_id = org["id"].as_str().unwrap_or_default();
    let org_name = org["name"].as_str().unwrap_or_default();

    // A membership is an ordinary row in an ordinary table — the hook writes it
    // directly, the way an admin's `POST /api/membership` would.
    ctx.execute(
        "INSERT INTO apiplant_membership (user_id, organization_id, role) \
         VALUES ($1::uuid, $2::uuid, $3)",
        &[json!(user_id), json!(org_id), json!(settings.role)],
    )?;
    ctx.info(&format!(
        "{email} joined {org_name} as {} via its {domain} domain",
        settings.role
    ));

    Ok(reply::proceed())
}

/// The lowercased domain of an email address, or `None` if it doesn't look like
/// one. `rsplit_once` takes the *last* `@`, which is the delimiter a local part
/// containing one can't fake.
fn domain_of(email: &str) -> Option<String> {
    let (local, domain) = email.trim().rsplit_once('@')?;
    let domain = domain.trim().to_lowercase();
    if local.is_empty() || domain.is_empty() || !domain.contains('.') {
        return None;
    }
    Some(domain)
}

apiplant_function::function! {
    name: "user_after_create",
    description: "Adds a new account to the organisation that owns its email domain.",
    method: Post,
    visibility: Private,   // a hook needs no endpoint of its own
    handler: user_after_create,
}
