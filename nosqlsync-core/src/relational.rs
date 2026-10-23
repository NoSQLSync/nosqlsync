//! Relationship inference.
//!
//! Field inference treats every nested value as one opaque JSON column: a
//! faithful copy of the documents, and a poor relational model. This module
//! answers the harder question of which nested structures should *become*
//! relational structure. An array of subdocuments is almost always a child
//! table, a consistently shaped embedded object is usually flattenable into
//! columns, an irregular blob really is JSON, and a repeated ObjectId is a
//! foreign key waiting to be declared.

use std::collections::HashSet;
use std::sync::LazyLock;

use indexmap::IndexMap;
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::collate::locale_cmp;
use crate::ejson::{detect, Detect};
use crate::types::{to_column_name, to_sql_type, Dialect, MongoType};

#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug)]
#[serde(rename_all = "kebab-case")]
pub enum NestedKind {
    EmbeddedObject,
    ObjectArray,
    ScalarArray,
}

#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug)]
#[serde(rename_all = "kebab-case")]
pub enum Recommendation {
    Flatten,
    ChildTable,
    Jsonb,
}

#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
#[serde(rename_all = "camelCase")]
pub struct SubField {
    pub name: String,
    pub mongo_type: MongoType,
    pub sql_type: String,
    /// Fraction of the containing structures where this field is present, 0–1.
    pub presence: f64,
}

#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
#[serde(rename_all = "camelCase")]
pub struct FlattenedColumn {
    pub path: String,
    pub column: String,
    pub sql_type: String,
}

#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
#[serde(rename_all = "camelCase")]
pub struct NestedStructure {
    pub path: String,
    pub kind: NestedKind,
    /// Fraction of sampled documents containing this path, 0–1.
    pub frequency: f64,
    /// Mean presence of its subfields, 0–1.
    pub consistency: f64,
    /// Mean element count, for arrays.
    pub average_length: f64,
    pub sub_fields: Vec<SubField>,
    pub recommendation: Recommendation,
    pub reason: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub child_table_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub flattened_columns: Option<Vec<FlattenedColumn>>,
}

#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug)]
#[serde(rename_all = "lowercase")]
pub enum Confidence {
    High,
    Medium,
}

#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
#[serde(rename_all = "camelCase")]
pub struct ForeignKeyCandidate {
    pub path: String,
    pub column: String,
    /// Best guess at the collection this points at, from the field name.
    pub references_collection: Option<String>,
    pub confidence: Confidence,
    pub reason: String,
}

#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
#[serde(rename_all = "camelCase")]
pub struct RelationalModel {
    pub root_table: String,
    pub documents_sampled: usize,
    pub nested: Vec<NestedStructure>,
    pub foreign_keys: Vec<ForeignKeyCandidate>,
}

/// Below this, a shape is irregular enough that columns would be mostly NULL.
const CONSISTENT_ENOUGH: f64 = 0.9;
/// Flattening more than this many columns makes the parent table unwieldy.
const MAX_FLATTEN_FIELDS: usize = 8;
/// Nested structures below this frequency are too rare to model.
const MIN_FREQUENCY: f64 = 0.05;

#[derive(Default)]
struct SubFieldStats {
    types: Vec<MongoType>,
    seen: usize,
}

struct StructureStats {
    kind: NestedKind,
    docs_containing: usize,
    instances: usize,
    total_elements: usize,
    sub_fields: IndexMap<String, SubFieldStats>,
}

#[derive(Default)]
struct FkStats {
    hits: usize,
    seen: usize,
}

struct Walk {
    structures: IndexMap<String, StructureStats>,
    fk: IndexMap<String, FkStats>,
    mode: Detect,
}

pub fn analyze_relationships(
    docs: &[Value],
    root_table: &str,
    dialect: Dialect,
    mode: Detect,
) -> RelationalModel {
    let mut w = Walk {
        structures: IndexMap::new(),
        fk: IndexMap::new(),
        mode,
    };
    for doc in docs {
        if let Value::Object(obj) = doc {
            let mut seen = HashSet::new();
            w.walk(obj, "", &mut seen);
        }
    }

    let total = docs.len();
    let mut nested: Vec<NestedStructure> = w
        .structures
        .iter()
        .map(|(path, s)| build_structure(path, s, total, dialect, root_table))
        .filter(|s| s.frequency >= MIN_FREQUENCY)
        .collect();
    nested.sort_by(|a, b| {
        b.frequency
            .partial_cmp(&a.frequency)
            .unwrap()
            .then_with(|| locale_cmp(&a.path, &b.path))
    });

    let mut foreign_keys: Vec<ForeignKeyCandidate> =
        w.fk.iter()
            .filter_map(|(path, s)| build_foreign_key(path, s))
            .collect();
    foreign_keys.sort_by(|a, b| locale_cmp(&a.path, &b.path));

    RelationalModel {
        root_table: root_table.to_string(),
        documents_sampled: total,
        nested,
        foreign_keys,
    }
}

impl Walk {
    fn walk(&mut self, obj: &Map<String, Value>, prefix: &str, seen: &mut HashSet<String>) {
        for (key, raw) in obj {
            let path = if prefix.is_empty() {
                key.clone()
            } else {
                format!("{prefix}.{key}")
            };
            let t = detect(raw, self.mode);

            if path != "_id" {
                let s = self.fk.entry(path.clone()).or_default();
                s.seen += 1;
                if t == MongoType::ObjectId || (t == MongoType::String && looks_like_reference(key))
                {
                    s.hits += 1;
                }
            }

            match (t, raw) {
                (MongoType::Array, Value::Array(elements)) => {
                    self.record_array(&path, elements, seen)
                }
                (MongoType::Object, Value::Object(inner)) => {
                    self.record_object(&path, inner, seen);
                    // Recurse so `address.geo` is modeled as well as `address`.
                    self.walk(inner, &path, seen);
                }
                _ => {}
            }
        }
    }

    fn record_array(&mut self, path: &str, elements: &[Value], seen: &mut HashSet<String>) {
        let objects: Vec<&Map<String, Value>> = elements
            .iter()
            .filter_map(|el| match el {
                Value::Object(o) if detect(el, self.mode) == MongoType::Object => Some(o),
                _ => None,
            })
            .collect();
        let kind = if objects.is_empty() {
            NestedKind::ScalarArray
        } else {
            NestedKind::ObjectArray
        };
        let mode = self.mode;
        let stats = ensure_stats(&mut self.structures, path, kind);
        if seen.insert(path.to_string()) {
            stats.docs_containing += 1;
        }
        stats.total_elements += elements.len();
        for el in objects {
            stats.instances += 1;
            record_sub_fields(stats, el, mode);
        }
    }

    fn record_object(&mut self, path: &str, obj: &Map<String, Value>, seen: &mut HashSet<String>) {
        let mode = self.mode;
        let stats = ensure_stats(&mut self.structures, path, NestedKind::EmbeddedObject);
        if seen.insert(path.to_string()) {
            stats.docs_containing += 1;
        }
        stats.instances += 1;
        record_sub_fields(stats, obj, mode);
    }
}

fn record_sub_fields(stats: &mut StructureStats, obj: &Map<String, Value>, mode: Detect) {
    for (k, v) in obj {
        let sub = stats.sub_fields.entry(k.clone()).or_default();
        sub.seen += 1;
        let t = detect(v, mode);
        if t != MongoType::Null {
            sub.types.push(t);
        }
    }
}

fn ensure_stats<'a>(
    structures: &'a mut IndexMap<String, StructureStats>,
    path: &str,
    kind: NestedKind,
) -> &'a mut StructureStats {
    let s = structures
        .entry(path.to_string())
        .or_insert_with(|| StructureStats {
            kind,
            docs_containing: 0,
            instances: 0,
            total_elements: 0,
            sub_fields: IndexMap::new(),
        });
    // An array holding objects in one document and scalars in another is an
    // object array as far as modeling goes — the objects are the interesting part.
    if kind == NestedKind::ObjectArray && s.kind == NestedKind::ScalarArray {
        s.kind = kind;
    }
    s
}

fn build_structure(
    path: &str,
    s: &StructureStats,
    total: usize,
    dialect: Dialect,
    root: &str,
) -> NestedStructure {
    let mut sub_fields: Vec<SubField> = s
        .sub_fields
        .iter()
        .map(|(name, st)| {
            let t = dominant_type(&st.types);
            SubField {
                name: name.clone(),
                mongo_type: t,
                sql_type: to_sql_type(t, dialect).to_string(),
                presence: if s.instances > 0 {
                    st.seen as f64 / s.instances as f64
                } else {
                    0.0
                },
            }
        })
        .collect();
    sub_fields.sort_by(|a, b| {
        b.presence
            .partial_cmp(&a.presence)
            .unwrap()
            .then_with(|| locale_cmp(&a.name, &b.name))
    });

    let consistency = if sub_fields.is_empty() {
        0.0
    } else {
        sub_fields.iter().map(|f| f.presence).sum::<f64>() / sub_fields.len() as f64
    };
    let frequency = if total > 0 {
        s.docs_containing as f64 / total as f64
    } else {
        0.0
    };
    let average_length = if s.docs_containing > 0 {
        s.total_elements as f64 / s.docs_containing as f64
    } else {
        0.0
    };
    let (recommendation, reason) = recommend(s.kind, consistency, sub_fields.len());

    let child_table_name =
        (recommendation == Recommendation::ChildTable).then(|| child_table_name(root, path));
    let flattened_columns = (recommendation == Recommendation::Flatten).then(|| {
        sub_fields
            .iter()
            .map(|f| FlattenedColumn {
                path: format!("{path}.{}", f.name),
                column: to_column_name(&format!("{path}_{}", f.name)),
                sql_type: f.sql_type.clone(),
            })
            .collect()
    });

    NestedStructure {
        path: path.to_string(),
        kind: s.kind,
        frequency,
        consistency,
        average_length,
        sub_fields,
        recommendation,
        reason,
        child_table_name,
        flattened_columns,
    }
}

fn recommend(kind: NestedKind, consistency: f64, field_count: usize) -> (Recommendation, String) {
    let r = |rec, s: &str| (rec, s.to_string());
    match kind {
        NestedKind::ScalarArray => r(
            Recommendation::Jsonb,
            "An array of scalars has no internal structure to normalise. Keep it as JSON, or split it into a child table later if you need to query the elements individually.",
        ),
        NestedKind::ObjectArray if consistency >= CONSISTENT_ENOUGH => r(
            Recommendation::ChildTable,
            "Repeating subdocuments with a consistent shape. This is a one-to-many relationship: a child table with a foreign key back to the parent is the relational form.",
        ),
        NestedKind::ObjectArray => r(
            Recommendation::Jsonb,
            "The subdocuments in this array vary too much in shape for a useful child table — most columns would be NULL. Keep it as JSON until the shape settles.",
        ),
        NestedKind::EmbeddedObject if consistency >= CONSISTENT_ENOUGH && field_count <= MAX_FLATTEN_FIELDS => r(
            Recommendation::Flatten,
            "A consistently-shaped embedded object. Flattening it into prefixed columns makes the fields directly queryable and indexable, with no join.",
        ),
        NestedKind::EmbeddedObject if field_count > MAX_FLATTEN_FIELDS => (
            Recommendation::Jsonb,
            format!(
                "Consistent, but {field_count} fields is too many to flatten without bloating the parent table. Keep it as JSON, or promote it to its own table."
            ),
        ),
        NestedKind::EmbeddedObject => r(
            Recommendation::Jsonb,
            "The fields of this object vary between documents, so flattened columns would be sparse. Keep it as JSON.",
        ),
    }
}

/// Most frequent type; ties go to the type seen first.
fn dominant_type(types: &[MongoType]) -> MongoType {
    let mut counts: IndexMap<MongoType, usize> = IndexMap::new();
    for t in types {
        *counts.entry(*t).or_default() += 1;
    }
    let mut best = MongoType::Null;
    let mut best_count = 0;
    for (t, c) in counts {
        if c > best_count {
            best = t;
            best_count = c;
        }
    }
    best
}

fn build_foreign_key(path: &str, s: &FkStats) -> Option<ForeignKeyCandidate> {
    if s.seen == 0 || s.hits == 0 {
        return None;
    }
    // Require the field to look like a reference nearly every time we see it.
    let ratio = s.hits as f64 / s.seen as f64;
    if ratio < CONSISTENT_ENOUGH {
        return None;
    }
    let leaf = path.rsplit('.').next().unwrap_or(path);
    let certain = ratio == 1.0;
    Some(ForeignKeyCandidate {
        path: path.to_string(),
        column: to_column_name(path),
        references_collection: guess_collection(leaf),
        confidence: if certain { Confidence::High } else { Confidence::Medium },
        reason: if certain {
            "Holds an ObjectId in every sampled document, which in MongoDB almost always points at another collection."
        } else {
            "Named like a reference and holds identifier-shaped values."
        }
        .to_string(),
    })
}

static ID_SUFFIX_I: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)[._]?id$").unwrap());
static ID_SUFFIX: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"Id$|ID$").unwrap());
static PLURAL_ES: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(s|x|z|ch|sh)$").unwrap());
static REFERENCE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?:^id$|[._]id$|[a-z0-9](?:Id|ID)$)").unwrap());

/// `authorId` → `authors`, `user_id` → `users`, `companyId` → `companies`.
pub fn guess_collection(field: &str) -> Option<String> {
    let base = ID_SUFFIX_I.replace(field, "");
    let base = ID_SUFFIX.replace(&base, "");
    if base.is_empty() {
        return None;
    }
    let lower = base.to_lowercase();
    if let Some(stem) = lower.strip_suffix('y') {
        return Some(format!("{stem}ies"));
    }
    if PLURAL_ES.is_match(&lower) {
        return Some(format!("{lower}es"));
    }
    Some(format!("{lower}s"))
}

/// Prefixed with the root table so two collections that both have an `items`
/// array don't collide on one child table.
pub fn child_table_name(root: &str, path: &str) -> String {
    to_column_name(&format!("{root}_{}", path.replace('.', "_")))
}

pub fn looks_like_reference(field: &str) -> bool {
    REFERENCE.is_match(field)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn model(docs: Vec<Value>) -> RelationalModel {
        analyze_relationships(&docs, "orders", Dialect::Postgresql, Detect::Json)
    }

    #[test]
    fn consistent_object_array_becomes_child_table() {
        let m = model(vec![
            json!({"_id": "507f1f77bcf86cd799439011", "items": [{"sku": "a", "qty": 1}, {"sku": "b", "qty": 2}]}),
            json!({"_id": "507f1f77bcf86cd799439012", "items": [{"sku": "c", "qty": 3}]}),
        ]);
        let items = m.nested.iter().find(|s| s.path == "items").unwrap();
        assert_eq!(items.kind, NestedKind::ObjectArray);
        assert_eq!(items.recommendation, Recommendation::ChildTable);
        assert_eq!(items.child_table_name.as_deref(), Some("orders_items"));
        assert!((items.average_length - 1.5).abs() < 1e-9);
    }

    #[test]
    fn consistent_object_flattens_and_irregular_stays_json() {
        let m = model(vec![
            json!({"address": {"city": "A", "zip": "1"}, "meta": {"a": 1}}),
            json!({"address": {"city": "B", "zip": "2"}, "meta": {"b": 1}}),
        ]);
        let addr = m.nested.iter().find(|s| s.path == "address").unwrap();
        assert_eq!(addr.recommendation, Recommendation::Flatten);
        assert_eq!(
            addr.flattened_columns.as_ref().unwrap()[0].column,
            "address_city"
        );
        let meta = m.nested.iter().find(|s| s.path == "meta").unwrap();
        assert_eq!(meta.recommendation, Recommendation::Jsonb);
    }

    #[test]
    fn foreign_keys_from_objectids_and_names() {
        let m = model(vec![
            json!({"customerId": {"$oid": "507f1f77bcf86cd799439011"}, "category_id": "c1", "paid": "yes"}),
        ]);
        let paths: Vec<_> = m.foreign_keys.iter().map(|f| f.path.as_str()).collect();
        assert_eq!(paths, ["category_id", "customerId"]);
        assert_eq!(
            m.foreign_keys[1].references_collection.as_deref(),
            Some("customers")
        );
        assert_eq!(guess_collection("companyId").as_deref(), Some("companies"));
        assert_eq!(guess_collection("boxId").as_deref(), Some("boxes"));
        assert_eq!(guess_collection("id"), None);
    }
}
