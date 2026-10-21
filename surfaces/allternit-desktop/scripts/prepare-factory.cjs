/**
 * Stage the Allternit Factory engine (`allternit-factory`) into resources/bin/.
 *
 * Desktop runs it (factory-engine-manager.ts: allternit-api's /api/factory
 * proxy forwards to it) and Gizzi finds it next to its own binary, so the two
 * ship side by side. Only `gizzi` goes on PATH; the engine never does.
 *
 * If a release job already lipo'd/copied the binary here, this is a no-op.
 * Otherwise cargo build --release -p allternit-factory for the host and copy.
 *
 * Skipped on Windows: the pane engine is Unix-only. Windows Gizzi drives a
 * Factory on another machine (SPEC §1).
 */

'use strict';

const fs = require('fs');
const path = require('path');
const { spawnSync } = require('child_process');

const desktopDir = path.resolve(__dirname, '..');
const repoRoot = path.resolve(desktopDir, '..', '..');
const { cargoBinaryCandidates } = require('./cargo-target.cjs');
const resourcesBin = path.join(desktopDir, 'resources', 'bin');
const name = 'allternit-factory';
const dest = path.join(resourcesBin, name);

function log(message) {
  process.stdout.write(`[prepare-factory] ${message}\n`);
}

function alreadyStaged() {
  try {
    return fs.existsSync(dest) && fs.statSync(dest).size > 1024 * 1024;
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
  log('cargo build --release -p allternit-factory');
  const result = spawnSync('cargo', ['build', '--release', '-p', 'allternit-factory'], {
    cwd: repoRoot,
    stdio: 'inherit',
    env: process.env,
  });
  if (result.status !== 0) {
    throw new Error(`cargo build -p allternit-factory exited ${result.status}`);
  }
}

function main() {
  if (process.platform === 'win32') {
    log('skipped on Windows (the pane engine is Unix-only)');
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
    throw new Error('allternit-factory binary not found after cargo build');
  }
  fs.mkdirSync(resourcesBin, { recursive: true });
  fs.copyFileSync(source, dest);
  fs.chmodSync(dest, 0o755);
  log(`staged ${source} -> ${dest}`);
}

try {
  main();
} catch (err) {
  process.stderr.write(`[prepare-factory] ✗ ${err.message}\n`);
  process.exit(1);
}
