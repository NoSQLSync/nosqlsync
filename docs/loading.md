# Loading and resuming

```sh
nosqlsync migrate --plan nosqlsync.plan.json
```

## What happens

For each collection in the plan, in order:

1. **Create.** The target schema (`--schema`, default `public`) is created if
   it doesn't exist, then the parent table and its child tables with
   `CREATE TABLE IF NOT EXISTS`.
2. **Check.** nosqlsync looks for a checkpoint for this table (see below) and
   decides whether to start, resume, skip, or refuse.
3. **Load.** Documents are read in batches of `--batch` (default 5,000) and
   written with `COPY`. Each batch is **one transaction** containing the
   parent rows, the child rows and the updated checkpoint.
4. **Finish.** The checkpoint is marked complete, indexes are created, and the
   table is analyzed (`ANALYZE`) so the query planner has statistics.

If one collection fails, nosqlsync reports the error and carries on with the
next one, then exits with status `1`.

## Checkpoints and resuming

Progress is stored in a table in the target schema:

```sql
_nosqlsync_checkpoints (table_name, collection, cursor, rows_loaded, completed, updated_at)
```

Because the checkpoint is committed in the same transaction as the rows, an
interrupted load (Ctrl-C, a crash, a network drop, `kill -9`) always leaves
the target at a clean batch boundary that the checkpoint describes exactly.
Run the same command again and it continues from there, with no duplicates
and no gaps.

- **MongoDB** is read in `_id` order, and resumes after the last committed
  `_id`. nosqlsync copies a collection as it is while being read; it doesn't
  track changes made during the load. If the source keeps changing, run
  `validate` afterwards, or use [live sync](https://nosqlsync.com/?ref=docs)
  for a cutover.
- **Files** resume by position. The checkpoint also records the file's size
  and modification time, and nosqlsync refuses to resume if the file has
  changed, because a position in a different file would skip or repeat
  documents.

A collection whose checkpoint is complete is skipped with
"already loaded (N rows)". Running `migrate` twice is safe.

## Starting over: `--fresh`

```sh
nosqlsync migrate --plan nosqlsync.plan.json --fresh
```

`--fresh` empties each table in the plan (`TRUNCATE … CASCADE`, which also
empties its child tables), deletes its checkpoint, and loads from the start.
Use it after changing the plan, after the source changed underneath an
interrupted load, or whenever you want a clean copy.

## What nosqlsync refuses to do

- **Write into a table it didn't load.** If a target table already has rows
  but no nosqlsync checkpoint, `migrate` stops for that table rather than mix
  its rows with yours. Use `--fresh` to replace the contents, or `--schema` to
  load into a different schema.
- **Resume across mixed `_id` types.** MongoDB only compares `_id` values of
  the same type, so resuming a collection whose `_id`s mix types (for example
  ObjectIds and strings) could skip documents. nosqlsync stops and asks for
  `--fresh` instead. `analyze` warns about this up front (`mixed-id-types`).
- **Load a document without an `_id`.** Every row needs a primary key.
- **Load duplicate `_id`s.** MongoDB guarantees unique `_id`s; exported files
  don't, and a duplicate stops that collection with an explanation.

## What gets reported

At the end of each collection:

- the rows loaded, plus child rows, and the time taken
- for each column where some values couldn't be converted: how many, the
  expected type, and an example. Those values were loaded as `NULL`.
- top-level fields that appeared in the documents but aren't in the plan,
  with counts. They were not loaded. Re-run `plan` with a larger `--sample`
  to include them.

## Performance

`COPY` is the fastest way to load PostgreSQL. As a reference point, on an
Apple M-series laptop loading NDJSON into a local PostgreSQL 14, a
200,000-document collection with 400,000 child rows takes under 3 seconds.
Real numbers depend mostly on network latency and the target's disk.

Larger batches (`--batch 20000`) reduce round trips on high-latency links;
smaller ones make each checkpoint, and so each resume, finer-grained.
