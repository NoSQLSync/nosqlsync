# Security policy

## Reporting a vulnerability

Please report security problems privately, not in a public issue:
**[Report a vulnerability](https://github.com/NoSQLSync/nosqlsync/security/advisories/new)**
(GitHub's private vulnerability reporting).

Include what you found, how to reproduce it, and the version
(`nosqlsync --version`). We'll acknowledge the report, keep you updated while
we fix it, and credit you in the advisory unless you'd rather not be named.

## Supported versions

nosqlsync is before 1.0, so security fixes go into the latest release only.
Upgrade to the newest version to get them.

## How releases are built

Every release is built by GitHub Actions from a tagged commit in this
repository:

- each binary on the [Releases](https://github.com/NoSQLSync/nosqlsync/releases)
  page has a `.sha256` checksum next to it
- the npm packages carry [npm provenance](https://docs.npmjs.com/generating-provenance-statements),
  linking each one to the commit and workflow run that built it
- the Homebrew formula pins each binary's SHA-256

## What nosqlsync does with your data

nosqlsync runs on your machine and talks only to the databases you point it
at. It reads from the source and never writes to it. It sends nothing to
NoSQLSync or anyone else. Connection strings can come from the
`NOSQLSYNC_SOURCE` and `NOSQLSYNC_TARGET` environment variables to keep them
out of shell history.
