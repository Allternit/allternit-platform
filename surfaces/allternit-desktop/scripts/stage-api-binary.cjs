#!/usr/bin/env node
/**
 * Lightweight helper to stage the allternit-api Rust binary into the desktop
 * resources directory without running the full build-desktop.sh pipeline.
 *
 * Useful during development when you have already built the API via
 * `cargo build --release` and want to iterate on the Electron package.
 */

const fs = require('fs');
const path = require('path');

const desktopDir = path.resolve(__dirname, '..');
const repoRoot = path.resolve(desktopDir, '..', '..');
const { cargoBinaryCandidates } = require('./cargo-target.cjs');
const resourcesBin = path.join(desktopDir, 'resources', 'bin');

const binaryName = process.platform === 'win32' ? 'allternit-api.exe' : 'allternit-api';

const candidates = cargoBinaryCandidates(repoRoot, binaryName);

function log(message) {
  process.stdout.write(`[stage-api-binary] ${message}\n`);
}

function main() {
  let source = null;
  for (const candidate of candidates) {
    if (fs.existsSync(candidate)) {
      source = candidate;
      break;
    }
  }

  if (!source) {
    process.stderr.write(
      `[stage-api-binary] ✗ allternit-api binary not found.\n` +
      `    Searched:\n` +
      candidates.map((c) => `      - ${c}`).join('\n') +
      `\n    Build it first:\n` +
      `      cargo build --release -p allternit-api\n`
    );
    process.exit(1);
  }

  fs.mkdirSync(resourcesBin, { recursive: true });
  const dest = path.join(resourcesBin, binaryName);
  fs.copyFileSync(source, dest);
  if (process.platform !== 'win32') {
    fs.chmodSync(dest, 0o755);
  }
  log(`✓ Staged ${source} -> ${dest}`);
}

main();
