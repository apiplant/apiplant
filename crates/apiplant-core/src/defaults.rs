//! Built-in resources.
//!
//! `organization`, `membership`, `user`, `api_key` and `oauth_connection` exist
//! in every app. Together they make apps multitenant out of the box: users join
//! organisations through memberships (which also carry their **role within that
//! organisation**), and every other resource is isolated per organisation by
//! default.
//!
//! Drop a `models/<name>.toml` with the same `name` to replace a built-in and
//! add fields or tweak permissions while keeping the machinery working.

use crate::schema::Resource;

/// The `organization` — the tenant. `global` because its rows *are* the
/// organisations; membership (not an `organization_id`) decides who sees them.
pub const ORGANIZATION_TOML: &str = r#"
[resource]
name = "organization"
scope = "global"
timestamps = true

[permissions]
list   = "member"          # organisations you belong to
read   = "member"
create = "authenticated"   # anyone may start one (and becomes its admin)
update = "role:admin"      # an admin *of that organisation*
delete = "role:admin"

[fields.name]
type = "string"
required = true

[fields.slug]
type = "string"
unique = true
"#;

/// A user's membership in an organisation, carrying their role there. The
/// join table behind the N:N between `user` and `organization`.
pub const MEMBERSHIP_TOML: &str = r#"
[resource]
name = "membership"
scope = "organization"
timestamps = true

[permissions]
list   = "member"          # members can see who else is in the org
read   = "member"
create = "role:admin"      # admins add members
update = "role:admin"
delete = "role:admin"

# Built-in function: lets `create` name the person by `email` instead of by
# `user_id`, and refuses a duplicate membership. The lookup has to happen here
# because `user` is only readable by people you already share an org with.
[hooks]
before_create = "apiplant_organization_join"

[fields.user_id]
type = "reference"
references = "user"
required = true

[fields.organization_id]
type = "reference"
references = "organization"
required = true

[fields.role]
type = "string"            # the member's *primary* role here, e.g. "admin".
                           # Further roles are `membership_role` rows; a
                           # `role:` permission is checked against all of them.
"#;

/// A single role held by a membership.
///
/// Roles are a set, not a slot: someone can be a `billing` *and* a `support`
/// person without either displacing the other, which one column cannot express.
/// [`MEMBERSHIP_TOML`]'s `role` stays as the member's **primary** role — it is
/// what existing apps and hook contexts read — and these rows are the rest.
/// Together they are the roles a `role:` permission is checked against.
///
/// `admin` is special: it satisfies every role check in its organisation
/// without needing a row per role, so granting someone `admin` grants them
/// everything the app defines.
pub const MEMBERSHIP_ROLE_TOML: &str = r#"
[resource]
name = "membership_role"
scope = "organization"
timestamps = true

[permissions]
list   = "member"          # members can see who holds what
read   = "member"
create = "role:admin"      # admins grant roles
update = "role:admin"
delete = "role:admin"

[fields.membership_id]
type = "reference"
references = "membership"
required = true
on_delete = "cascade"      # a removed member takes their roles with them

[fields.organization_id]
type = "reference"
references = "organization"
required = true

[fields.role]
type = "string"
required = true
"#;

/// The default `user`: global (users are shared across organisations) with
/// email + password auth. Extend via `models/users.toml`.
pub const USER_TOML: &str = r#"
[resource]
name = "user"
scope = "global"
timestamps = true

[permissions]
list   = "member"          # people you share an organisation with
read   = "member"
create = "public"          # registration
update = "owner"
delete = "private"

[auth]
identity_field = "email"
password_field = "password_hash"
oauth_providers = []

[fields.email]
type = "string"
required = true
unique = true
max_length = 320

[fields.password_hash]
type = "string"
hidden = true

[fields.display_name]
type = "string"
"#;

/// Default `api_key` resource (global). A valid key authenticates as its owner.
pub const API_KEY_TOML: &str = r#"
[resource]
name = "api_key"
scope = "global"
timestamps = true

# "Api key" is what titleising the name produces, and it is not how anybody
# writes it.
[admin]
label = "API key"
plural = "API keys"

[permissions]
list   = "owner"
read   = "owner"
create = "authenticated"
update = "private"
delete = "owner"

[fields.name]
type = "string"

[fields.token_hash]
type = "string"
required = true
unique = true
hidden = true

[fields.owner_id]
type = "reference"
references = "user"
required = true
"#;

/// Default `oauth_connection` resource (global) linking a user to a provider.
pub const OAUTH_TOML: &str = r#"
[resource]
name = "oauth_connection"
scope = "global"
timestamps = true

[permissions]
list   = "owner"
read   = "owner"
create = "private"
update = "private"
delete = "owner"

[fields.provider]
type = "string"
required = true

[fields.provider_user_id]
type = "string"
required = true

[fields.owner_id]
type = "reference"
references = "user"
required = true
"#;

/// The name → embedded-TOML table of built-ins, in dependency order so foreign
/// keys resolve (organization and user before membership/api_key/oauth).
pub fn builtins() -> Vec<(&'static str, &'static str)> {
    vec![
        ("organization", ORGANIZATION_TOML),
        ("user", USER_TOML),
        ("membership", MEMBERSHIP_TOML),
        ("membership_role", MEMBERSHIP_ROLE_TOML),
        ("api_key", API_KEY_TOML),
        ("oauth_connection", OAUTH_TOML),
    ]
}

/// Parse one built-in by its embedded TOML. Panics on a malformed built-in —
/// that would be a bug in this crate, caught by the test below.
pub fn parse_builtin(toml_src: &str) -> Resource {
    let r: Resource = toml::from_str(toml_src).expect("built-in resource TOML is valid");
    r.validate().expect("built-in resource is valid");
    r
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_builtins_parse() {
        for (name, src) in builtins() {
            let r = parse_builtin(src);
            assert_eq!(r.meta.name, name);
        }
    }
}
