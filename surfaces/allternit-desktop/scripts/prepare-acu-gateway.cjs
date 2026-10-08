#!/usr/bin/env node
/**
 * Stage the ACU computer-use Python tree for packaging.
 *
 * Copies domains/computer-use/core (source only — not an existing .venv) into
 * resources/computer-use/acu, then creates a fresh venv there with the
 * gateway extras (fastapi, uvicorn). Desktop spawn uses that interpreter so
 * launch.py does not depend on a random system python.
 */
'use strict';

const fs = require('fs');
const path = require('path');
const { execFileSync } = require('child_process');

const desktopDir = path.resolve(__dirname, '..');
const repoRoot = path.resolve(desktopDir, '..', '..');
const src = path.join(repoRoot, 'domains', 'computer-use', 'core');
const dest = path.join(desktopDir, 'resources', 'computer-use', 'acu');

const SKIP_DIRS = new Set([
  '.venv',
  'venv',
  '__pycache__',
  '.pytest_cache',
  'tests',
  'build',
  '.git',
  'golden-paths',
  'evaluation',
  'node_modules',
]);

function log(message) {
  process.stdout.write(`[prepare-acu-gateway] ${message}\n`);
}

function copyTree(from, to) {
  fs.mkdirSync(to, { recursive: true });
  for (const entry of fs.readdirSync(from, { withFileTypes: true })) {
    if (SKIP_DIRS.has(entry.name) || entry.name.endsWith('.pyc')) continue;
    const nextFrom = path.join(from, entry.name);
    const nextTo = path.join(to, entry.name);
    if (entry.isDirectory()) copyTree(nextFrom, nextTo);
    else fs.copyFileSync(nextFrom, nextTo);
  }
}

if (!fs.existsSync(path.join(src, 'launch.py'))) {
  process.stderr.write(`[prepare-acu-gateway] ✗ missing ${path.join(src, 'launch.py')}\n`);
  process.exit(1);
}

fs.rmSync(dest, { recursive: true, force: true });
copyTree(src, dest);
if (!fs.existsSync(path.join(dest, 'launch.py'))) {
  process.stderr.write('[prepare-acu-gateway] ✗ launch.py did not copy\n');
  process.exit(1);
}
log(`staged ${dest}`);

function resolvePython() {
  const candidates = [
    process.env.ALLTERNIT_ACU_PYTHON,
    '/opt/homebrew/bin/python3.12',
    '/opt/homebrew/bin/python3.11',
    '/usr/local/bin/python3.12',
    '/usr/local/bin/python3.11',
    'python3.11',
    'python3',
  ].filter(Boolean);
  for (const candidate of candidates) {
    try {
      execFileSync(candidate, ['-c', 'import venv, ensurepip'], { stdio: 'ignore', timeout: 5000 });
      return candidate;
    } catch {
      /* try next */
    }
  }
  return null;
}

function installGatewayVenv(tree) {
  const python = resolvePython();
  if (!python) {
    process.stderr.write(
      '[prepare-acu-gateway] ✗ no python3 with venv/ensurepip; cannot vendor uvicorn\n',
    );
    process.exit(1);
  }
  const venv = path.join(tree, '.venv');
  const venvPython = process.platform === 'win32'
    ? path.join(venv, 'Scripts', 'python.exe')
    : path.join(venv, 'bin', 'python');
  log(`creating gateway venv with ${python}`);
  execFileSync(python, ['-m', 'venv', venv], { stdio: 'inherit' });
  execFileSync(venvPython, ['-m', 'pip', 'install', '--upgrade', 'pip'], { stdio: 'inherit' });
  execFileSync(
    venvPython,
    ['-m', 'pip', 'install', 'fastapi>=0.110.0', 'pydantic>=2.0.0', 'uvicorn[standard]>=0.27.0', 'starlette>=0.37.0', 'httpx>=0.27.0', "anthropic>=1.12.0; python_version >= '3.10'"],
    { stdio: 'inherit', cwd: tree },
  );
  execFileSync(venvPython, ['-c', 'import uvicorn, fastapi'], { stdio: 'inherit' });
  log(`gateway venv ready (${venvPython})`);
}

installGatewayVenv(dest);
