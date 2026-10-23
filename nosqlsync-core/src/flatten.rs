//! Acting on the relational model.
//!
//! A consistently shaped embedded object becomes prefixed columns
//! (`address` → `address_city`, `address_zip`): directly queryable and
//! indexable, with no JSON operators. A flattened field is just a mapping whose
//! `source_field` is a dot path, so the row transform needs no special case.

use std::collections::{BTreeMap, HashSet};

use serde::{Deserialize, Serialize};

use crate::collate::locale_cmp;
use crate::relational::{NestedKind, NestedStructure, Recommendation, RelationalModel};
use crate::types::{to_column_name, FieldMapping};

#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug)]
#[serde(rename_all = "kebab-case")]
pub enum NestedDecision {
    Flatten,
    Jsonb,
    ChildTable,
}

/// Decisions by dot path, e.g. `{ "address": "flatten", "tags": "jsonb" }`.
pub type NestedDecisions = BTreeMap<String, NestedDecision>;

pub fn is_flattenable(s: &NestedStructure) -> bool {
    s.kind == NestedKind::EmbeddedObject && !s.sub_fields.is_empty()
}

pub fn is_child_tableable(s: &NestedStructure) -> bool {
    s.kind == NestedKind::ObjectArray && !s.sub_fields.is_empty()
}

/// The model's own recommendation, for paths the user has not decided:
/// relational by default, not JSON by default.
pub fn default_decisions(model: &RelationalModel, child_tables: bool) -> NestedDecisions {
    let mut d = NestedDecisions::new();
    for s in &model.nested {
        if is_flattenable(s) {
            let v = if s.recommendation == Recommendation::Flatten {
                NestedDecision::Flatten
            } else {
                NestedDecision::Jsonb
            };
            d.insert(s.path.clone(), v);
        } else if child_tables && is_child_tableable(s) {
            let v = if s.recommendation == Recommendation::ChildTable {
                NestedDecision::ChildTable
            } else {
                NestedDecision::Jsonb
            };
            d.insert(s.path.clone(), v);
        }
    }
    d
}

/// Requested decisions, restricted to paths that can actually take them.
pub fn sanitize_decisions(
    model: &RelationalModel,
    requested: Option<&NestedDecisions>,
    child_tables: bool,
) -> NestedDecisions {
    let mut d = default_decisions(model, child_tables);
    let Some(requested) = requested else { return d };
    for (path, decision) in requested {
        if !d.contains_key(path) {
            continue;
        }
        let s = model.nested.iter().find(|s| &s.path == path).unwrap();
        let ok = match decision {
            NestedDecision::Jsonb => true,
            NestedDecision::Flatten => is_flattenable(s),
            NestedDecision::ChildTable => is_child_tableable(s),
        };
        if ok {
            d.insert(path.clone(), *decision);
        }
    }
    d
}

/// Expand every `flatten` path into one mapping per subfield and drop the JSON
/// column of every `child-table` path (its data moves to its own table).
///
/// Shallower paths go first, so flattening both `address` and `address.geo`
/// works: the first pass leaves `address.geo` as JSON next to its siblings,
/// and the second expands it.
pub fn apply_nested_decisions(
    mappings: &[FieldMapping],
    model: &RelationalModel,
    decisions: &NestedDecisions,
) -> Vec<FieldMapping> {
    let find = |p: &str| model.nested.iter().find(|s| s.path == p);
    let mut to_flatten: Vec<&NestedStructure> = decisions
        .iter()
        .filter(|(_, d)| **d == NestedDecision::Flatten)
        .filter_map(|(p, _)| find(p).filter(|s| is_flattenable(s)))
        .collect();
    to_flatten.sort_by(|a, b| {
        depth(&a.path)
            .cmp(&depth(&b.path))
            .then_with(|| locale_cmp(&a.path, &b.path))
    });

    let child_paths: HashSet<&str> = decisions
        .iter()
        .filter(|(p, d)| {
            **d == NestedDecision::ChildTable && find(p).is_some_and(is_child_tableable)
        })
        .map(|(p, _)| p.as_str())
        .collect();

    let mut result: Vec<FieldMapping> = mappings
        .iter()
        .filter(|m| !child_paths.contains(m.source_field.as_str()))
        .cloned()
        .collect();
    for s in to_flatten {
        result = expand_one(result, s);
    }
    result
}

fn depth(path: &str) -> usize {
    path.split('.').count()
}

fn expand_one(mappings: Vec<FieldMapping>, s: &NestedStructure) -> Vec<FieldMapping> {
    // Not mapped — it may sit under a parent that stayed JSON — so flattening
    // it would invent columns from nothing.
    let Some(index) = mappings.iter().position(|m| m.source_field == s.path) else {
        return mappings;
    };
    let parent = &mappings[index];
    if parent.is_primary_key {
        return mappings;
    }
    let mut taken: HashSet<String> = mappings.iter().map(|m| m.target_column.clone()).collect();
    taken.remove(&parent.target_column);

    let expanded: Vec<FieldMapping> = s
        .sub_fields
        .iter()
        .map(|f| {
            let column = unique_column(&format!("{}_{}", s.path, f.name), &taken);
            taken.insert(column.clone());
            FieldMapping {
                source_field: format!("{}.{}", s.path, f.name),
                target_column: column,
                mongo_types: vec![f.mongo_type],
                resolved_mongo_type: f.mongo_type,
                sql_type: f.sql_type.clone(),
                // The parent object can be absent entirely; never NOT NULL.
                nullable: true,
                is_primary_key: false,
                sample_values: vec![],
                null_fraction: 1.0 - f.presence,
            }
        })
        .collect();

    let mut out = mappings[..index].to_vec();
    out.extend(expanded);
    out.extend_from_slice(&mappings[index + 1..]);
    out
}

pub(crate) fn unique_column(raw: &str, taken: &HashSet<String>) -> String {
    let base = to_column_name(raw);
    if !taken.contains(&base) {
        return base;
    }
    let mut n = 2;
    while taken.contains(&format!("{base}_{n}")) {
        n += 1;
    }
    format!("{base}_{n}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ejson::Detect;
    use crate::infer::infer_schema;
    use crate::relational::analyze_relationships;
    use crate::types::Dialect;
    use serde_json::json;

    #[test]
    fn flattens_nested_objects_in_depth_order() {
        let docs = vec![
            json!({"_id": 1, "address": {"city": "A", "geo": {"lat": 1.5, "lng": 2.5}}}),
            json!({"_id": 2, "address": {"city": "B", "geo": {"lat": 3.5, "lng": 4.5}}}),
        ];
        let fields = infer_schema(&docs, Dialect::Postgresql, Detect::Json).fields;
        let model = analyze_relationships(&docs, "users", Dialect::Postgresql, Detect::Json);
        let d = sanitize_decisions(&model, None, true);
        let out = apply_nested_decisions(&fields, &model, &d);
        let cols: Vec<_> = out.iter().map(|m| m.target_column.as_str()).collect();
        assert_eq!(
            cols,
            ["id", "address_city", "address_geo_lat", "address_geo_lng"]
        );
        assert_eq!(out[2].sql_type, "DOUBLE PRECISION");
    }

    #[test]
    fn user_can_keep_json_and_bad_requests_are_ignored() {
        let docs = vec![json!({"address": {"city": "A"}, "tags": ["x"]})];
        let model = analyze_relationships(&docs, "t", Dialect::Postgresql, Detect::Json);
        let mut req = NestedDecisions::new();
        req.insert("address".into(), NestedDecision::Jsonb);
        req.insert("tags".into(), NestedDecision::Flatten);
        let d = sanitize_decisions(&model, Some(&req), true);
        assert_eq!(d.get("address"), Some(&NestedDecision::Jsonb));
        assert_eq!(d.get("tags"), None);
    }
}
