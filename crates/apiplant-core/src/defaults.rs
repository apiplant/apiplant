//! Built-in resources.
//!
//! `user`, `role` and `api_key` exist in every app. If the developer drops a
//! `models/users.toml` (etc.) into the app, that file *replaces* the default,
//! letting them add fields or tweak permissions while keeping the machinery
//! (auth, ownership, api-key lookup) working. When absent, these embedded
//! definitions are used verbatim.

use crate::schema::Resource;

/// Default `user` resource: email + password auth, extendable via `models/users.toml`.
pub const USER_TOML: &str = r#"
[resource]
name = "user"
timestamps = true

[permissions]
list = "role:admin"
read = "owner"
create = "public"      # registration
update = "owner"
delete = "role:admin"

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

[fields.role_id]
type = "reference"
references = "role"
"#;

/// Default `role` resource used for permission checks.
pub const ROLE_TOML: &str = r#"
[resource]
name = "role"
timestamps = true

[permissions]
list = "authenticated"
read = "authenticated"
create = "role:admin"
update = "role:admin"
delete = "role:admin"

[fields.name]
type = "string"
required = true
unique = true

[fields.description]
type = "text"
"#;

/// Default `api_key` resource. A valid key authenticates as its owning user.
pub const API_KEY_TOML: &str = r#"
[resource]
name = "api_key"
timestamps = true

[permissions]
list = "owner"
read = "owner"
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

/// Default `oauth_connection` resource linking a user to a third-party identity.
pub const OAUTH_TOML: &str = r#"
[resource]
name = "oauth_connection"
timestamps = true

[permissions]
list = "owner"
read = "owner"
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

/// The name → embedded-TOML table of built-ins, in dependency order (role
/// before user before api_key/oauth, so foreign keys resolve).
pub fn builtins() -> Vec<(&'static str, &'static str)> {
    vec![
        ("role", ROLE_TOML),
        ("user", USER_TOML),
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
