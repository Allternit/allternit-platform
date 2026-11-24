/**
 * Stage the Computers console elevation helper (`allternit-host-helper`) into
 * resources/bin/, next to allternit-api.
 *
 * The runtime finds it there: `host_probe::elevation::bundled_helper()` looks
 * for HELPER_BIN beside its own executable (`current_exe().parent()`), which in
 * a packaged app is Resources/bin (macOS) or resources\bin (Windows/Linux), and
 * in a runtime update is bin/ of the installed version. Without it,
 * GET /v1/host/elevation/plan and POST /v1/host/elevation/setup answer 503
 * `helper_missing`. The helper is never run from here: the setup sheet copies
 * it to its root-owned home (Linux /usr/local/libexec/allternit, macOS
 * LaunchDaemon, Windows %ProgramFiles%\Allternit\HostHelper) after one OS
 * admin prompt.
 *
 * If a release job already built/lipo'd it here, this is a no-op. Otherwise
 * cargo build --release -p host-probe --bin allternit-host-helper and copy.
 * All three desktop OSes.
 */

'use strict';

const fs = require('fs');
const path = require('path');
const { spawnSync } = require('child_process');

const desktopDir = path.resolve(__dirname, '..');
const repoRoot = path.resolve(desktopDir, '..', '..');
const { cargoBinaryCandidates } = require('./cargo-target.cjs');
const resourcesBin = path.join(desktopDir, 'resources', 'bin');
// Same name as host_probe::helper::HELPER_BIN.
const name = process.platform === 'win32' ? 'allternit-host-helper.exe' : 'allternit-host-helper';
const dest = path.join(resourcesBin, name);

function log(message) {
  process.stdout.write(`[prepare-host-helper] ${message}\n`);
}

function alreadyStaged() {
  try {
    return fs.existsSync(dest) && fs.statSync(dest).size > 1024;
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
  log('cargo build --release -p host-probe --bin allternit-host-helper');
  const result = spawnSync(
    'cargo',
    ['build', '--release', '-p', 'host-probe', '--bin', 'allternit-host-helper'],
    { cwd: repoRoot, stdio: 'inherit', env: process.env },
  );
  if (result.status !== 0) {
    throw new Error(`cargo build -p host-probe exited ${result.status}`);
  }
}

function main() {
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
    throw new Error(`${name} not found after cargo build`);
  }
  fs.mkdirSync(resourcesBin, { recursive: true });
  fs.copyFileSync(source, dest);
  if (process.platform !== 'win32') fs.chmodSync(dest, 0o755);
  log(`staged ${source} -> ${dest}`);
}

try {
  main();
} catch (err) {
  process.stderr.write(`[prepare-host-helper] ✗ ${err.message}\n`);
  process.exit(1);
}
