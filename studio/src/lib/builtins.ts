/**
 * The five resources every apiplant app has whether or not a file describes
 * them, transcribed from `crates/apiplant-core/src/defaults.rs`. The studio
 * shows them alongside custom resources; editing one writes a `models/*.toml`
 * that replaces the default, exactly as the framework intends.
 */

import { parseResource } from "./toml";
import type { Resource } from "./types";

export const BUILTIN_NAMES = ["organization", "user", "membership", "api_key", "oauth_connection"] as const;
export type BuiltinName = (typeof BUILTIN_NAMES)[number];

/** Conventional file name for a built-in, matching the docs (`user` → users.toml). */
export const BUILTIN_FILENAME: Record<BuiltinName, string> = {
  organization: "organizations.toml",
  user: "users.toml",
  membership: "memberships.toml",
  api_key: "api_keys.toml",
  oauth_connection: "oauth_connections.toml",
};

export const BUILTIN_SUMMARY: Record<BuiltinName, string> = {
  organization: "The tenant. Membership decides who sees it, so it is global.",
  user: "Login identity. Carries the [auth] section the framework authenticates against.",
  membership: "Joins a user to an organisation and carries their role there.",
  api_key: "A hashed key that authenticates as its owning user.",
  oauth_connection: "Links a user to an external identity provider.",
};

const ORGANIZATION_TOML = `
[resource]
name = "organization"
scope = "global"
timestamps = true

[permissions]
list   = "member"
read   = "member"
create = "authenticated"
update = "role:admin"
delete = "role:admin"

[fields.name]
type = "string"
required = true

[fields.slug]
type = "string"
unique = true
`;

const USER_TOML = `
[resource]
name = "user"
scope = "global"
timestamps = true

[permissions]
list   = "authenticated"
read   = "owner"
create = "public"
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
`;

const MEMBERSHIP_TOML = `
[resource]
name = "membership"
scope = "organization"
timestamps = true

[permissions]
list   = "member"
read   = "member"
create = "role:admin"
update = "role:admin"
delete = "role:admin"

[fields.user_id]
type = "reference"
references = "user"
required = true

[fields.organization_id]
type = "reference"
references = "organization"
required = true

[fields.role]
type = "string"
`;

const API_KEY_TOML = `
[resource]
name = "api_key"
scope = "global"
timestamps = true

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
`;

const OAUTH_TOML = `
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
`;

const SOURCES: Record<BuiltinName, string> = {
  organization: ORGANIZATION_TOML,
  user: USER_TOML,
  membership: MEMBERSHIP_TOML,
  api_key: API_KEY_TOML,
  oauth_connection: OAUTH_TOML,
};

/** A fresh copy of a built-in's default definition. */
export function builtinResource(name: BuiltinName): Resource {
  return parseResource(SOURCES[name]);
}
