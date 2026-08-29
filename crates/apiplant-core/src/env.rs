//! Environment-variable references in an app's TOML files.
//!
//! Every TOML file apiplant reads from an app directory — `main.toml`, each
//! `resources/*.toml`, each `functions/*.toml` — goes through [`expand_document`]
//! before it is deserialized, so any string value may name variables:
//!
//! ```toml
//! [database]
//! url = "$DATABASE_URL"
//!
//! [email]
//! api_key = "${SENDGRID_API_KEY}"
//! ```
//!
//! This is what lets a `main.toml` you commit hold no credentials: the file
//! describes *where* the secret comes from, and the deployment supplies it.
//!
//! ## The syntax
//!
//! | Written | Means |
//! |---------|-------|
//! | `$VAR`, `${VAR}` | the variable's value, or `""` (with a warning) when unset |
//! | `${VAR:-default}` | the variable's value, or `default` when unset or empty |
//! | `$$` | a literal `$` |
//! | `$` followed by anything else | itself, unchanged |
//!
//! A name is a letter or `_` followed by letters, digits or `_`. Anything that
//! isn't one — `$19.99`, `US$`, `a$b` — is left exactly as written, so text that
//! merely contains a dollar sign needs no escaping and only a genuine ambiguity
//! (`$$USD`) calls for `$$`.
//!
//! Several references can appear in one string, which is the case that makes
//! this worth having over a whole-value substitution:
//!
//! ```toml
//! url = "postgres://$DB_USER:$DB_PASSWORD@$DB_HOST:${DB_PORT:-5432}/$DB_NAME"
//! ```
//!
//! ## Where it happens
//!
//! On the **parsed document**, not the raw text: a value is substituted into a
//! string that TOML has already produced, so a password containing `"` or `\`
//! can't turn into a syntax error or, worse, into extra TOML. Keys are never
//! expanded — a table named by the environment would make a file's shape
//! depend on the deployment, which is not what this is for.

use std::borrow::Cow;

/// Parse a TOML file, expanding environment references in its string values.
///
/// This is how every app-directory file is read, so `$VAR` works the same in
/// `main.toml`, in a resource and in a function's config.
///
/// A file with no `$` in it is deserialized straight from its text rather than
/// through [`toml::Value`], which keeps the line and column in a parse error.
/// That is the overwhelmingly common case, so the diagnostics an author sees
/// for a typo are unchanged by this feature existing.
pub fn parse_toml<T: serde::de::DeserializeOwned>(
    text: &str,
    source: &str,
) -> Result<T, toml::de::Error> {
    if !text.contains('$') {
        return toml::from_str(text);
    }
    let mut document: toml::Value = toml::from_str(text)?;
    expand_document(&mut document, source);
    document.try_into()
}

/// Expand every string value in a parsed TOML document, in place.
///
/// Walks tables and arrays recursively. Non-string scalars are untouched, and
/// so are keys.
pub fn expand_document(value: &mut toml::Value, source: &str) {
    match value {
        toml::Value::String(text) => {
            if let Cow::Owned(expanded) = expand(text, source) {
                *text = expanded;
            }
        }
        toml::Value::Array(items) => {
            for item in items {
                expand_document(item, source);
            }
        }
        toml::Value::Table(table) => {
            for (_key, item) in table.iter_mut() {
                expand_document(item, source);
            }
        }
        _ => {}
    }
}

/// Expand `$VAR`, `${VAR}`, `${VAR:-default}` and `$$` in one string.
///
/// `source` names the file for the warning an unset variable produces; it has
/// no effect on the result.
///
/// Borrows when there is nothing to do, which is the overwhelmingly common
/// case — most strings in a TOML file contain no `$` at all.
pub fn expand<'a>(text: &'a str, source: &str) -> Cow<'a, str> {
    if !text.contains('$') {
        return Cow::Borrowed(text);
    }

    let bytes = text.as_bytes();
    let mut out = String::with_capacity(text.len());
    let mut i = 0;

    while i < bytes.len() {
        if bytes[i] != b'$' {
            // Copy the whole run up to the next `$` at once.
            let next = text[i..].find('$').map(|n| i + n).unwrap_or(bytes.len());
            out.push_str(&text[i..next]);
            i = next;
            continue;
        }

        // `$$` is the escape, and the only reason a literal dollar ever needs
        // one: every other unrecognised `$` is already left alone below.
        if bytes.get(i + 1) == Some(&b'$') {
            out.push('$');
            i += 2;
            continue;
        }

        match parse_reference(&text[i..]) {
            Some(reference) => {
                out.push_str(&resolve(&reference, source));
                i += reference.length;
            }
            // Not a reference: `$19.99`, a trailing `$`, `${` with no `}`.
            None => {
                out.push('$');
                i += 1;
            }
        }
    }

    Cow::Owned(out)
}

/// One `$…` reference and how much of the input it occupied.
struct Reference {
    name: String,
    /// The `${VAR:-default}` fallback, when the reference gave one.
    default: Option<String>,
    /// Bytes consumed, including the leading `$`.
    length: usize,
}

/// Read a reference at the start of `text`, which begins with `$`.
///
/// `None` means "this `$` doesn't introduce one" — an unterminated `${`, or a
/// name that doesn't start with a letter or `_`. Both are left as written
/// rather than reported: a `$` in prose is far more likely than a typo, and a
/// config file that refuses to load over a price is a worse trade.
fn parse_reference(text: &str) -> Option<Reference> {
    let rest = &text[1..];

    if let Some(body) = rest.strip_prefix('{') {
        let end = body.find('}')?;
        let inner = &body[..end];
        // `${VAR:-default}`; the default may itself be empty (`${VAR:-}`) and
        // may contain anything but the closing brace.
        let (name, default) = match inner.split_once(":-") {
            Some((name, default)) => (name, Some(default.to_string())),
            None => (inner, None),
        };
        if !is_name(name) {
            return None;
        }
        return Some(Reference {
            name: name.to_string(),
            default,
            // `$` + `{` + inner + `}`
            length: 2 + end + 1,
        });
    }

    let name: String = rest
        .chars()
        .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
        .collect();
    if !is_name(&name) {
        return None;
    }
    let length = 1 + name.len();
    Some(Reference {
        name,
        default: None,
        length,
    })
}

/// Whether `name` is a usable variable name: non-empty, starting with a letter
/// or `_`, and otherwise letters, digits and `_`.
fn is_name(name: &str) -> bool {
    let mut chars = name.chars();
    match chars.next() {
        Some(first) if first.is_ascii_alphabetic() || first == '_' => {}
        _ => return false,
    }
    chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// The value a reference stands for.
///
/// An unset variable with no default expands to the empty string and warns.
/// The alternative — leaving `$DATABASE_URL` in place — hands the literal text
/// to whatever consumes it, and fails much later and much less clearly.
fn resolve(reference: &Reference, source: &str) -> String {
    match std::env::var(&reference.name) {
        // An empty variable takes the default too: `${PORT:-5432}` with
        // `PORT=""` means the same thing as `PORT` being unset, and this is the
        // reading that makes `export PORT=` harmless.
        Ok(value) if !value.is_empty() => value,
        _ => match &reference.default {
            Some(default) => default.clone(),
            None => {
                tracing::warn!(
                    variable = %reference.name,
                    file = source,
                    "environment variable is not set; using an empty string \
                     (write ${{{}:-…}} to give a default)",
                    reference.name
                );
                String::new()
            }
        },
    }
}

/// Load `<root>/.env` into the process environment, if the file is there.
///
/// This is the other half of `$VAR` in a TOML file: the file says where a
/// secret comes from, and in development `.env` is where it comes from. It is
/// read once, before any app file, so `url = "$DATABASE_URL"` resolves the same
/// whether the variable was exported by a shell, a container or this file.
///
/// **A variable already set wins.** A deployment that exports `DATABASE_URL`
/// means it, and a `.env` that happened to ship in the image must not quietly
/// replace it. That also makes `FOO=x apiplant serve` work the way every other
/// program has taught people to expect.
///
/// The format is the usual one: `KEY=value` a line, `#` comments, blank lines
/// ignored, an optional `export ` prefix. A value may be quoted — `'…'` is
/// literal, `"…"` understands `\n`, `\t`, `\"` and `\\` — and an unquoted value
/// is trimmed and ends at a ` #` comment.
///
/// A malformed line is skipped with a warning rather than failing the boot: a
/// stray line in a developer's scratch file is not a reason to refuse to start,
/// and the warning names the line.
pub fn load_dotenv(root: &std::path::Path) {
    let path = root.join(".env");
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        // Not having one is the normal case in production.
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return,
        Err(error) => {
            tracing::warn!(file = %path.display(), %error, "could not read .env");
            return;
        }
    };

    let mut loaded = 0usize;
    for (number, line) in text.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let line = line.strip_prefix("export ").unwrap_or(line).trim_start();
        let Some((name, value)) = line.split_once('=') else {
            tracing::warn!(
                file = %path.display(),
                line = number + 1,
                "ignoring a .env line with no `=`"
            );
            continue;
        };
        let name = name.trim();
        if !is_name(name) {
            tracing::warn!(
                file = %path.display(),
                line = number + 1,
                "ignoring a .env line whose name is not a variable name"
            );
            continue;
        }
        if std::env::var_os(name).is_some() {
            continue;
        }
        // SAFETY-adjacent: this runs during load, before the server spawns the
        // threads that would make a concurrent `setenv` a problem.
        std::env::set_var(name, dotenv_value(value));
        loaded += 1;
    }
    if loaded > 0 {
        tracing::info!(file = %path.display(), variables = loaded, "loaded .env");
    }
}

/// The value half of a `.env` line, unquoted.
fn dotenv_value(raw: &str) -> String {
    let value = raw.trim();
    if let Some(inner) = value.strip_prefix('\'').and_then(|v| v.strip_suffix('\'')) {
        // Single quotes are literal, which is what a password full of
        // backslashes wants.
        return inner.to_string();
    }
    if let Some(inner) = value.strip_prefix('"').and_then(|v| v.strip_suffix('"')) {
        let mut out = String::with_capacity(inner.len());
        let mut chars = inner.chars();
        while let Some(c) = chars.next() {
            if c != '\\' {
                out.push(c);
                continue;
            }
            match chars.next() {
                Some('n') => out.push('\n'),
                Some('r') => out.push('\r'),
                Some('t') => out.push('\t'),
                Some(other) => out.push(other),
                None => out.push('\\'),
            }
        }
        return out;
    }
    // Unquoted: a trailing ` #` starts a comment. Requiring the space keeps a
    // `#` inside a value — a URL fragment, a generated password — intact.
    match value.split_once(" #") {
        Some((before, _)) => before.trim_end().to_string(),
        None => value.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The process environment is shared by every test in the binary, and
    /// cargo runs them on threads, so tests that set variables have to take
    /// turns — otherwise one test's cleanup unsets another's variable.
    static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    /// Set variables for one test, and remove them afterwards even if it
    /// fails. Holds [`ENV_LOCK`] for its lifetime, so only construct one at a
    /// time within a test.
    struct Vars {
        pairs: &'static [(&'static str, &'static str)],
        _guard: std::sync::MutexGuard<'static, ()>,
    }

    impl Vars {
        fn set(pairs: &'static [(&'static str, &'static str)]) -> Vars {
            // A test that fails while holding the lock poisons it; the
            // remaining tests still want their turn.
            let guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
            for (name, value) in pairs {
                std::env::set_var(name, value);
            }
            Vars {
                pairs,
                _guard: guard,
            }
        }
    }

    impl Drop for Vars {
        fn drop(&mut self) {
            for (name, _) in self.pairs {
                std::env::remove_var(name);
            }
        }
    }

    fn expanded(text: &str) -> String {
        expand(text, "test.toml").into_owned()
    }

    #[test]
    fn both_spellings_of_a_reference_expand() {
        let _vars = Vars::set(&[("APIPLANT_T_HOST", "db.example.com")]);

        assert_eq!(expanded("$APIPLANT_T_HOST"), "db.example.com");
        assert_eq!(expanded("${APIPLANT_T_HOST}"), "db.example.com");
        // A bare `$VAR` ends where the name does, so it can be embedded.
        assert_eq!(
            expanded("postgres://$APIPLANT_T_HOST:5432/db"),
            "postgres://db.example.com:5432/db"
        );
        // …and braces are how you butt a reference against a name character.
        assert_eq!(expanded("${APIPLANT_T_HOST}_1"), "db.example.com_1");
    }

    #[test]
    fn several_references_expand_in_one_string() {
        let _vars = Vars::set(&[
            ("APIPLANT_T_USER", "user01"),
            ("APIPLANT_T_PASS", "veryToughPas$w0rd"),
            ("APIPLANT_T_HOST", "some-host.tld"),
            ("APIPLANT_T_NAME", "my_database"),
        ]);

        assert_eq!(
            expanded(
                "mysql://$APIPLANT_T_USER:$APIPLANT_T_PASS@$APIPLANT_T_HOST:\
                 ${APIPLANT_T_PORT:-3306}/$APIPLANT_T_NAME"
            ),
            "mysql://user01:veryToughPas$w0rd@some-host.tld:3306/my_database"
        );
    }

    #[test]
    fn a_default_covers_an_unset_or_empty_variable() {
        let _vars = Vars::set(&[("APIPLANT_T_EMPTY", ""), ("APIPLANT_T_REGION", "eu-west-1")]);

        assert_eq!(expanded("${APIPLANT_T_UNSET:-us-east-1}"), "us-east-1");
        // `export VAR=` should behave like "not set", or a blank in the
        // environment would silently defeat the default.
        assert_eq!(expanded("${APIPLANT_T_EMPTY:-us-east-1}"), "us-east-1");
        // A default may be empty, and may contain anything but `}`.
        assert_eq!(expanded("${APIPLANT_T_UNSET:-}"), "");
        assert_eq!(
            expanded("${APIPLANT_T_UNSET:-postgres://a:b@c/d}"),
            "postgres://a:b@c/d"
        );

        assert_eq!(expanded("${APIPLANT_T_REGION:-us-east-1}"), "eu-west-1");
    }

    #[test]
    fn an_unset_variable_without_a_default_expands_to_nothing() {
        assert_eq!(expanded("$APIPLANT_T_MISSING"), "");
        assert_eq!(expanded("a${APIPLANT_T_MISSING}b"), "ab");
    }

    /// The escape, and — more importantly — the far more common case of a `$`
    /// that was never meant as a reference at all.
    #[test]
    fn dollars_that_are_not_references_survive() {
        assert_eq!(expanded("$$19.99"), "$19.99");
        assert_eq!(expanded("$$"), "$");
        assert_eq!(expanded("$$$$"), "$$");

        // No escape needed: none of these can be read as a name.
        assert_eq!(expanded("$19.99"), "$19.99");
        assert_eq!(expanded("100 US$"), "100 US$");
        assert_eq!(expanded("a $ b"), "a $ b");
        assert_eq!(expanded("${unterminated"), "${unterminated");
        assert_eq!(expanded("${}"), "${}");
        assert_eq!(expanded("${1BAD}"), "${1BAD}");
    }

    #[test]
    fn text_without_a_dollar_is_returned_untouched() {
        assert!(matches!(
            expand("postgres://localhost/db", "test.toml"),
            Cow::Borrowed(_)
        ));
        assert_eq!(expanded(""), "");
    }

    #[test]
    fn a_document_is_expanded_through_tables_and_arrays() {
        let _vars = Vars::set(&[
            ("APIPLANT_T_DOC_URL", "postgres://db/app"),
            ("APIPLANT_T_DOC_ORIGIN", "https://example.com"),
        ]);

        let mut document: toml::Value = toml::from_str(
            r#"
            title = "no references here"
            port = 5432

            [database]
            url = "$APIPLANT_T_DOC_URL"

            [server]
            origins = ["$APIPLANT_T_DOC_ORIGIN", "http://localhost:3000"]

            [[hooks]]
            target = "${APIPLANT_T_DOC_ORIGIN}/hook"
            "#,
        )
        .unwrap();
        expand_document(&mut document, "main.toml");

        assert_eq!(
            document["database"]["url"].as_str(),
            Some("postgres://db/app")
        );
        assert_eq!(
            document["server"]["origins"][0].as_str(),
            Some("https://example.com")
        );
        assert_eq!(
            document["server"]["origins"][1].as_str(),
            Some("http://localhost:3000")
        );
        assert_eq!(
            document["hooks"][0]["target"].as_str(),
            Some("https://example.com/hook")
        );
        // Non-strings are left alone, and so is a string with nothing to do.
        assert_eq!(document["port"].as_integer(), Some(5432));
        assert_eq!(document["title"].as_str(), Some("no references here"));
    }

    /// The reason expansion happens on the parsed document rather than on the
    /// file's text: a value that looks like TOML must stay a value.
    #[test]
    fn an_expanded_value_cannot_inject_toml() {
        let _vars = Vars::set(&[("APIPLANT_T_EVIL", "\"\nadmin = true\n[x]\ny = \"")]);

        let mut document: toml::Value = toml::from_str(r#"password = "$APIPLANT_T_EVIL""#).unwrap();
        expand_document(&mut document, "main.toml");

        // One string value, exactly as the variable had it — no new keys.
        assert_eq!(
            document["password"].as_str(),
            Some("\"\nadmin = true\n[x]\ny = \"")
        );
        assert_eq!(document.as_table().unwrap().len(), 1);
    }

    /// Keys name the shape of the file, and the shape is the author's, not the
    /// deployment's.
    #[test]
    fn keys_are_not_expanded() {
        let _vars = Vars::set(&[("APIPLANT_T_KEY", "surprise")]);

        let mut document: toml::Value = toml::from_str(r#"'$APIPLANT_T_KEY' = "value""#).unwrap();
        expand_document(&mut document, "main.toml");

        assert!(document.get("$APIPLANT_T_KEY").is_some());
        assert!(document.get("surprise").is_none());
    }

    #[test]
    fn a_dotenv_file_fills_in_unset_variables_only() {
        let _vars = Vars::set(&[("APIPLANT_T_ALREADY", "from the shell")]);
        let dir = std::env::temp_dir().join(format!("apiplant-dotenv-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join(".env"),
            "# a comment\n\n\
             export APIPLANT_T_PLAIN=hello\n\
             APIPLANT_T_SPACED =  spaced out  # trailing comment\n\
             APIPLANT_T_SINGLE='raw \\n value'\n\
             APIPLANT_T_DOUBLE=\"line\\nbreak\"\n\
             APIPLANT_T_ALREADY=from the file\n\
             not a variable line\n",
        )
        .unwrap();

        load_dotenv(&dir);

        assert_eq!(std::env::var("APIPLANT_T_PLAIN").unwrap(), "hello");
        assert_eq!(std::env::var("APIPLANT_T_SPACED").unwrap(), "spaced out");
        assert_eq!(std::env::var("APIPLANT_T_SINGLE").unwrap(), "raw \\n value");
        assert_eq!(std::env::var("APIPLANT_T_DOUBLE").unwrap(), "line\nbreak");
        // The environment wins: a deployment that exported it meant it.
        assert_eq!(
            std::env::var("APIPLANT_T_ALREADY").unwrap(),
            "from the shell"
        );

        for name in [
            "APIPLANT_T_PLAIN",
            "APIPLANT_T_SPACED",
            "APIPLANT_T_SINGLE",
            "APIPLANT_T_DOUBLE",
        ] {
            std::env::remove_var(name);
        }
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// No `.env` is the normal case in production, and must be silent.
    #[test]
    fn a_missing_dotenv_is_not_an_error() {
        load_dotenv(std::path::Path::new("/nonexistent/apiplant/app"));
    }
}
