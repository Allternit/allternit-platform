/**
 * Stage the Phone Link Apple ID signer (`allternit-sideload`) into resources/bin/.
 *
 * Desktop runs it to sign the iPhone helper with the user's own Apple ID
 * (src/main/phone-ios-runner.ts SideloadSigner, picked in phone-link-ipc.ts
 * only when the file exists). It is a standalone cargo workspace
 * (cmd/allternit-sideload/Cargo.toml explains why), so it builds with
 * --manifest-path, not -p.
 *
 * REQUIRED on macOS: without it every user's iPhone setup ends with
 * signer_unavailable. Its anisette server (anisette.allternit.com) is live
 * since 2026-10-17. A failed build stops packaging. ALLTERNIT_SKIP_SIDELOAD=1
 * skips it on purpose (dev builds only; Phone Link then says iPhone setup
 * isn't available).
 *
 * If a release job already lipo'd/copied the binary here, this is a no-op.
 * macOS only: the iPhone flow (go-ios, Keychain session) is macOS-first.
 */

'use strict';

const fs = require('fs');
const path = require('path');
const { spawnSync } = require('child_process');

const desktopDir = path.resolve(__dirname, '..');
const repoRoot = path.resolve(desktopDir, '..', '..');
const crateDir = path.join(repoRoot, 'cmd', 'allternit-sideload');
const { cargoBinaryCandidates } = require('./cargo-target.cjs');
const resourcesBin = path.join(desktopDir, 'resources', 'bin');
const name = 'allternit-sideload';
const dest = path.join(resourcesBin, name);
const required = process.env.ALLTERNIT_REQUIRE_SIDELOAD !== '0';

function log(message) {
  process.stdout.write(`[prepare-sideload] ${message}\n`);
}

function alreadyStaged() {
  try {
    return fs.existsSync(dest) && fs.statSync(dest).size > 1024 * 1024;
  } catch {
    return false;
  }
}

/** Where cargo actually puts this crate's output (a parent .cargo/config can move it to the repo root). */
function cargoMetadataTarget() {
  const r = spawnSync('cargo', ['metadata', '--format-version', '1', '--no-deps', '--manifest-path', path.join(crateDir, 'Cargo.toml')], {
    cwd: repoRoot,
    encoding: 'utf8',
  });
  try {
    return r.status === 0 ? JSON.parse(r.stdout).target_directory : null;
  } catch {
    return null;
  }
}

function findBuilt() {
  // cargo's own answer first, then CARGO_TARGET_DIR, then the crate's target/.
  const fromMetadata = cargoMetadataTarget();
  const candidates = [
    ...(fromMetadata ? [path.join(fromMetadata, 'release', name)] : []),
    ...cargoBinaryCandidates(crateDir, name, { releaseOnly: true }),
  ];
  for (const candidate of candidates) {
    if (fs.existsSync(candidate) && fs.statSync(candidate).isFile()) return candidate;
  }
  return null;
}

function cargoBuild() {
  log('cargo build --release --manifest-path cmd/allternit-sideload/Cargo.toml');
  const result = spawnSync('cargo', ['build', '--release', '--manifest-path', path.join(crateDir, 'Cargo.toml')], {
    cwd: repoRoot,
    stdio: 'inherit',
    env: process.env,
  });
  if (result.status !== 0) {
    throw new Error(`cargo build of allternit-sideload exited ${result.status}`);
  }
}

function main() {
  if (process.platform !== 'darwin') {
    log('skipped (macOS only)');
    return;
  }
  if (process.env.ALLTERNIT_SKIP_SIDELOAD === '1') {
    log('skipped (ALLTERNIT_SKIP_SIDELOAD=1); Phone Link iPhone setup will be unavailable in this build');
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
    throw new Error('allternit-sideload binary not found after cargo build');
  }
  fs.mkdirSync(resourcesBin, { recursive: true });
  fs.copyFileSync(source, dest);
  fs.chmodSync(dest, 0o755);
  log(`staged ${source} -> ${dest}`);
}

try {
  main();
} catch (err) {
  if (required) {
    process.stderr.write(`[prepare-sideload] ✗ ${err.message}\n`);
    process.exit(1);
  }
  process.stderr.write(
    `[prepare-sideload] WARNING: ${err.message}. Packaging continues without it (optional sidecar); ` +
      'Phone Link iPhone setup will say it is unavailable (ALLTERNIT_REQUIRE_SIDELOAD=0 was set).\n'
  );
}
