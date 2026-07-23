//! Conversions between JSON (the wire format) and typed SQL values.

use apiplant_core::FieldType;
use sea_orm::sea_query::Value as SqlValue;

/// Convert a JSON value into a typed SQL value for the given column type.
/// Returns a human-readable error on a type mismatch (surfaced as a 400).
pub fn json_to_sql(ty: FieldType, v: &serde_json::Value) -> Result<SqlValue, String> {
    if v.is_null() {
        return Ok(null_for(ty));
    }
    Ok(match ty {
        FieldType::String | FieldType::Text => {
            SqlValue::from(v.as_str().ok_or("expected a string")?.to_string())
        }
        FieldType::Integer => {
            SqlValue::from(i32::try_from(v.as_i64().ok_or("expected an integer")?).map_err(|_| "integer out of range")?)
        }
        FieldType::BigInt => SqlValue::from(v.as_i64().ok_or("expected an integer")?),
        FieldType::Float => SqlValue::from(v.as_f64().ok_or("expected a number")?),
        FieldType::Boolean => SqlValue::from(v.as_bool().ok_or("expected a boolean")?),
        FieldType::Uuid | FieldType::Reference => {
            let s = v.as_str().ok_or("expected a UUID string")?;
            SqlValue::from(uuid::Uuid::parse_str(s).map_err(|e| e.to_string())?)
        }
        FieldType::Timestamp => {
            let s = v.as_str().ok_or("expected an RFC3339 timestamp")?;
            SqlValue::from(chrono::DateTime::parse_from_rfc3339(s).map_err(|e| e.to_string())?)
        }
        FieldType::Json => SqlValue::from(v.clone()),
    })
}

/// The correctly-typed SQL `NULL` for a column type (Postgres cares about the
/// type of a bound null).
pub fn null_for(ty: FieldType) -> SqlValue {
    match ty {
        FieldType::String | FieldType::Text => SqlValue::String(None),
        FieldType::Integer => SqlValue::Int(None),
        FieldType::BigInt => SqlValue::BigInt(None),
        FieldType::Float => SqlValue::Double(None),
        FieldType::Boolean => SqlValue::Bool(None),
        FieldType::Uuid | FieldType::Reference => SqlValue::Uuid(None),
        FieldType::Timestamp => SqlValue::ChronoDateTimeWithTimeZone(None),
        FieldType::Json => SqlValue::Json(None),
    }
}

/// Convert a raw query-string value (always a string) into a typed SQL value
/// for a column, used for `?field=value` filtering.
pub fn string_to_sql(ty: FieldType, s: &str) -> Result<SqlValue, String> {
    Ok(match ty {
        FieldType::String | FieldType::Text => SqlValue::from(s.to_string()),
        FieldType::Integer => SqlValue::from(s.parse::<i32>().map_err(|_| "expected an integer")?),
        FieldType::BigInt => SqlValue::from(s.parse::<i64>().map_err(|_| "expected an integer")?),
        FieldType::Float => SqlValue::from(s.parse::<f64>().map_err(|_| "expected a number")?),
        FieldType::Boolean => SqlValue::from(s.parse::<bool>().map_err(|_| "expected a boolean")?),
        FieldType::Uuid | FieldType::Reference => {
            SqlValue::from(uuid::Uuid::parse_str(s).map_err(|e| e.to_string())?)
        }
        FieldType::Timestamp => {
            SqlValue::from(chrono::DateTime::parse_from_rfc3339(s).map_err(|e| e.to_string())?)
        }
        FieldType::Json => SqlValue::from(serde_json::Value::String(s.to_string())),
    })
}

/// Best-effort conversion for *untyped* params coming from function `.so`s via
/// the raw-query host callback (we don't know the target column type there).
pub fn json_param(v: &serde_json::Value) -> SqlValue {
    use serde_json::Value as J;
    match v {
        J::Null => SqlValue::String(None),
        J::Bool(b) => SqlValue::from(*b),
        J::Number(n) if n.is_i64() => SqlValue::from(n.as_i64().unwrap()),
        J::Number(n) if n.is_u64() => SqlValue::from(n.as_u64().unwrap() as i64),
        J::Number(n) => SqlValue::from(n.as_f64().unwrap()),
        J::String(s) => SqlValue::from(s.clone()),
        other => SqlValue::from(other.clone()),
    }
}
