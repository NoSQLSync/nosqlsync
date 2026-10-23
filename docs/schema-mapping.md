# How the schema is chosen

nosqlsync looks at a sample of each collection (1,000 documents by default)
and proposes one table per collection, plus child tables where they fit.
Every choice below ends up in the [plan file](plan-file.md), where you can
change it.

## Tables and columns

- **One table per collection**, named after it in `snake_case`
  (`OrderItems` → `order_items`).
- **Column names** are converted the same way: `createdAt` → `created_at`,
  `userID` → `user_id`, `HTTPReferer` → `http_referer`, `first name` →
  `first_name`. A name starting with a digit gets a leading underscore
  (`2fa` → `_2fa`).
- **Collisions.** If two fields end up with the same column name (`userId`
  and `user_id`), the second gets a suffix (`user_id_2`) and `analyze`
  reports it.
- **Primary key.** `_id` becomes the column `id`, `NOT NULL PRIMARY KEY`.
- **Everything else is nullable.** A sample can't prove that a field is in
  every document, and a `NOT NULL` guessed from a sample would reject the
  first document that proved it wrong.
- **Column order.** `id` first, then the other columns alphabetically.

## Types

| Source type | PostgreSQL type |
|---|---|
| `ObjectId` | `VARCHAR(24)`, the hex string |
| `String` | `TEXT` |
| `Int32` | `INTEGER` |
| `Int64` (`NumberLong`), or any integer beyond 32 bits | `BIGINT` |
| `Double` | `DOUBLE PRECISION` |
| `Decimal128` | `NUMERIC`, loaded digit for digit, never through a float |
| `Boolean` | `BOOLEAN` |
| `Date`, BSON `Timestamp` | `TIMESTAMPTZ` |
| `UUID` (binary subtype 4) | `UUID` |
| other binary | `BYTEA` |
| object or array kept whole | `JSONB` |
| `null` only | `TEXT` |

The `_id` column is `VARCHAR(24)` for ObjectIds; an `_id` of another type
keeps that type (a long string `_id` becomes `TEXT`, a numeric one `INTEGER`
or `BIGINT`).

The `BIGINT`, `NUMERIC` and `UUID` choices apply to every column, including
fields inside a flattened object or a child table.

### Fields with more than one type

When a field holds different types in different documents, it gets the type
that can hold all of them:

- integers and doubles → `DOUBLE PRECISION`
- any mix that includes an object or an array → `JSONB`
- any other mix of scalars → the first of binary, string, double, integer,
  boolean, date, ObjectId that appears, so in practice usually `TEXT`

`analyze` reports these as `mixed-types`, with how often each type appears.

## Nested documents

This is where nosqlsync differs from a plain copy. For each nested path it
decides between three things:

| Decision | Used for | Result |
|---|---|---|
| `flatten` | an embedded object with the same fields in ≥ 90% of documents, and at most 8 fields | prefixed columns: `address.city` → `address_city` |
| `child-table` | an array of subdocuments with the same fields in ≥ 90% of elements | its own table, one row per element |
| `jsonb` | everything else: irregular objects, objects with more than 8 fields, arrays of scalars, irregular arrays | one `JSONB` column |

"The same fields" is measured as consistency: the average, over an object's
fields, of how often each field is present. Nested paths found in fewer than
5% of documents get no decision: a top-level one becomes a `JSONB` column, and
a deeper one stays inside its parent's value.

Flattening applies at every depth, so `address.geo.lat` can become
`address_geo_lat` when both `address` and `address.geo` are flattened.

### Child tables

An array of subdocuments at `items` in `orders` becomes:

```sql
CREATE TABLE "orders_items" (
  "orders_id" VARCHAR(24) NOT NULL,
  "array_index" INTEGER NOT NULL,
  "qty" INTEGER,
  "sku" TEXT,
  PRIMARY KEY ("orders_id", "array_index"),
  FOREIGN KEY ("orders_id") REFERENCES "orders"("id") ON DELETE CASCADE
);
```

- The table is named `<parent>_<path>`, the foreign key column `<parent>_id`.
- `array_index` keeps the element's position in the array, so order is never
  lost.
- Elements that aren't objects (a stray scalar in an array of objects) are
  skipped.
- The array's column is removed from the parent table; the data lives only in
  the child table.

## Indexes

nosqlsync indexes the columns you will most likely join on:

- fields holding ObjectIds (other than `_id`), which in MongoDB almost always
  point at another collection
- string fields named like references: `userId`, `user_id`, `authorID`,
  `user.id`

At most 10 per table, named `idx_<table>_<column>`. They are created **after**
the bulk load, which is much faster than loading into an indexed table.

nosqlsync does not create foreign keys between collections. Collections load
independently and in no fixed order, so a foreign key would reject rows whose
parent hasn't loaded yet. Add them yourself once the data is in, if you want
them; `analyze --format json` lists the candidates it found.

## How values are converted

Each value is converted to its column's type as it loads:

- **Integers** keep every digit, including `Int64` values above 2⁵³.
  Fractional values in an integer column are truncated.
- **Dates** accept BSON dates, ISO-8601 strings, `YYYY-MM-DD`, and epoch
  milliseconds.
- **Booleans** accept `true` and `false`, numbers (`0` is false, any other
  number true), and the strings `true`/`false`, `t`/`f`, `yes`/`no`, `y`/`n`,
  `on`/`off` and `1`/`0` in any case. An empty string loads as `NULL`. Any other
  value, such as `"maybe"`, is reported rather than guessed.
- **Text** columns store strings as they are, and numbers and booleans as
  their text. Objects and arrays in a text column are stored as JSON.
- **JSONB** columns store the value with Extended JSON unwrapped: ObjectIds
  as hex strings, dates as ISO strings, `NumberLong` as numbers.

A value that can't be converted, such as the string `"n/a"` in an integer
column, is loaded as `NULL`. `migrate` counts these per column and prints an
example at the end, so nothing is lost silently.
