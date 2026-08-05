//! Runtime migrations.
//!
//! apiplant has no hand-written migration files: the resource schemas *are* the
//! desired state. On boot the migrator makes the database match them —
//! idempotently. It creates missing tables and adds missing columns (an
//! additive strategy that is safe to run on every start). Destructive changes
//! (dropping/retyping columns) are intentionally left to the operator.

use apiplant_core::schema::Field;
use apiplant_core::{App, FieldType, Resource};
use sea_orm::{ConnectionTrait, DatabaseBackend, Statement};
use std::collections::HashSet;

use crate::ident::quote_ident;
use crate::Error;

/// Bring the database in line with every resource in the app.
///
/// Three additive passes, all idempotent: create missing tables, add missing
/// columns, then add missing foreign-key constraints for `reference` fields.
/// FKs come last so every referenced table already exists.
pub async fn migrate(conn: &impl ConnectionTrait, app: &App) -> Result<(), Error> {
    for resource in app.resources_in_dependency_order() {
        create_table_if_absent(conn, resource).await?;
        add_missing_columns(conn, resource).await?;
    }
    for resource in app.resources_in_dependency_order() {
        add_foreign_keys(conn, resource, app).await?;
    }
    Ok(())
}

/// Add an FK constraint for each `reference` field, if not already present.
async fn add_foreign_keys(
    conn: &impl ConnectionTrait,
    r: &Resource,
    app: &App,
) -> Result<(), Error> {
    let table = quote_ident(&r.table_name())?;
    for reference in r.references() {
        let Some(target) = app.resources.get(&reference.target) else {
            tracing::warn!(
                resource = %r.meta.name,
                field = %reference.field,
                target = %reference.target,
                "reference points at an unknown resource; skipping FK"
            );
            continue;
        };
        // Deterministic name lets us skip re-adding it on the next boot.
        let constraint = format!("fk_{}_{}", r.table_name(), reference.field);
        if constraint_exists(conn, &constraint).await? {
            continue;
        }
        let sql = format!(
            "ALTER TABLE {table} ADD CONSTRAINT {con} \
             FOREIGN KEY ({col}) REFERENCES {target_tbl}(\"id\") ON DELETE {action}",
            con = quote_ident(&constraint)?,
            col = quote_ident(&reference.field)?,
            target_tbl = quote_ident(&target.table_name())?,
            action = reference.on_delete.to_sql(),
        );
        conn.execute(Statement::from_string(DatabaseBackend::Postgres, sql))
            .await?;
        tracing::info!(
            resource = %r.meta.name,
            field = %reference.field,
            target = %reference.target,
            "migrated: added foreign key"
        );
    }
    Ok(())
}

async fn constraint_exists(conn: &impl ConnectionTrait, name: &str) -> Result<bool, Error> {
    let stmt = Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        "SELECT 1 FROM pg_constraint WHERE conname = $1 LIMIT 1",
        [name.into()],
    );
    Ok(conn.query_one(stmt).await?.is_some())
}

/// SQL column type for a field, honouring `max_length` on strings.
fn column_type(field: &Field) -> String {
    match field.ty {
        FieldType::String => match field.max_length {
            Some(n) => format!("varchar({n})"),
            None => "varchar".to_string(),
        },
        // A file field holds the URL it is served from — long enough for a
        // signed CDN link, short enough to stay indexable.
        FieldType::File => format!("varchar({})", field.max_length.unwrap_or(1024)),
        FieldType::Text => "text".to_string(),
        FieldType::Integer => "integer".to_string(),
        FieldType::BigInt => "bigint".to_string(),
        FieldType::Float => "double precision".to_string(),
        FieldType::Boolean => "boolean".to_string(),
        FieldType::Uuid | FieldType::Reference => "uuid".to_string(),
        FieldType::Timestamp => "timestamptz".to_string(),
        FieldType::Json => "jsonb".to_string(),
    }
}

/// A `DEFAULT <literal>` clause for a field's declared default, or empty.
fn default_clause(field: &Field) -> String {
    let Some(v) = &field.default else {
        return String::new();
    };
    match v {
        serde_json::Value::Bool(b) => format!(" DEFAULT {b}"),
        serde_json::Value::Number(n) => format!(" DEFAULT {n}"),
        serde_json::Value::String(s) => format!(" DEFAULT '{}'", s.replace('\'', "''")),
        _ => String::new(),
    }
}

async fn create_table_if_absent(conn: &impl ConnectionTrait, r: &Resource) -> Result<(), Error> {
    let table = quote_ident(&r.table_name())?;
    let mut cols: Vec<String> = vec![format!(
        "{} uuid PRIMARY KEY DEFAULT gen_random_uuid()",
        quote_ident("id")?
    )];

    for (name, field) in &r.fields {
        let mut col = format!("{} {}", quote_ident(name)?, column_type(field));
        col.push_str(&default_clause(field));
        if field.required {
            col.push_str(" NOT NULL");
        }
        if field.unique {
            col.push_str(" UNIQUE");
        }
        cols.push(col);
    }

    if r.meta.timestamps {
        cols.push(format!(
            "{} timestamptz NOT NULL DEFAULT now()",
            quote_ident("created_at")?
        ));
        cols.push(format!(
            "{} timestamptz NOT NULL DEFAULT now()",
            quote_ident("updated_at")?
        ));
    }

    let sql = format!("CREATE TABLE IF NOT EXISTS {table} ({})", cols.join(", "));
    conn.execute(Statement::from_string(DatabaseBackend::Postgres, sql))
        .await?;
    tracing::debug!(table = %r.table_name(), "ensured table");
    Ok(())
}

async fn add_missing_columns(conn: &impl ConnectionTrait, r: &Resource) -> Result<(), Error> {
    let existing = existing_columns(conn, &r.table_name()).await?;
    let table = quote_ident(&r.table_name())?;

    for (name, field) in &r.fields {
        if existing.contains(name.as_str()) {
            continue;
        }
        // New column on an existing table: apply its default but never NOT NULL
        // without one, or the ALTER would fail on populated tables.
        let sql = format!(
            "ALTER TABLE {table} ADD COLUMN IF NOT EXISTS {} {}{}",
            quote_ident(name)?,
            column_type(field),
            default_clause(field),
        );
        conn.execute(Statement::from_string(DatabaseBackend::Postgres, sql))
            .await?;
        tracing::info!(table = %r.table_name(), column = %name, "migrated: added column");
    }
    Ok(())
}

async fn existing_columns(
    conn: &impl ConnectionTrait,
    table: &str,
) -> Result<HashSet<String>, Error> {
    let stmt = Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        "SELECT column_name FROM information_schema.columns \
         WHERE table_schema = current_schema() AND table_name = $1",
        [table.into()],
    );
    let rows = conn.query_all(stmt).await?;
    let mut set = HashSet::new();
    for row in rows {
        set.insert(row.try_get::<String>("", "column_name")?);
    }
    Ok(set)
}

#[cfg(test)]
mod tests {
    use super::*;
    use apiplant_core::schema::OnDelete;

    fn field(ty: FieldType) -> Field {
        Field {
            ty,
            references: None,
            required: false,
            unique: false,
            hidden: false,
            default: None,
            max_length: None,
            on_delete: Some(OnDelete::Restrict),
            admin: Default::default(),
        }
    }

    #[test]
    fn column_type_honours_max_length_and_json_types() {
        let mut string = field(FieldType::String);
        string.max_length = Some(320);
        assert_eq!(column_type(&string), "varchar(320)");

        assert_eq!(column_type(&field(FieldType::Reference)), "uuid");
        assert_eq!(column_type(&field(FieldType::Json)), "jsonb");
        assert_eq!(column_type(&field(FieldType::Timestamp)), "timestamptz");
    }

    #[test]
    fn default_clause_renders_scalars_and_escapes_strings() {
        let mut text = field(FieldType::String);
        text.default = Some(serde_json::json!("O'Hara"));
        assert_eq!(default_clause(&text), " DEFAULT 'O''Hara'");

        let mut number = field(FieldType::Integer);
        number.default = Some(serde_json::json!(42));
        assert_eq!(default_clause(&number), " DEFAULT 42");

        let mut structured = field(FieldType::Json);
        structured.default = Some(serde_json::json!({ "nested": true }));
        assert_eq!(default_clause(&structured), "");
    }
}
