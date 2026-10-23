//! Per-field schema inference.

use indexmap::IndexMap;
use serde_json::Value;

use crate::collate::locale_cmp;
use crate::ejson::{as_uuid, detect, is_decimal, is_int64, is_js_integer, sample_value, Detect};
use crate::types::{
    resolve_conflicting_types, to_column_name, to_sql_type, wide_int_type, Dialect, FieldMapping,
    MongoType,
};

/// What was observed for one top-level field across the sampled documents.
#[derive(Default, Clone, Debug)]
pub struct FieldStats {
    /// The detected type of every non-null occurrence, repeats included.
    pub types: Vec<MongoType>,
    /// The first three non-null values, as [`sample_value`] renders them.
    pub sample_values: Vec<Value>,
    /// Documents in which the field is present, including as `null`.
    pub seen_count: usize,
    /// Documents in which the field is an explicit `null`.
    pub null_count: usize,
    /// An integer outside the 32-bit range, or a BSON Int64, was seen.
    pub wide_int: bool,
    /// A Decimal128 was seen; it needs an exact type, not a float.
    pub decimal: bool,
    /// Binary values seen, and how many of them were UUIDs.
    pub binaries: usize,
    /// Binary values that were UUIDs (`$uuid`, or `$binary` subtype 04).
    pub uuids: usize,
}

/// Field stats keyed by dot path, in first-seen order.
pub type Stats = IndexMap<String, FieldStats>;

/// Accumulate type info for each top-level field of one document.
/// Nested fields are not visited; [`crate::relational`] handles those.
pub fn collect_fields(doc: &Value, stats: &mut Stats, mode: Detect) {
    let Value::Object(obj) = doc else { return };
    for (key, value) in obj {
        let t = detect(value, mode);
        let field = stats.entry(key.clone()).or_default();
        field.seen_count += 1;
        if t == MongoType::Null {
            field.null_count += 1;
            continue;
        }
        field.types.push(t);
        let mut hints = field.hints();
        hints.observe(t, value);
        (field.wide_int, field.decimal, field.binaries, field.uuids) =
            (hints.wide_int, hints.decimal, hints.binaries, hints.uuids);
        if field.sample_values.len() < 3 {
            field.sample_values.push(sample_value(value));
        }
    }
}

impl FieldStats {
    fn hints(&self) -> TypeHints {
        TypeHints {
            wide_int: self.wide_int,
            decimal: self.decimal,
            binaries: self.binaries,
            uuids: self.uuids,
        }
    }
}

/// What the values seen for one field say about the precise column type it
/// needs. Shared by top-level fields and by the subfields of flattened objects
/// and child tables, so every column is typed the same way.
#[derive(Default, Clone, Copy, Debug)]
pub(crate) struct TypeHints {
    wide_int: bool,
    decimal: bool,
    binaries: usize,
    uuids: usize,
}

impl TypeHints {
    /// Record one non-null value of detected type `t`.
    pub(crate) fn observe(&mut self, t: MongoType, value: &Value) {
        // The value's size is only a sample; a BSON Int64 says the app stores 64-bit numbers.
        if t == MongoType::Int && (!fits_i32(value) || is_int64(value)) {
            self.wide_int = true;
        }
        if is_decimal(value) {
            self.decimal = true;
        }
        if t == MongoType::Binary {
            self.binaries += 1;
            if as_uuid(value).is_some() {
                self.uuids += 1;
            }
        }
    }

    /// The column type for values resolved to `resolved`.
    pub(crate) fn sql_type(&self, resolved: MongoType, dialect: Dialect) -> &'static str {
        if resolved == MongoType::Int && self.wide_int {
            return wide_int_type(dialect);
        }
        precise_type(resolved, self, dialect).unwrap_or_else(|| to_sql_type(resolved, dialect))
    }
}

/// PostgreSQL types that keep more than the generic map does. Departures from
/// the TypeScript engine, which maps Decimal128 to a float and UUIDs to bytes.
fn precise_type(resolved: MongoType, hints: &TypeHints, dialect: Dialect) -> Option<&'static str> {
    if dialect != Dialect::Postgresql {
        return None;
    }
    match resolved {
        // Money stored as Decimal128 must not round through a float.
        MongoType::Double if hints.decimal => Some(NUMERIC),
        MongoType::Binary if hints.binaries > 0 && hints.uuids == hints.binaries => Some(UUID),
        _ => None,
    }
}

/// The PostgreSQL type inference picks for a field that held a Decimal128.
/// [`crate::transform::coerce`] matches on it to emit an exact [`Numeric`](crate::transform::Cell::Numeric) cell.
pub const NUMERIC: &str = "NUMERIC";
/// The PostgreSQL type inference picks for a binary field whose every value was a UUID.
pub const UUID: &str = "UUID";

fn fits_i32(value: &Value) -> bool {
    let n = match value {
        Value::Number(n) if is_js_integer(n) => n.as_f64(),
        Value::Object(o) => o
            .get("$numberInt")
            .or_else(|| o.get("$numberLong"))
            .and_then(Value::as_str)
            .and_then(|s| s.parse::<f64>().ok()),
        _ => None,
    };
    n.is_none_or(|f| f >= i32::MIN as f64 && f <= i32::MAX as f64)
}

/// Accumulated stats → ordered field mappings (`_id` first, then alphabetical).
///
/// Two deliberate departures from the TypeScript engine, both of which make a
/// load succeed that would otherwise fail:
/// - `_id` keeps `VARCHAR(24)` only when it really is an ObjectId; a string or
///   numeric `_id` gets its own type instead of being truncated into 24 chars.
/// - an integer field holding values beyond 32 bits becomes `BIGINT`.
///
/// Column-name collisions (`userId` and `user_id` both → `user_id`) are
/// resolved with a numeric suffix so the DDL stays valid.
pub fn build_field_mappings(
    stats: &Stats,
    total_docs: usize,
    dialect: Dialect,
) -> Vec<FieldMapping> {
    let mut mappings: Vec<FieldMapping> = stats
        .iter()
        .map(|(field, stat)| {
            let all_types = if stat.types.is_empty() {
                vec![MongoType::Null]
            } else {
                stat.types.clone()
            };
            let resolved = resolve_conflicting_types(&all_types);
            let is_pk = field == "_id";
            let mut unique = Vec::new();
            for t in &all_types {
                if !unique.contains(t) {
                    unique.push(*t);
                }
            }
            let sql_type = if is_pk && resolved == MongoType::ObjectId {
                "VARCHAR(24)".to_string()
            } else {
                stat.hints().sql_type(resolved, dialect).to_string()
            };
            FieldMapping {
                source_field: field.clone(),
                target_column: if is_pk {
                    "id".into()
                } else {
                    to_column_name(field)
                },
                mongo_types: unique,
                resolved_mongo_type: resolved,
                sql_type,
                // A sample can never prove a field is in every document, so only _id is NOT NULL.
                nullable: !is_pk,
                is_primary_key: is_pk,
                sample_values: stat.sample_values.clone(),
                null_fraction: if total_docs > 0 {
                    (stat.null_count + (total_docs - stat.seen_count.min(total_docs))) as f64
                        / total_docs as f64
                } else {
                    0.0
                },
            }
        })
        .collect();

    mappings.sort_by(|a, b| match (a.is_primary_key, b.is_primary_key) {
        (true, false) => std::cmp::Ordering::Less,
        (false, true) => std::cmp::Ordering::Greater,
        _ => locale_cmp(&a.source_field, &b.source_field),
    });
    dedupe_columns(&mut mappings);
    mappings
}

/// Give every mapping a distinct column name, keeping the first occurrence as-is.
pub fn dedupe_columns(mappings: &mut [FieldMapping]) {
    let mut taken = std::collections::HashSet::new();
    for m in mappings.iter_mut() {
        if taken.insert(m.target_column.clone()) {
            continue;
        }
        let mut n = 2;
        while taken.contains(&format!("{}_{}", m.target_column, n)) {
            n += 1;
        }
        m.target_column = format!("{}_{}", m.target_column, n);
        taken.insert(m.target_column.clone());
    }
}

/// The output of [`infer_schema`].
pub struct InferenceResult {
    /// One mapping per top-level field, ordered as [`build_field_mappings`] orders them.
    pub fields: Vec<FieldMapping>,
    /// The raw per-field stats the mappings were built from.
    pub stats: Stats,
    /// Number of documents passed in.
    pub total_docs_sampled: usize,
}

/// Infer one column per top-level field of `docs`: [`collect_fields`] over
/// every document, then [`build_field_mappings`]. Non-object documents are skipped
/// but still count towards the total, so they raise every field's `null_fraction`.
pub fn infer_schema(docs: &[Value], dialect: Dialect, mode: Detect) -> InferenceResult {
    let mut stats = Stats::new();
    for doc in docs {
        collect_fields(doc, &mut stats, mode);
    }
    InferenceResult {
        fields: build_field_mappings(&stats, docs.len(), dialect),
        stats,
        total_docs_sampled: docs.len(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn infer(docs: Vec<Value>) -> Vec<FieldMapping> {
        infer_schema(&docs, Dialect::Postgresql, Detect::Json).fields
    }

    #[test]
    fn id_first_then_alphabetical() {
        let f = infer(vec![
            json!({"zeta": 1, "_id": "507f1f77bcf86cd799439011", "Alpha": "x"}),
        ]);
        let cols: Vec<_> = f.iter().map(|m| m.target_column.as_str()).collect();
        assert_eq!(cols, ["id", "alpha", "zeta"]);
        assert_eq!(f[0].sql_type, "VARCHAR(24)");
        assert!(!f[0].nullable && f[0].is_primary_key);
    }

    #[test]
    fn null_fraction_counts_missing_and_null() {
        let f = infer(vec![json!({"a": 1}), json!({"a": null}), json!({})]);
        assert!((f[0].null_fraction - 2.0 / 3.0).abs() < 1e-9);
        assert!(f[0].nullable);
    }

    #[test]
    fn wide_ints_become_bigint() {
        let f = infer(vec![json!({"n": 1}), json!({"n": 5_000_000_000_i64})]);
        assert_eq!(f[0].sql_type, "BIGINT");
        let f = infer(vec![json!({"n": {"$numberLong": "5000000000"}})]);
        assert_eq!(f[0].sql_type, "BIGINT");
    }

    #[test]
    fn precise_bson_types() {
        let docs = vec![json!({
            "_id": {"$numberLong": "1"},
            "price": {"$numberDecimal": "12.30"},
            "token": {"$binary": {"base64": "ABEiM0RVZneImaq7zN3u/w==", "subType": "04"}},
            "blob": {"$binary": {"base64": "3q2+7w==", "subType": "00"}},
        })];
        let f = infer_schema(&docs, Dialect::Postgresql, Detect::Bson).fields;
        let ty = |c: &str| {
            f.iter()
                .find(|m| m.target_column == c)
                .unwrap()
                .sql_type
                .clone()
        };
        assert_eq!(ty("id"), "BIGINT");
        assert_eq!(ty("price"), "NUMERIC");
        assert_eq!(ty("token"), "UUID");
        assert_eq!(ty("blob"), "BYTEA");
        // Other dialects keep the platform's generic mapping.
        let f = infer_schema(&docs, Dialect::Mysql, Detect::Bson).fields;
        assert_eq!(
            f.iter()
                .find(|m| m.target_column == "price")
                .unwrap()
                .sql_type,
            "DOUBLE"
        );
    }

    #[test]
    fn non_objectid_primary_keys_keep_their_type() {
        let f = infer(vec![json!({"_id": "user-with-a-very-long-identifier"})]);
        assert_eq!(f[0].sql_type, "TEXT");
        let f = infer(vec![json!({"_id": 7})]);
        assert_eq!(f[0].sql_type, "INTEGER");
    }

    #[test]
    fn colliding_columns_are_suffixed() {
        let f = infer(vec![json!({"userId": 1, "user_id": 2})]);
        let cols: Vec<_> = f.iter().map(|m| m.target_column.as_str()).collect();
        assert_eq!(cols, ["user_id", "user_id_2"]);
    }
}
