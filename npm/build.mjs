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
  { name: 'darwin-arm64', os: 'darwin', cpu: 'arm64', target: 'aarch64-apple-darwin' },
  { name: 'darwin-x64', os: 'darwin', cpu: 'x64', target: 'x86_64-apple-darwin' },
  { name: 'linux-arm64', os: 'linux', cpu: 'arm64', target: 'aarch64-unknown-linux-gnu', libc: 'glibc' },
  { name: 'linux-x64', os: 'linux', cpu: 'x64', target: 'x86_64-unknown-linux-gnu', libc: 'glibc' },
]

const common = {
  version,
  license: 'MIT OR Apache-2.0',
  homepage: 'https://github.com/NoSQLSync/nosqlsync',
  repository: { type: 'git', url: 'git+https://github.com/NoSQLSync/nosqlsync.git' },
  bugs: 'https://github.com/NoSQLSync/nosqlsync/issues',
}

const write = (dir, name, json) => writeFileSync(path.join(dir, name), JSON.stringify(json, null, 2) + '\n')
const licenses = (dir) => {
  for (const f of ['LICENSE-MIT', 'LICENSE-APACHE']) copyFileSync(path.join(root, f), path.join(dir, f))
}

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
    description: `The nosqlsync binary for ${p.os} ${p.cpu}. Install \`nosqlsync\` instead of this package.`,
    ...common,
    os: [p.os],
    cpu: [p.cpu],
    ...(p.libc ? { libc: [p.libc] } : {}),
    files: ['bin/nosqlsync', 'LICENSE-MIT', 'LICENSE-APACHE'],
    preferUnplugged: true,
  })
  writeFileSync(
    path.join(dir, 'README.md'),
    `# @nosqlsync/${p.name}\n\nThe [nosqlsync](https://github.com/NoSQLSync/nosqlsync) binary for ${p.os} ${p.cpu}.\nYou don't need this package directly: \`npx nosqlsync\` installs it for you.\n`
  )
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
  keywords: ['mongodb', 'postgresql', 'postgres', 'migration', 'database-migration', 'schema', 'etl', 'cli'],
  bin: { nosqlsync: 'bin/nosqlsync.js' },
  files: ['bin/nosqlsync.js', 'README.md', 'LICENSE-MIT', 'LICENSE-APACHE'],
  engines: { node: '>=18' },
  optionalDependencies: Object.fromEntries(PLATFORMS.map((p) => [`@nosqlsync/${p.name}`, version])),
})

console.log(`Built ${PLATFORMS.length + 1} packages for ${version} in ${out}`)
