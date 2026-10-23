# Changelog

All notable changes to nosqlsync are recorded here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and versions follow
[Semantic Versioning](https://semver.org/spec/v2.0.0.html). Before 1.0, a minor
version (0.x.0) may change behaviour or the plan file format.

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

[0.1.2]: https://github.com/NoSQLSync/nosqlsync/compare/v0.1.1...v0.1.2
[0.1.1]: https://github.com/NoSQLSync/nosqlsync/compare/v0.1.0...v0.1.1
[0.1.0]: https://github.com/NoSQLSync/nosqlsync/releases/tag/v0.1.0
