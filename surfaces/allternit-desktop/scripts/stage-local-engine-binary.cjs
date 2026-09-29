#!/usr/bin/env node
/**
 * Lightweight helper to stage the local-engine Rust binary into the desktop
 * resources directory without running the full build-desktop.sh pipeline.
 *
 * Useful during development when you have already built the engine via
 * `cargo build --release -p allternit-local-engine` and want to iterate on
 * the Electron package.
 *
 * Note: cargo names the binary `local-engine` (see [[bin]] in
 * services/local-engine/Cargo.toml); this script stages it as
 * allternit-local-engine so resources/bin keeps one naming convention.
 */

const fs = require('fs');
const path = require('path');

const desktopDir = path.resolve(__dirname, '..');
const repoRoot = path.resolve(desktopDir, '..', '..');
const { cargoBinaryCandidates } = require('./cargo-target.cjs');
const resourcesBin = path.join(desktopDir, 'resources', 'bin');

const sourceName = process.platform === 'win32' ? 'local-engine.exe' : 'local-engine';
const stagedName = process.platform === 'win32' ? 'allternit-local-engine.exe' : 'allternit-local-engine';

const candidates = cargoBinaryCandidates(repoRoot, sourceName);

function log(message) {
  process.stdout.write(`[stage-local-engine-binary] ${message}\n`);
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
      `[stage-local-engine-binary] ✗ local-engine binary not found.\n` +
      `    Searched:\n` +
      candidates.map((c) => `      - ${c}`).join('\n') +
      `\n    Build it first:\n` +
      `      cargo build --release -p allternit-local-engine\n`
    );
    process.exit(1);
  }

  fs.mkdirSync(resourcesBin, { recursive: true });
  const dest = path.join(resourcesBin, stagedName);
  fs.copyFileSync(source, dest);
  if (process.platform !== 'win32') {
    fs.chmodSync(dest, 0o755);
  }
  log(`✓ Staged ${source} -> ${dest}`);
}

main();
