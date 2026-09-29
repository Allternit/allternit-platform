#!/usr/bin/env node
/**
 * Stage the allternit-api Rust binary for packaging.
 *
 * Looks for a local build in target/release or target/debug and copies it into
 * resources/bin/. There is no runtime/GitHub fallback: github.com/allternit/platform
 * does not exist, and this Mac cannot cross-compile Windows/Linux openssl-sys.
 *
 * Native CI runners (release-desktop.yml) cargo-build on the target OS and copy
 * the binary into resources/bin/ before electron-builder. Local cross-packs can
 * set ALLTERNIT_ALLOW_MISSING_API=1; the packaged app then fails closed at boot.
 */

'use strict';

const fs = require('fs');
const path = require('path');

const desktopDir = path.resolve(__dirname, '..');
const repoRoot = path.resolve(desktopDir, '..', '..');
const { cargoBinaryCandidates } = require('./cargo-target.cjs');
const resourcesBin = path.join(desktopDir, 'resources', 'bin');
const binaryName = process.platform === 'win32' ? 'allternit-api.exe' : 'allternit-api';

function log(message) {
  process.stdout.write(`[prepare-api-binary] ${message}\n`);
}

function errorAndExit(message) {
  process.stderr.write(`[prepare-api-binary] ✗ ${message}\n`);
  process.exit(1);
}

function findLocalBinary() {
  const candidates = [
    ...cargoBinaryCandidates(repoRoot, binaryName),
    path.join(resourcesBin, binaryName),
  ];
  for (const candidate of candidates) {
    if (fs.existsSync(candidate) && fs.statSync(candidate).isFile()) {
      return candidate;
    }
  }
  return null;
}

function copyLocalBinary(source) {
  fs.mkdirSync(resourcesBin, { recursive: true });
  const dest = path.join(resourcesBin, binaryName);
  if (path.resolve(source) !== path.resolve(dest)) {
    fs.copyFileSync(source, dest);
  }
  if (process.platform !== 'win32') {
    fs.chmodSync(dest, 0o755);
  }
  log(`✓ Staged local binary: ${source} -> ${dest}`);
}

function main() {
  const localBinary = findLocalBinary();
  if (localBinary) {
    copyLocalBinary(localBinary);
    return;
  }

  const hint =
    'No local allternit-api binary found in target/release, target/debug, or resources/bin.\n' +
    '    Build on the target OS: cargo build --release -p allternit-api\n' +
    '    CI already does this on macos-latest / windows-latest / ubuntu-latest.\n' +
    '    Cross-compiling from macOS is not supported here.';

  if (process.env.ALLTERNIT_ALLOW_MISSING_API === '1') {
    log(`WARNING: ${hint}`);
    log('Continuing because ALLTERNIT_ALLOW_MISSING_API=1. The packaged app will fail closed at runtime.');
    return;
  }

  errorAndExit(hint + '\n    Or set ALLTERNIT_ALLOW_MISSING_API=1 for an installer that reports the missing sidecar at boot.');
}

main();
