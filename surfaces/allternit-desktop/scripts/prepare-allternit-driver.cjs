#!/usr/bin/env node
/**
 * Stage the Allternit Driver sidecar for packaging.
 *
 * 1. Copies domains/computer-use/driver (the sidecar package and the forked
 *    arc-driver, no tests) into resources/computer-use/driver.
 * 2. Stages a relocatable CPython 3.12 (python-build-standalone, pinned by
 *    sha256) per target into resources/computer-use/driver-python/<os>-<arch>.
 *    A venv would point at the build machine's Python, which users don't have.
 * 3. Keeps pip (the Decision Runtime installs its packages with it). On
 *    macOS targets, installs the pinned PyObjC frameworks the arc engine
 *    needs into that Python. Linux and Windows run the Cua engine only and
 *    need nothing beyond the standard library.
 *
 * 4. Copies the Decision Runtime scorer sidecar (domains/decision-runtime:
 *    the allternit_decisions package, its pinned requirements, NOTICE) into
 *    resources/decision-runtime. It runs on this same Python and installs its
 *    packages and model on first use, so nothing heavy ships in the app.
 *
 * Targets follow prepare-cua-driver.cjs: ALLTERNIT_PACK_OS (+ ALLTERNIT_PACK_ARCH)
 * picks one; a Mac stages every target, so one Mac can pack them all.
 */
'use strict';

const crypto = require('node:crypto');
const fs = require('node:fs');
const https = require('node:https');
const os = require('node:os');
const path = require('node:path');
const { execFileSync, spawnSync } = require('node:child_process');

const desktopDir = path.resolve(__dirname, '..');
const repoRoot = path.resolve(desktopDir, '..', '..');
const driverSrc = path.join(repoRoot, 'domains', 'computer-use', 'driver');
const driverDest = path.join(desktopDir, 'resources', 'computer-use', 'driver');
const pythonRoot = path.join(desktopDir, 'resources', 'computer-use', 'driver-python');
const decisionsSrc = path.join(repoRoot, 'domains', 'decision-runtime');
const decisionsDest = path.join(desktopDir, 'resources', 'decision-runtime');

const PBS = '20260602';
const PY = '3.12.13';
const PYOBJC = '12.2.2';
const PYOBJC_PACKAGES = ['ApplicationServices', 'Cocoa', 'Quartz', 'Vision'].map(
  (name) => `pyobjc-framework-${name}==${PYOBJC}`,
);
const PYTHONS = {
  'darwin-arm64': ['aarch64-apple-darwin', '7426b06a7ba06e2db637ace3a6658db55d357c6c2083d5b13e0d46a293a3d5ad'],
  'darwin-x64': ['x86_64-apple-darwin', 'ed741cca5783263844e051cedbb9e0728a8e8c0a903038ac910a2664c36e532a'],
  'linux-x64': ['x86_64-unknown-linux-gnu', '191b5188b42886fb8a14968d714571e8f3d1cef92ac7ad4c7e24cc4d0929b194'],
  'linux-arm64': ['aarch64-unknown-linux-gnu', '437c8514d7ebf42db9e989d73971ce49778fd466e5187d753c18234bbd28ad66'],
  'win32-x64': ['x86_64-pc-windows-msvc', 'dbada31f91a4fff934dae85e7998d91f1e926135bd88ffed4921a337d5680f48'],
};
// Parts of the standard distribution the driver never loads (~25 MB).
// The live map's event observers (domains/computer-use/driver/allternit_driver/live/):
// pure-Python wheels, so pip on the build host stages them for any target.
const OBSERVER_PACKAGES = { linux: ['jeepney==0.9.0'], win32: ['comtypes==1.4.11'] };
const PRUNE = ['test', 'idlelib', 'tkinter', 'turtledemo', 'ensurepip', 'lib2to3', 'pydoc_data', '__phello__'];
const SKIP = new Set(['__pycache__', '.pytest_cache', 'tests', '.venv']);

function log(message) {
  process.stdout.write(`[prepare-allternit-driver] ${message}\n`);
}

function copyTree(from, to) {
  fs.mkdirSync(to, { recursive: true });
  for (const entry of fs.readdirSync(from, { withFileTypes: true })) {
    if (SKIP.has(entry.name) || entry.name.endsWith('.pyc')) continue;
    const src = path.join(from, entry.name);
    const dst = path.join(to, entry.name);
    if (entry.isDirectory()) copyTree(src, dst);
    else fs.copyFileSync(src, dst);
  }
}

function targets() {
  const osName = process.env.ALLTERNIT_PACK_OS;
  if (osName) {
    const arch = process.env.ALLTERNIT_PACK_ARCH || (osName === 'darwin' ? null : 'x64');
    const keys = Object.keys(PYTHONS).filter((k) => k.startsWith(`${osName}-`) && (!arch || k.endsWith(`-${arch}`)));
    if (keys.length === 0) throw new Error(`No driver Python for ALLTERNIT_PACK_OS=${osName} ALLTERNIT_PACK_ARCH=${arch}`);
    return keys;
  }
  if (process.env.ALLTERNIT_PACK_ALL === '1' || process.platform === 'darwin') {
    return ['darwin-arm64', 'darwin-x64', 'linux-x64', 'win32-x64'];
  }
  const key = `${process.platform}-${process.arch}`;
  return PYTHONS[key] ? [key] : [];
}

function download(url, destination, redirects = 0) {
  if (redirects > 5) return Promise.reject(new Error('Too many redirects'));
  return new Promise((resolve, reject) => {
    https.get(url, { headers: { 'User-Agent': 'allternit-desktop-packager' } }, (response) => {
      if (response.statusCode >= 300 && response.statusCode < 400 && response.headers.location) {
        response.resume();
        download(response.headers.location, destination, redirects + 1).then(resolve, reject);
        return;
      }
      if (response.statusCode !== 200) {
        response.resume();
        reject(new Error(`download failed with HTTP ${response.statusCode}: ${url}`));
        return;
      }
      const file = fs.createWriteStream(destination, { mode: 0o600 });
      response.pipe(file);
      file.on('finish', () => file.close(resolve));
      file.on('error', reject);
    }).on('error', reject);
  });
}

function pythonExe(dir, key) {
  return key.startsWith('win32') ? path.join(dir, 'python.exe') : path.join(dir, 'bin', 'python3');
}

function sitePackages(dir, key) {
  return key.startsWith('win32')
    ? path.join(dir, 'Lib', 'site-packages')
    : path.join(dir, 'lib', `python${PY.split('.').slice(0, 2).join('.')}`, 'site-packages');
}

async function stagePython(key) {
  const [triple, sha256] = PYTHONS[key];
  const dir = path.join(pythonRoot, key);
  const stamp = path.join(dir, '.allternit-python.json');
  const observers = OBSERVER_PACKAGES[key.split('-')[0]] || [];
  const want = { pbs: PBS, python: PY, sha256, pyobjc: key.startsWith('darwin') ? PYOBJC : null, observers, pip: true };
  try {
    if (JSON.stringify(JSON.parse(fs.readFileSync(stamp, 'utf8'))) === JSON.stringify(want)) {
      log(`Python ${PY} already staged for ${key}`);
      return;
    }
  } catch {
    /* stage it */
  }
  fs.rmSync(dir, { recursive: true, force: true });
  const asset = `cpython-${PY}+${PBS}-${triple}-install_only_stripped.tar.gz`;
  const url = `https://github.com/astral-sh/python-build-standalone/releases/download/${PBS}/${encodeURIComponent(asset)}`;
  const tmp = fs.mkdtempSync(path.join(os.tmpdir(), `allternit-driver-py-${key}-`));
  try {
    const archive = path.join(tmp, 'python.tar.gz');
    await download(url, archive);
    const actual = crypto.createHash('sha256').update(fs.readFileSync(archive)).digest('hex');
    if (actual !== sha256) throw new Error(`Python checksum mismatch for ${key}: expected ${sha256}, got ${actual}`);
    const untar = spawnSync('tar', ['-xzf', archive, '-C', tmp], { encoding: 'utf8' });
    if (untar.status !== 0) throw new Error(untar.stderr || `could not extract ${asset}`);
    fs.mkdirSync(path.dirname(dir), { recursive: true });
    fs.renameSync(path.join(tmp, 'python'), dir);
  } finally {
    fs.rmSync(tmp, { recursive: true, force: true });
  }
  const lib = key.startsWith('win32') ? path.join(dir, 'Lib') : path.dirname(sitePackages(dir, key));
  for (const name of PRUNE) fs.rmSync(path.join(lib, name), { recursive: true, force: true });

  if (key.startsWith('darwin')) {
    // PyObjC ships universal2 wheels, so one pip run serves both Mac arches.
    const runner = [pythonExe(dir, key), pythonExe(path.join(pythonRoot, 'darwin-arm64'), 'darwin-arm64'), 'python3']
      .find((candidate) => spawnSync(candidate, ['-m', 'pip', '--version'], { stdio: 'ignore' }).status === 0);
    if (!runner) throw new Error('no Python with pip available to install PyObjC');
    execFileSync(runner, [
      '-m', 'pip', 'install', '--quiet', '--disable-pip-version-check', '--no-compile',
      '--only-binary=:all:', '--platform', 'macosx_11_0_universal2', '--python-version', '3.12',
      '--implementation', 'cp', '--target', sitePackages(dir, key), ...PYOBJC_PACKAGES,
    ], { stdio: 'inherit' });
  }
  if (observers.length) {
    const runner = [pythonExe(dir, key), 'python3', 'python', pythonExe(path.join(pythonRoot, 'darwin-arm64'), 'darwin-arm64')]
      .find((candidate) => spawnSync(candidate, ['-m', 'pip', '--version'], { stdio: 'ignore' }).status === 0);
    if (!runner) throw new Error('no Python with pip available to stage the live-map observer packages');
    execFileSync(runner, [
      '-m', 'pip', 'install', '--quiet', '--disable-pip-version-check', '--no-compile', '--no-deps',
      '--only-binary=:all:', '--target', sitePackages(dir, key), ...observers,
    ], { stdio: 'inherit' });
  }
  // Build-time only: headers, Tcl/Tk, PyObjC's own test suite (~35 MB). pip
  // stays: the Decision Runtime sidecar installs its pinned packages with it
  // on first use (domains/decision-runtime/allternit_decisions/runtime.py).
  const site = sitePackages(dir, key);
  for (const entry of fs.existsSync(site) ? fs.readdirSync(site) : []) {
    if (/^PyObjCTest([-.].*)?$/.test(entry)) fs.rmSync(path.join(site, entry), { recursive: true, force: true });
  }
  const top = key.startsWith('win32') ? dir : path.join(dir, 'lib');
  for (const entry of fs.readdirSync(top)) {
    if (/^(tcl|tk|itcl|thread|libtcl|libtk|tcl8|tk8)/i.test(entry) || /^config-3/.test(entry)) {
      fs.rmSync(path.join(top, entry), { recursive: true, force: true });
    }
  }
  for (const extra of ['include', 'share', 'tcl']) fs.rmSync(path.join(dir, extra), { recursive: true, force: true });
  fs.writeFileSync(stamp, JSON.stringify(want));
  log(`staged Python ${PY} for ${key} at ${dir}`);
}

(async () => {
  if (!fs.existsSync(path.join(driverSrc, 'allternit_driver', '__main__.py'))) {
    throw new Error(`missing ${path.join(driverSrc, 'allternit_driver', '__main__.py')}`);
  }
  fs.rmSync(driverDest, { recursive: true, force: true });
  copyTree(driverSrc, driverDest);
  log(`staged driver source at ${driverDest}`);
  if (!fs.existsSync(path.join(decisionsSrc, 'allternit_decisions', '__main__.py'))) {
    throw new Error(`missing ${path.join(decisionsSrc, 'allternit_decisions', '__main__.py')}`);
  }
  fs.rmSync(decisionsDest, { recursive: true, force: true });
  copyTree(decisionsSrc, decisionsDest);
  log(`staged decision runtime source at ${decisionsDest}`);
  for (const key of targets()) await stagePython(key);
})().catch((error) => {
  process.stderr.write(`[prepare-allternit-driver] ✗ ${error.message}\n`);
  process.exitCode = 1;
});
