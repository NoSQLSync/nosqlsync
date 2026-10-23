# Commands and options

```text
nosqlsync analyze   Sample collections and report the inferred schema and what will bite you
nosqlsync plan      Design the target tables and write an editable plan plus its SQL
nosqlsync migrate   Bulk-load collections into PostgreSQL with COPY. Resumable
nosqlsync validate  Check counts and compare sampled rows field by field
```

`nosqlsync --version` prints the version; `nosqlsync <command> --help` prints
a command's options.

## Options every command takes

| Option | Default | Meaning |
|---|---|---|
| `-s`, `--source <SOURCE>` | `$NOSQLSYNC_SOURCE` | a `mongodb://` or `mongodb+srv://` connection string, or a `.json` / `.ndjson` / `.jsonl` file, or a directory of them. See [Sources](sources.md). Required. |
| `--db <DB>` | from the connection string | the MongoDB database, when the connection string doesn't name one |
| `-c`, `--collections <LIST>` | all | only these collections, comma-separated |
| `--sample <N>` | `1000` | documents sampled per collection to infer the schema; `0` reads all of them |

## `analyze`

```sh
nosqlsync analyze -s "$MONGO_URL" [--format text|json|markdown]
```

Prints the inferred columns, relational structure, indexes, and risks for each
collection. Writes nothing.

| Option | Default | Meaning |
|---|---|---|
| `--format <FORMAT>` | `text` | `text` for the terminal, `markdown` for docs and pull requests, `json` for tooling |

The JSON output is an array with one entry per collection:
`{ "collection", "plan", "model", "risks" }`. `plan` has the same shape as a
collection in the [plan file](plan-file.md); `model` holds every nested path
found, with its kind, frequency, consistency, recommendation and reason, plus
the foreign-key candidates; `risks` is the list below.

### Risks

| Code | Level | Meaning |
|---|---|---|
| `mixed-types` | warning | the field holds several types; the message gives how often each appears and the column type chosen |
| `mixed-id-types` | warning | `_id` holds several types, so an interrupted load can't resume; load with `--fresh` |
| `column-collision` | warning | the field's column name was already taken, so it was renamed with a suffix |
| `irregular-array` | warning | an array of subdocuments whose shape varies too much for a child table, so it stays `JSONB` |
| `huge-array` | warning | arrays average more than 1,000 elements: as a child table, that many rows per document |
| `wide-integer` | info | `Int64` values or integers beyond 32 bits, so the column is `BIGINT` |
| `sparse` | info | present in fewer than half of the sampled documents |
| `reserved-word` | info | the column name is an SQL keyword; it's quoted in the DDL, but hand-written queries must quote it too |
| `deep-nesting` | info | nested four or more levels deep |

## `plan`

```sh
nosqlsync plan -s "$MONGO_URL" [--out FILE] [--sql FILE] [--plan EXISTING]
```

| Option | Default | Meaning |
|---|---|---|
| `-o`, `--out <FILE>` | `nosqlsync.plan.json` | where to write the plan |
| `--sql <FILE>` | `nosqlsync.schema.sql` | where to write the DDL script |
| `--plan <FILE>` | none | an existing plan whose decisions, renames and retypes to keep |

See [The plan file](plan-file.md).

## `migrate`

```sh
nosqlsync migrate -s "$MONGO_URL" -t "$PG_URL" --plan nosqlsync.plan.json [--batch N] [--fresh]
```

| Option | Default | Meaning |
|---|---|---|
| `-t`, `--target <URL>` | `$NOSQLSYNC_TARGET` | `postgres://user:pass@host:port/db`; honors `sslmode` (for example `?sslmode=require`). Required. |
| `--schema <NAME>` | `public` | schema to create the tables in; created if missing |
| `--plan <FILE>` | none | the plan to load with; without one, a plan is inferred with the defaults |
| `--batch <N>` | `5000` | documents per `COPY` batch, and per checkpoint |
| `--fresh` | off | empty the target tables and load from scratch |

See [Loading and resuming](loading.md).

## `validate`

```sh
nosqlsync validate -s "$MONGO_URL" -t "$PG_URL" --plan nosqlsync.plan.json [--compare N] [--report FILE]
```

| Option | Default | Meaning |
|---|---|---|
| `-t`, `--target <URL>` | `$NOSQLSYNC_TARGET` | as for `migrate`. Required. |
| `--schema <NAME>` | `public` | the schema the tables are in |
| `--plan <FILE>` | none | the plan to validate against; without one, a plan is inferred with the defaults |
| `--compare <N>` | `200` | documents compared per collection |
| `--report <FILE>` | none | write a report; `.json` writes JSON, anything else Markdown |

See [Validation](validation.md).

## Environment variables

| Variable | Used as |
|---|---|
| `NOSQLSYNC_SOURCE` | default for `--source` |
| `NOSQLSYNC_TARGET` | default for `--target` |

Setting these keeps connection strings and passwords out of your shell
history, and `--help` doesn't display their values.

## Exit codes

| Code | Meaning |
|---|---|
| `0` | success; for `validate`, passed or incomplete |
| `1` | an error, or for `migrate`, at least one collection failed |
| `2` | `validate` found differences (failed), or the command line was invalid |
