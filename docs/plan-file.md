# The plan file

`nosqlsync plan` writes `nosqlsync.plan.json`: every decision about every
collection's tables. `migrate` and `validate` read only the plan, so the
tables that get created are exactly the tables someone reviewed. Commit it
with your migration code.

`plan` also writes `nosqlsync.schema.sql`, the `CREATE TABLE` and
`CREATE INDEX` statements the plan produces. That file is for review; you
don't need to run it, `migrate` creates the tables itself.

## Shape

An abridged plan for an `orders` collection:

```json
{
  "version": 1,
  "generatedBy": "nosqlsync 0.1.1",
  "dialect": "postgresql",
  "collections": [
    {
      "collection": "orders",
      "table": "orders",
      "documentsSampled": 1000,
      "decisions": {
        "address": "flatten",
        "items": "child-table"
      },
      "fields": [
        {
          "sourceField": "_id",
          "targetColumn": "id",
          "mongoTypes": ["objectId"],
          "resolvedMongoType": "objectId",
          "sqlType": "VARCHAR(24)",
          "nullable": false,
          "isPrimaryKey": true,
          "sampleValues": ["64b7f0c2a1e4f3b2c1d0e9f1"],
          "nullFraction": 0
        },
        {
          "sourceField": "address.city",
          "targetColumn": "address_city",
          "mongoTypes": ["string"],
          "resolvedMongoType": "string",
          "sqlType": "TEXT",
          "nullable": true,
          "isPrimaryKey": false,
          "sampleValues": [],
          "nullFraction": 0
        }
      ],
      "childTables": [
        {
          "sourcePath": "items",
          "tableName": "orders_items",
          "parentIdColumn": "orders_id",
          "fields": [
            { "sourceField": "sku", "targetColumn": "sku", "sqlType": "TEXT", "…": "…" }
          ]
        }
      ],
      "indexes": [
        { "name": "idx_orders_customer_id", "column": "customer_id", "reason": "objectid-reference" }
      ]
    }
  ]
}
```

### Collection fields

| Field | Meaning |
|---|---|
| `collection` | the source collection, or the file name without its extension |
| `table` | the target table name |
| `documentsSampled` | how many documents the plan was inferred from |
| `decisions` | what to do with each nested path: `flatten`, `jsonb` or `child-table` (see [How the schema is chosen](schema-mapping.md#nested-documents)) |
| `fields` | the parent table's columns, in order |
| `childTables` | one entry per path decided `child-table` |
| `indexes` | indexes created after the load |

### Column fields

| Field | Meaning |
|---|---|
| `sourceField` | dot path in the document; `address.city` reads `doc.address.city` |
| `targetColumn` | the column name |
| `sqlType` | the column's PostgreSQL type |
| `resolvedMongoType` | the source type values are converted from |
| `mongoTypes` | every type seen in the sample |
| `nullable` | `false` only for the primary key |
| `isPrimaryKey` | `true` only for `_id` |
| `sampleValues` | up to three example values, for reading the plan |
| `nullFraction` | 0–1, how often the field was null or missing in the sample |

In a child table, `sourceField` is relative to one array element.

## Editing the plan

### Change a decision

Edit `decisions`, then re-run `plan` with the edited file:

```sh
nosqlsync plan --plan nosqlsync.plan.json
```

nosqlsync samples the data again and applies your decisions. A decision that
doesn't fit the data is ignored and the default is kept: you can always
choose `jsonb`, but `flatten` only works on an embedded object and
`child-table` only on an array of subdocuments.

### Rename or retype a column

Edit `targetColumn` or `sqlType` under `fields` (or under a child table's
`fields`). Re-running `plan --plan` keeps these edits, matched by
`sourceField`. The `table` name you set is kept too.

You can also edit a plan and pass it straight to `migrate` without
re-running `plan`.

Whatever you change, keep the types loadable: `migrate` converts each value
using `resolvedMongoType`, so retyping a `string` column to `INTEGER` will fail
on the first value that isn't a number. Widening is always safe: `INTEGER` to
`BIGINT`, anything to `TEXT`.

### Remove a column

Delete its entry from `fields`. The field is then not loaded. For a
top-level field, `migrate` also lists it under "fields not in the plan", so
the omission stays visible.

### Load only some collections

Delete the collection from `collections`, or keep the plan whole and pass
`-c orders,users` to `migrate` and `validate`.

## Running without a plan

`migrate` and `validate` without `--plan` infer a plan with the defaults on
the spot. That's convenient for a first try. For a real migration, write the
plan first and pass it to both, so they agree on the schema.
