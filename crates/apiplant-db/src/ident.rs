//! SQL identifier quoting.
//!
//! Table and column names come from developer-authored TOML, so they are
//! *trusted-ish* — but we still refuse anything that isn't a plain identifier
//! and always double-quote, so a stray character can never break out into
//! injectable SQL. Bound values always use `$n` parameters (see `crud`).

use crate::Error;

/// Validate and double-quote an identifier. Errors on anything outside
/// `[A-Za-z_][A-Za-z0-9_]*`.
pub fn quote_ident(name: &str) -> Result<String, Error> {
    let mut chars = name.chars();
    let ok_start = chars
        .next()
        .map(|c| c.is_ascii_alphabetic() || c == '_')
        .unwrap_or(false);
    let ok_rest = name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_');
    if !ok_start || !ok_rest {
        return Err(Error::Schema(format!("invalid SQL identifier: `{name}`")));
    }
    Ok(format!("\"{name}\""))
}
