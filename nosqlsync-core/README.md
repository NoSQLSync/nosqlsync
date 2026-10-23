# nosqlsync-core

Schema inference and relational modeling for NoSQL → SQL migrations: the
engine behind the [`nosqlsync`](https://crates.io/crates/nosqlsync) CLI.

Give it sampled MongoDB documents as Extended JSON and it gives back a
relational schema you'd actually design. Embedded objects become columns,
arrays of subdocuments become child tables with foreign keys, and reference
fields get indexes. It also gives you the code to turn each document into
typed rows, encode them for PostgreSQL `COPY`, and check what landed.

It's pure: no database drivers, no I/O, no clock. That makes it easy to embed,
test, and compile to WebAssembly.

**Want the command-line tool instead?** `cargo install nosqlsync`, or see the
[nosqlsync README](https://github.com/NoSQLSync/nosqlsync#readme).

## Example

```rust
use nosqlsync_core::ejson::Detect;
use nosqlsync_core::plan::{analyze_collection, Plan};
use nosqlsync_core::Dialect;
use serde_json::json;

let docs = vec![
    json!({
        "_id": {"$oid": "507f1f77bcf86cd799439011"},
        "address": {"city": "Lisbon", "zip": "1100"},
        "items": [{"sku": "a", "qty": 1}, {"sku": "b", "qty": 2}],
    }),
    json!({
        "_id": {"$oid": "507f1f77bcf86cd799439012"},
        "address": {"city": "Porto", "zip": "4000"},
        "items": [{"sku": "c", "qty": 3}],
    }),
];

let analysis = analyze_collection("Orders", &docs, Dialect::Postgresql, Detect::Bson, None);
let plan = analysis.plan;

// `_id` becomes the primary key, `address` is flattened into columns,
// and the `items` array moves to its own table.
let columns: Vec<&str> = plan.fields.iter().map(|f| f.target_column.as_str()).collect();
assert_eq!(columns, ["id", "address_city", "address_zip"]);
assert_eq!(plan.child_tables[0].table_name, "orders_items");

let sql = Plan::new(Dialect::Postgresql, "example", vec![plan]).sql_script();
assert!(sql.contains("FOREIGN KEY (\"orders_id\") REFERENCES \"orders\"(\"id\")"));
```

`analysis.risks` lists what will cause trouble in a migration (mixed types,
sparse fields, column-name collisions, SQL keywords, irregular arrays), and
`analysis.model` holds every nested structure found with the reason for its
recommendation.

## Modules

| Module | What it does |
|---|---|
| `infer` | per-field type inference and column naming |
| `relational` | finds nested structure worth modeling, and foreign-key candidates |
| `flatten`, `child_tables` | apply flatten / child-table / JSONB decisions |
| `indexes` | picks the reference columns to index |
| `plan` | the editable migration plan and its SQL |
| `transform` | documents → typed cells, and PostgreSQL `COPY` text encoding |
| `validate` | compares source rows with target rows, and the report verdict |
| `risks` | the "what will bite you" report |
| `ejson` | Extended JSON detection and unwrapping, input parsing, DynamoDB JSON |
| `types` | source types, SQL dialects and type maps, DDL |

Full API documentation: [docs.rs/nosqlsync-core](https://docs.rs/nosqlsync-core).

## Dialects

Type mapping and DDL generation cover PostgreSQL, MySQL, MariaDB, BigQuery,
Snowflake, Redshift, ClickHouse and SQL Server. Child tables are produced for
PostgreSQL, MySQL and MariaDB. The `nosqlsync` CLI currently loads
PostgreSQL only.

## License

Licensed under either of [Apache License, Version 2.0](https://github.com/NoSQLSync/nosqlsync/blob/main/LICENSE-APACHE)
or [MIT license](https://github.com/NoSQLSync/nosqlsync/blob/main/LICENSE-MIT) at your option.
