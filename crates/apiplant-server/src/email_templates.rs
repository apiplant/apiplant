//! The `emails/` directory: an app's own wording for the messages it sends.
//!
//! [`emails`](crate::emails) composes three messages the framework sends by
//! itself, and they are deliberately plain — they have to work in an app that
//! has configured nothing but a provider and a `from` address. This module is
//! how an app says something else instead, without giving up the flows: drop a
//! file in `emails/` and it is used in place of the built-in one.
//!
//! ```text
//! emails/
//!   verification.liquid        # replaces "Confirm your email"
//!   verification.text.liquid   # its plain-text half (optional)
//!   password_reset.liquid
//!   invitation.liquid
//!   welcome.liquid             # a new one, sent from a function
//! ```
//!
//! A file named after one of the three built-ins **overrides** it. Any other
//! name is a new template, which nothing sends on its own — a function asks for
//! it by name through `send_email`.
//!
//! ## Why Liquid
//!
//! For what it cannot do. A template here is written by whoever writes the
//! app's copy, edited in the studio, and rendered on the server with a token in
//! scope; it must not be able to read a file, call out, or reach anything it
//! was not handed. Liquid has no escape hatch to close — the variables in scope
//! are exactly the ones passed in, and `{{ }}` interpolates rather than
//! executes.
//!
//! ## The subject line
//!
//! A message is a subject *and* a body, so a template carries both: TOML front
//! matter, fenced with `---`, ahead of the markup.
//!
//! ```text
//! ---
//! subject = "Confirm your email for {{ app_name }}"
//! ---
//! <p>Hello — <a href="{{ url }}">confirm your address</a>.</p>
//! ```
//!
//! The subject is a template too, so it can name the app or the organisation.
//! A file with no front matter falls back to the built-in subject when it is
//! overriding one, and to the app's name when it is not.
//!
//! ## The plain-text half
//!
//! `<name>.text.liquid` beside it, when the app wants to write one. Without it
//! the text half is derived from the rendered HTML — tags dropped, links kept
//! as their URL — which is worth more than no text part at all, since a message
//! with only HTML is scored as spam by most of the things that score messages.

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;

use anyhow::{Context, Result};
use liquid::{Parser, ParserBuilder};

/// The directory an app's templates live in, under its root.
pub const TEMPLATE_DIR: &str = "emails";

/// The three the framework sends itself. A file named after one replaces it.
pub const BUILTIN_TEMPLATES: [&str; 3] = ["invitation", "verification", "password_reset"];

/// One template: a subject and a body, each compiled once at boot.
struct Template {
    subject: Option<liquid::Template>,
    html: liquid::Template,
    text: Option<liquid::Template>,
}

/// Every template an app supplied, compiled and ready to render.
///
/// Built once at boot rather than per message: a template that does not parse
/// should stop the app rather than surface as a failed send hours later, when
/// somebody is waiting on a password reset.
#[derive(Default)]
pub struct EmailTemplates {
    by_name: BTreeMap<String, Template>,
}

/// A compiled template has nothing readable in it, so the names are the whole
/// of what a debug print can usefully say.
impl std::fmt::Debug for EmailTemplates {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        f.debug_struct("EmailTemplates")
            .field("names", &self.names())
            .finish()
    }
}

/// What a rendered template produces — the same three parts as a built-in.
#[derive(Debug)]
pub struct Rendered {
    pub subject: String,
    pub text: String,
    pub html: String,
}

impl EmailTemplates {
    /// Compile every `*.liquid` under `<root>/emails`.
    ///
    /// An absent directory is not an error — most apps have none. A file that
    /// does not parse is: it was written to be used, and quietly falling back
    /// to the built-in would be indistinguishable from the override working.
    pub fn load(root: &Path) -> Result<EmailTemplates> {
        let dir = root.join(TEMPLATE_DIR);
        if !dir.is_dir() {
            return Ok(EmailTemplates::default());
        }
        let parser = ParserBuilder::with_stdlib()
            .build()
            .context("could not build the template parser")?;

        // Two passes, because the text half is a property of a template rather
        // than a template of its own, and the directory is in no useful order.
        let mut sources: BTreeMap<String, (Option<String>, Option<String>)> = BTreeMap::new();
        for entry in
            std::fs::read_dir(&dir).with_context(|| format!("reading {}", dir.display()))?
        {
            let path = entry?.path();
            if path.extension().and_then(|e| e.to_str()) != Some("liquid") {
                continue;
            }
            let stem = path
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or_default()
                .to_string();
            let body = std::fs::read_to_string(&path)
                .with_context(|| format!("reading {}", path.display()))?;
            match stem.strip_suffix(".text") {
                Some(name) => sources.entry(name.to_string()).or_default().1 = Some(body),
                None => sources.entry(stem).or_default().0 = Some(body),
            }
        }

        let mut by_name = BTreeMap::new();
        for (name, (html, text)) in sources {
            // A lone `<name>.text.liquid` has no message to be the text half
            // of. Refusing it names the missing file, which is more use than
            // an override that silently never applies.
            let Some(html) = html else {
                anyhow::bail!(
                    "emails/{name}.text.liquid has no emails/{name}.liquid to be the text half of"
                );
            };
            let (front_matter, markup) = split_front_matter(&html);
            let subject = match front_matter {
                Some(matter) => subject_of(&matter, &name)?,
                None => None,
            };
            by_name.insert(
                name.clone(),
                Template {
                    subject: subject
                        .map(|s| compile(&parser, &s, &format!("emails/{name}.liquid subject")))
                        .transpose()?,
                    html: compile(&parser, markup, &format!("emails/{name}.liquid"))?,
                    text: text
                        .map(|body| {
                            let (_, markup) = split_front_matter(&body);
                            compile(&parser, markup, &format!("emails/{name}.text.liquid"))
                        })
                        .transpose()?,
                },
            );
        }
        Ok(EmailTemplates { by_name })
    }

    /// Whether the app supplied a template by this name.
    pub fn has(&self, name: &str) -> bool {
        self.by_name.contains_key(name)
    }

    /// Every template the app supplied, in name order — for the studio, and for
    /// the error a function gets when it asks for one that is not there.
    pub fn names(&self) -> Vec<String> {
        self.by_name.keys().cloned().collect()
    }

    /// Render `name` with `vars`.
    ///
    /// `fallback_subject` is used when the template declared none: the built-in
    /// subject for an override, so replacing only the body of a message does
    /// not cost it its subject line.
    pub fn render(
        &self,
        name: &str,
        vars: &liquid::Object,
        fallback_subject: &str,
    ) -> Result<Rendered> {
        let template = self
            .by_name
            .get(name)
            .with_context(|| format!("no email template `{name}` in {TEMPLATE_DIR}/"))?;

        let html = template
            .html
            .render(vars)
            .with_context(|| format!("rendering {TEMPLATE_DIR}/{name}.liquid"))?;
        let subject = match &template.subject {
            Some(subject) => subject.render(vars).with_context(|| {
                format!("rendering the subject of {TEMPLATE_DIR}/{name}.liquid")
            })?,
            None => fallback_subject.to_string(),
        };
        let text = match &template.text {
            Some(text) => text
                .render(vars)
                .with_context(|| format!("rendering {TEMPLATE_DIR}/{name}.text.liquid"))?,
            None => text_from_html(&html),
        };
        Ok(Rendered {
            subject,
            text,
            html,
        })
    }
}

fn compile(parser: &Parser, source: &str, what: &str) -> Result<liquid::Template> {
    parser
        .parse(source)
        .with_context(|| format!("could not parse {what}"))
}

/// Split `---` fenced front matter off the top of a template.
///
/// Only at the very beginning, and only with the fence on its own line, so a
/// `---` inside the markup — a horizontal rule, an em-dash line — is body.
fn split_front_matter(source: &str) -> (Option<String>, &str) {
    let body = source
        .strip_prefix("---\n")
        .or_else(|| source.strip_prefix("---\r\n"));
    let Some(body) = body else {
        return (None, source);
    };
    for delimiter in ["\n---\n", "\r\n---\r\n", "\n---\r\n"] {
        if let Some(end) = body.find(delimiter) {
            return (
                Some(body[..end].to_string()),
                &body[end + delimiter.len()..],
            );
        }
    }
    // An opening fence with no closing one is a typo, not a template whose
    // first line happens to be `---`; treating it as body would mail the TOML.
    (None, source)
}

/// Read `subject` out of the front matter.
fn subject_of(matter: &str, name: &str) -> Result<Option<String>> {
    let table: toml::Value = toml::from_str(matter)
        .with_context(|| format!("the front matter of {TEMPLATE_DIR}/{name}.liquid is not TOML"))?;
    Ok(table
        .get("subject")
        .and_then(|value| value.as_str())
        .map(str::to_string))
}

/// A readable plain-text version of rendered HTML.
///
/// Not a general converter and not trying to be: it drops tags, keeps the text
/// between them, and writes an `<a href>` out as `text <url>` so the link
/// survives. Enough that a client showing the text part shows a usable message,
/// which is the whole reason to send one.
fn text_from_html(html: &str) -> String {
    let mut out = String::with_capacity(html.len() / 2);
    let mut rest = html;
    while let Some(open) = rest.find('<') {
        out.push_str(&rest[..open]);
        let tail = &rest[open..];
        let Some(close) = tail.find('>') else { break };
        let tag = &tail[1..close];
        let lower = tag.to_ascii_lowercase();
        // Elements whose content is machinery rather than words. A template
        // that is a whole document — which the ones the studio scaffolds are —
        // otherwise puts its stylesheet in the plain-text part of the message.
        if let Some(name) = element_to_skip(&lower) {
            let after = &tail[close + 1..];
            let end = after
                .to_ascii_lowercase()
                .find(&format!("</{name}"))
                .unwrap_or(after.len());
            rest = &after[end..];
            continue;
        }
        // A link's destination is the one thing that cannot survive as text on
        // its own, so it is written out beside the words.
        if let Some(href) = lower.strip_prefix("a ").and_then(|_| attr(tag, "href")) {
            out.push_str(&format!(" <{href}> "));
        }
        // The tags that are a line break in every reading of the document.
        if matches!(
            lower.trim_end_matches('/').trim().split(' ').next(),
            Some("p" | "br" | "div" | "tr" | "h1" | "h2" | "h3" | "li" | "table")
        ) || lower.starts_with('/')
        {
            out.push('\n');
        }
        rest = &tail[close + 1..];
    }
    out.push_str(rest);

    // Entities we emit, and the whitespace all of the above leaves behind.
    let text = out
        .replace("&nbsp;", " ")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&amp;", "&");
    let mut lines: Vec<&str> = text.lines().map(str::trim).collect();
    lines.dedup_by(|a, b| a.is_empty() && b.is_empty());
    lines
        .into_iter()
        .collect::<Vec<_>>()
        .join("\n")
        .trim()
        .to_string()
        + "\n"
}

/// The name of an element whose *content* is not text a person should read.
///
/// `head` is not among them: a document keeps `<title>` out of the text this
/// way, and everything else in a head is skipped for its own sake — an element
/// list is easier to be sure of than tracking where the head ends.
fn element_to_skip(lower: &str) -> Option<&'static str> {
    let name = lower.trim_end_matches('/').trim().split(' ').next()?;
    ["style", "script", "title"]
        .into_iter()
        .find(|candidate| *candidate == name)
}

/// The value of one attribute in a tag's source, unquoted.
fn attr(tag: &str, name: &str) -> Option<String> {
    let at = tag.to_ascii_lowercase().find(&format!("{name}=\""))?;
    let rest = &tag[at + name.len() + 2..];
    let end = rest.find('"')?;
    Some(rest[..end].to_string())
}

/// Shared handle: compiled once, read by every send.
pub type SharedTemplates = Arc<EmailTemplates>;

#[cfg(test)]
mod tests {
    use super::*;

    fn write(dir: &Path, name: &str, body: &str) {
        std::fs::create_dir_all(dir.join(TEMPLATE_DIR)).unwrap();
        std::fs::write(dir.join(TEMPLATE_DIR).join(name), body).unwrap();
    }

    fn temp(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("apiplant-tpl-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn an_app_with_no_directory_has_no_templates() {
        let root = temp("none");
        let templates = EmailTemplates::load(&root).unwrap();
        assert!(templates.names().is_empty());
        assert!(!templates.has("verification"));
    }

    #[test]
    fn front_matter_carries_the_subject_and_the_rest_is_the_body() {
        let root = temp("subject");
        write(
            &root,
            "verification.liquid",
            "---\nsubject = \"Confirm for {{ app_name }}\"\n---\n<p>Hi — <a href=\"{{ url }}\">confirm</a>.</p>\n",
        );
        let templates = EmailTemplates::load(&root).unwrap();
        let vars = liquid::object!({ "app_name": "Acme", "url": "https://x.test/go" });
        let rendered = templates.render("verification", &vars, "fallback").unwrap();

        assert_eq!(rendered.subject, "Confirm for Acme");
        assert!(rendered.html.contains("https://x.test/go"));
        assert!(
            !rendered.html.contains("subject ="),
            "front matter leaked into the body"
        );
        // No text half was written, so one is derived — and the link survives
        // it, which is the only part of an HTML message that cannot.
        assert!(rendered.text.contains("confirm"));
        assert!(rendered.text.contains("<https://x.test/go>"));
    }

    #[test]
    fn a_template_without_front_matter_keeps_the_subject_it_is_replacing() {
        // Replacing the body of a built-in should not silently cost it a
        // subject line.
        let root = temp("nofm");
        write(&root, "password_reset.liquid", "<p>Reset it.</p>\n");
        let templates = EmailTemplates::load(&root).unwrap();
        let rendered = templates
            .render(
                "password_reset",
                &liquid::object!({}),
                "Reset your password",
            )
            .unwrap();
        assert_eq!(rendered.subject, "Reset your password");
        assert_eq!(rendered.html.trim(), "<p>Reset it.</p>");
    }

    #[test]
    fn a_written_text_half_is_used_instead_of_a_derived_one() {
        let root = temp("texthalf");
        write(&root, "welcome.liquid", "<p>Hello {{ name }}</p>");
        write(&root, "welcome.text.liquid", "Hello {{ name }}, in plain.");
        let templates = EmailTemplates::load(&root).unwrap();
        let rendered = templates
            .render("welcome", &liquid::object!({ "name": "Bo" }), "Welcome")
            .unwrap();
        assert_eq!(rendered.text, "Hello Bo, in plain.");
        assert_eq!(rendered.html, "<p>Hello Bo</p>");
    }

    #[test]
    fn a_derived_text_half_carries_no_stylesheet() {
        // A template written as a whole document — which is what a mail client
        // needs, and what the studio scaffolds — has a head full of things that
        // are not words. None of them belong in the plain-text part.
        let root = temp("document");
        write(
            &root,
            "welcome.liquid",
            "<!doctype html><html><head><title>Acme</title>\
             <style>.ap-pad { padding:20px; }</style></head>\
             <body><p>Hello {{ name }}</p></body></html>",
        );
        let templates = EmailTemplates::load(&root).unwrap();
        let rendered = templates
            .render("welcome", &liquid::object!({ "name": "Bo" }), "Welcome")
            .unwrap();

        assert_eq!(rendered.text.trim(), "Hello Bo");
        // The HTML half keeps all of it — only the text derivation drops them.
        assert!(rendered.html.contains("padding:20px"));
    }

    #[test]
    fn a_template_that_does_not_parse_stops_the_app() {
        // Rather than surfacing hours later as a password reset that never
        // arrived, which is where a lazy compile would put it.
        let root = temp("broken");
        write(&root, "verification.liquid", "{% if %}");
        let error = EmailTemplates::load(&root).unwrap_err().to_string();
        assert!(
            error.contains("verification.liquid"),
            "unhelpful error: {error}"
        );
    }

    #[test]
    fn a_text_half_with_nothing_to_halve_is_an_error() {
        let root = temp("orphan");
        write(&root, "welcome.text.liquid", "just text");
        let error = EmailTemplates::load(&root).unwrap_err().to_string();
        assert!(error.contains("welcome.liquid"), "unhelpful error: {error}");
    }

    #[test]
    fn asking_for_a_template_that_is_not_there_says_so() {
        let root = temp("missing");
        let templates = EmailTemplates::load(&root).unwrap();
        let error = templates
            .render("nope", &liquid::object!({}), "")
            .unwrap_err()
            .to_string();
        assert!(error.contains("nope"), "unhelpful error: {error}");
    }
}

