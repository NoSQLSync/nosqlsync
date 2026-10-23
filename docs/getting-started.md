# Getting started

This walks through one complete migration: install, look at the data, review
the schema, load it, and check it.

## 1. Install

Pick one:

```sh
npx nosqlsync --version                 # no install; needs Node.js 18+
brew install nosqlsync/tap/nosqlsync    # macOS or Linux
cargo install nosqlsync                 # with a Rust toolchain
```

Or download a binary for macOS (Apple Silicon or Intel) or Linux (x86_64 or
arm64) from [Releases](https://github.com/NoSQLSync/nosqlsync/releases).

The examples below write `nosqlsync`; with npx, write `npx nosqlsync`.

## 2. Point it at your data

Keep connection strings out of your shell history by putting them in the
environment:

```sh
export NOSQLSYNC_SOURCE="mongodb+srv://user:pass@cluster.example.net/shop"
export NOSQLSYNC_TARGET="postgres://user:pass@db.example.com:5432/shop?sslmode=require"
```

The source can also be a JSON or NDJSON file, or a directory of them, such as
the output of `mongoexport`. See [Sources](sources.md).

nosqlsync only reads from the source. It never writes to MongoDB.

## 3. Analyze

```sh
nosqlsync analyze
```

nosqlsync samples up to 1,000 documents per collection (`--sample`) and
prints, for each one:

- the columns it would create, with their PostgreSQL types and how often each
  field is present
- the relational structure it found: embedded objects it will flatten into
  columns, arrays of subdocuments it will turn into child tables, and what it
  will keep as `JSONB`
- the reference columns it will index
- **what will bite you**: fields holding several types, sparse fields, names
  that collide once converted to `snake_case`, SQL keywords, irregular arrays

Nothing is written anywhere. Use `-c orders,users` to look at specific
collections only.

## 4. Plan

```sh
nosqlsync plan
```

This writes two files:

- `nosqlsync.plan.json`: every decision, per collection. Edit it, commit it,
  review it like code.
- `nosqlsync.schema.sql`: the `CREATE TABLE` and `CREATE INDEX` statements
  the plan produces, for review.

To change a decision, for example to keep `meta` as `JSONB` instead of
flattening it, edit `decisions` in the plan and run
`nosqlsync plan --plan nosqlsync.plan.json`. To rename or retype a column,
edit it under `fields`. See [The plan file](plan-file.md).

## 5. Migrate

```sh
nosqlsync migrate --plan nosqlsync.plan.json
```

nosqlsync creates the tables (in the `public` schema unless you pass
`--schema`) and loads each collection with `COPY`, 5,000 documents per batch.
Each batch is committed together with a checkpoint, so you can stop it at any
point, with Ctrl-C or a crash, and run the same command again to continue.
See [Loading and resuming](loading.md).

At the end it reports, per collection, any value that didn't fit its column
(loaded as `NULL`, never dropped silently) and any field it saw that isn't in
the plan.

## 6. Validate

```sh
nosqlsync validate --plan nosqlsync.plan.json --report report.md
```

This compares row counts exactly, then checks 200 documents per collection
(`--compare`) column by column against the rows in PostgreSQL, including
their child rows. It exits with status `2` if anything differs, so you can use
it in CI. See [Validation](validation.md).

## Next

- Run the whole thing against a copy of production first, and keep the plan
  file with your migration code.
- If something goes wrong, [Troubleshooting](troubleshooting.md) lists every
  error and its fix.
