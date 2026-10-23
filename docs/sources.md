# Sources

`--source` (or `NOSQLSYNC_SOURCE`) is either a MongoDB connection string or a
path to files. nosqlsync only ever reads from the source.

## MongoDB

Any `mongodb://` or `mongodb+srv://` connection string works, including
MongoDB Atlas.

```sh
nosqlsync analyze -s "mongodb+srv://user:pass@cluster.example.net/shop"
```

- **Database.** Taken from the path of the connection string (`/shop`), or
  from `--db shop` when the string doesn't include one.
- **Collections.** By default, every collection in the database. Views and
  `system.*` collections are skipped. Pick specific ones with
  `-c orders,users`.
- **Types.** Documents are read with the official MongoDB Rust driver, so
  every BSON type keeps its exact type: `ObjectId`, `Date`, `Int32`, `Int64`,
  `Double`, `Decimal128`, `Binary` and UUID, `Timestamp` and others. See
  [How the schema is chosen](schema-mapping.md).
- **Sampling.** `analyze` and `plan` use MongoDB's `$sample` to pick up to
  `--sample` documents (default 1,000) at random. `--sample 0` reads every
  document.
- **Loading.** `migrate` reads each collection in `_id` order, which uses the
  `_id` index that every collection has.
- **Permissions.** A user with the `read` role on the database is enough.

## JSON and NDJSON files

```sh
nosqlsync analyze -s orders.ndjson
nosqlsync analyze -s ./export/          # one collection per file
```

Accepted files:

- a `.json` file holding a JSON array of documents, or a single document
  (pretty-printed is fine)
- a `.ndjson` or `.jsonl` file with one document per line, which is what
  `mongoexport` writes by default

A directory loads every `.json`, `.ndjson` and `.jsonl` file in it, each as
its own collection named after the file (`orders.ndjson` becomes `orders`).

**Size.** NDJSON is read line by line, so files of any size work. A `.json`
array is parsed whole, so it has to fit in memory; prefer NDJSON for large
exports.

**Types.** Plain JSON has no dates or ObjectIds, so nosqlsync reads the same
signals `mongoexport` and most APIs produce:

- Extended JSON wrappers such as `{"$oid": "…"}`, `{"$date": "…"}` and
  `{"$numberLong": "…"}`
- strings shaped like an ISO-8601 timestamp (`2024-03-01T10:00:00Z`) are
  treated as dates
- 24-character hexadecimal strings are treated as ObjectIds

**Sampling.** For files, `--sample N` reads the first N documents.

## DynamoDB exports

DynamoDB JSON, as written by DynamoDB's export to S3 or by `aws dynamodb
scan`, is unwrapped automatically:

```json
{"Item": {"id": {"S": "u1"}, "age": {"N": "36"}, "tags": {"SS": ["a", "b"]}}}
```

becomes `{"id": "u1", "age": 36, "tags": ["a", "b"]}`. All attribute types are
handled (`S`, `N`, `BOOL`, `NULL`, `M`, `L`, `SS`, `NS`, `B`, `BS`), with or
without the `Item` wrapper.

Every row needs a primary key, which comes from a field named `_id`. A
DynamoDB export uses its own key attribute names, so for now rename the key
attribute to `_id` in the export before loading it.
