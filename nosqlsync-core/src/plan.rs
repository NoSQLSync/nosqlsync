//! A migration plan: everything decided about each collection's target tables.
//!
//! The plan is a JSON file a person can read, edit, commit and review. It is the
//! single source of truth for `migrate` and `validate`, so the tables that get
//! created are exactly the tables someone signed off on.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::child_tables::{build_child_tables, child_table_ddl, supports_child_tables, ChildTable};
use crate::ejson::Detect;
use crate::flatten::{apply_nested_decisions, sanitize_decisions, NestedDecisions};
use crate::indexes::{plan_indexes, TargetIndex};
use crate::infer::infer_schema;
use crate::relational::{analyze_relationships, RelationalModel};
use crate::risks::{assess, Risk};
use crate::types::{
    generate_ddl_with, keep_saved_columns, quote_ident, to_column_name, DdlOptions, Dialect,
    FieldMapping,
};

pub const PLAN_VERSION: u32 = 1;

#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Plan {
    pub version: u32,
    pub generated_by: String,
    pub dialect: Dialect,
    pub collections: Vec<CollectionPlan>,
}

#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
#[serde(rename_all = "camelCase")]
pub struct CollectionPlan {
    pub collection: String,
    pub table: String,
    pub documents_sampled: usize,
    /// What to do with each nested path: `flatten`, `jsonb` or `child-table`.
    /// Edit this and re-run `nosqlsync plan --plan <file>` to apply it.
    pub decisions: NestedDecisions,
    pub fields: Vec<FieldMapping>,
    pub child_tables: Vec<ChildTable>,
    pub indexes: Vec<TargetIndex>,
}

/// Everything learned from one collection's sample.
pub struct Analysis {
    pub plan: CollectionPlan,
    pub model: RelationalModel,
    pub risks: Vec<Risk>,
}

/// Build (or rebuild) one collection's plan from sampled documents.
///
/// With `previous`, the user's edits survive: their decisions are re-applied,
/// and fields they renamed or retyped keep the saved column name and type.
pub fn analyze_collection(
    collection: &str,
    docs: &[Value],
    dialect: Dialect,
    mode: Detect,
    previous: Option<&CollectionPlan>,
) -> Analysis {
    let table = previous
        .map(|p| p.table.clone())
        .unwrap_or_else(|| to_column_name(collection));
    let inferred = infer_schema(docs, dialect, mode);
    let model = analyze_relationships(docs, &table, dialect, mode);
    let decisions = sanitize_decisions(
        &model,
        previous.map(|p| &p.decisions),
        supports_child_tables(dialect),
    );
    let mut fields = apply_nested_decisions(&inferred.fields, &model, &decisions);
    if let Some(prev) = previous {
        fields = keep_saved_columns(fields, &prev.fields);
    }
    let mut child_tables = build_child_tables(&model, &decisions, &table, dialect);
    if let Some(prev) = previous {
        for ct in &mut child_tables {
            if let Some(saved) = prev
                .child_tables
                .iter()
                .find(|s| s.source_path == ct.source_path)
            {
                ct.fields = keep_saved_columns(std::mem::take(&mut ct.fields), &saved.fields);
            }
        }
    }
    let indexes = plan_indexes(&table, &fields);
    let risks = assess(&inferred.stats, docs.len(), &fields, &model);
    Analysis {
        plan: CollectionPlan {
            collection: collection.to_string(),
            table,
            documents_sampled: docs.len(),
            decisions,
            fields,
            child_tables,
            indexes,
        },
        model,
        risks,
    }
}

impl CollectionPlan {
    pub fn primary_key(&self) -> Option<&FieldMapping> {
        self.fields.iter().find(|f| f.is_primary_key)
    }

    /// `CREATE TABLE` for the parent, then each child table.
    pub fn create_statements(&self, dialect: Dialect, if_not_exists: bool) -> Vec<String> {
        let mut out = vec![generate_ddl_with(
            &self.table,
            &self.fields,
            dialect,
            DdlOptions { if_not_exists },
        )];
        if let Some(pk) = self.primary_key() {
            for ct in &self.child_tables {
                out.push(child_table_ddl(ct, &self.table, pk, dialect, if_not_exists));
            }
        }
        out
    }

    pub fn index_statements(&self, dialect: Dialect) -> Vec<String> {
        self.indexes
            .iter()
            .map(|i| {
                format!(
                    "CREATE INDEX IF NOT EXISTS {} ON {} ({});",
                    quote_ident(&i.name, dialect),
                    quote_ident(&self.table, dialect),
                    quote_ident(&i.column, dialect)
                )
            })
            .collect()
    }
}

impl Plan {
    pub fn new(dialect: Dialect, generated_by: &str, collections: Vec<CollectionPlan>) -> Self {
        Plan {
            version: PLAN_VERSION,
            generated_by: generated_by.to_string(),
            dialect,
            collections,
        }
    }

    /// A complete, reviewable SQL script for the whole plan.
    pub fn sql_script(&self) -> String {
        let mut s = format!(
            "-- Generated by {}. Review before running.\n",
            self.generated_by
        );
        for c in &self.collections {
            s.push_str(&format!(
                "\n-- {} ({} documents sampled)\n",
                c.collection, c.documents_sampled
            ));
            for stmt in c.create_statements(self.dialect, true) {
                s.push_str(&stmt);
                s.push_str("\n\n");
            }
            for stmt in c.index_statements(self.dialect) {
                s.push_str(&stmt);
                s.push('\n');
            }
        }
        s
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::flatten::NestedDecision;
    use serde_json::json;

    fn docs() -> Vec<Value> {
        vec![
            json!({"_id": {"$oid": "507f1f77bcf86cd799439011"}, "customerId": {"$oid": "507f1f77bcf86cd799439099"}, "address": {"city": "A", "zip": "1"}, "items": [{"sku": "a", "qty": 1}]}),
            json!({"_id": {"$oid": "507f1f77bcf86cd799439012"}, "customerId": {"$oid": "507f1f77bcf86cd799439098"}, "address": {"city": "B", "zip": "2"}, "items": [{"sku": "b", "qty": 2}]}),
        ]
    }

    #[test]
    fn relational_by_default() {
        let a = analyze_collection("Orders", &docs(), Dialect::Postgresql, Detect::Bson, None);
        let p = &a.plan;
        assert_eq!(p.table, "orders");
        let cols: Vec<_> = p.fields.iter().map(|f| f.target_column.as_str()).collect();
        assert_eq!(cols, ["id", "address_city", "address_zip", "customer_id"]);
        assert_eq!(p.child_tables[0].table_name, "orders_items");
        assert_eq!(p.indexes[0].column, "customer_id");
        let sql = Plan::new(Dialect::Postgresql, "test", vec![p.clone()]).sql_script();
        assert!(sql.contains("CREATE TABLE IF NOT EXISTS \"orders_items\""));
        assert!(sql.contains("CREATE INDEX IF NOT EXISTS \"idx_orders_customer_id\""));
    }

    #[test]
    fn replanning_keeps_user_edits() {
        let first =
            analyze_collection("orders", &docs(), Dialect::Postgresql, Detect::Bson, None).plan;
        let mut edited = first.clone();
        edited
            .decisions
            .insert("items".into(), NestedDecision::Jsonb);
        edited
            .fields
            .iter_mut()
            .find(|f| f.target_column == "customer_id")
            .unwrap()
            .target_column = "buyer_id".into();
        let again = analyze_collection(
            "orders",
            &docs(),
            Dialect::Postgresql,
            Detect::Bson,
            Some(&edited),
        )
        .plan;
        assert!(again.child_tables.is_empty());
        assert!(again
            .fields
            .iter()
            .any(|f| f.target_column == "items" && f.sql_type == "JSONB"));
        assert!(again.fields.iter().any(|f| f.target_column == "buyer_id"));
    }
}
