//! Child tables for arrays of subdocuments.
//!
//! One child table per array path, with a foreign key back to the parent row
//! and a composite primary key of (parent id, array index). Loads replace a
//! parent's full set of child rows rather than diffing elements, which keeps
//! every write idempotent.

use std::collections::HashSet;

use serde::{Deserialize, Serialize};

use crate::flatten::{is_child_tableable, unique_column, NestedDecision, NestedDecisions};
use crate::relational::{child_table_name, RelationalModel};
use crate::types::{quote_ident, to_column_name, Dialect, FieldMapping};

#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
#[serde(rename_all = "camelCase")]
pub struct ChildTable {
    /// Dot path of the array in the parent document, e.g. `items`.
    pub source_path: String,
    pub table_name: String,
    /// FK column in the child table, referencing the parent's primary key.
    pub parent_id_column: String,
    /// One mapping per subfield; `source_field` is relative to one array element.
    pub fields: Vec<FieldMapping>,
}

/// Dialects whose writers know how to load child rows.
pub fn supports_child_tables(dialect: Dialect) -> bool {
    matches!(
        dialect,
        Dialect::Postgresql | Dialect::Mysql | Dialect::Mariadb
    )
}

pub fn build_child_tables(
    model: &RelationalModel,
    decisions: &NestedDecisions,
    parent_table: &str,
    dialect: Dialect,
) -> Vec<ChildTable> {
    if !supports_child_tables(dialect) {
        return vec![];
    }
    // BTreeMap iteration is already sorted by path.
    decisions
        .iter()
        .filter(|(_, d)| **d == NestedDecision::ChildTable)
        .filter_map(|(p, _)| {
            model
                .nested
                .iter()
                .find(|s| &s.path == p)
                .filter(|s| is_child_tableable(s))
        })
        .map(|s| {
            let parent_id_column = to_column_name(&format!("{parent_table}_id"));
            let mut taken: HashSet<String> =
                [parent_id_column.clone(), "array_index".to_string()].into();
            let fields = s
                .sub_fields
                .iter()
                .map(|f| {
                    let column = unique_column(&f.name, &taken);
                    taken.insert(column.clone());
                    FieldMapping {
                        source_field: f.name.clone(),
                        target_column: column,
                        mongo_types: vec![f.mongo_type],
                        resolved_mongo_type: f.mongo_type,
                        sql_type: f.sql_type.clone(),
                        nullable: true,
                        is_primary_key: false,
                        sample_values: vec![],
                        null_fraction: 1.0 - f.presence,
                    }
                })
                .collect();
            ChildTable {
                source_path: s.path.clone(),
                table_name: child_table_name(parent_table, &s.path),
                parent_id_column,
                fields,
            }
        })
        .collect()
}

/// `CREATE TABLE` for one child table, with the FK back to the parent's current primary key.
pub fn child_table_ddl(
    table: &ChildTable,
    parent_table: &str,
    parent_pk: &FieldMapping,
    dialect: Dialect,
    if_not_exists: bool,
) -> String {
    let q = |n: &str| quote_ident(n, dialect);
    let ine = if if_not_exists { "IF NOT EXISTS " } else { "" };
    let mut lines = vec![
        format!(
            "  {} {} NOT NULL",
            q(&table.parent_id_column),
            parent_pk.sql_type
        ),
        format!("  {} INTEGER NOT NULL", q("array_index")),
    ];
    lines.extend(
        table
            .fields
            .iter()
            .map(|f| format!("  {} {}", q(&f.target_column), f.sql_type)),
    );
    lines.push(format!(
        "  PRIMARY KEY ({}, {})",
        q(&table.parent_id_column),
        q("array_index")
    ));
    lines.push(format!(
        "  FOREIGN KEY ({}) REFERENCES {}({}) ON DELETE CASCADE",
        q(&table.parent_id_column),
        q(parent_table),
        q(&parent_pk.target_column)
    ));
    format!(
        "CREATE TABLE {ine}{} (\n{}\n);",
        q(&table.table_name),
        lines.join(",\n")
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ejson::Detect;
    use crate::flatten::sanitize_decisions;
    use crate::infer::infer_schema;
    use crate::relational::analyze_relationships;
    use serde_json::json;

    #[test]
    fn builds_child_table_ddl() {
        let docs = vec![
            json!({"_id": {"$oid": "507f1f77bcf86cd799439011"}, "items": [{"sku": "a", "qty": 2}]}),
        ];
        let fields = infer_schema(&docs, Dialect::Postgresql, Detect::Json).fields;
        let model = analyze_relationships(&docs, "orders", Dialect::Postgresql, Detect::Json);
        let d = sanitize_decisions(&model, None, true);
        let tables = build_child_tables(&model, &d, "orders", Dialect::Postgresql);
        assert_eq!(tables.len(), 1);
        let ddl = child_table_ddl(&tables[0], "orders", &fields[0], Dialect::Postgresql, false);
        assert_eq!(
            ddl,
            "CREATE TABLE \"orders_items\" (\n  \"orders_id\" VARCHAR(24) NOT NULL,\n  \"array_index\" INTEGER NOT NULL,\n  \"qty\" INTEGER,\n  \"sku\" TEXT,\n  PRIMARY KEY (\"orders_id\", \"array_index\"),\n  FOREIGN KEY (\"orders_id\") REFERENCES \"orders\"(\"id\") ON DELETE CASCADE\n);"
        );
        assert!(build_child_tables(&model, &d, "orders", Dialect::Bigquery).is_empty());
    }
}
