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
//! [MongoDB Extended JSON]: https://www.mongodb.com/docs/manual/reference/mongodb-extended-json/

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
