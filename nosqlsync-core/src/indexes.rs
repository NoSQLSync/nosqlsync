//! Which target columns to index.
//!
//! The primary key is already indexed. What is missing after a migration is the
//! reference columns: a field holding an ObjectId almost always points at
//! another collection, and those are exactly the columns people JOIN on.
//!
//! Deliberately no foreign key constraints: collections load independently and
//! in no particular order, so a real FK would reject rows whose parent has not
//! landed yet.

use std::collections::HashSet;
use std::sync::LazyLock;

use regex::Regex;
use serde::{Deserialize, Serialize};

use crate::types::{FieldMapping, MongoType};

/// Postgres truncates identifiers at 63 bytes; MySQL allows 64.
const MAX_IDENTIFIER: usize = 63;
/// Each index costs write throughput, so this is a shortlist.
const MAX_INDEXES: usize = 10;

static REFERENCE_NAME: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?:^id$|[._]id$|[a-z0-9](?:Id|ID)$)").unwrap());

#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug)]
#[serde(rename_all = "kebab-case")]
/// Why a column was chosen for an index.
pub enum IndexReason {
    /// The column holds ObjectIds.
    ObjectidReference,
    /// A string column whose source field name looks like a reference (`user_id`, `authorId`).
    NamedReference,
}

#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
/// A single-column index on the parent table.
pub struct TargetIndex {
    /// Index name from [`index_name`].
    pub name: String,
    /// Indexed column.
    pub column: String,
    /// Why it was chosen.
    pub reason: IndexReason,
}

/// Indexes for the reference columns among `mappings`, in mapping order, at
/// most 10. The primary key is skipped, as is any column whose index name
/// collides with one already chosen.
pub fn plan_indexes(table: &str, mappings: &[FieldMapping]) -> Vec<TargetIndex> {
    let mut out = Vec::new();
    let mut used = HashSet::new();
    for m in mappings {
        if m.is_primary_key {
            continue;
        }
        let reason = match m.resolved_mongo_type {
            MongoType::ObjectId => IndexReason::ObjectidReference,
            MongoType::String if REFERENCE_NAME.is_match(&m.source_field) => {
                IndexReason::NamedReference
            }
            _ => continue,
        };
        let name = index_name(table, &m.target_column);
        if !used.insert(name.clone()) {
            continue;
        }
        out.push(TargetIndex {
            name,
            column: m.target_column.clone(),
            reason,
        });
        if out.len() >= MAX_INDEXES {
            break;
        }
    }
    out
}

/// `idx_<table>_<column>`, lowercased, with anything outside `[A-Za-z0-9_]`
/// replaced by `_`, and cut to its last 63 characters if longer.
/// Deterministic, so re-running against an existing table is a no-op.
pub fn index_name(table: &str, column: &str) -> String {
    let base: String = format!("idx_{table}_{column}")
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect::<String>()
        .to_lowercase();
    if base.len() <= MAX_IDENTIFIER {
        return base;
    }
    // Keep the tail, which carries the column — the distinguishing part.
    base[base.len() - MAX_IDENTIFIER..].to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ejson::Detect;
    use crate::infer::infer_schema;
    use crate::types::Dialect;
    use serde_json::json;

    #[test]
    fn indexes_reference_columns_only() {
        let docs = vec![
            json!({"_id": {"$oid": "507f1f77bcf86cd799439011"}, "authorId": {"$oid": "507f1f77bcf86cd799439012"}, "user_id": "u1", "paid": "no", "valid": "x"}),
        ];
        let fields = infer_schema(&docs, Dialect::Postgresql, Detect::Json).fields;
        let idx = plan_indexes("posts", &fields);
        let cols: Vec<_> = idx.iter().map(|i| (i.column.as_str(), i.reason)).collect();
        assert_eq!(
            cols,
            [
                ("author_id", IndexReason::ObjectidReference),
                ("user_id", IndexReason::NamedReference)
            ]
        );
        assert_eq!(idx[0].name, "idx_posts_author_id");
    }

    #[test]
    fn long_names_keep_the_tail() {
        let n = index_name(&"t".repeat(80), "col");
        assert_eq!(n.len(), 63);
        assert!(n.ends_with("_col"));
    }
}
