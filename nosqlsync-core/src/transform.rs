//! Documents → typed row cells, and the Postgres COPY text encoding.
//!
//! Coercion follows the platform worker's `coerceValue` with one difference:
//! a value that cannot be coerced (the string `"n/a"` in an integer column)
//! becomes NULL and is counted, instead of failing the whole batch. The count
//! is reported at the end of a load, so nothing is lost silently.

use base64::Engine;
use chrono::{DateTime, NaiveDate, Utc};
use serde_json::Value;

use crate::child_tables::ChildTable;
use crate::ejson::{as_uuid, plain_json, unwrap_value};
use crate::infer::{NUMERIC, UUID};
use crate::types::{FieldMapping, MongoType};

#[derive(Clone, PartialEq, Debug)]
/// One typed value for one column of one row.
pub enum Cell {
    /// SQL NULL: a missing or null source value, or one that failed coercion.
    Null,
    /// Text, ObjectId hex, or a UUID string.
    Text(String),
    /// An integer; Int64 values are exact.
    Int(i64),
    /// A double, including NaN and ±Infinity.
    Float(f64),
    /// A boolean.
    Bool(bool),
    /// A point in time. COPY encodes it at millisecond precision.
    Timestamp(DateTime<Utc>),
    /// Binary data, written as bytea hex.
    Bytes(Vec<u8>),
    /// An array or object, Extended JSON wrappers unwrapped by [`plain_json`].
    Json(Value),
    /// An exact decimal (from Decimal128), kept as its digits.
    Numeric(String),
}

/// A value that could not be coerced to its column's type.
#[derive(Clone, Debug, PartialEq)]
pub struct CoercionIssue {
    /// Target column name.
    pub column: String,
    /// The column's resolved source type.
    pub expected: MongoType,
    /// The offending value as JSON, cut to 60 characters plus `…`.
    pub value: String,
}

/// The value at a dot path, or `null` when any step is missing or not an object.
pub fn get_nested<'a>(doc: &'a Value, path: &str) -> &'a Value {
    let mut cur = doc;
    for part in path.split('.') {
        match cur {
            Value::Object(o) => match o.get(part) {
                Some(v) => cur = v,
                None => return &Value::Null,
            },
            _ => return &Value::Null,
        }
    }
    cur
}

/// Coerce one source value to the cell its column expects.
///
/// `null` is always [`Cell::Null`]. Booleans are JavaScript truthiness, so
/// they never fail: `0`, `""` and `NaN` are false, and any other string
/// (including `"false"`) is true. Dates accept `$date`, `$timestamp`, RFC 3339
/// or `YYYY-MM-DD` strings, and integers as epoch milliseconds.
pub fn coerce(value: &Value, m: &FieldMapping) -> Result<Cell, CoercionIssue> {
    if value.is_null() {
        return Ok(Cell::Null);
    }
    let fail = || CoercionIssue {
        column: m.target_column.clone(),
        expected: m.resolved_mongo_type,
        value: preview(value),
    };
    let plain = unwrap_value(value);
    // Precise PostgreSQL types chosen by inference (see `infer::precise_type`).
    if m.sql_type == NUMERIC {
        return match &plain {
            Value::String(s) if is_decimal_literal(s) => Ok(Cell::Numeric(s.clone())),
            Value::Number(n) => Ok(Cell::Numeric(js_number(n))),
            _ => Err(fail()),
        };
    }
    if m.sql_type == UUID {
        return as_uuid(value).map(Cell::Text).ok_or_else(fail);
    }
    Ok(match m.resolved_mongo_type {
        MongoType::ObjectId | MongoType::String | MongoType::Null | MongoType::Unknown => {
            Cell::Text(to_text(&plain))
        }
        MongoType::Date => Cell::Timestamp(to_timestamp(value).ok_or_else(fail)?),
        MongoType::Int => Cell::Int(to_i64(&plain).ok_or_else(fail)?),
        MongoType::Double => Cell::Float(to_f64(&plain).ok_or_else(fail)?),
        MongoType::Boolean => Cell::Bool(truthy(&plain)),
        MongoType::Array | MongoType::Object => Cell::Json(plain_json(value)),
        MongoType::Binary => Cell::Bytes(to_bytes(value).ok_or_else(fail)?),
    })
}

fn preview(v: &Value) -> String {
    let s = v.to_string();
    if s.chars().count() > 60 {
        format!("{}…", s.chars().take(60).collect::<String>())
    } else {
        s
    }
}

/// `String(value)` from JavaScript, except objects and arrays become JSON
/// rather than `[object Object]`.
pub fn to_text(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Number(n) => js_number(n),
        Value::Bool(b) => b.to_string(),
        Value::Null => String::new(),
        other => plain_json(other).to_string(),
    }
}

fn js_number(n: &serde_json::Number) -> String {
    if let Some(i) = n.as_i64() {
        return i.to_string();
    }
    if let Some(u) = n.as_u64() {
        return u.to_string();
    }
    let f = n.as_f64().unwrap_or(f64::NAN);
    if f.fract() == 0.0 && f.abs() < 1e21 {
        format!("{f:.0}")
    } else {
        f.to_string()
    }
}

/// Integers parse exactly; going through f64 would round Int64 values above 2⁵³.
/// Fractional values truncate, as `Math.trunc` does on the platform.
fn to_i64(v: &Value) -> Option<i64> {
    match v {
        Value::Number(n) if n.as_i64().is_some() => n.as_i64(),
        Value::String(s) if s.trim().parse::<i64>().is_ok() => s.trim().parse().ok(),
        other => to_f64(other)
            .filter(|f| f.is_finite() && f.abs() < 9.2e18)
            .map(|f| f.trunc() as i64),
    }
}

fn is_decimal_literal(s: &str) -> bool {
    let s = s.strip_prefix('-').unwrap_or(s);
    let mut parts = s.splitn(2, '.');
    let int = parts.next().unwrap_or("");
    let frac = parts.next().unwrap_or("");
    !(int.is_empty() && frac.is_empty())
        && int.bytes().all(|b| b.is_ascii_digit())
        && frac.bytes().all(|b| b.is_ascii_digit())
}

fn to_f64(v: &Value) -> Option<f64> {
    match v {
        Value::Number(n) => n.as_f64(),
        Value::String(s) => {
            let s = s.trim();
            match s {
                "NaN" => Some(f64::NAN),
                "Infinity" => Some(f64::INFINITY),
                "-Infinity" => Some(f64::NEG_INFINITY),
                _ => s.parse::<f64>().ok(),
            }
        }
        Value::Bool(b) => Some(if *b { 1.0 } else { 0.0 }),
        _ => None,
    }
}

fn truthy(v: &Value) -> bool {
    match v {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::Number(n) => n.as_f64().is_some_and(|f| f != 0.0 && !f.is_nan()),
        Value::String(s) => !s.is_empty(),
        _ => true,
    }
}

fn to_timestamp(v: &Value) -> Option<DateTime<Utc>> {
    match v {
        // A BSON Timestamp: seconds since the epoch plus an ordinal.
        Value::Object(o) if o.len() == 1 && o.contains_key("$timestamp") => o["$timestamp"]
            .get("t")
            .and_then(Value::as_i64)
            .and_then(|t| DateTime::from_timestamp(t, 0)),
        Value::Object(o) if o.len() == 1 && o.contains_key("$date") => match &o["$date"] {
            Value::String(s) => parse_date(s),
            Value::Number(n) => n.as_i64().and_then(DateTime::from_timestamp_millis),
            Value::Object(inner) => inner
                .get("$numberLong")
                .and_then(Value::as_str)
                .and_then(|s| s.parse::<i64>().ok())
                .and_then(DateTime::from_timestamp_millis),
            _ => None,
        },
        Value::String(s) => parse_date(s),
        // Epoch milliseconds, which is how many apps store "dates".
        Value::Number(n) => n.as_i64().and_then(DateTime::from_timestamp_millis),
        _ => None,
    }
}

fn parse_date(s: &str) -> Option<DateTime<Utc>> {
    if let Ok(d) = DateTime::parse_from_rfc3339(s) {
        return Some(d.with_timezone(&Utc));
    }
    if let Ok(d) = NaiveDate::parse_from_str(s, "%Y-%m-%d") {
        return Some(d.and_hms_opt(0, 0, 0)?.and_utc());
    }
    None
}

fn to_bytes(v: &Value) -> Option<Vec<u8>> {
    match v {
        Value::Object(o) => {
            let b = o.get("$binary")?;
            let b64 = b
                .get("base64")
                .and_then(Value::as_str)
                .or_else(|| b.as_str())?;
            base64::engine::general_purpose::STANDARD.decode(b64).ok()
        }
        Value::String(s) => Some(s.as_bytes().to_vec()),
        _ => None,
    }
}

/// One row for the parent table, in mapping order.
pub fn transform_doc(
    doc: &Value,
    mappings: &[FieldMapping],
    issues: &mut Vec<CoercionIssue>,
) -> Vec<Cell> {
    mappings
        .iter()
        .map(|m| {
            coerce(get_nested(doc, &m.source_field), m).unwrap_or_else(|e| {
                issues.push(e);
                Cell::Null
            })
        })
        .collect()
}

/// One row per object element of the child table's array: `[parent id, array index, …fields]`.
/// Non-object elements (a shape the sample didn't see) are skipped rather than guessed at.
pub fn child_rows(
    doc: &Value,
    table: &ChildTable,
    parent_pk: &Cell,
    issues: &mut Vec<CoercionIssue>,
) -> Vec<Vec<Cell>> {
    let Value::Array(elements) = get_nested(doc, &table.source_path) else {
        return vec![];
    };
    elements
        .iter()
        .enumerate()
        .filter(|(_, el)| {
            el.is_object() && crate::ejson::wrapper_type(el.as_object().unwrap()).is_none()
        })
        .map(|(i, el)| {
            let mut row = Vec::with_capacity(table.fields.len() + 2);
            row.push(parent_pk.clone());
            row.push(Cell::Int(i as i64));
            for f in &table.fields {
                row.push(
                    coerce(get_nested(el, &f.source_field), f).unwrap_or_else(|e| {
                        issues.push(e);
                        Cell::Null
                    }),
                );
            }
            row
        })
        .collect()
}

// --- COPY text format ---------------------------------------------------------

/// Encode one cell for `COPY … FROM STDIN` (text format).
pub fn copy_cell(cell: &Cell, out: &mut String) {
    use std::fmt::Write;
    let raw: std::borrow::Cow<str> = match cell {
        Cell::Null => {
            out.push_str("\\N");
            return;
        }
        Cell::Text(s) => s.as_str().into(),
        Cell::Int(i) => i.to_string().into(),
        Cell::Float(f) => {
            if f.is_nan() {
                "NaN".into()
            } else if f.is_infinite() {
                if *f > 0.0 {
                    "Infinity".into()
                } else {
                    "-Infinity".into()
                }
            } else {
                f.to_string().into()
            }
        }
        Cell::Bool(b) => (if *b { "t" } else { "f" }).into(),
        Cell::Timestamp(t) => t.format("%Y-%m-%dT%H:%M:%S%.3fZ").to_string().into(),
        Cell::Bytes(b) => {
            // bytea hex format; the leading backslash is escaped like any other.
            out.push_str("\\\\x");
            for byte in b {
                let _ = write!(out, "{byte:02x}");
            }
            return;
        }
        Cell::Json(v) => v.to_string().into(),
        Cell::Numeric(s) => s.as_str().into(),
    };
    for c in raw.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            // Postgres text and JSONB both reject NUL; drop it rather than fail the batch.
            '\0' => {}
            c => out.push(c),
        }
    }
}

/// Encode one row for `COPY … FROM STDIN`: tab-separated cells and a trailing newline.
pub fn copy_row(row: &[Cell], out: &mut String) {
    for (i, cell) in row.iter().enumerate() {
        if i > 0 {
            out.push('\t');
        }
        copy_cell(cell, out);
    }
    out.push('\n');
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn m(t: MongoType) -> FieldMapping {
        FieldMapping {
            source_field: "f".into(),
            target_column: "f".into(),
            mongo_types: vec![t],
            resolved_mongo_type: t,
            sql_type: String::new(),
            nullable: true,
            is_primary_key: false,
            sample_values: vec![],
            null_fraction: 0.0,
        }
    }

    #[test]
    fn precise_types_and_exact_int64() {
        let mut n = m(MongoType::Double);
        n.sql_type = NUMERIC.into();
        assert_eq!(
            coerce(&json!({"$numberDecimal": "12.30"}), &n).unwrap(),
            Cell::Numeric("12.30".into())
        );
        assert!(coerce(&json!({"$numberDecimal": "NaN"}), &n).is_err());
        let mut u = m(MongoType::Binary);
        u.sql_type = UUID.into();
        let v = json!({"$binary": {"base64": "ABEiM0RVZneImaq7zN3u/w==", "subType": "04"}});
        assert_eq!(
            coerce(&v, &u).unwrap(),
            Cell::Text("00112233-4455-6677-8899-aabbccddeeff".into())
        );
        // 2^53 + 1 survives; through f64 it would become 2^53.
        assert_eq!(
            coerce(
                &json!({"$numberLong": "9007199254740993"}),
                &m(MongoType::Int)
            )
            .unwrap(),
            Cell::Int(9_007_199_254_740_993)
        );
        assert_eq!(
            coerce(
                &json!({"$timestamp": {"t": 1700000000, "i": 1}}),
                &m(MongoType::Date)
            )
            .unwrap(),
            Cell::Timestamp(DateTime::from_timestamp(1_700_000_000, 0).unwrap())
        );
    }

    #[test]
    fn coerces_extended_json() {
        assert_eq!(
            coerce(&json!({"$oid": "abc"}), &m(MongoType::ObjectId)).unwrap(),
            Cell::Text("abc".into())
        );
        assert_eq!(
            coerce(&json!({"$numberLong": "5000000000"}), &m(MongoType::Int)).unwrap(),
            Cell::Int(5_000_000_000)
        );
        assert_eq!(
            coerce(&json!(2.9), &m(MongoType::Int)).unwrap(),
            Cell::Int(2)
        );
        assert_eq!(
            coerce(&json!({"$date": {"$numberLong": "0"}}), &m(MongoType::Date)).unwrap(),
            Cell::Timestamp(DateTime::from_timestamp_millis(0).unwrap())
        );
        assert_eq!(
            coerce(&json!("yes"), &m(MongoType::Boolean)).unwrap(),
            Cell::Bool(true)
        );
        assert_eq!(
            coerce(&json!(42), &m(MongoType::String)).unwrap(),
            Cell::Text("42".into())
        );
        assert!(coerce(&json!("n/a"), &m(MongoType::Int)).is_err());
    }

    #[test]
    fn copy_escaping() {
        let mut s = String::new();
        copy_row(
            &[
                Cell::Text("a\tb\\c\nd".into()),
                Cell::Null,
                Cell::Bool(true),
                Cell::Bytes(vec![0xde, 0xad]),
            ],
            &mut s,
        );
        assert_eq!(s, "a\\tb\\\\c\\nd\t\\N\tt\t\\\\xdead\n");
    }

    #[test]
    fn nested_lookup() {
        let d = json!({"a": {"b": {"c": 1}}, "x": 5});
        assert_eq!(get_nested(&d, "a.b.c"), &json!(1));
        assert_eq!(get_nested(&d, "x.y"), &Value::Null);
        assert_eq!(get_nested(&d, "missing"), &Value::Null);
    }
}
