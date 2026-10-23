# nosqlsync

**Migrate MongoDB to PostgreSQL with a schema you'd actually design.**

Most MongoDB → Postgres tools give you one table per collection with a `JSONB`
column for everything nested. `nosqlsync` reads your documents and proposes the
relational schema instead: embedded objects become columns, arrays of
subdocuments become child tables with foreign keys, and reference fields get
indexes. You review it as a plain JSON plan and SQL file, then load it with
`COPY`, resumably, and finish with a report that compares the source and the
target row by row.

```text
$ nosqlsync analyze -s mongodb://localhost/shop -c orders

orders → table orders (300 documents sampled)

  column           type               non-null  source field
  id               VARCHAR(24)            100%  _id PK
  address_city     TEXT                   100%  address.city
  address_zip      TEXT                   100%  address.zip
  created_at       TIMESTAMPTZ            100%  createdAt
  customer_id      VARCHAR(24)            100%  customerId
  total            TEXT                   100%  total
  …

  Relational structure
    address: flattened into columns. A consistently-shaped embedded object…
    items: child table orders_items. Repeating subdocuments with a consistent shape…
    meta: kept as JSONB. The fields of this object vary between documents…

  Indexes on reference columns: customer_id

  What will bite you
    [warn] total: Holds int 47%, double 42%, string 11%. It will be stored as TEXT (string).
    [warn] history: Array of subdocuments whose shape varies (62% consistent), so it stays JSON…
    [info] couponCode: Present in only 39% of sampled documents.
```

## Install

Download a binary for macOS (Apple Silicon or Intel) or Linux (x86_64 or
arm64) from [Releases](https://github.com/NoSQLSync/nosqlsync/releases), unpack
it, and put `nosqlsync` on your `PATH`.

Or with a Rust toolchain:

```sh
cargo install nosqlsync
```

## Use it

```sh
# 1. See what you're dealing with.
nosqlsync analyze  -s "mongodb+srv://user:pass@cluster.example.net/shop"

# 2. Write the plan (nosqlsync.plan.json) and its DDL (nosqlsync.schema.sql).
nosqlsync plan     -s "$MONGO_URL"

# 3. Load it. Interrupt it, re-run the same command, and it resumes.
nosqlsync migrate  -s "$MONGO_URL" -t "postgres://user:pass@host/db" --plan nosqlsync.plan.json

# 4. Check it. Exits 2 if anything differs.
nosqlsync validate -s "$MONGO_URL" -t "$PG_URL" --plan nosqlsync.plan.json --report report.md
```

`--source` and `--target` can also come from `NOSQLSYNC_SOURCE` and
`NOSQLSYNC_TARGET`, which keeps passwords out of your shell history.

### Sources

- **MongoDB**: any `mongodb://` or `mongodb+srv://` URI. Read with the official
  Rust driver; documents keep their exact BSON types.
- **JSON files**: a `.json` array, or `.ndjson` / `.jsonl` with one document per
  line (what `mongoexport` writes). Point `-s` at a directory to load every
  file in it, one collection per file. NDJSON is streamed, so file size isn't
  limited by memory.
- **DynamoDB exports**: lines in DynamoDB JSON (`{"Item": {"name": {"S": "Ada"}}}`,
  as written by DynamoDB's export to S3) are unwrapped automatically.

## The plan

`nosqlsync plan` writes everything it decided to `nosqlsync.plan.json`. Commit it
and review it like code. Two ways to change it:

- **Change a decision.** Each nested path has one, `flatten`, `jsonb` or
  `child-table`:
  ```json
  "decisions": { "address": "flatten", "items": "child-table", "meta": "jsonb" }
  ```
  Re-run `nosqlsync plan --plan nosqlsync.plan.json` to apply it. Your other edits
  are kept.
- **Rename or retype a column.** Edit `targetColumn` or `sqlType` under `fields`.

`migrate` and `validate` only ever use the plan, so the tables that get created
are exactly the ones you reviewed.

### How the schema is chosen

| Source | Becomes |
|---|---|
| `_id` | primary key `id` (`VARCHAR(24)` for ObjectIds, the native type otherwise) |
| embedded object, same shape in ≥ 90% of documents, ≤ 8 fields | prefixed columns: `address_city`, `address_zip` |
| array of subdocuments, consistent shape | child table `orders_items` keyed by `(orders_id, array_index)`, with `FOREIGN KEY … ON DELETE CASCADE` |
| irregular object or array, array of scalars | `JSONB` |
| field holding ObjectIds, or named like `userId` / `user_id` | indexed (created after the load) |
| field with several types | the type that holds them all: int + double → `DOUBLE PRECISION`, mixed scalars → `TEXT` |
| `Int64` (`NumberLong`), or integers beyond 32 bits | `BIGINT` |
| `Decimal128` | `NUMERIC`, loaded digit for digit, never through a float |
| `UUID` (binary subtype 4) | `UUID` |
| `Date`, BSON `Timestamp` | `TIMESTAMPTZ` |
| other binary | `BYTEA` |
| `camelCase` / `HTTPHeader` / `first name` | `camel_case` / `http_header` / `first_name` |

Every column except the primary key is nullable. A sample can't prove that a
field is present in every document, and a `NOT NULL` guessed from a sample
would reject the first document that proved it wrong.

## Loading

- **COPY, in batches.** Each batch (default 5,000 documents) is one transaction
  that writes the parent rows, the child rows and a checkpoint. Kill the process
  at any point and the target is left at a clean batch boundary.
- **Resumable.** Re-run the same command to continue. MongoDB is read in `_id`
  order and resumes after the last committed `_id`. Files resume by position,
  and refuse to resume if the file has changed since.
- **Safe by default.** `migrate` won't write into a table that already has rows it
  didn't load. Pass `--fresh` to truncate the tables and start over.
- **Nothing dropped silently.** A value that doesn't fit its column (`"n/a"` in
  an integer column) is loaded as `NULL` and counted, and fields that aren't in
  the plan are listed at the end.

On a laptop (Apple M-series, NDJSON → local PostgreSQL 14) a 200,000-document
collection with 400,000 child rows loads in under 3 seconds.

## Validation

`validate` counts rows on both sides exactly, then takes a sample of source
documents (`--compare`, default 200). It runs each one through the same
transform the load used, and compares it column by column with the target row
and each of its child rows. Timestamps compare as instants, JSON compares with
keys in sorted order, and numbers compare as numbers, so a difference in how a
value is written never counts as a mismatch. The report (`--report report.md`
or `.json`) says **passed**, **failed** or **incomplete**. A check that couldn't
fully run is reported as incomplete, never as passed.

## Scope

This is a one-time bulk migration tool, and it does that well. It does not
keep the target in sync afterwards. For a zero-downtime cutover, where the
target follows live changes until you switch over,
[NoSQLSync Cloud](https://nosqlsync.com/?ref=readme) runs change data capture
from the same plan.

Currently out of scope: targets other than PostgreSQL, Firestore's native
export format, and `_id` values of mixed types when resuming (load those
collections with `--fresh`).

## Development

```sh
cargo test --workspace
cargo clippy --workspace --all-targets
```

`nosqlsync-core` is the engine: pure, with no I/O, so it can also be compiled to
WebAssembly. `nosqlsync` (in `nosqlsync-cli/`) adds the MongoDB and file sources and the Postgres
loader.

The engine makes the same schema decisions as the hosted NoSQLSync platform: a
parity suite runs both on the same generated documents and diffs every field
mapping, relational decision, child table and index. Where this crate
deliberately differs (Int64, Decimal128, UUID, BSON Timestamp, non-ObjectId
`_id`, colliding column names), the difference is there to keep a load correct.

Issues and pull requests are welcome.

## License

Licensed under either of [Apache License, Version 2.0](LICENSE-APACHE) or
[MIT license](LICENSE-MIT) at your option.
