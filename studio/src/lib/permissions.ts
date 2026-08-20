/**
 * Policy strings, taken apart and put back together.
 *
 * A clause is one string — a level, optionally a role name, optionally narrowed
 * to a class of organisation — but the form edits those three separately, and
 * the role and class halves are names nothing in the app directory declares.
 * They are membership and organisation *data*, so the only defence against
 * `role:manger` silently granting nothing is to offer what the project already
 * spells somewhere; that is what `policyVocabulary` collects.
 */

import {
  ACTIONS,
  ORG_CLASS_SUFFIX,
  type PermissionSet,
  type TomlTable,
} from "./types";
import { studio } from "./store";
import { parseTable } from "./toml";

/** A policy string taken apart; `role` is meaningful only when level is `role`. */
export interface Subject {
  level: string;
  role: string;
  orgClass: string;
}

export function parsePolicy(policy: string): Subject {
  const at = policy.indexOf(ORG_CLASS_SUFFIX);
  const bare = at === -1 ? policy : policy.slice(0, at);
  const orgClass = at === -1 ? "" : policy.slice(at + ORG_CLASS_SUFFIX.length);
  return bare.startsWith("role:")
    ? { level: "role", role: bare.slice(5), orgClass }
    : { level: bare, role: "", orgClass };
}

export function formatPolicy(subject: Subject): string {
  const bare = subject.level === "role" ? `role:${subject.role}` : subject.level;
  return subject.orgClass ? `${bare}${ORG_CLASS_SUFFIX}${subject.orgClass}` : bare;
}

/** Role and class names the project already uses, for the completion lists. */
export function policyVocabulary(): { roles: string[]; classes: string[] } {
  const roles = new Set<string>();
  const classes = new Set<string>();

  for (const entry of studio.project?.resources ?? [])
    for (const action of ACTIONS)
      for (const rule of entry.resource.permissions[action] ?? []) {
        const subject = parsePolicy(rule.policy);
        if (subject.level === "role" && subject.role) roles.add(subject.role);
        if (subject.orgClass) classes.add(subject.orgClass);
      }

  // Seed rows are the other half of the vocabulary: a role the app grants but
  // no policy mentions yet is exactly the one somebody is about to type.
  for (const [path, file] of Object.entries(studio.project?.files ?? {})) {
    if (!path.startsWith("seed/") || !path.endsWith(".toml") || !file.current) continue;
    let table: TomlTable;
    try {
      table = parseTable(file.current);
    } catch {
      continue;
    }
    for (const rows of Object.values(table)) {
      if (!Array.isArray(rows)) continue;
      for (const row of rows) {
        if (typeof row !== "object" || row === null || Array.isArray(row)) continue;
        const record = row as TomlTable;
        if (typeof record.role === "string") roles.add(record.role);
        if (typeof record.org_class === "string") classes.add(record.org_class);
      }
    }
  }

  return { roles: [...roles].sort(), classes: [...classes].sort() };
}

/**
 * What is wrong with one action's clauses, in the reader's own words.
 *
 * The server does not reject any of these — every one of them loads, answers
 * requests, and does something other than what it looks like it says, which is
 * the only kind of permission bug worth a warning. The rules mirror how a set
 * is actually evaluated: `deny` is consulted before anything else, the first
 * positive clause that matches wins, and `private` means "not exposed" only
 * when it is the whole set, so a `private` clause with company is a clause
 * naming nobody on an action that is still very much exposed.
 */
export function permissionConflicts(rules: PermissionSet): string[] {
  const issues: string[] = [];
  const word = (rule: { policy: string; effect: string }) =>
    `\`${rule.effect} ${rule.policy}\``;

  const seen = new Set<string>();
  for (const rule of rules) {
    const key = `${rule.effect} ${rule.policy}`;
    if (seen.has(key)) issues.push(`${word(rule)} is written twice.`);
    seen.add(key);
  }

  const positive = rules.filter((rule) => rule.effect !== "deny");
  const denials = rules.filter((rule) => rule.effect === "deny");
  const levelOf = (rule: { policy: string }) => parsePolicy(rule.policy).level;

  // `private` is the absence of an endpoint, and only a set that says nothing
  // else can say it — see `PolicySet::is_private` on the server.
  if (rules.some((rule) => levelOf(rule) === "private") && rules.length > 1) {
    issues.push(
      "A clause naming no-one only closes the action when it is the only clause; here the others still expose it, and it grants nobody anything.",
    );
  }

  // A denial is consulted first and does not care which clause allowed the
  // caller, so one naming everybody empties every grant above it.
  if (denials.some((rule) => levelOf(rule) === "public") && positive.length) {
    issues.push(
      "`deny everybody` refuses every caller, so the clauses allowing anybody never take effect.",
    );
  }

  for (const denial of denials) {
    const twin = positive.find((rule) => rule.policy === denial.policy);
    if (twin) {
      issues.push(
        `${word(twin)} and ${word(denial)} name the same caller, and deny is consulted first — the grant never applies.`,
      );
    }
  }

  // Not wrong, but not what it looks like: the broadest positive clause already
  // matched, so anything narrower below it is never reached.
  if (
    positive.some((rule) => rule.effect === "allow" && levelOf(rule) === "public") &&
    positive.length > 1
  ) {
    issues.push(
      "`allow everybody` already matches every caller, so the other clauses allowing anybody add nothing.",
    );
  }

  return issues;
}
