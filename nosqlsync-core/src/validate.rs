//! Comparing a source row with what landed in the target, and the report verdict.
//!
//! The target side is read back as JSON (`row_to_json`), so each comparison
//! normalizes both sides by the column's type: timestamps compare as instants
//! (Postgres keeps microseconds and its own offset format), JSON compares with
//! sorted keys (JSONB does not keep key order), and numbers compare as numbers.

use base64::Engine;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::transform::Cell;
use crate::types::FieldMapping;

/// Sort object keys recursively and turn every number into an f64, so two
/// equal JSON values always serialize identically.
pub fn canonicalize(v: &Value) -> Value {
    match v {
        Value::Array(a) => Value::Array(a.iter().map(canonicalize).collect()),
        Value::Object(o) => {
            let mut keys: Vec<&String> = o.keys().collect();
            keys.sort();
            Value::Object(
                keys.into_iter()
                    .map(|k| (k.clone(), canonicalize(&o[k])))
                    .collect(),
            )
        }
        Value::Number(n) => n
            .as_f64()
            .and_then(serde_json::Number::from_f64)
            .map(Value::Number)
            .unwrap_or_else(|| v.clone()),
        other => other.clone(),
    }
}

fn cell_key(cell: &Cell) -> String {
    match cell {
        Cell::Null => "null".into(),
        Cell::Text(s) => format!("s:{s}"),
        Cell::Int(i) => format!("n:{}", *i as f64),
        Cell::Float(f) => format!("n:{f}"),
        Cell::Bool(b) => format!("b:{b}"),
        Cell::Timestamp(t) => format!("t:{}", t.timestamp_micros() / 1000),
        Cell::Bytes(b) => format!("x:{}", base64::engine::general_purpose::STANDARD.encode(b)),
        Cell::Json(v) => format!("j:{}", canonicalize(&strip_nul(v))),
        Cell::Numeric(s) => format!("d:{}", normalize_decimal(s)),
    }
}

// The COPY encoder drops NUL characters, so the expected side must too.
fn strip_nul(v: &Value) -> Value {
    match v {
        Value::String(s) => Value::String(s.replace('\0', "")),
        Value::Array(a) => Value::Array(a.iter().map(strip_nul).collect()),
        Value::Object(o) => {
            Value::Object(o.iter().map(|(k, x)| (k.clone(), strip_nul(x))).collect())
        }
        other => other.clone(),
    }
}

/// Read one `row_to_json` value back into a key comparable with [`cell_key`],
/// using the expected cell to know what type the column holds.
fn target_key(expected: &Cell, actual: &Value) -> String {
    if actual.is_null() {
        return "null".into();
    }
    match expected {
        Cell::Text(_) => format!(
            "s:{}",
            actual
                .as_str()
                .map(str::to_string)
                .unwrap_or_else(|| actual.to_string())
        ),
        Cell::Int(_) | Cell::Float(_) => match actual {
            Value::Number(n) => format!("n:{}", n.as_f64().unwrap_or(f64::NAN)),
            Value::String(s) => format!("n:{}", s.parse::<f64>().unwrap_or(f64::NAN)),
            other => format!("n:{other}"),
        },
        Cell::Bool(_) => format!(
            "b:{}",
            actual
                .as_bool()
                .map(|b| b.to_string())
                .unwrap_or_else(|| actual.to_string())
        ),
        Cell::Timestamp(_) => match actual.as_str().and_then(parse_pg_timestamp) {
            Some(t) => format!("t:{}", t.timestamp_micros() / 1000),
            None => format!("t:{actual}"),
        },
        Cell::Bytes(_) => match actual
            .as_str()
            .and_then(|s| s.strip_prefix("\\x"))
            .and_then(hex_decode)
        {
            Some(b) => format!("x:{}", base64::engine::general_purpose::STANDARD.encode(b)),
            None => format!("x:{actual}"),
        },
        Cell::Json(_) => format!("j:{}", canonicalize(actual)),
        Cell::Numeric(_) => match actual {
            Value::Number(n) => format!("d:{}", normalize_decimal(&n.to_string())),
            Value::String(s) => format!("d:{}", normalize_decimal(s)),
            other => format!("d:{other}"),
        },
        Cell::Null => format!("{actual}"),
    }
}

/// `12.30` → `12.3`, `-0.0` → `0`, `5.` → `5`: equal decimals, equal strings.
/// Target values arrive through JSON as f64, so this is exact to ~15 significant digits.
pub fn normalize_decimal(s: &str) -> String {
    let s = s.trim();
    let (neg, digits) = match s.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, s),
    };
    let (int, frac) = digits.split_once('.').unwrap_or((digits, ""));
    let int = int.trim_start_matches('0');
    let frac = frac.trim_end_matches('0');
    let int = if int.is_empty() { "0" } else { int };
    let body = if frac.is_empty() {
        int.to_string()
    } else {
        format!("{int}.{frac}")
    };
    if neg && body != "0" {
        format!("-{body}")
    } else {
        body
    }
}

fn parse_pg_timestamp(s: &str) -> Option<DateTime<Utc>> {
    // row_to_json writes e.g. "2024-01-02T03:04:05.123+00:00"; short offsets like "+00" also occur.
    DateTime::parse_from_rfc3339(s)
        .or_else(|_| DateTime::parse_from_str(&format!("{s}00"), "%Y-%m-%dT%H:%M:%S%.f%z"))
        .or_else(|_| DateTime::parse_from_str(s, "%Y-%m-%dT%H:%M:%S%.f%:z"))
        .ok()
        .map(|d| d.with_timezone(&Utc))
}

fn hex_decode(s: &str) -> Option<Vec<u8>> {
    if !s.len().is_multiple_of(2) {
        return None;
    }
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).ok())
        .collect()
}

/// Column names whose target value differs from the expected cell.
pub fn compare_row(
    expected: &[Cell],
    columns: &[FieldMapping],
    actual: &serde_json::Map<String, Value>,
) -> Vec<String> {
    let mut out = Vec::new();
    for (cell, m) in expected.iter().zip(columns) {
        let got = actual.get(&m.target_column).unwrap_or(&Value::Null);
        if cell_key(cell) != target_key(cell, got) {
            out.push(m.target_column.clone());
        }
    }
    out
}

/// Same comparison for columns addressed by name, used for child rows.
pub fn compare_named(
    expected: &[(String, Cell)],
    actual: &serde_json::Map<String, Value>,
) -> Vec<String> {
    expected
        .iter()
        .filter(|(col, cell)| {
            cell_key(cell) != target_key(cell, actual.get(col).unwrap_or(&Value::Null))
        })
        .map(|(col, _)| col.clone())
        .collect()
}

#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug)]
#[serde(rename_all = "lowercase")]
pub enum Verdict {
    Passed,
    Failed,
    Incomplete,
}

#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Mismatch {
    pub primary_key: String,
    /// Table the mismatch is in (the parent, or a child table).
    pub table: String,
    pub columns: Vec<String>,
}

#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
#[serde(rename_all = "camelCase")]
pub struct TableReport {
    pub collection: String,
    pub table: String,
    pub source_count: Option<u64>,
    pub target_count: u64,
    pub count_match: Option<bool>,
    pub sampled: usize,
    /// Sampled documents with no row in the target at all.
    pub missing: Vec<String>,
    pub mismatches: Vec<Mismatch>,
    pub verdict: Verdict,
}

/// Passed only when the full check ran and found nothing. A clean result from a
/// partial check (no source count, nothing sampled) is incomplete, never passed.
pub fn verdict(
    count_match: Option<bool>,
    sampled: usize,
    missing: usize,
    mismatches: usize,
) -> Verdict {
    if count_match == Some(false) || missing > 0 || mismatches > 0 {
        Verdict::Failed
    } else if count_match.is_none() || sampled == 0 {
        Verdict::Incomplete
    } else {
        Verdict::Passed
    }
}

pub fn overall(tables: &[TableReport]) -> Verdict {
    if tables.iter().any(|t| t.verdict == Verdict::Failed) {
        Verdict::Failed
    } else if tables.is_empty() || tables.iter().any(|t| t.verdict == Verdict::Incomplete) {
        Verdict::Incomplete
    } else {
        Verdict::Passed
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::MongoType;
    use serde_json::json;

    fn col(name: &str) -> FieldMapping {
        FieldMapping {
            source_field: name.into(),
            target_column: name.into(),
            mongo_types: vec![MongoType::String],
            resolved_mongo_type: MongoType::String,
            sql_type: "TEXT".into(),
            nullable: true,
            is_primary_key: false,
            sample_values: vec![],
            null_fraction: 0.0,
        }
    }

    #[test]
    fn tolerates_representation_differences() {
        let expected = vec![
            Cell::Timestamp(DateTime::from_timestamp_millis(1_700_000_000_123).unwrap()),
            Cell::Json(json!({"b": 1, "a": [1, 2]})),
            Cell::Int(5),
            Cell::Float(1.5),
            Cell::Bytes(vec![0xde, 0xad]),
        ];
        let cols: Vec<_> = ["t", "j", "i", "f", "x"].into_iter().map(col).collect();
        let actual = json!({"t": "2023-11-14T22:13:20.123+00:00", "j": {"a": [1, 2], "b": 1.0}, "i": 5, "f": 1.5, "x": "\\xdead"});
        assert!(compare_row(&expected, &cols, actual.as_object().unwrap()).is_empty());
    }

    #[test]
    fn flags_real_differences() {
        let expected = vec![Cell::Text("a".into()), Cell::Null];
        let cols = vec![col("s"), col("n")];
        let actual = json!({"s": "b", "n": "x"});
        assert_eq!(
            compare_row(&expected, &cols, actual.as_object().unwrap()),
            ["s", "n"]
        );
    }

    #[test]
    fn verdicts() {
        assert_eq!(verdict(Some(true), 10, 0, 0), Verdict::Passed);
        assert_eq!(verdict(Some(true), 0, 0, 0), Verdict::Incomplete);
        assert_eq!(verdict(None, 10, 0, 0), Verdict::Incomplete);
        assert_eq!(verdict(Some(false), 10, 0, 0), Verdict::Failed);
        assert_eq!(verdict(Some(true), 10, 0, 1), Verdict::Failed);
    }
}
