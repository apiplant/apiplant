/**
 * The `emails/` directory, as the studio sees it.
 *
 * The server compiles `emails/*.liquid` at boot and sends them in place of its
 * own messages ([`email_templates.rs`]). Everything here mirrors that reading
 * of the directory — which files pair up, where the front matter ends, what
 * variables a message is rendered with — so that what the studio previews is
 * what the server will send.
 *
 * Rendering itself is LiquidJS, the same template language in a browser build.
 * It is not the Rust implementation, so a difference is possible in principle;
 * in practice the overlap that matters for an email — interpolation, `if`,
 * `for`, the standard filters — is the part both implement to the same spec.
 */

import { Liquid } from "liquidjs";

import { parseTable } from "./toml";
import type { ScannedFile } from "./fs";

/** The directory the framework reads, relative to the app root. */
export const EMAIL_DIR = "emails";

/** One variable a template is rendered with, and something to preview it as. */
export interface EmailVariable {
  name: string;
  description: string;
  sample: string;
}

/** A message the framework sends itself, which a file here replaces. */
export interface BuiltinEmail {
  name: string;
  title: string;
  /** What sending it means, for the page that offers to override it. */
  description: string;
  /** The subject used when a template declares none. */
  subject: string;
  /** The variables it adds on top of [`COMMON_VARIABLES`]. */
  variables: EmailVariable[];
}

/**
 * The variables every message carries: facts, not prose. Matches `Links::vars`.
 */
export const COMMON_VARIABLES: EmailVariable[] = [
  { name: "app_name", description: "What the app calls itself.", sample: "Acme" },
  {
    name: "logo_url",
    description: "Absolute URL of `[email] logo`, or empty when there is none.",
    sample: "",
  },
  { name: "url", description: "The link this message exists to deliver.", sample: "https://acme.test/admin/#/verify-email?token=verify_abc123" },
  { name: "expires_in", description: "How long that link lasts, in words.", sample: "24 hours" },
];

export const BUILTIN_EMAILS: BuiltinEmail[] = [
  {
    name: "verification",
    title: "Email verification",
    description:
      "Sent on registration when [auth] require_email_verification is on. The link confirms the address.",
    subject: "Confirm your email for {{ app_name }}",
    variables: [],
  },
  {
    name: "password_reset",
    title: "Password reset",
    description:
      "Sent when somebody asks to reset a password. The existing password keeps working until the link is used.",
    subject: "Reset your {{ app_name }} password",
    variables: [],
  },
  {
    name: "invitation",
    title: "Organisation invitation",
    description:
      "Sent when a member invites somebody to an organisation. Opening the link lets them choose a password and join.",
    subject: "You're invited to join {{ organization }}",
    variables: [
      { name: "organization", description: "The organisation being joined.", sample: "Acme Ltd" },
      {
        name: "inviter",
        description: "Who sent the invitation, or empty when unknown.",
        sample: "Bo",
      },
    ],
  },
];

export function builtinEmail(name: string): BuiltinEmail | undefined {
  return BUILTIN_EMAILS.find((entry) => entry.name === name);
}

/** Every variable a template of this name is rendered with. */
export function variablesFor(name: string): EmailVariable[] {
  return [...COMMON_VARIABLES, ...(builtinEmail(name)?.variables ?? [])];
}

/** Sample values for a preview: the declared ones, plus whatever else is used. */
export function sampleValues(name: string): Record<string, string> {
  const values: Record<string, string> = {};
  for (const variable of variablesFor(name)) values[variable.name] = variable.sample;
  return values;
}

// ---- reading the directory --------------------------------------------------

export interface EmailEntry {
  /** Template name — the file stem, without `.liquid`. */
  name: string;
  /** `emails/<name>.liquid`. */
  path: string;
  /** `emails/<name>.text.liquid`, when the app wrote a plain-text half. */
  textPath: string | null;
  /** Whether this replaces one of the messages the framework sends itself. */
  builtin: boolean;
}

export function emailPath(name: string): string {
  return `${EMAIL_DIR}/${name}.liquid`;
}

export function emailTextPath(name: string): string {
  return `${EMAIL_DIR}/${name}.text.liquid`;
}

/**
 * The templates in a scanned directory.
 *
 * A lone `<name>.text.liquid` is an error on the server — it has no message to
 * be the text half of — so it is surfaced as a problem rather than listed as a
 * template that would fail to boot.
 */
export function detectEmails(scanned: ScannedFile[]): {
  entries: EmailEntry[];
  problems: { path: string; message: string }[];
} {
  const html = new Set<string>();
  const text = new Set<string>();

  for (const file of scanned) {
    if (!file.path.startsWith(`${EMAIL_DIR}/`) || !file.path.endsWith(".liquid")) continue;
    // Only files directly in emails/ are loaded by the framework.
    const relative = file.path.slice(EMAIL_DIR.length + 1);
    if (relative.includes("/")) continue;
    const stem = relative.slice(0, -".liquid".length);
    if (stem.endsWith(".text")) text.add(stem.slice(0, -".text".length));
    else html.add(stem);
  }

  const problems: { path: string; message: string }[] = [];
  for (const name of text) {
    if (html.has(name)) continue;
    problems.push({
      path: emailTextPath(name),
      message: `has no ${emailPath(name)} to be the text half of`,
    });
  }

  const entries = [...html]
    .sort((a, b) => a.localeCompare(b))
    .map((name) => ({
      name,
      path: emailPath(name),
      textPath: text.has(name) ? emailTextPath(name) : null,
      builtin: !!builtinEmail(name),
    }));

  return { entries, problems };
}

// ---- front matter -----------------------------------------------------------

export interface SplitTemplate {
  /** The TOML between the `---` fences, when there is any. */
  frontMatter: string | null;
  body: string;
}

/**
 * Split `---` fenced front matter off the top, exactly as the server does:
 * only at the very beginning, and only with the fence on its own line, so a
 * `---` inside the markup is body.
 */
export function splitFrontMatter(source: string): SplitTemplate {
  const opened = source.startsWith("---\n")
    ? source.slice(4)
    : source.startsWith("---\r\n")
      ? source.slice(5)
      : null;
  if (opened === null) return { frontMatter: null, body: source };

  for (const delimiter of ["\n---\n", "\r\n---\r\n", "\n---\r\n"]) {
    const end = opened.indexOf(delimiter);
    if (end !== -1) {
      return { frontMatter: opened.slice(0, end), body: opened.slice(end + delimiter.length) };
    }
  }
  // An opening fence with no closing one is a typo, not a body starting `---`.
  return { frontMatter: null, body: source };
}

/** The `subject` in the front matter — itself a template — or null. */
export function subjectSource(source: string): string | null {
  const { frontMatter } = splitFrontMatter(source);
  if (frontMatter === null) return null;
  try {
    const table = parseTable(frontMatter);
    return typeof table.subject === "string" ? table.subject : null;
  } catch {
    return null;
  }
}

/** Replace (or add) the subject in the front matter, keeping the body as-is. */
export function withSubject(source: string, subject: string): string {
  const { body } = splitFrontMatter(source);
  const quoted = JSON.stringify(subject);
  if (!subject.trim()) return body;
  return `---\nsubject = ${quoted}\n---\n${body}`;
}

// ---- the derived plain-text half --------------------------------------------

/**
 * A readable plain-text version of rendered HTML — a port of `text_from_html`,
 * so the preview of a template with no `.text.liquid` shows what will be sent
 * rather than an approximation of it.
 */
export function textFromHtml(html: string): string {
  let out = "";
  let rest = html;

  for (;;) {
    const open = rest.indexOf("<");
    if (open === -1) break;
    out += rest.slice(0, open);
    const tail = rest.slice(open);
    const close = tail.indexOf(">");
    if (close === -1) break;
    const tag = tail.slice(1, close);
    const lower = tag.toLowerCase();

    // Elements whose content is machinery rather than words — a whole-document
    // template would otherwise put its stylesheet in the plain-text part.
    const skipped = elementToSkip(lower);
    if (skipped) {
      const after = tail.slice(close + 1);
      const end = after.toLowerCase().indexOf(`</${skipped}`);
      rest = end === -1 ? "" : after.slice(end);
      continue;
    }

    // A link's destination is the one thing that cannot survive as text.
    if (lower.startsWith("a ")) {
      const href = attr(tag, "href");
      if (href) out += ` <${href}> `;
    }
    const first = lower.replace(/\/+$/, "").trim().split(" ")[0];
    if (["p", "br", "div", "tr", "h1", "h2", "h3", "li", "table"].includes(first ?? "") || lower.startsWith("/")) {
      out += "\n";
    }
    rest = tail.slice(close + 1);
  }
  out += rest;

  const text = out
    .replaceAll("&nbsp;", " ")
    .replaceAll("&quot;", '"')
    .replaceAll("&#39;", "'")
    .replaceAll("&lt;", "<")
    .replaceAll("&gt;", ">")
    .replaceAll("&amp;", "&");

  const lines: string[] = [];
  for (const line of text.split("\n").map((part) => part.trim())) {
    if (line === "" && lines[lines.length - 1] === "") continue;
    lines.push(line);
  }
  return `${lines.join("\n").trim()}\n`;
}

/** Elements whose content is not text a person should read. */
function elementToSkip(lower: string): string | null {
  const name = lower.replace(/\/+$/, "").trim().split(" ")[0] ?? "";
  return ["style", "script", "title"].includes(name) ? name : null;
}

function attr(tag: string, name: string): string | null {
  const at = tag.toLowerCase().indexOf(`${name}="`);
  if (at === -1) return null;
  const rest = tag.slice(at + name.length + 2);
  const end = rest.indexOf('"');
  return end === -1 ? null : rest.slice(0, end);
}

// ---- rendering --------------------------------------------------------------

/**
 * One engine for the page. `strictVariables` is off deliberately: an unknown
 * variable renders as nothing, which is what the server does too, so a preview
 * of a half-written template still shows the layout instead of an error.
 */
const engine = new Liquid({ cache: false });

/**
 * The names a template binds for itself, which are therefore not values
 * anybody has to pass in.
 *
 * LiquidJS reports every name it sees, including the ones the template creates
 * — `{% assign %}`, `{% capture %}`, a `for` variable, `forloop` inside one.
 * Offering those in the values form would invite somebody to fill in a box that
 * cannot affect anything. They come out of the source rather than the parse
 * tree because LiquidJS does not expose scope; the shapes are fixed enough by
 * the tag syntax for that to be reliable.
 */
function locallyBound(source: string): Set<string> {
  const bound = new Set(["forloop", "tablerowloop"]);
  const patterns = [
    /\{%-?\s*assign\s+([A-Za-z_][\w-]*)\s*=/g,
    /\{%-?\s*capture\s+([A-Za-z_][\w-]*)\s*-?%\}/g,
    /\{%-?\s*(?:for|tablerow)\s+([A-Za-z_][\w-]*)\s+in\s/g,
    /\{%-?\s*(?:increment|decrement)\s+([A-Za-z_][\w-]*)\s*-?%\}/g,
  ];
  for (const pattern of patterns) {
    for (const match of source.matchAll(pattern)) bound.add(match[1]);
  }
  return bound;
}

/**
 * Every value a template actually reads, as dotted paths — `url`, `org.name`.
 *
 * `null` when the source does not parse, which is most keystrokes while a tag
 * is being typed: the caller keeps what it last had rather than emptying the
 * form under the cursor.
 */
export function usedVariables(source: string): string[] | null {
  let paths: string[];
  try {
    paths = engine.fullVariablesSync(source);
  } catch {
    return null;
  }
  const bound = locallyBound(source);
  const used = paths.filter((path) => !bound.has(path.split(".")[0] ?? path));
  return [...new Set(used)].sort((a, b) => a.localeCompare(b));
}

/**
 * Turn the flat form — keys like `org.name` — into the object Liquid reads.
 *
 * Shallower keys are set first, so a template using both `{{ org }}` and
 * `{{ org.name }}` ends up with the object: a name with something under it is
 * the more specific statement about what the value is.
 */
export function expandValues(values: Record<string, string>): Record<string, unknown> {
  const scope: Record<string, unknown> = {};
  const keys = Object.keys(values).sort((a, b) => a.split(".").length - b.split(".").length);

  for (const key of keys) {
    const segments = key.split(".");
    const leaf = segments.pop()!;
    let cursor = scope;
    for (const segment of segments) {
      const next = cursor[segment];
      if (typeof next !== "object" || next === null) cursor[segment] = {};
      cursor = cursor[segment] as Record<string, unknown>;
    }
    // A scalar already sitting where an object was built stays replaced.
    if (typeof cursor[leaf] !== "object" || cursor[leaf] === null) cursor[leaf] = values[key];
  }
  return scope;
}

/**
 * The variables a whole message reads: its subject, its body, and the written
 * text half when there is one.
 *
 * The front matter is skipped — it is TOML, not markup — but the subject inside
 * it is a template of its own, so it is scanned as one.
 */
export function usedVariablesIn(source: string, textSource?: string | null): string[] | null {
  const parts = [
    subjectSource(source) ?? "",
    splitFrontMatter(source).body,
    textSource ? splitFrontMatter(textSource).body : "",
  ];
  return usedVariables(parts.join("\n"));
}

/**
 * The names a template loops over, wherever they are iterated.
 *
 * A box to type a string into cannot stand in for a list, so a row like this
 * says so rather than looking broken when filling it in changes nothing.
 */
export function iteratedVariables(source: string): string[] {
  const found = new Set<string>();
  for (const match of source.matchAll(/\{%-?\s*(?:for|tablerow)\s+[A-Za-z_][\w-]*\s+in\s+([\w.]+)/g)) {
    found.add(match[1]);
  }
  return [...found];
}

/** A row in the values form: what it is, and where it came from. */
export interface FormVariable extends EmailVariable {
  /** The framework passes this one to this message. */
  declared: boolean;
  /** The template actually reads it. */
  used: boolean;
  /** The template loops over it, so a typed value cannot preview it. */
  iterated: boolean;
}

/**
 * The values form for a template: what the framework passes, plus whatever the
 * template turned out to read.
 *
 * Both directions matter. A name the template reads that nothing passes renders
 * as nothing in the sent message — which looks like a preview problem and is
 * not — and a declared name the template ignores is just a value going spare.
 */
export function formVariables(
  name: string,
  used: readonly string[],
  iterated: readonly string[] = [],
): FormVariable[] {
  const declared = variablesFor(name);
  const seen = new Set(declared.map((variable) => variable.name));
  const loops = new Set(iterated);

  const describe = (path: string): string => {
    if (loops.has(path)) return "A list this template loops over — the preview shows it empty.";
    return builtinEmail(name)
      ? "Read by this template, but not passed to this message — it renders as nothing."
      : "Read by this template. Whatever calls send_email has to pass it.";
  };

  const extra = used
    .filter((path) => !seen.has(path))
    .map((path) => ({
      name: path,
      description: describe(path),
      sample: "",
      declared: false,
      used: true,
      iterated: loops.has(path),
    }));

  return [
    ...declared.map((variable) => ({
      ...variable,
      declared: true,
      used: used.some((path) => path === variable.name || path.startsWith(`${variable.name}.`)),
      iterated: loops.has(variable.name),
    })),
    ...extra,
  ];
}

export interface RenderedEmail {
  subject: string;
  html: string;
  text: string;
  /** The first thing that failed to parse or render, when something did. */
  error: string | null;
}

/**
 * Render a template the way the server will: subject from the front matter
 * (falling back to the built-in one), body below it, and a text half that is
 * either the written one or derived from the rendered HTML.
 */
export function renderEmail(
  name: string,
  source: string,
  values: Record<string, string>,
  textSource?: string | null,
): RenderedEmail {
  const { body } = splitFrontMatter(source);
  const declared = subjectSource(source);
  const fallback = builtinEmail(name)?.subject ?? "{{ app_name }}";
  const scope = expandValues(values);

  let error: string | null = null;
  const render = (template: string, what: string): string => {
    try {
      return engine.parseAndRenderSync(template, scope);
    } catch (failure) {
      error ??= `${what}: ${failure instanceof Error ? failure.message : String(failure)}`;
      return "";
    }
  };

  const subject = render(declared ?? fallback, "subject");
  const html = render(body, "body");
  const text = textSource
    ? render(splitFrontMatter(textSource).body, "text half")
    : textFromHtml(html);

  return { subject, html, text, error };
}

/** Whether the front matter is TOML at all — a boot failure if it is not. */
export function frontMatterError(source: string): string | null {
  const { frontMatter } = splitFrontMatter(source);
  if (frontMatter === null) return null;
  try {
    parseTable(frontMatter);
    return null;
  } catch (error) {
    return `front matter is not TOML: ${error instanceof Error ? error.message : String(error)}`;
  }
}

// ---- scaffolds --------------------------------------------------------------

/** The sentences the built-in message uses, as Liquid a person can edit. */
function copyFor(name: string): { lead: string; call: string; note: string } {
  switch (name) {
    case "invitation":
      return {
        lead: "{% if inviter != '' %}{{ inviter }} has invited you{% else %}You have been invited{% endif %} to join {{ organization }} on {{ app_name }}.",
        call: "Accept the invitation",
        note: "Opening the link lets you choose a password and join. It stops working in {{ expires_in }}.",
      };
    case "password_reset":
      return {
        lead: "Somebody asked to reset the password for this {{ app_name }} account.",
        call: "Choose a new password",
        note: "The link stops working in {{ expires_in }}. If this wasn't you, ignore this message — your password has not changed.",
      };
    case "verification":
      return {
        lead: "Confirm this address to finish setting up your {{ app_name }} account.",
        call: "Confirm my address",
        note: "The link stops working in {{ expires_in }}.",
      };
    default:
      return {
        lead: "Something happened in {{ app_name }} that you should know about.",
        call: "Open {{ app_name }}",
        note: "The link stops working in {{ expires_in }}.",
      };
  }
}

/**
 * A starting template: the framework's own layout, written out as a file.
 *
 * Tables and inline styles throughout, because a mail client is not a browser —
 * Outlook lays out with tables and Gmail strips `<style>` blocks. Starting from
 * this rather than from an empty file means an edited subject line or a changed
 * sentence does not also cost the message its rendering in half the clients.
 */
export function scaffoldEmail(name: string): string {
  const builtin = builtinEmail(name);
  const subject = builtin?.subject ?? `A message from {{ app_name }}`;
  const { lead, call, note } = copyFor(name);

  return `---
subject = ${JSON.stringify(subject)}
---
<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width,initial-scale=1">
<meta name="color-scheme" content="light dark">
<title>{{ app_name }}</title>
<style>
@media only screen and (max-width:600px) {
  .ap-pad { padding-left:20px !important; padding-right:20px !important; }
  .ap-button { display:block !important; text-align:center !important; }
}
</style>
</head>
<body style="margin:0;padding:0;width:100%;background:#f4f5f7;font-family:-apple-system,BlinkMacSystemFont,'Segoe UI',Roboto,Helvetica,Arial,sans-serif;color:#1a1c1f;">
<table role="presentation" cellpadding="0" cellspacing="0" border="0" width="100%" style="background:#f4f5f7;">
<tr><td align="center" style="padding:24px 12px;">
<table role="presentation" cellpadding="0" cellspacing="0" border="0" width="600" style="width:100%;max-width:600px;background:#ffffff;border-radius:14px;border:1px solid #e3e6ea;overflow:hidden;">

<tr><td class="ap-pad" style="padding:22px 32px;background:#14161a;">
<table role="presentation" cellpadding="0" cellspacing="0" border="0"><tr>
{% if logo_url != '' %}<td style="padding:0 12px 0 0;vertical-align:middle;"><img src="{{ logo_url }}" alt="{{ app_name }}" height="36" style="display:block;border:0;height:36px;width:auto;"></td>{% endif %}
<td style="vertical-align:middle;font-size:18px;line-height:1.3;font-weight:700;color:#ffffff;letter-spacing:-0.2px;">{{ app_name }}</td>
</tr></table>
</td></tr>

<tr><td class="ap-pad" style="padding:32px 32px 8px;font-size:16px;line-height:1.6;color:#1a1c1f;">${lead}</td></tr>

<tr><td class="ap-pad" style="padding:20px 32px 8px;">
<table role="presentation" cellpadding="0" cellspacing="0" border="0"><tr><td style="border-radius:8px;background:#14161a;">
<a class="ap-button" href="{{ url }}" style="display:inline-block;padding:13px 26px;border-radius:8px;background:#14161a;color:#ffffff;text-decoration:none;font-size:15px;font-weight:600;line-height:1;">${call}</a>
</td></tr></table>
</td></tr>

<tr><td class="ap-pad" style="padding:16px 32px 0;font-size:13px;line-height:1.6;color:#6b7280;">Or paste this link into your browser:</td></tr>
<tr><td class="ap-pad" style="padding:4px 32px 0;font-size:13px;line-height:1.6;word-break:break-all;"><a href="{{ url }}" style="color:#4b5563;text-decoration:underline;">{{ url }}</a></td></tr>

<tr><td class="ap-pad" style="padding:24px 32px 0;"><div style="height:1px;background:#e3e6ea;line-height:1px;font-size:0;">&nbsp;</div></td></tr>
<tr><td class="ap-pad" style="padding:16px 32px 28px;font-size:13px;line-height:1.6;color:#6b7280;">${note}</td></tr>

</table>
<table role="presentation" cellpadding="0" cellspacing="0" border="0" width="600" style="width:100%;max-width:600px;">
<tr><td class="ap-pad" style="padding:16px 32px 0;font-size:12px;line-height:1.6;color:#9aa1ab;text-align:center;">You are receiving this because somebody used this address at {{ app_name }}.</td></tr>
</table>
</td></tr>
</table>
</body></html>
`;
}

/** The plain-text half, when an app chooses to write one rather than derive it. */
export function scaffoldEmailText(name: string): string {
  const { lead, call, note } = copyFor(name);
  return `${lead}

${call}:
{{ url }}

${note}
`;
}

/** Template names are file stems, so they have to be usable as one. */
export const EMAIL_NAME_RULE = /^[a-z0-9][a-z0-9_-]*$/;
