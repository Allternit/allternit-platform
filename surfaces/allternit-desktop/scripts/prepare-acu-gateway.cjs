#!/usr/bin/env node
/**
 * Stage the ACU computer-use Python tree for packaging.
 *
 * Copies domains/computer-use/core (source only — not an existing .venv) into
 * resources/computer-use/acu, then installs the gateway packages (fastapi,
 * uvicorn) per target into resources/computer-use/acu-site/<os>-<arch>. The
 * packaged app runs launch.py on the bundled driver Python
 * (prepare-allternit-driver.cjs) with that dir on PYTHONPATH. A venv would
 * point at the build machine's Python: users don't have it, and its dangling
 * symlinks make macOS code signing fail.
 */
'use strict';

const fs = require('fs');
const path = require('path');
const { execFileSync } = require('child_process');

const desktopDir = path.resolve(__dirname, '..');
const repoRoot = path.resolve(desktopDir, '..', '..');
const src = path.join(repoRoot, 'domains', 'computer-use', 'core');
const dest = path.join(desktopDir, 'resources', 'computer-use', 'acu');
const siteRoot = path.join(desktopDir, 'resources', 'computer-use', 'acu-site');

const PACKAGES = [
  'fastapi>=0.110.0',
  'pydantic>=2.0.0',
  // Plain uvicorn: [standard] pulls uvloop, whose host-evaluated marker breaks
  // cross-target installs (no Windows wheel). Nothing here needs it.
  'uvicorn>=0.27.0',
  'starlette>=0.37.0',
  'httpx>=0.27.0',
  'anthropic>=1.12.0',
];
// pip --platform tags per bundled driver Python (same keys as
// prepare-allternit-driver.cjs). pip expands each to older compatible tags.
const PLATFORMS = {
  'darwin-arm64': ['macosx_11_0_arm64'],
  'darwin-x64': ['macosx_10_13_x86_64'],
  'linux-x64': ['manylinux_2_17_x86_64', 'manylinux2014_x86_64'],
  'linux-arm64': ['manylinux_2_17_aarch64', 'manylinux2014_aarch64'],
  'win32-x64': ['win_amd64'],
};

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
      execFileSync(candidate, ['-m', 'pip', '--version'], { stdio: 'ignore', timeout: 5000 });
      return candidate;
    } catch {
      /* try next */
    }
  }
  return null;
}

function targets() {
  const osName = process.env.ALLTERNIT_PACK_OS;
  if (osName) {
    const arch = process.env.ALLTERNIT_PACK_ARCH || (osName === 'darwin' ? null : 'x64');
    const keys = Object.keys(PLATFORMS).filter((k) => k.startsWith(`${osName}-`) && (!arch || k.endsWith(`-${arch}`)));
    if (keys.length === 0) throw new Error(`No ACU target for ALLTERNIT_PACK_OS=${osName} ALLTERNIT_PACK_ARCH=${arch}`);
    return keys;
  }
  if (process.env.ALLTERNIT_PACK_ALL === '1' || process.platform === 'darwin') {
    return ['darwin-arm64', 'darwin-x64', 'linux-x64', 'win32-x64'];
  }
  const key = `${process.platform}-${process.arch}`;
  return PLATFORMS[key] ? [key] : [];
}

function installGatewayPackages(key) {
  const python = resolvePython();
  if (!python) {
    process.stderr.write(
      '[prepare-acu-gateway] ✗ no python3 with pip; cannot vendor uvicorn\n',
    );
    process.exit(1);
  }
  const site = path.join(siteRoot, key);
  fs.rmSync(site, { recursive: true, force: true });
  log(`installing gateway packages for ${key} with ${python}`);
  execFileSync(python, [
    '-m', 'pip', 'install', '--quiet', '--disable-pip-version-check', '--no-compile',
    '--only-binary=:all:', '--python-version', '3.12', '--implementation', 'cp',
    ...PLATFORMS[key].flatMap((tag) => ['--platform', tag]),
    '--target', site, ...PACKAGES,
  ], { stdio: 'inherit' });
  // Console scripts carry the build host's shebang; launch.py imports uvicorn.
  fs.rmSync(path.join(site, 'bin'), { recursive: true, force: true });
  for (const pkg of ['uvicorn', 'fastapi']) {
    if (!fs.existsSync(path.join(site, pkg))) {
      process.stderr.write(`[prepare-acu-gateway] ✗ ${pkg} missing from ${site}\n`);
      process.exit(1);
    }
  }
  log(`gateway packages ready (${site})`);
}

for (const key of targets()) installGatewayPackages(key);
