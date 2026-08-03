//! Seed data: an app's `seed/` directory, loaded into the database.
//!
//! A seed directory holds one file per resource, named after it —
//! `seed/organization.toml`, `seed/user.toml`, `seed/product.csv` — whose rows
//! become that resource's rows. It is the fixture an app starts life with: an
//! administrator who can sign in, the organisation they administer, and enough
//! underneath for the dashboard to have something to show.
//!
//! TOML is the primary format, because it is the format the app is already
//! written in — a seed file looks like the model beside it:
//!
//! ```toml
//! [[row]]
//! id = "acme"
//! name = "Acme, Inc."
//! slug = "acme"
//! ```
//!
//! CSV is accepted for the same job at a hundred rows, where a header line and
//! a column per field says it better than a hundred `[[row]]` headers — and
//! because it is what a spreadsheet or a `COPY … TO` exports.
//!
//! Three things make either enough, without a migration format or a script per
//! app:
//!
//! * **Aliases instead of UUIDs.** Anywhere an id is expected — the `id`
//!   column, or any `reference` field — a value that is not a UUID is taken as
//!   a name and hashed into one ([`uuid_for`]). `acme` means the same row in
//!   every file, so `seed/membership.toml` can say `organization_id = "acme"`
//!   without anyone minting a UUID by hand.
//! * **Idempotence.** Because those ids are derived rather than random,
//!   inserting is `ON CONFLICT DO NOTHING`: seeding twice inserts once, and a
//!   seed file that grew a row since the last run adds only that row. Rows
//!   already present are never overwritten — someone who edited the fixture in
//!   the dashboard keeps their edit.
//! * **Passwords.** A `password` column on a resource that declares one
//!   ([`AuthSpec`](apiplant_core::schema::AuthSpec)) is hashed into the
//!   password field, so the seeded administrator can actually sign in and the
//!   file stays readable.
//!
//! Seeding runs in dependency order, so a file may reference rows from a file
//! it is listed before.

use apiplant_core::schema::FieldType;
use apiplant_core::{App, Resource};
use sea_orm::sea_query::Value as SqlValue;
use sea_orm::{ConnectionTrait, DatabaseBackend, Statement};
use serde_json::Value as Json;
use std::path::{Path, PathBuf};
use uuid::Uuid;

use crate::ident::quote_ident;
use crate::{value, Error};

/// What one seed file did.
#[derive(Debug, Clone)]
pub struct FileReport {
    pub resource: String,
    /// Rows the database did not already have.
    pub inserted: u64,
    /// Rows whose id was already there, and were therefore left alone.
    pub skipped: u64,
}

/// What a whole `seed/` directory did.
#[derive(Debug, Clone, Default)]
pub struct Report {
    pub files: Vec<FileReport>,
}

impl Report {
    pub fn inserted(&self) -> u64 {
        self.files.iter().map(|f| f.inserted).sum()
    }

    pub fn skipped(&self) -> u64 {
        self.files.iter().map(|f| f.skipped).sum()
    }

    /// True when there was no `seed/` directory, or nothing in it.
    pub fn is_empty(&self) -> bool {
        self.files.is_empty()
    }
}

/// Load `<app>/seed/` into the database.
///
/// A missing directory is not an error — most apps have no fixture — but a file
/// naming a resource the app does not define is, because a typo that silently
/// seeds nothing is the whole failure mode this is meant to avoid.
pub async fn seed(conn: &impl ConnectionTrait, app: &App) -> Result<Report, Error> {
    seed_dir(conn, app, &app.root.join("seed")).await
}

/// Load a specific directory of seed files, for an app whose fixtures live
/// somewhere other than `seed/`.
pub async fn seed_dir(conn: &impl ConnectionTrait, app: &App, dir: &Path) -> Result<Report, Error> {
    if !dir.is_dir() {
        return Ok(Report::default());
    }

    // Collect `<resource>.{toml,csv}`, and refuse the ones that name nothing.
    let mut files: Vec<(String, PathBuf)> = Vec::new();
    let entries = std::fs::read_dir(dir)
        .map_err(|e| Error::Schema(format!("cannot read {}: {e}", dir.display())))?;
    for entry in entries {
        let path = entry
            .map_err(|e| Error::Schema(format!("cannot read {}: {e}", dir.display())))?
            .path();
        match path.extension().and_then(|e| e.to_str()) {
            Some("toml") | Some("csv") => {}
            _ => continue,
        }
        let name = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or_default()
            .to_string();
        if !app.resources.contains_key(&name) {
            return Err(Error::Schema(format!(
                "{}: no resource named `{name}` — a seed file is named after the \
                 resource it fills",
                path.display()
            )));
        }
        if let Some((_, other)) = files.iter().find(|(existing, _)| existing == &name) {
            return Err(Error::Schema(format!(
                "{name} is seeded twice, by {} and {} — one file per resource",
                other.display(),
                path.display()
            )));
        }
        files.push((name, path));
    }

    // Parents before children, so a reference always finds its row.
    let mut report = Report::default();
    for resource in app.resources_in_dependency_order() {
        let Some((_, path)) = files.iter().find(|(name, _)| name == &resource.meta.name) else {
            continue;
        };
        let file = seed_file(conn, resource, path).await?;
        tracing::info!(
            resource = %file.resource,
            inserted = file.inserted,
            skipped = file.skipped,
            "seeded"
        );
        report.files.push(file);
    }
    Ok(report)
}

/// One row on its way into the database: columns in file order, each value
/// still in the shape its format produced.
type Row = Vec<(String, Raw)>;

/// A value as it was written, before it knows what column it is for.
#[derive(Debug, Clone)]
enum Raw {
    /// From CSV: a string that the column's type will parse.
    Text(String),
    /// From TOML: already typed.
    Typed(Json),
}

/// Seed one resource from its file.
async fn seed_file(
    conn: &impl ConnectionTrait,
    r: &Resource,
    path: &Path,
) -> Result<FileReport, Error> {
    let origin = path.display().to_string();
    let text = std::fs::read_to_string(path)
        .map_err(|e| Error::Schema(format!("cannot read {origin}: {e}")))?;
    let rows = if path.extension().and_then(|e| e.to_str()) == Some("csv") {
        csv_rows(&text)
    } else {
        toml_rows(&text)
    }
    .map_err(|e| Error::Schema(format!("{origin}: {e}")))?;

    let password_field = r.auth.as_ref().map(|a| a.password_field.clone());
    let table = quote_ident(&r.table_name())?;
    let mut inserted = 0u64;
    let mut skipped = 0u64;

    for (index, row) in rows.into_iter().enumerate() {
        let position = index + 1;
        let mut columns: Vec<String> = Vec::new();
        let mut params: Vec<SqlValue> = Vec::new();
        let mut id = None;

        for (column, raw) in row {
            let known = column == "id"
                || r.fields.contains_key(&column)
                || (column == "password" && password_field.is_some());
            if !known {
                return Err(Error::Schema(format!(
                    "{origin}: row {position}: `{column}` is not a field of `{}`",
                    r.meta.name
                )));
            }
            if column == "id" {
                id = Some(uuid_for(&as_key(&raw).map_err(|e| {
                    Error::Schema(format!("{origin}: row {position}: `id`: {e}"))
                })?));
                continue;
            }
            if Some(column.as_str()) == password_field.as_deref() {
                return Err(Error::Schema(format!(
                    "{origin}: row {position}: set `password` rather than `{column}` — \
                     seeding hashes it"
                )));
            }
            if column == "password" {
                let field = password_field.as_deref().expect("checked just above");
                let plaintext = as_key(&raw)
                    .map_err(|e| Error::Schema(format!("{origin}: row {position}: {e}")))?;
                let hash = apiplant_auth::Authenticator::hash_password_with_argon2(&plaintext)
                    .map_err(|e| Error::Schema(format!("{origin}: row {position}: {e}")))?;
                columns.push(field.to_string());
                params.push(SqlValue::from(hash));
                continue;
            }

            let field = &r.fields[&column];
            let sql = to_sql(field.ty, &raw)
                .map_err(|e| Error::Schema(format!("{origin}: row {position}: `{column}`: {e}")))?;
            let Some(sql) = sql else { continue };
            columns.push(column);
            params.push(sql);
        }

        // Without an explicit id, the row is still given a derived one — from
        // the resource and its position in the file — so that re-running the
        // seed inserts nothing twice.
        let id = id.unwrap_or_else(|| uuid_for(&format!("{}#{position}", r.meta.name)));
        columns.insert(0, "id".to_string());
        params.insert(0, SqlValue::from(id));

        let quoted: Vec<String> = columns
            .iter()
            .map(|c| quote_ident(c))
            .collect::<Result<_, _>>()?;
        let placeholders: Vec<String> = (1..=quoted.len()).map(|n| format!("${n}")).collect();
        let sql = format!(
            "INSERT INTO {table} ({}) VALUES ({}) ON CONFLICT (\"id\") DO NOTHING",
            quoted.join(", "),
            placeholders.join(", ")
        );
        let result = conn
            .execute(Statement::from_sql_and_values(
                DatabaseBackend::Postgres,
                sql,
                params,
            ))
            .await?;
        if result.rows_affected() > 0 {
            inserted += 1;
        } else {
            skipped += 1;
        }
    }

    Ok(FileReport {
        resource: r.meta.name.clone(),
        inserted,
        skipped,
    })
}

/// The string behind a value that has to be one — an id, an alias, a password.
fn as_key(raw: &Raw) -> Result<String, String> {
    match raw {
        Raw::Text(s) => Ok(s.clone()),
        Raw::Typed(Json::String(s)) => Ok(s.clone()),
        Raw::Typed(_) => Err("expected a string".to_string()),
    }
}

/// Convert one written value for one column, or `None` when the row leaves the
/// column out and the database's own default should apply.
fn to_sql(ty: FieldType, raw: &Raw) -> Result<Option<SqlValue>, String> {
    // A reference or an id is written as the alias its target row uses, in
    // either format.
    if matches!(ty, FieldType::Reference | FieldType::Uuid) {
        return Ok(Some(SqlValue::from(uuid_for(&as_key(raw)?))));
    }
    Ok(Some(match raw {
        Raw::Text(s) if s.is_empty() => return Ok(None),
        // CSV has one type; the column says what it means.
        Raw::Text(s) if ty == FieldType::Json => {
            SqlValue::from(serde_json::from_str::<Json>(s).map_err(|e| format!("not JSON: {e}"))?)
        }
        Raw::Text(s) => value::string_to_sql(ty, s)?,
        Raw::Typed(Json::Null) => return Ok(None),
        // TOML is typed, but a number written for a text column (a postcode, a
        // version) is a spelling, not a mistake — so a scalar is accepted
        // wherever a string is wanted.
        Raw::Typed(v) if matches!(ty, FieldType::String | FieldType::Text) && !v.is_string() => {
            match v {
                Json::Object(_) | Json::Array(_) => return Err("expected a string".to_string()),
                other => SqlValue::from(other.to_string()),
            }
        }
        Raw::Typed(v) => value::json_to_sql(ty, v)?,
    }))
}

/// Parse a TOML seed file: an array of `[[row]]` tables.
///
/// A datetime is handed on as its RFC 3339 text, which is what a `timestamp`
/// column parses — so a seed file may write a bare TOML datetime and does not
/// have to quote it.
fn toml_rows(text: &str) -> Result<Vec<Row>, String> {
    let doc: toml::Value = toml::from_str(text).map_err(|e| e.to_string())?;
    let table = doc
        .as_table()
        .ok_or("expected a table of `[[row]]` entries")?;
    for key in table.keys() {
        if key != "row" {
            return Err(format!(
                "`{key}` is not `row` — a seed file is a list of `[[row]]` tables"
            ));
        }
    }
    let Some(rows) = table.get("row") else {
        return Ok(Vec::new());
    };
    let rows = rows
        .as_array()
        .ok_or("`row` must be written as `[[row]]` tables")?;

    rows.iter()
        .enumerate()
        .map(|(index, row)| {
            let row = row
                .as_table()
                .ok_or_else(|| format!("row {} is not a table", index + 1))?;
            Ok(row
                .iter()
                .map(|(k, v)| (k.clone(), Raw::Typed(toml_to_json(v))))
                .collect())
        })
        .collect()
}

/// TOML's values as JSON's, which is the shape the column converters take.
fn toml_to_json(v: &toml::Value) -> Json {
    match v {
        toml::Value::String(s) => Json::String(s.clone()),
        toml::Value::Integer(i) => Json::from(*i),
        toml::Value::Float(f) => Json::from(*f),
        toml::Value::Boolean(b) => Json::Bool(*b),
        // RFC 3339 text, which is what a timestamp column reads.
        toml::Value::Datetime(d) => Json::String(d.to_string()),
        toml::Value::Array(items) => Json::Array(items.iter().map(toml_to_json).collect()),
        toml::Value::Table(t) => Json::Object(
            t.iter()
                .map(|(k, v)| (k.clone(), toml_to_json(v)))
                .collect(),
        ),
    }
}

/// Parse a CSV seed file into the same rows a TOML one produces: the header
/// names the columns, and each record pairs with it.
fn csv_rows(text: &str) -> Result<Vec<Row>, String> {
    let records = parse_csv(text)?;
    let mut records = records.into_iter();
    let Some(header) = records.next() else {
        return Ok(Vec::new());
    };
    let header: Vec<String> = header
        .into_iter()
        .map(|c| c.text.trim().to_string())
        .collect();

    records
        .enumerate()
        .map(|(index, record)| {
            if record.len() > header.len() {
                return Err(format!(
                    "row {}: {} values for {} columns",
                    index + 1,
                    record.len(),
                    header.len()
                ));
            }
            Ok(header
                .iter()
                .cloned()
                .zip(record)
                // An empty, unquoted cell is "no value" — the column is left
                // out and the database's own default (or NULL) applies. `""`
                // is an empty string, which is a different thing and sometimes
                // what is wanted.
                .filter(|(_, cell)| !cell.text.is_empty() || cell.quoted)
                .map(|(column, cell)| (column, Raw::Text(cell.text)))
                .collect())
        })
        .collect()
}

/// The id a seed file's key stands for.
///
/// A real UUID is itself, so a fixture may pin an id exactly. Anything else is
/// a name — `acme`, `admin`, `widget-blue` — hashed into a UUID, the same one
/// every time and on every machine. That determinism is what makes seeding
/// idempotent and lets one file point at another's rows by a readable word.
pub fn uuid_for(key: &str) -> Uuid {
    if let Ok(uuid) = Uuid::parse_str(key) {
        return uuid;
    }
    let digest = apiplant_auth::Authenticator::hash_api_key(&format!("apiplant-seed:{key}"));
    let mut bytes = [0u8; 16];
    for (i, byte) in bytes.iter_mut().enumerate() {
        // The digest is hex, so two characters per byte.
        *byte = u8::from_str_radix(&digest[i * 2..i * 2 + 2], 16).unwrap_or(0);
    }
    // Version 8 (custom) with the RFC 4122 variant: honest about being derived
    // rather than random, and still a well-formed UUID to everything that
    // looks at one.
    bytes[6] = (bytes[6] & 0x0f) | 0x80;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    Uuid::from_bytes(bytes)
}

/// One CSV cell, and whether it arrived quoted.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Cell {
    text: String,
    quoted: bool,
}

/// Parse RFC 4180 CSV: commas separate, `"` quotes, `""` is a literal quote
/// inside a quoted field, and a quoted field may span lines.
///
/// Two departures, both for files people edit by hand: a line whose first
/// character is `#` is a comment, and a blank line is nothing at all.
fn parse_csv(text: &str) -> Result<Vec<Vec<Cell>>, String> {
    let mut rows: Vec<Vec<Cell>> = Vec::new();
    let mut row: Vec<Cell> = Vec::new();
    let mut cell = String::new();
    let mut quoted = false;
    let mut in_quotes = false;
    // Only meaningful at the very start of a line, which is where the comment
    // and blank-line rules apply.
    let mut at_line_start = true;
    let mut chars = text.chars().peekable();

    while let Some(c) = chars.next() {
        if at_line_start {
            if c == '#' {
                for c in chars.by_ref() {
                    if c == '\n' {
                        break;
                    }
                }
                continue;
            }
            if c == '\n' {
                continue;
            }
            if c == '\r' && chars.peek() == Some(&'\n') {
                chars.next();
                continue;
            }
            at_line_start = false;
        }

        if in_quotes {
            if c == '"' {
                if chars.peek() == Some(&'"') {
                    chars.next();
                    cell.push('"');
                } else {
                    in_quotes = false;
                }
            } else {
                cell.push(c);
            }
            continue;
        }

        match c {
            '"' if cell.is_empty() => {
                in_quotes = true;
                quoted = true;
            }
            '"' => return Err("a quote may only open a field".to_string()),
            ',' => row.push(Cell {
                text: std::mem::take(&mut cell),
                quoted: std::mem::take(&mut quoted),
            }),
            '\r' if chars.peek() == Some(&'\n') => {}
            '\n' => {
                row.push(Cell {
                    text: std::mem::take(&mut cell),
                    quoted: std::mem::take(&mut quoted),
                });
                rows.push(std::mem::take(&mut row));
                at_line_start = true;
            }
            _ => cell.push(c),
        }
    }

    if in_quotes {
        return Err("a quoted field was never closed".to_string());
    }
    // A file not ending in a newline still ends in a row.
    if !cell.is_empty() || quoted || !row.is_empty() {
        row.push(Cell { text: cell, quoted });
        rows.push(row);
    }
    Ok(rows)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cells(row: &[Cell]) -> Vec<&str> {
        row.iter().map(|c| c.text.as_str()).collect()
    }

    fn column<'a>(row: &'a Row, name: &str) -> &'a Raw {
        &row.iter().find(|(c, _)| c == name).expect("column").1
    }

    #[test]
    fn parses_quotes_commas_and_newlines() {
        let rows = parse_csv("a,b\n1,\"two, and\"\n\"line\nbreak\",\"say \"\"hi\"\"\"\n").unwrap();
        assert_eq!(cells(&rows[0]), ["a", "b"]);
        assert_eq!(cells(&rows[1]), ["1", "two, and"]);
        assert_eq!(cells(&rows[2]), ["line\nbreak", "say \"hi\""]);
    }

    #[test]
    fn comments_and_blank_lines_are_skipped() {
        let rows = parse_csv("# a note\nname\n\nacme\n").unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(cells(&rows[1]), ["acme"]);
    }

    #[test]
    fn an_empty_csv_cell_is_left_out_but_an_empty_string_is_not() {
        let rows = csv_rows("a,b\n,\"\"\n").unwrap();
        assert!(rows[0].iter().all(|(c, _)| c != "a"));
        assert!(matches!(column(&rows[0], "b"), Raw::Text(s) if s.is_empty()));
    }

    #[test]
    fn a_final_row_without_a_newline_still_counts() {
        assert_eq!(parse_csv("a\n1").unwrap().len(), 2);
    }

    #[test]
    fn an_unterminated_quote_is_an_error() {
        assert!(parse_csv("a\n\"oops\n").is_err());
    }

    #[test]
    fn toml_rows_are_read_in_order_with_their_types() {
        let rows = toml_rows(
            r#"
            [[row]]
            id = "acme"
            name = "Acme, Inc."
            seats = 12
            active = true

            [[row]]
            id = "globex"
            name = "Globex"
            "#,
        )
        .unwrap();
        assert_eq!(rows.len(), 2);
        assert!(matches!(
            column(&rows[0], "seats"),
            Raw::Typed(Json::Number(_))
        ));
        assert!(matches!(
            column(&rows[0], "active"),
            Raw::Typed(Json::Bool(true))
        ));
        assert!(matches!(column(&rows[1], "id"), Raw::Typed(Json::String(s)) if s == "globex"));
    }

    #[test]
    fn a_toml_file_that_is_not_rows_says_so() {
        let err = toml_rows("[[organization]]\nname = \"Acme\"\n").unwrap_err();
        assert!(err.contains("`[[row]]`"), "{err}");
    }

    #[test]
    fn a_toml_datetime_becomes_a_timestamp() {
        let rows = toml_rows("[[row]]\nat = 2024-01-31T09:00:00Z\n").unwrap();
        let sql = to_sql(FieldType::Timestamp, column(&rows[0], "at")).unwrap();
        assert!(sql.is_some());
    }

    #[test]
    fn a_number_is_accepted_where_a_string_column_wants_one() {
        let rows = toml_rows("[[row]]\npostcode = 90210\n").unwrap();
        let sql = to_sql(FieldType::String, column(&rows[0], "postcode")).unwrap();
        assert_eq!(sql, Some(SqlValue::from("90210".to_string())));
    }

    #[test]
    fn aliases_are_stable_and_uuids_pass_through() {
        assert_eq!(uuid_for("acme"), uuid_for("acme"));
        assert_ne!(uuid_for("acme"), uuid_for("globex"));
        let explicit = "0f1e2d3c-4b5a-6978-8796-a5b4c3d2e1f0";
        assert_eq!(uuid_for(explicit).to_string(), explicit);
        // Well-formed: version 8, RFC 4122 variant.
        let derived = uuid_for("acme");
        assert_eq!(derived.get_version_num(), 8);
        assert_eq!(derived.as_bytes()[8] & 0xc0, 0x80);
    }
}
