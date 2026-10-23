// Build the npm packages for one release from its binary tarballs.
//
//   node npm/build.mjs v0.1.1 <dir-with-release-tarballs> <out-dir>
//
// Writes <out-dir>/<platform>/ for each @nosqlsync/<platform> package and
// <out-dir>/nosqlsync/ for the launcher package, all at the tag's version.
// Publish the platform packages first: the launcher depends on them.
import { execFileSync } from 'node:child_process'
import { chmodSync, copyFileSync, mkdirSync, mkdtempSync, rmSync, writeFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import path from 'node:path'
import { fileURLToPath } from 'node:url'

const [tag, dist, out] = process.argv.slice(2)
if (!tag?.startsWith('v') || !dist || !out) {
  console.error('usage: node npm/build.mjs <tag, e.g. v0.1.1> <tarball-dir> <out-dir>')
  process.exit(2)
}
const version = tag.slice(1)
const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..')

// npm name → Rust target the release builds.
const PLATFORMS = [
  { name: 'darwin-arm64', os: 'darwin', cpu: 'arm64', target: 'aarch64-apple-darwin', label: 'macOS on Apple Silicon (arm64)' },
  { name: 'darwin-x64', os: 'darwin', cpu: 'x64', target: 'x86_64-apple-darwin', label: 'macOS on Intel (x64)' },
  { name: 'linux-arm64', os: 'linux', cpu: 'arm64', target: 'aarch64-unknown-linux-gnu', libc: 'glibc', label: 'Linux on arm64 (glibc)' },
  { name: 'linux-x64', os: 'linux', cpu: 'x64', target: 'x86_64-unknown-linux-gnu', libc: 'glibc', label: 'Linux on x64 (glibc)' },
]

const common = {
  version,
  license: 'MIT OR Apache-2.0',
  homepage: 'https://github.com/NoSQLSync/nosqlsync#readme',
  repository: { type: 'git', url: 'git+https://github.com/NoSQLSync/nosqlsync.git' },
  bugs: 'https://github.com/NoSQLSync/nosqlsync/issues',
}

const write = (dir, name, json) => writeFileSync(path.join(dir, name), JSON.stringify(json, null, 2) + '\n')
const licenses = (dir) => {
  for (const f of ['LICENSE-MIT', 'LICENSE-APACHE']) copyFileSync(path.join(root, f), path.join(dir, f))
}

// The README shown on each platform package's npm page. People land here from
// search or a lockfile, so it says what the package is and where to go instead.
const platformReadme = (p) => `# @nosqlsync/${p.name}

The [nosqlsync](https://www.npmjs.com/package/nosqlsync) ${version} binary for **${p.label}**.

**You don't need to install this package yourself.** It's installed
automatically, as an optional dependency, when you run:

\`\`\`sh
npx nosqlsync --help
# or
npm install -g nosqlsync
\`\`\`

npm then picks the one platform package that matches your machine:

| Package | Platform |
|---|---|
${PLATFORMS.map((q) => `| [\`@nosqlsync/${q.name}\`](https://www.npmjs.com/package/@nosqlsync/${q.name}) | ${q.label} |`).join('\n')}

The binary in this package is the one attached to the
[${tag} GitHub Release](https://github.com/NoSQLSync/nosqlsync/releases/tag/${tag}),
checked against its published SHA-256 before publishing, and the package carries
npm provenance linking it to the GitHub Actions run that built it.

## What is nosqlsync?

nosqlsync migrates MongoDB to PostgreSQL with a schema you'd actually design:
embedded objects become columns, arrays of subdocuments become child tables, and
every load is resumable and validated.

- [README and quick start](https://github.com/NoSQLSync/nosqlsync#readme)
- [Documentation](https://github.com/NoSQLSync/nosqlsync/tree/main/docs)
- [Report an issue](https://github.com/NoSQLSync/nosqlsync/issues)

Licensed under MIT or Apache-2.0.
`

rmSync(out, { recursive: true, force: true })

for (const p of PLATFORMS) {
  const dir = path.join(out, p.name)
  mkdirSync(path.join(dir, 'bin'), { recursive: true })
  const tarball = path.join(dist, `nosqlsync-${tag}-${p.target}.tar.gz`)
  const scratch = mkdtempSync(path.join(tmpdir(), 'nosqlsync-npm-'))
  execFileSync('tar', ['-xzf', tarball, '-C', scratch])
  copyFileSync(path.join(scratch, `nosqlsync-${tag}-${p.target}`, 'nosqlsync'), path.join(dir, 'bin', 'nosqlsync'))
  chmodSync(path.join(dir, 'bin', 'nosqlsync'), 0o755)
  rmSync(scratch, { recursive: true, force: true })
  licenses(dir)
  write(dir, 'package.json', {
    name: `@nosqlsync/${p.name}`,
    description: `The nosqlsync binary for ${p.label}. Install \`nosqlsync\` instead of this package.`,
    ...common,
    keywords: ['nosqlsync', 'mongodb', 'postgresql', 'migration', 'binary', p.os, p.cpu],
    os: [p.os],
    cpu: [p.cpu],
    ...(p.libc ? { libc: [p.libc] } : {}),
    files: ['bin/nosqlsync', 'LICENSE-MIT', 'LICENSE-APACHE'],
    preferUnplugged: true,
  })
  writeFileSync(path.join(dir, 'README.md'), platformReadme(p))
}

const main = path.join(out, 'nosqlsync')
mkdirSync(path.join(main, 'bin'), { recursive: true })
copyFileSync(path.join(root, 'npm', 'nosqlsync', 'bin', 'nosqlsync.js'), path.join(main, 'bin', 'nosqlsync.js'))
chmodSync(path.join(main, 'bin', 'nosqlsync.js'), 0o755)
copyFileSync(path.join(root, 'README.md'), path.join(main, 'README.md'))
licenses(main)
write(main, 'package.json', {
  name: 'nosqlsync',
  description: "Migrate MongoDB to PostgreSQL with a schema you'd actually design. Analyze, plan, migrate with COPY, validate.",
  ...common,
  keywords: [
    'mongodb', 'postgresql', 'postgres', 'migration', 'database-migration', 'data-migration',
    'mongodb-to-postgres', 'nosql', 'sql', 'schema', 'schema-design', 'etl', 'dynamodb', 'cli',
  ],
  bin: { nosqlsync: 'bin/nosqlsync.js' },
  files: ['bin/nosqlsync.js', 'README.md', 'LICENSE-MIT', 'LICENSE-APACHE'],
  engines: { node: '>=18' },
  optionalDependencies: Object.fromEntries(PLATFORMS.map((p) => [`@nosqlsync/${p.name}`, version])),
})

console.log(`Built ${PLATFORMS.length + 1} packages for ${version} in ${out}`)
