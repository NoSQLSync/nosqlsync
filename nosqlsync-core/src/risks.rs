//! The "here's what will bite you" report: things in the data that break or
//! degrade a NoSQL → SQL migration, found before anything is written.

use serde::{Deserialize, Serialize};

use crate::infer::Stats;
use crate::relational::{NestedKind, Recommendation, RelationalModel};
use crate::types::{to_column_name, FieldMapping, MongoType};

#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
#[serde(rename_all = "lowercase")]
/// How much a [`Risk`] matters. Orders `Warning` before `Info`.
pub enum Severity {
    /// Data may load differently than expected or a load may not resume.
    Warning,
    /// Worth knowing; nothing is lost.
    Info,
}

#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
#[serde(rename_all = "camelCase")]
/// One finding in the risk report.
pub struct Risk {
    /// Warning or info.
    pub severity: Severity,
    /// Stable machine-readable code, e.g. `mixed-types`.
    pub code: String,
    /// Source dot path the finding is about.
    pub path: String,
    /// One-sentence explanation for a person.
    pub message: String,
}

/// Words that are legal as quoted identifiers but trip people up in hand-written SQL.
const RESERVED: &[&str] = &[
    "all",
    "analyse",
    "analyze",
    "and",
    "any",
    "array",
    "as",
    "asc",
    "both",
    "case",
    "cast",
    "check",
    "collate",
    "column",
    "constraint",
    "create",
    "current_date",
    "current_time",
    "current_timestamp",
    "current_user",
    "default",
    "desc",
    "distinct",
    "do",
    "else",
    "end",
    "except",
    "false",
    "fetch",
    "for",
    "foreign",
    "from",
    "grant",
    "group",
    "having",
    "in",
    "index",
    "into",
    "is",
    "join",
    "key",
    "leading",
    "left",
    "limit",
    "natural",
    "not",
    "null",
    "offset",
    "on",
    "only",
    "or",
    "order",
    "primary",
    "references",
    "right",
    "select",
    "session_user",
    "table",
    "then",
    "to",
    "trailing",
    "true",
    "union",
    "unique",
    "user",
    "using",
    "values",
    "when",
    "where",
    "window",
    "with",
];

/// Build the risk report for one collection, warnings first.
///
/// Codes: `mixed-id-types`, `mixed-types` (non-numeric mixes only; Int with
/// Double is not flagged), `column-collision`, `irregular-array` and
/// `huge-array` (over 1000 elements on average) are warnings; `wide-integer`,
/// `sparse` (present in under 50% of documents), `reserved-word` and
/// `deep-nesting` (4 or more levels) are info. `total` is the number of
/// documents sampled.
pub fn assess(
    stats: &Stats,
    total: usize,
    mappings: &[FieldMapping],
    model: &RelationalModel,
) -> Vec<Risk> {
    let mut out = Vec::new();
    let mut push = |severity, code: &str, path: &str, message: String| {
        out.push(Risk {
            severity,
            code: code.into(),
            path: path.into(),
            message,
        })
    };

    for (path, s) in stats {
        // Type frequencies, most common first.
        let mut counts: Vec<(MongoType, usize)> = Vec::new();
        for t in &s.types {
            match counts.iter_mut().find(|(x, _)| x == t) {
                Some((_, c)) => *c += 1,
                None => counts.push((*t, 1)),
            }
        }
        counts.sort_by_key(|c| std::cmp::Reverse(c.1));
        let non_null: usize = counts.iter().map(|c| c.1).sum();

        if path == "_id" {
            if counts.len() > 1 {
                push(Severity::Warning, "mixed-id-types", path, format!(
                    "`_id` holds {} types ({}). MongoDB only compares `_id` within one type, so an interrupted load cannot resume reliably; load this collection with --fresh.",
                    counts.len(), describe(&counts, non_null)));
            }
            continue;
        }

        let numeric_only = counts
            .iter()
            .all(|(t, _)| matches!(t, MongoType::Int | MongoType::Double));
        if counts.len() > 1 && !numeric_only {
            let m = mappings.iter().find(|m| &m.source_field == path);
            let into = m
                .map(|m| {
                    format!(
                        " It will be stored as {} ({}).",
                        m.sql_type,
                        m.resolved_mongo_type.as_str()
                    )
                })
                .unwrap_or_default();
            push(
                Severity::Warning,
                "mixed-types",
                path,
                format!("Holds {}.{into}", describe(&counts, non_null)),
            );
        }
        if s.wide_int {
            push(
                Severity::Info,
                "wide-integer",
                path,
                "Has integers beyond 32 bits, so it is typed BIGINT rather than INTEGER.".into(),
            );
        }
        let presence = if total > 0 {
            s.seen_count as f64 / total as f64
        } else {
            1.0
        };
        if presence < 0.5 {
            push(
                Severity::Info,
                "sparse",
                path,
                format!(
                    "Present in only {:.0}% of sampled documents.",
                    presence * 100.0
                ),
            );
        }
    }

    // Two fields that collapse onto the same column name.
    for m in mappings {
        let natural = if m.is_primary_key {
            "id".to_string()
        } else {
            to_column_name(&m.source_field)
        };
        if natural != m.target_column && !m.source_field.contains('.') {
            push(Severity::Warning, "column-collision", &m.source_field, format!(
                "Maps to `{natural}`, which another field already uses, so it was renamed to `{}`.", m.target_column));
        }
        if RESERVED.contains(&m.target_column.as_str()) {
            push(Severity::Info, "reserved-word", &m.source_field, format!(
                "Column `{}` is an SQL keyword. It is quoted in the DDL, but hand-written queries must quote it too.", m.target_column));
        }
    }

    for s in &model.nested {
        let depth = s.path.split('.').count();
        if depth >= 4 {
            push(
                Severity::Info,
                "deep-nesting",
                &s.path,
                format!("Nested {depth} levels deep."),
            );
        }
        if s.kind == NestedKind::ObjectArray && s.recommendation == Recommendation::Jsonb {
            push(Severity::Warning, "irregular-array", &s.path, format!(
                "Array of subdocuments whose shape varies ({:.0}% consistent), so it stays JSON instead of becoming a child table.",
                s.consistency * 100.0));
        }
        if s.kind == NestedKind::ObjectArray && s.average_length > 1000.0 {
            push(Severity::Warning, "huge-array", &s.path, format!(
                "Arrays average {:.0} elements. As a child table that is {:.0} rows per document.", s.average_length, s.average_length));
        }
    }

    out.sort_by_key(|a| a.severity);
    out
}

fn describe(counts: &[(MongoType, usize)], total: usize) -> String {
    counts
        .iter()
        .map(|(t, c)| {
            format!(
                "{} {:.0}%",
                t.as_str(),
                *c as f64 * 100.0 / total.max(1) as f64
            )
        })
        .collect::<Vec<_>>()
        .join(", ")
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
    fn finds_mixed_types_collisions_and_keywords() {
        let docs = vec![
            json!({"_id": 1, "price": "9.99", "userId": "a", "user_id": "b", "order": 1}),
            json!({"_id": 2, "price": 9.99, "order": 2}),
            json!({"_id": 3, "price": 10, "order": 3}),
        ];
        let r = infer_schema(&docs, Dialect::Postgresql, Detect::Json);
        let model = analyze_relationships(&docs, "t", Dialect::Postgresql, Detect::Json);
        let risks = assess(&r.stats, 3, &r.fields, &model);
        let codes: Vec<_> = risks
            .iter()
            .map(|r| (r.code.as_str(), r.path.as_str()))
            .collect();
        assert!(codes.contains(&("mixed-types", "price")));
        assert!(codes.contains(&("column-collision", "userId")));
        assert!(codes.contains(&("reserved-word", "order")));
        assert!(codes.contains(&("sparse", "userId")));
    }
}
