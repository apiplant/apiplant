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

import { ACTIONS, ORG_CLASS_SUFFIX, type TomlTable } from "./types";
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
