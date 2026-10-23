#!/usr/bin/env node
// Launcher for `npx nosqlsync` / `npm i -g nosqlsync`.
//
// The real program is a native binary shipped in one of the
// @nosqlsync/<platform> packages. npm installs only the one whose `os` and
// `cpu` match this machine (they are optionalDependencies of this package),
// and this script runs it with the same arguments, stdio and exit code.
'use strict'

const { spawnSync } = require('node:child_process')
const fs = require('node:fs')

const PLATFORMS = {
  'darwin arm64': '@nosqlsync/darwin-arm64',
  'darwin x64': '@nosqlsync/darwin-x64',
  'linux arm64': '@nosqlsync/linux-arm64',
  'linux x64': '@nosqlsync/linux-x64',
}

function fail(message) {
  console.error(`nosqlsync: ${message}`)
  process.exit(1)
}

const key = `${process.platform} ${process.arch}`
const pkg = PLATFORMS[key]
if (!pkg) {
  fail(
    `there is no prebuilt binary for ${process.platform}/${process.arch}.\n` +
      'Install from source instead: cargo install nosqlsync'
  )
}

let binary
try {
  binary = require.resolve(`${pkg}/bin/nosqlsync`)
} catch {
  fail(
    `${pkg} is not installed. It is an optional dependency of nosqlsync, so it is\n` +
      'skipped by --omit=optional / --no-optional. Reinstall without that flag.'
  )
}

// npm normally keeps the executable bit; restore it if a packer dropped it.
try {
  fs.accessSync(binary, fs.constants.X_OK)
} catch {
  try {
    fs.chmodSync(binary, 0o755)
  } catch {
    // Read-only install; spawning below reports the real error.
  }
}

const result = spawnSync(binary, process.argv.slice(2), { stdio: 'inherit' })
if (result.error) {
  if (result.error.code === 'ENOENT' && process.platform === 'linux') {
    fail(
      'the binary could not start. The Linux builds need glibc, so they do not run on\n' +
        'musl distributions such as Alpine. Use a glibc image, or: cargo install nosqlsync'
    )
  }
  fail(result.error.message)
}
if (result.signal) {
  process.kill(process.pid, result.signal)
}
process.exit(result.status ?? 1)
