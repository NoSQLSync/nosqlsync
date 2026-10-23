# Changelog

All notable changes to nosqlsync are recorded here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and versions follow
[Semantic Versioning](https://semver.org/spec/v2.0.0.html). Before 1.0, a minor
version (0.x.0) may change behaviour or the plan file format.

## [0.1.3] - 2026-10-24

### Fixed

- **Booleans:** a `BOOLEAN` column loaded the string `"false"` as `true`
  (JavaScript truthiness). Strings now load as `true` or `false` only when they
  clearly mean it (`true`/`false`, `t`/`f`, `yes`/`no`, `y`/`n`, `on`/`off`,
  `1`/`0`, any case), an empty string loads as `NULL`, and anything else is
  reported as a value that couldn't be converted.
- **Nested types:** fields inside flattened objects and child tables now get
  the same `BIGINT` (Int64), `NUMERIC` (Decimal128) and `UUID` types as
  top-level fields. Before, an `Int64` above 2³¹ in a child table made the load
  fail, and a `Decimal128` lost precision.
- **MongoDB connection errors:** a clear message for connection refused, an
  unknown host, rejected credentials and TLS failures, naming the hosts but
  never the password. It now arrives in 10 seconds instead of 30, and a
  timeout set in the connection string still takes precedence.

## [0.1.2] - 2026-10-23

### Added

- Documentation in [`docs/`](docs/README.md): getting started, sources, how the
  schema is chosen, the plan file, loading and resuming, validation, every
  command and option, and troubleshooting for every error message.
- API documentation for every public item in `nosqlsync-core`, with a
  crate-level example that runs as a test.
- A README for `nosqlsync-core` aimed at library users.
- Keywords and a full README for each `@nosqlsync/<platform>` npm package.
- `CONTRIBUTING.md` and `SECURITY.md`.

### Changed

- Help text for `analyze --format` and `validate --plan`, which had none.
- READMEs use absolute links, so they work on npm and crates.io.

## [0.1.1] - 2026-10-23

### Added

- Published to crates.io as `nosqlsync` (the CLI) and `nosqlsync-core` (the
  engine): `cargo install nosqlsync`.
- Homebrew tap: `brew install nosqlsync/tap/nosqlsync`, updated automatically
  on each release.
- npm packages: `npx nosqlsync` and `npm install -g nosqlsync`, with prebuilt
  binaries for macOS and Linux and npm provenance.

### Changed

- The CLI's crate is renamed from `nosqlsync-cli` to `nosqlsync`.

## [0.1.0] - 2026-10-23

First release.

- `analyze`, `plan`, `migrate` and `validate` for MongoDB → PostgreSQL.
- Sources: MongoDB, JSON and NDJSON files, DynamoDB JSON exports.
- Embedded objects flattened into columns, arrays of subdocuments as child
  tables, reference columns indexed.
- Loads with `COPY`, one transaction per batch with a checkpoint, so an
  interrupted load resumes exactly.
- Validation of exact counts and sampled rows, with Markdown or JSON reports.
- Binaries for macOS (Apple Silicon and Intel) and Linux (x86_64 and arm64).

[0.1.3]: https://github.com/NoSQLSync/nosqlsync/compare/v0.1.2...v0.1.3
[0.1.2]: https://github.com/NoSQLSync/nosqlsync/compare/v0.1.1...v0.1.2
[0.1.1]: https://github.com/NoSQLSync/nosqlsync/compare/v0.1.0...v0.1.1
[0.1.0]: https://github.com/NoSQLSync/nosqlsync/releases/tag/v0.1.0
