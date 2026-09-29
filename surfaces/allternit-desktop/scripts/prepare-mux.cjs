/**
 * Stage allternit-mux (PTY daemon gizzi auto-spawns for /pty) into
 * resources/bin/. GizziManager resolves resources/bin/allternit-mux first.
 *
 * If a release job already lipo'd/copied the binary here, this is a no-op.
 * Otherwise cargo-build --release -p allternit-mux for the host and copy.
 *
 * Skipped on Windows: the mux API is a Unix socket and gizzi's /pty
 * integration dials it, so there is nothing a Windows build could serve.
 */

'use strict';

const fs = require('fs');
const path = require('path');
const { spawnSync } = require('child_process');

const desktopDir = path.resolve(__dirname, '..');
const repoRoot = path.resolve(desktopDir, '..', '..');
const { cargoBinaryCandidates } = require('./cargo-target.cjs');
const resourcesBin = path.join(desktopDir, 'resources', 'bin');
const destName = process.platform === 'win32' ? 'allternit-mux.exe' : 'allternit-mux';
const dest = path.join(resourcesBin, destName);
const cargoName = process.platform === 'win32' ? 'allternit-mux.exe' : 'allternit-mux';

function log(message) {
  process.stdout.write(`[prepare-mux] ${message}\n`);
}

function alreadyStaged() {
  try {
    return fs.existsSync(dest) && fs.statSync(dest).size > 100 * 1024;
  } catch {
    return false;
  }
}

function cargoCandidates() {
  return [
    ...cargoBinaryCandidates(repoRoot, cargoName, { releaseOnly: true }),
    path.join(repoRoot, 'cmd', 'allternit-mux', 'target', 'release', cargoName),
  ];
}

function findBuilt() {
  for (const candidate of cargoCandidates()) {
    if (fs.existsSync(candidate) && fs.statSync(candidate).isFile()) return candidate;
  }
  return null;
}

function cargoBuild() {
  log('cargo build --release -p allternit-mux');
  const result = spawnSync('cargo', ['build', '--release', '-p', 'allternit-mux'], {
    cwd: repoRoot,
    stdio: 'inherit',
    env: process.env,
  });
  if (result.status !== 0) {
    throw new Error(`cargo build -p allternit-mux exited ${result.status}`);
  }
}

function main() {
  if (process.platform === 'win32') {
    log('skipped on Windows (Unix-socket daemon)');
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
    throw new Error('allternit-mux binary not found after cargo build');
  }
  fs.mkdirSync(resourcesBin, { recursive: true });
  fs.copyFileSync(source, dest);
  if (process.platform !== 'win32') fs.chmodSync(dest, 0o755);
  log(`staged ${source} -> ${dest}`);
}

try {
  main();
} catch (err) {
  process.stderr.write(`[prepare-mux] ✗ ${err.message}\n`);
  process.exit(1);
}
