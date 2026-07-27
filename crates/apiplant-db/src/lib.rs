//! # apiplant-db
//!
//! The database layer. It has two jobs:
//!
//! * **Migrations** ([`migrate`]) — make Postgres match the resource schemas.
//! * **CRUD** ([`Db`]) — build parameterised statements for a [`Resource`] at
//!   runtime and hand rows back as plain JSON.
//!
//! Rows come back as JSON by letting Postgres do the conversion (`to_jsonb` /
//! `jsonb_agg`), so the executor only ever extracts a single JSON column and
//! never needs a compile-time entity for a table it only learned about from a
//! TOML file. Values always travel as `$n` bind parameters; only validated,
//! double-quoted identifiers are ever interpolated.

mod ident;
pub mod migrate;
pub mod value;

use apiplant_core::Resource;
use sea_orm::sea_query::Value as SqlValue;
use sea_orm::{
    ConnectOptions, ConnectionTrait, Database, DatabaseBackend, DatabaseConnection, Statement,
};
use uuid::Uuid;

use ident::quote_ident;
pub use migrate::migrate;

/// Database errors.
#[derive(thiserror::Error, Debug)]
pub enum Error {
    #[error("database: {0}")]
    Db(#[from] sea_orm::DbErr),
    #[error("schema: {0}")]
    Schema(String),
    #[error("bad input: {0}")]
    BadInput(String),
}

/// An extra predicate applied to a query: equality (owner/org scoping,
/// `?field=` filters) or membership (`id IN (…)`, e.g. "organisations you belong
/// to"). Column names are always validated and quoted; values are always bound.
#[derive(Clone)]
pub enum Filter {
    /// `column = value`.
    Eq { column: String, value: SqlValue },
    /// `column IN (values…)`. An empty set matches no rows.
    In {
        column: String,
        values: Vec<SqlValue>,
    },
    /// `column ILIKE '%value%'` — a case-insensitive substring match, which is
    /// what a search box means by "search". The pattern's own wildcards are
    /// escaped, so a term containing `%` looks for a per-cent sign.
    Contains { column: String, value: String },
}

impl Filter {
    pub fn eq(column: impl Into<String>, value: impl Into<SqlValue>) -> Self {
        Filter::Eq {
            column: column.into(),
            value: value.into(),
        }
    }

    pub fn in_(column: impl Into<String>, values: Vec<SqlValue>) -> Self {
        Filter::In {
            column: column.into(),
            values,
        }
    }

    /// Convenience: `column IN (…uuids)`.
    pub fn in_uuids(column: impl Into<String>, ids: Vec<Uuid>) -> Self {
        Filter::In {
            column: column.into(),
            values: ids.into_iter().map(SqlValue::from).collect(),
        }
    }

    pub fn contains(column: impl Into<String>, value: impl Into<String>) -> Self {
        Filter::Contains {
            column: column.into(),
            value: value.into(),
        }
    }

    fn column(&self) -> &str {
        match self {
            Filter::Eq { column, .. }
            | Filter::In { column, .. }
            | Filter::Contains { column, .. } => column,
        }
    }
}

/// A connection pool plus the dynamic CRUD executor.
#[derive(Clone)]
pub struct Db {
    conn: DatabaseConnection,
}

impl Db {
    /// Open a pool against the given Postgres URL.
    pub async fn connect(url: &str, max_connections: u32) -> Result<Self, Error> {
        let mut opt = ConnectOptions::new(url.to_owned());
        opt.max_connections(max_connections).sqlx_logging(false);
        let conn = Database::connect(opt).await?;
        Ok(Db { conn })
    }

    /// Access the underlying connection (used by [`migrate`]).
    pub fn connection(&self) -> &DatabaseConnection {
        &self.conn
    }

    // --- CRUD -------------------------------------------------------------

    /// `GET /<resource>` — a JSON array of rows, newest first.
    pub async fn list(
        &self,
        r: &Resource,
        filters: &[Filter],
        limit: i64,
        offset: i64,
    ) -> Result<serde_json::Value, Error> {
        let table = quote_ident(&r.table_name())?;
        let (where_sql, mut params, n) = self.build_where(filters)?;
        let order = if r.meta.timestamps {
            "ORDER BY created_at DESC"
        } else {
            ""
        };
        let limit_ph = format!("${}", n);
        let offset_ph = format!("${}", n + 1);
        params.push(SqlValue::from(limit));
        params.push(SqlValue::from(offset));

        let hidden = self.hidden_subtraction(r)?;
        let sql = format!(
            "SELECT coalesce(jsonb_agg(to_jsonb(t){hidden}), '[]'::jsonb) AS result \
             FROM (SELECT * FROM {table} {where_sql} {order} LIMIT {limit_ph} OFFSET {offset_ph}) t"
        );
        let row = self
            .conn
            .query_one(Statement::from_sql_and_values(
                DatabaseBackend::Postgres,
                sql,
                params,
            ))
            .await?
            .ok_or_else(|| Error::Db(sea_orm::DbErr::Custom("no aggregate row".into())))?;
        Ok(row.try_get::<serde_json::Value>("", "result")?)
    }

    /// `GET /<resource>/<id>` — one row or `None`.
    pub async fn get(
        &self,
        r: &Resource,
        id: Uuid,
        filters: &[Filter],
    ) -> Result<Option<serde_json::Value>, Error> {
        let table = quote_ident(&r.table_name())?;
        let mut all = vec![Filter::eq("id", id)];
        all.extend_from_slice(filters);
        let (where_sql, params, _) = self.build_where(&all)?;
        let hidden = self.hidden_subtraction(r)?;
        let sql = format!(
            "SELECT to_jsonb(t){hidden} AS result FROM (SELECT * FROM {table} {where_sql} LIMIT 1) t"
        );
        let row = self
            .conn
            .query_one(Statement::from_sql_and_values(
                DatabaseBackend::Postgres,
                sql,
                params,
            ))
            .await?;
        match row {
            Some(row) => Ok(Some(row.try_get::<serde_json::Value>("", "result")?)),
            None => Ok(None),
        }
    }

    /// `POST /<resource>` — insert and return the created row.
    pub async fn create(
        &self,
        r: &Resource,
        data: &serde_json::Map<String, serde_json::Value>,
    ) -> Result<serde_json::Value, Error> {
        let table = quote_ident(&r.table_name())?;
        let mut cols = Vec::new();
        let mut placeholders = Vec::new();
        let mut params: Vec<SqlValue> = Vec::new();
        let mut n = 1;
        for (name, field) in &r.fields {
            if let Some(v) = data.get(name) {
                cols.push(quote_ident(name)?);
                placeholders.push(format!("${n}"));
                params.push(value::json_to_sql(field.ty, v).map_err(Error::BadInput)?);
                n += 1;
            }
        }

        let hidden = self.hidden_subtraction(r)?;
        let returning = format!("RETURNING (to_jsonb({table}.*){hidden}) AS result");
        let sql = if cols.is_empty() {
            format!("INSERT INTO {table} DEFAULT VALUES {returning}")
        } else {
            format!(
                "INSERT INTO {table} ({}) VALUES ({}) {returning}",
                cols.join(", "),
                placeholders.join(", ")
            )
        };
        let row = self
            .conn
            .query_one(Statement::from_sql_and_values(
                DatabaseBackend::Postgres,
                sql,
                params,
            ))
            .await?
            .ok_or_else(|| Error::Db(sea_orm::DbErr::Custom("insert returned no row".into())))?;
        Ok(row.try_get::<serde_json::Value>("", "result")?)
    }

    /// `PATCH /<resource>/<id>` — update present fields, return the new row.
    pub async fn update(
        &self,
        r: &Resource,
        id: Uuid,
        data: &serde_json::Map<String, serde_json::Value>,
        filters: &[Filter],
    ) -> Result<Option<serde_json::Value>, Error> {
        let table = quote_ident(&r.table_name())?;
        let mut assignments = Vec::new();
        let mut params: Vec<SqlValue> = Vec::new();
        let mut n = 1;
        for (name, field) in &r.fields {
            if let Some(v) = data.get(name) {
                assignments.push(format!("{} = ${n}", quote_ident(name)?));
                params.push(value::json_to_sql(field.ty, v).map_err(Error::BadInput)?);
                n += 1;
            }
        }
        if r.meta.timestamps {
            assignments.push("updated_at = now()".to_string());
        }
        if assignments.is_empty() {
            return self.get(r, id, filters).await;
        }

        let mut where_parts = vec![format!("{} = ${n}", quote_ident("id")?)];
        params.push(SqlValue::from(id));
        n += 1;
        for f in filters {
            where_parts.push(Self::render_filter(f, &mut params, &mut n)?);
        }

        let hidden = self.hidden_subtraction(r)?;
        let sql = format!(
            "UPDATE {table} SET {} WHERE {} RETURNING (to_jsonb({table}.*){hidden}) AS result",
            assignments.join(", "),
            where_parts.join(" AND "),
        );
        let row = self
            .conn
            .query_one(Statement::from_sql_and_values(
                DatabaseBackend::Postgres,
                sql,
                params,
            ))
            .await?;
        match row {
            Some(row) => Ok(Some(row.try_get::<serde_json::Value>("", "result")?)),
            None => Ok(None),
        }
    }

    /// `DELETE /<resource>/<id>` — returns whether a row was removed.
    pub async fn delete(&self, r: &Resource, id: Uuid, filters: &[Filter]) -> Result<bool, Error> {
        let table = quote_ident(&r.table_name())?;
        let mut all = vec![Filter::eq("id", id)];
        all.extend_from_slice(filters);
        let (where_sql, params, _) = self.build_where(&all)?;
        let res = self
            .conn
            .execute(Statement::from_sql_and_values(
                DatabaseBackend::Postgres,
                format!("DELETE FROM {table} {where_sql}"),
                params,
            ))
            .await?;
        Ok(res.rows_affected() > 0)
    }

    /// Fetch multiple rows of a resource by id (used for relation expansion).
    /// `filters` carry the caller's authorization scope, so an expansion can
    /// never reach a row a direct read would have refused. Returns a JSON array
    /// with hidden fields stripped; order is unspecified.
    pub async fn fetch_by_ids(
        &self,
        r: &Resource,
        ids: &[Uuid],
        filters: &[Filter],
    ) -> Result<serde_json::Value, Error> {
        if ids.is_empty() {
            return Ok(serde_json::Value::Array(Vec::new()));
        }
        let table = quote_ident(&r.table_name())?;
        let mut all = vec![Filter::in_uuids("id", ids.to_vec())];
        all.extend_from_slice(filters);
        let (where_sql, params, _) = self.build_where(&all)?;
        let hidden = self.hidden_subtraction(r)?;
        let sql = format!(
            "SELECT coalesce(jsonb_agg(to_jsonb(t){hidden}), '[]'::jsonb) AS result \
             FROM (SELECT * FROM {table} {where_sql}) t"
        );
        let row = self
            .conn
            .query_one(Statement::from_sql_and_values(
                DatabaseBackend::Postgres,
                sql,
                params,
            ))
            .await?
            .ok_or_else(|| Error::Db(sea_orm::DbErr::Custom("no aggregate row".into())))?;
        Ok(row.try_get::<serde_json::Value>("", "result")?)
    }

    /// Raw query bridge used by function `.so`s. `SELECT`/`WITH` statements come
    /// back as a JSON array of rows; anything else returns `{"rows_affected":n}`.
    pub async fn raw_json(
        &self,
        sql: &str,
        params: &[serde_json::Value],
    ) -> Result<serde_json::Value, Error> {
        let vals: Vec<SqlValue> = params.iter().map(value::json_param).collect();
        let head = sql.trim_start();
        let is_query = (head.len() >= 6 && head[..6].eq_ignore_ascii_case("select"))
            || (head.len() >= 4 && head[..4].eq_ignore_ascii_case("with"));

        if is_query {
            let wrapped =
                format!("SELECT coalesce(jsonb_agg(t), '[]'::jsonb) AS result FROM ({sql}) t");
            let row = self
                .conn
                .query_one(Statement::from_sql_and_values(
                    DatabaseBackend::Postgres,
                    wrapped,
                    vals,
                ))
                .await?
                .ok_or_else(|| Error::Db(sea_orm::DbErr::Custom("no aggregate row".into())))?;
            Ok(row.try_get::<serde_json::Value>("", "result")?)
        } else {
            let res = self
                .conn
                .execute(Statement::from_sql_and_values(
                    DatabaseBackend::Postgres,
                    sql.to_string(),
                    vals,
                ))
                .await?;
            Ok(serde_json::json!({ "rows_affected": res.rows_affected() }))
        }
    }

    // --- helpers ----------------------------------------------------------

    /// Build a `WHERE …` clause from filters; returns the SQL, the bound values,
    /// and the next free parameter index.
    fn build_where(&self, filters: &[Filter]) -> Result<(String, Vec<SqlValue>, usize), Error> {
        if filters.is_empty() {
            return Ok((String::new(), Vec::new(), 1));
        }
        let mut parts = Vec::new();
        let mut params = Vec::new();
        let mut n = 1;
        for f in filters {
            parts.push(Self::render_filter(f, &mut params, &mut n)?);
        }
        Ok((format!("WHERE {}", parts.join(" AND ")), params, n))
    }

    /// Render one filter to SQL, appending its bound values to `params` and
    /// advancing the `$n` counter.
    fn render_filter(
        f: &Filter,
        params: &mut Vec<SqlValue>,
        n: &mut usize,
    ) -> Result<String, Error> {
        let col = quote_ident(f.column())?;
        Ok(match f {
            Filter::Eq { value, .. } => {
                let part = format!("{col} = ${n}");
                params.push(value.clone());
                *n += 1;
                part
            }
            Filter::In { values, .. } => {
                if values.is_empty() {
                    return Ok("false".to_string());
                }
                let placeholders: Vec<String> = values
                    .iter()
                    .map(|v| {
                        let p = format!("${n}");
                        params.push(v.clone());
                        *n += 1;
                        p
                    })
                    .collect();
                format!("{col} IN ({})", placeholders.join(", "))
            }
            Filter::Contains { value, .. } => {
                // The term is bound, so it cannot be SQL — but it is still a
                // LIKE *pattern*, and an unescaped `%` would match everything.
                let escaped = value
                    .replace('\\', "\\\\")
                    .replace('%', "\\%")
                    .replace('_', "\\_");
                let part = format!("{col}::text ILIKE ${n}");
                params.push(SqlValue::from(format!("%{escaped}%")));
                *n += 1;
                part
            }
        })
    }

    /// `- 'col'` fragments that strip hidden fields from a `to_jsonb` result.
    fn hidden_subtraction(&self, r: &Resource) -> Result<String, Error> {
        let mut s = String::new();
        for (name, field) in &r.fields {
            if field.hidden {
                quote_ident(name)?; // validate the identifier before embedding
                s.push_str(&format!(" - '{name}'"));
            }
        }
        Ok(s)
    }
}
