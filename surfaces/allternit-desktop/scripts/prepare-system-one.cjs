/**
 * Stage System One (S1) for packaging (Q28, Q29):
 *   - `bun build --compile tools/system-one-local/src/cli.ts` → resources/bin/system-one
 *     (ships through the existing resources/bin/ → bin/ extraResource);
 *   - tools/system-one-local/laya/*.sh → resources/laya/ (extraResource laya/).
 *
 * uv and Laya itself are NOT bundled: SystemOneManager installs Laya into
 * ~/Library/Application Support/Allternit/laya on first run.
 * Set ALLTERNIT_SYSTEM_ONE_FORCE=1 to recompile when a binary is already staged.
 */

'use strict';

const fs = require('fs');
const os = require('os');
const path = require('path');
const { spawnSync } = require('child_process');

const desktopDir = path.resolve(__dirname, '..');
const repoRoot = path.resolve(desktopDir, '..', '..');
const s1Dir = path.join(repoRoot, 'tools', 'system-one-local');
const entry = path.join(s1Dir, 'src', 'cli.ts');
const layaSrc = path.join(s1Dir, 'laya');
const resourcesBin = path.join(desktopDir, 'resources', 'bin');
const layaDest = path.join(desktopDir, 'resources', 'laya');
const dest = path.join(resourcesBin, process.platform === 'win32' ? 'system-one.exe' : 'system-one');

function log(message) {
  process.stdout.write(`[prepare-system-one] ${message}\n`);
}

function findBun() {
  const candidates = [
    process.env.BUN,
    path.join(os.homedir(), '.bun', 'bin', 'bun'),
    '/opt/homebrew/bin/bun',
    '/usr/local/bin/bun',
  ].filter(Boolean);
  const which = spawnSync(process.platform === 'win32' ? 'where' : 'which', ['bun'], { encoding: 'utf8' });
  if (which.status === 0 && which.stdout.trim()) candidates.unshift(which.stdout.trim().split(/\r?\n/)[0]);
  return candidates.find((c) => fs.existsSync(c)) ?? null;
}

function compile() {
  if (fs.existsSync(dest) && fs.statSync(dest).size > 1024 * 1024 && process.env.ALLTERNIT_SYSTEM_ONE_FORCE !== '1') {
    log(`already staged: ${dest}`);
    return;
  }
  const bun = findBun();
  if (!bun) throw new Error('bun not found (needed to compile System One); install bun or set BUN=/path/to/bun');
  fs.mkdirSync(resourcesBin, { recursive: true });
  log(`${bun} build --compile ${path.relative(repoRoot, entry)} → ${dest}`);
  const result = spawnSync(bun, ['build', '--compile', '--minify', entry, '--outfile', dest], {
    cwd: s1Dir,
    stdio: 'inherit',
    env: process.env,
  });
  if (result.status !== 0) throw new Error(`bun build --compile exited ${result.status}`);
  fs.chmodSync(dest, 0o755);
}

function stageLaya() {
  fs.rmSync(layaDest, { recursive: true, force: true });
  fs.mkdirSync(layaDest, { recursive: true });
  for (const name of fs.readdirSync(layaSrc)) {
    if (!name.endsWith('.sh')) continue;
    fs.copyFileSync(path.join(layaSrc, name), path.join(layaDest, name));
    fs.chmodSync(path.join(layaDest, name), 0o755);
    log(`staged laya/${name}`);
  }
}

try {
  compile();
  stageLaya();
} catch (err) {
  process.stderr.write(`[prepare-system-one] ✗ ${err.message}\n`);
  process.exit(1);
}
