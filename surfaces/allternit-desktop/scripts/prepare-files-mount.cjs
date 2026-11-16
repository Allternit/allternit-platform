/**
 * Stage the Files drive mount helper (`allternit-files-mount`) into resources/bin/.
 *
 * Gizzi finds it next to its own binary: `gizzi files mount` (FUSE on Linux,
 * WebDAV on macOS) and Files uploads over 4 MiB run it
 * (cmd/gizzi-code/src/cli/files/drive.ts mountBinary). Only `gizzi` goes on
 * PATH; the helper never does.
 *
 * If a release job already built/lipo'd it here, this is a no-op. Otherwise
 * cargo build --release -p allternit-files-mount for the host and copy.
 *
 * macOS and Linux only: Windows gets the Files drive through the cfapi helper.
 */

'use strict';

const fs = require('fs');
const path = require('path');
const { spawnSync } = require('child_process');

const desktopDir = path.resolve(__dirname, '..');
const repoRoot = path.resolve(desktopDir, '..', '..');
const { cargoBinaryCandidates } = require('./cargo-target.cjs');
const resourcesBin = path.join(desktopDir, 'resources', 'bin');
const name = 'allternit-files-mount';
const dest = path.join(resourcesBin, name);

function log(message) {
  process.stdout.write(`[prepare-files-mount] ${message}\n`);
}

function alreadyStaged() {
  try {
    return fs.existsSync(dest) && fs.statSync(dest).size > 256 * 1024;
  } catch {
    return false;
  }
}

function findBuilt() {
  for (const candidate of cargoBinaryCandidates(repoRoot, name, { releaseOnly: true })) {
    if (fs.existsSync(candidate) && fs.statSync(candidate).isFile()) return candidate;
  }
  return null;
}

function cargoBuild() {
  log('cargo build --release -p allternit-files-mount');
  const result = spawnSync('cargo', ['build', '--release', '-p', 'allternit-files-mount'], {
    cwd: repoRoot,
    stdio: 'inherit',
    env: process.env,
  });
  if (result.status !== 0) {
    throw new Error(`cargo build -p allternit-files-mount exited ${result.status}`);
  }
}

function main() {
  if (process.platform === 'win32') {
    log('skipped on Windows (the Files drive uses the cfapi helper there)');
    return;
  }
  if (alreadyStaged()) {
    log(`already staged ${dest}`);
    return;
  }
  let source = findBuilt();
  if (!source) {
    cargoBuild();
    source = findBuilt();
  }
  if (!source) {
    throw new Error('allternit-files-mount binary not found after cargo build');
  }
  fs.mkdirSync(resourcesBin, { recursive: true });
  fs.copyFileSync(source, dest);
  fs.chmodSync(dest, 0o755);
  log(`staged ${source} -> ${dest}`);
}

try {
  main();
} catch (err) {
  process.stderr.write(`[prepare-files-mount] ✗ ${err.message}\n`);
  process.exit(1);
}
