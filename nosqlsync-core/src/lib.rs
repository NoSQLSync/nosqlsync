//! Schema inference and relational modeling for NoSQL → SQL migrations.
//!
//! This crate is the engine behind the `nosqlsync` CLI and is a port of the
//! TypeScript engine that runs the hosted NoSQLSync platform, so both give the
//! same answer for the same documents. It is pure: no drivers, no I/O, no
//! clock. Documents come in as [MongoDB Extended JSON] (`serde_json::Value`),
//! and everything else is plain data.
//!
//! The pipeline:
//!
//! 1. [`infer`] — walk sampled documents, resolve one SQL type per field.
//! 2. [`relational`] — find nested structure worth modeling: embedded objects
//!    to flatten, arrays of subdocuments to split into child tables, and
//!    reference fields.
//! 3. [`flatten`] and [`child_tables`] — apply those decisions to the mappings.
//! 4. [`indexes`] — index the reference columns people will JOIN on.
//! 5. [`transform`] — turn each document into typed row cells.
//! 6. [`validate`] — compare a source row against what landed in the target.
//!
//! [`plan::analyze_collection`] runs steps 1–4 for one collection and adds the
//! [`risks`] report; the resulting [`plan::CollectionPlan`] is what gets saved,
//! reviewed and executed.
//!
//! # Example
//!
//! ```
//! use nosqlsync_core::ejson::Detect;
//! use nosqlsync_core::plan::{analyze_collection, Plan};
//! use nosqlsync_core::Dialect;
//! use serde_json::json;
//!
//! let docs = vec![
//!     json!({
//!         "_id": {"$oid": "507f1f77bcf86cd799439011"},
//!         "address": {"city": "Lisbon", "zip": "1100"},
//!         "items": [{"sku": "a", "qty": 1}, {"sku": "b", "qty": 2}],
//!     }),
//!     json!({
//!         "_id": {"$oid": "507f1f77bcf86cd799439012"},
//!         "address": {"city": "Porto", "zip": "4000"},
//!         "items": [{"sku": "c", "qty": 3}],
//!     }),
//! ];
//!
//! let analysis = analyze_collection("Orders", &docs, Dialect::Postgresql, Detect::Bson, None);
//! let plan = analysis.plan;
//!
//! // `_id` becomes the primary key, `address` is flattened into columns,
//! // and the `items` array moves to its own table.
//! let columns: Vec<&str> = plan.fields.iter().map(|f| f.target_column.as_str()).collect();
//! assert_eq!(columns, ["id", "address_city", "address_zip"]);
//! assert_eq!(plan.child_tables[0].table_name, "orders_items");
//!
//! let sql = Plan::new(Dialect::Postgresql, "example", vec![plan]).sql_script();
//! assert!(sql.contains("CREATE TABLE IF NOT EXISTS \"orders\""));
//! assert!(sql.contains("FOREIGN KEY (\"orders_id\") REFERENCES \"orders\"(\"id\")"));
//! ```
//!
//! [MongoDB Extended JSON]: https://www.mongodb.com/docs/manual/reference/mongodb-extended-json/

#![warn(missing_docs)]

pub mod child_tables;
pub mod collate;
pub mod ejson;
pub mod flatten;
pub mod indexes;
pub mod infer;
pub mod plan;
pub mod relational;
pub mod risks;
pub mod transform;
pub mod types;
pub mod validate;

pub use types::{Dialect, FieldMapping, MongoType};

// Compiles and runs the code in README.md as a test, so the example shown on
// crates.io can't fall out of date.
#[doc = include_str!("../README.md")]
#[cfg(doctest)]
pub struct ReadmeDoctests;
