# homebrew-core formula (not submitted yet)

`nosqlsync.rb` here is the formula for submitting nosqlsync to
[homebrew-core](https://github.com/Homebrew/homebrew-core), which is what makes
a plain `brew install nosqlsync` work and lists it on brew.sh. It is kept here
until the project qualifies. Until then, people install from our tap:
`brew install nosqlsync/tap/nosqlsync`.

It differs from the tap's formula on purpose: homebrew-core builds everything
from source, so this one compiles the tagged source with Rust rather than
downloading the prebuilt release binaries.

## When it qualifies

Homebrew's audit rejects a new GitHub-hosted formula unless the repository:

- is at least **30 days old** (this one was created 2026-10-23), and
- has at least **75 stars, 30 forks or 30 watchers**. These thresholds triple
  (225 / 90 / 90) when the pull request comes from the repository's owner.

## How to submit

1. Point `url` at the latest release tag and update `sha256`:
   ```sh
   curl -sL https://github.com/NoSQLSync/nosqlsync/archive/refs/tags/vX.Y.Z.tar.gz | shasum -a 256
   ```
2. Check it builds from source and passes the strict new-formula audit:
   ```sh
   brew install --build-from-source ./nosqlsync.rb
   brew test nosqlsync
   brew audit --new --strict --online nosqlsync
   ```
3. Open the pull request against Homebrew/homebrew-core, as Homebrew's
   [guide](https://docs.brew.sh/Adding-Software-to-Homebrew) describes.
4. Once it is merged, `brew install nosqlsync` works everywhere. Keep the tap
   for one release or two, then point the README at the plain command.
