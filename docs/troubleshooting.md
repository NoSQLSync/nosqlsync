# Troubleshooting

Errors are printed as `error: <message>`, sometimes followed by the cause
from MongoDB or PostgreSQL after a colon. Look up the first part here.

## Connecting

nosqlsync waits up to 10 seconds for a MongoDB server before giving up. To
wait longer, add `serverSelectionTimeoutMS` to the connection string, for
example `?serverSelectionTimeoutMS=30000`. The messages below name the hosts
from `--source`, and never the user or password.

### ``could not reach MongoDB at host:port (connection refused)``

Nothing is listening at that host and port. Check that `mongod` is running and
that the host and port in `--source` are right (`27017` by default).

### ``could not resolve the MongoDB host …``

The hostname doesn't exist in DNS. Check it for typos. For MongoDB Atlas, copy
the connection string again from the cluster's **Connect** dialog.

### ``MongoDB at … rejected the credentials``

Check the user and password, and that the user is defined on the database in
`authSource`, which is usually `admin`. Passwords with characters such as `@`,
`:` or `/` must be percent-encoded (`@` → `%40`).

### ``the TLS connection to MongoDB at … failed``

The connection string asks for TLS (`tls=true`, or any `mongodb+srv://` string)
and the TLS handshake failed. Either the server isn't set up for TLS, or its
certificate isn't trusted. Check the `tls` options in `--source`.

### ``could not reach MongoDB at … within 10 seconds``

The server didn't answer in time and gave no other reason. Check the host, the
port and the network path. For MongoDB Atlas, add your IP address to the
project's **Network Access** list.

### `could not parse the MongoDB connection string`

The string isn't a valid MongoDB URI. Passwords with characters such as `@`,
`:` or `/` must be percent-encoded (`@` → `%40`).

### `no database in the connection string; pass --db <name>`

The connection string has no `/database` path. Add it to the string, or pass
`--db`.

### `could not connect to PostgreSQL`

Check the host, port, user, password and database in `--target`. Managed
PostgreSQL (Supabase, Neon, RDS and others) usually needs TLS: add
`?sslmode=require` to the URL.

### `target must be a postgres:// or postgresql:// URL`

`--target` has to be a PostgreSQL URL; other databases aren't supported yet.

### ``could not use schema `name` ``

nosqlsync runs `CREATE SCHEMA IF NOT EXISTS` and sets the `search_path`. The
user needs permission to create the schema, or the schema must already exist
and be usable by the user.

## Finding the data

### ``collection `name` not found (have: …)``

A collection in `-c` doesn't exist. The message lists the ones that do. Names
are case-sensitive.

### ``source `…` is neither a mongodb:// URI nor an existing file or directory``

`--source` didn't start with `mongodb://` or `mongodb+srv://`, and no file or
directory exists at that path. Check for a typo, or quote the value.

### `no .json, .ndjson or .jsonl files in …`

The directory has no files with those extensions.

### `… line N: not valid JSON (expected one object per line)`

An NDJSON file has a line that isn't JSON, often a truncated last line from an
interrupted export. Fix or remove that line.

### ``documents have no `_id` field, so there is no primary key to load or resume by``

Every row needs a primary key, and nosqlsync uses `_id`. MongoDB always has
one; an exported file or DynamoDB export may not. Rename the key field to
`_id`.

## Loading

### ``table `name` already has rows that nosqlsync didn't load``

nosqlsync won't write into a table it didn't fill, so your data is never mixed
with a load. Use `--fresh` to empty the table and load it, or `--schema` to
load somewhere else.

### `… changed since the interrupted load, so its saved position no longer applies`

A file source was changed after an interrupted load, so resuming by position
would skip or repeat documents. Run again with `--fresh`.

### `` `_id` holds several types … resuming could skip documents. Re-run with --fresh. ``

The collection's `_id`s mix types, and MongoDB only compares `_id` within one
type, so a resume could miss documents. Load it with `--fresh`; it then runs
start to finish.

### ``the source has duplicate `_id` values``

Two documents have the same `_id`. MongoDB never allows this, but exported
files can have it. Remove the duplicates from the file.

### `COPY into … failed`

PostgreSQL rejected a batch, and nothing from that batch was written. The
cause follows the message. Common ones:

- **`value too long for type character varying(24)`**: a field typed
  `VARCHAR(24)` holds a longer value outside the sample. Set its `sqlType` to
  `TEXT` in the plan.
- **`invalid input syntax for type integer`** after you retyped a column:
  `migrate` converts values from the field's source type, so a `string` field
  retyped to `INTEGER` fails on the first non-number. Widen instead, or keep
  `TEXT`.
- **`permission denied`**: the user needs `CREATE` on the schema and
  `INSERT` on the tables.

Fix the cause and run the same command again; it resumes from the last
committed batch.

### ``N value(s) in `column` were not valid … and were loaded as NULL``

Not an error: some values didn't fit the column's type (for example `"n/a"` in
an integer column) and were loaded as `NULL`. The message shows an example. If
those values matter, retype the column (for example to `TEXT`) and reload with
`--fresh`.

### `fields not in the plan were not loaded: …`

Not an error: documents had top-level fields the plan doesn't include,
usually because the sample didn't contain them. Re-run `plan` with a larger
`--sample` (or `--sample 0` for every document), then `migrate --fresh`.

## Plans

### `… is not a valid nosqlsync plan`

The file isn't valid plan JSON, often from a hand edit. Check for a trailing
comma or a missing quote; `jq . nosqlsync.plan.json` points at the problem.

### `this version of the CLI loads PostgreSQL plans only`

The plan's `dialect` isn't `postgresql`.

## Validation

### The result is `incomplete`

`validate` couldn't fully run a check, so it won't claim the data is right.
Usually the source count failed, or there was nothing to sample. The report
says which.

## Still stuck?

[Open an issue](https://github.com/NoSQLSync/nosqlsync/issues) with the
command you ran and the full error. Remove passwords from connection strings
first.
