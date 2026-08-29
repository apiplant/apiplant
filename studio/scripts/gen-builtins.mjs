/**
 * Generate `src/lib/builtins.ts` from `crates/apiplant-core/src/defaults.rs`.
 *
 * The studio runs entirely in the browser against a directory handle — there is
 * no server to ask what the framework's built-in resources are, so it carries a
 * copy. Transcribing that copy by hand is how it came to show five of fourteen;
 * extracting it from the Rust source is how it stays honest.
 *
 * Run `npm run gen:builtins` after touching the built-ins in defaults.rs.
 */

import { readFileSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const DEFAULTS = join(here, "../../crates/apiplant-core/src/defaults.rs");
const OUTPUT = join(here, "../src/lib/builtins.ts");

/** Always present, in the dependency order `builtins()` uses. */
const ALWAYS = [
  ["organization", "ORGANIZATION_TOML", "organizations.toml", "The tenant. Membership decides who sees it, so it is global."],
];

/** Added only when [auth] is enabled, in `auth_builtins()` order. */
const AUTH = [
  ["user", "USER_TOML", "users.toml", "Login identity. Carries the [auth] section the framework authenticates against."],
  ["membership", "MEMBERSHIP_TOML", "memberships.toml", "Joins a user to an organisation and carries their role there."],
  ["membership_role", "MEMBERSHIP_ROLE_TOML", "membership_roles.toml", "Extra roles a membership holds, beyond the one on the membership itself."],
  ["api_key", "API_KEY_TOML", "api_keys.toml", "A hashed key that authenticates as its owning user."],
  ["oauth_connection", "OAUTH_TOML", "oauth_connections.toml", "Links a user to an external identity provider."],
  ["invitation", "INVITATION_TOML", "invitations.toml", "A pending invite to an organisation, issued by POST /auth/invitations."],
  ["auth_token", "AUTH_TOKEN_TOML", "auth_tokens.toml", "Single-use email tokens for address verification and password reset. Private throughout."],
];

/** Added only when [payments] names a provider, in `billing_builtins()` order. */
const BILLING = [
  ["billing_product", "BILLING_PRODUCT_TOML", "billing_products.toml", "Something you sell, mirrored from the payment provider's catalogue."],
  ["billing_price", "BILLING_PRICE_TOML", "billing_prices.toml", "What a product costs — one-off or recurring, per currency."],
  ["billing_customer", "BILLING_CUSTOMER_TOML", "billing_customers.toml", "Ties a user or organisation to the provider's customer record."],
  ["billing_subscription", "BILLING_SUBSCRIPTION_TOML", "billing_subscriptions.toml", "An active recurring plan and where it is in its cycle."],
  ["billing_payment", "BILLING_PAYMENT_TOML", "billing_payments.toml", "One payment, recorded when the provider says it settled."],
  ["billing_event", "BILLING_EVENT_TOML", "billing_events.toml", "The raw webhook log — what the provider said and whether it was handled."],
];

const source = readFileSync(DEFAULTS, "utf8");

function tomlFor(constant) {
  const start = source.indexOf(`pub const ${constant}: &str = r#"`);
  if (start < 0) throw new Error(`${constant} not found in defaults.rs`);
  const from = source.indexOf('r#"', start) + 3;
  const to = source.indexOf('"#;', from);
  if (to < 0) throw new Error(`${constant} is not terminated`);
  return source.slice(from, to);
}

const all = [...ALWAYS, ...AUTH, ...BILLING];

const record = (entries, value) =>
  entries.map(([name, , file, summary]) => `  ${name}: ${JSON.stringify(value(file, summary))},`).join("\n");

const sources = all
  .map(([name, constant]) => `  ${name}: \`\n${tomlFor(constant).replace(/[\\`$]/g, (c) => `\\${c}`).trim()}\n\`,`)
  .join("\n\n");

const out = `/**
 * The resources every apiplant app has whether or not a file describes them,
 * generated from \`crates/apiplant-core/src/defaults.rs\` by
 * \`scripts/gen-builtins.mjs\` — do not edit by hand.
 *
 * The studio shows them alongside custom resources; editing one writes a
 * \`resources/*.toml\` that replaces the default, exactly as the framework intends.
 * Two sets are conditional, and the studio lists them on exactly the conditions
 * the framework adds them under: the account tables when \`[auth]\` is enabled,
 * and the billing tables when \`[payments]\` is on and names a provider.
 */

import { parseResource } from "./toml";
import type { Resource } from "./types";

export const ALWAYS_BUILTIN_NAMES = [
${ALWAYS.map(([name]) => `  "${name}",`).join("\n")}
] as const;

/** Present only when \`[auth].enabled\` is not \`false\`. */
export const AUTH_BUILTIN_NAMES = [
${AUTH.map(([name]) => `  "${name}",`).join("\n")}
] as const;

/** Present only when \`[payments]\` is on and names a provider. */
export const BILLING_BUILTIN_NAMES = [
${BILLING.map(([name]) => `  "${name}",`).join("\n")}
] as const;

export const BUILTIN_NAMES = [
  ...ALWAYS_BUILTIN_NAMES,
  ...AUTH_BUILTIN_NAMES,
  ...BILLING_BUILTIN_NAMES,
] as const;
export type BuiltinName = (typeof BUILTIN_NAMES)[number];

/** Conventional file name for a built-in, matching the docs (\`user\` → users.toml). */
export const BUILTIN_FILENAME: Record<BuiltinName, string> = {
${record(all, (file) => file)}
};

export const BUILTIN_SUMMARY: Record<BuiltinName, string> = {
${record(all, (_file, summary) => summary)}
};

const SOURCES: Record<BuiltinName, string> = {
${sources}
};

/** A fresh copy of a built-in's default definition. */
export function builtinResource(name: BuiltinName): Resource {
  return parseResource(SOURCES[name]);
}
`;

writeFileSync(OUTPUT, out);
console.log(`wrote ${OUTPUT} — ${all.length} built-ins`);
