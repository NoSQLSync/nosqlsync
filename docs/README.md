# nosqlsync documentation

nosqlsync migrates MongoDB to PostgreSQL with a relational schema: it reads
your documents, proposes tables you can review and edit, loads them with
`COPY`, and checks the result.

**Start here:** [Getting started](getting-started.md) takes you from install
to a validated migration in about ten minutes.

## Guides

- [Getting started](getting-started.md): install, analyze, plan, migrate, validate
- [Sources](sources.md): MongoDB connection strings, JSON and NDJSON files, DynamoDB exports
- [How the schema is chosen](schema-mapping.md): types, nested documents, child tables, indexes
- [The plan file](plan-file.md): what's in `nosqlsync.plan.json` and how to edit it
- [Loading and resuming](loading.md): batches, checkpoints, `--fresh`, and what is never overwritten
- [Validation](validation.md): what `validate` compares, and what passed, failed and incomplete mean
- [Troubleshooting](troubleshooting.md): every error message, what causes it, and the fix

## Reference

- [Commands and options](commands.md): every flag, environment variable and exit code
- [nosqlsync-core on docs.rs](https://docs.rs/nosqlsync-core): the Rust library behind the CLI

## Scope

nosqlsync does a one-time bulk migration from MongoDB (or exported JSON) to
PostgreSQL. It does not keep the target in sync afterwards. For a
zero-downtime cutover, where PostgreSQL follows live changes until you switch
over, [NoSQLSync Cloud](https://nosqlsync.com/?ref=docs) runs change data
capture from the same plan.
