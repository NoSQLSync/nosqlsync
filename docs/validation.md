# Validation

```sh
nosqlsync validate --plan nosqlsync.plan.json --report report.md
```

## What it checks

For each collection in the plan:

1. **Row counts, exactly.** It counts the documents in the source (a full
   count for MongoDB, every line for files) and the rows in the target table.
2. **Sampled rows, column by column.** It takes `--compare` documents
   (default 200), runs each through the same conversion `migrate` used, looks
   up the target row by primary key, and compares every column.
3. **Child rows.** For each sampled document, it compares the number of child
   rows in each child table, then each row's `array_index` and columns.

Sampled documents are picked at random from MongoDB (`$sample`), and evenly
spaced through the file for file sources.

Comparisons ignore differences in how a value is written, and only flag
differences in the value itself:

- timestamps compare as instants, whatever the offset or precision
- JSON compares with keys in sorted order (`JSONB` doesn't keep key order)
- numbers compare as numbers (`1.0` equals `1`), and decimals ignore trailing
  zeros (`12.30` equals `12.3`)
- binary compares as bytes

## Results

Each collection, and the run as a whole, gets one of three results:

| Result | Meaning |
|---|---|
| **passed** | counts match, and every compared row and child row matched |
| **failed** | counts differ, or a sampled document has no row in the target, or a row differs |
| **incomplete** | the check couldn't fully run (no source count, or nothing to sample), so it can't vouch for the data |

A check that couldn't fully run is never reported as passed.

`validate` exits with status `2` when the result is failed, so a CI job or
script can gate on it. It exits `0` for passed and incomplete; check the
printed result if incomplete should also stop you.

## The report

`--report report.md` writes a Markdown report: a table per collection with
counts, rows compared, missing and mismatched rows, and the result, followed by
every missing primary key and every mismatched column. `--report report.json`
writes the same as JSON, for tooling.

A report says how it was checked, and what that does and doesn't prove: a
sample can show a load is wrong, but it can't prove every row is right. To
check more rows, raise `--compare`.

## When it fails

- **Counts differ.** Usually the source changed after the load (new
  documents, deletions), or the load stopped part way. Run `migrate` again: it
  resumes an unfinished load, or skips a finished one. Use `--fresh` to reload
  from scratch.
- **Missing rows.** The document is in the source but not the target. Same
  causes as above.
- **Mismatched columns.** The value in the target differs from what the source
  converts to now. Either the document changed after it was loaded, or the
  target was modified. The report lists the primary key and the columns, so
  you can look at both.
