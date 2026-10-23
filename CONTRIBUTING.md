# Contributing to nosqlsync

Thanks for helping. Bug reports, documentation fixes and pull requests are all
welcome.

## Reporting a bug

[Open an issue](https://github.com/NoSQLSync/nosqlsync/issues) with:

- the command you ran, and the full output (`nosqlsync --version` too)
- where the data came from (MongoDB version, or file format) and the target
  PostgreSQL version
- if you can, a few anonymized documents that reproduce it

Remove passwords from connection strings before you paste them.

For a security problem, don't open an issue; see [SECURITY.md](SECURITY.md).

## Building

You need a stable Rust toolchain ([rustup.rs](https://rustup.rs)).

```sh
cargo build
cargo test --workspace
```

The repository is a Cargo workspace:

| Directory | Crate | What it is |
|---|---|---|
| `nosqlsync-core/` | `nosqlsync-core` | the engine: schema inference, relational modeling, row transforms, validation. Pure, no I/O. |
| `nosqlsync-cli/` | `nosqlsync` | the command-line tool: MongoDB and file sources, the PostgreSQL loader |
| `npm/` | – | the npm launcher and the script that builds the npm packages |
| `docs/` | – | the user documentation |

## Before opening a pull request

CI runs these, and a pull request can't merge until they pass:

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

CI also runs an end-to-end job that seeds MongoDB, then runs `analyze`, `plan`,
`migrate` and `validate` against PostgreSQL. To run the same thing locally,
start MongoDB and PostgreSQL, then:

```sh
mongosh mongodb://localhost:27017/shop tests/e2e/seed.js
export NOSQLSYNC_SOURCE=mongodb://localhost:27017/shop
export NOSQLSYNC_TARGET=postgres://postgres:postgres@localhost:5432/shop
cargo run -p nosqlsync -- plan
cargo run -p nosqlsync -- migrate --plan nosqlsync.plan.json
cargo run -p nosqlsync -- validate --plan nosqlsync.plan.json
```

## Guidelines

- **Keep the engine pure.** `nosqlsync-core` takes plain data in and returns
  plain data out: no database drivers, files, network or clock. I/O belongs in
  `nosqlsync-cli`.
- **Correctness first.** A change to how types are chosen or values are
  converted needs a test showing the data round-trips. A value nosqlsync can't
  convert must be reported, never dropped silently.
- **Document behaviour you change.** Every public item in `nosqlsync-core` is
  documented (CI-enforced with `#![warn(missing_docs)]`), and user-visible
  behaviour is described in `docs/`. Update both, and add a line to
  `CHANGELOG.md` under the unreleased version.
- **Keep pull requests focused.** One change per pull request is easier to
  review and to revert.

## License

By contributing, you agree that your contributions are licensed under the
project's dual license, [MIT](LICENSE-MIT) or [Apache-2.0](LICENSE-APACHE),
without any additional terms.
