#!/usr/bin/env node
/**
 * Builds native/keychain/build/keychain.node: the macOS Keychain addon that
 * reads the "<app> Safe Storage" password off the Electron main thread (see
 * native/keychain/README.md).
 *
 * One clang++ call makes a universal arm64 + x86_64 Node-API bundle. Node-API
 * is ABI-stable, so the same file loads in plain node and in Electron with no
 * node-gyp and no Electron rebuild. electron-builder ships it through the mac
 * extraResources entry to Contents/Resources/native/keychain/keychain.node.
 *
 * No-op off macOS. Skips the compile when the output is newer than every
 * source file (and this script).
 */

const { execFileSync } = require('child_process');
const fs = require('fs');
const path = require('path');

const desktopDir = path.resolve(__dirname, '..');
const addonDir = path.join(desktopDir, 'native', 'keychain');
const srcDir = path.join(addonDir, 'src');
const includeDir = path.join(addonDir, 'include', 'node-api');
const outDir = path.join(addonDir, 'build');
const outFile = path.join(outDir, 'keychain.node');
const REQUIRED_ARCHS = ['arm64', 'x86_64'];

function log(message) {
  process.stdout.write(`[prepare-keychain-addon] ${message}\n`);
}

function fail(message) {
  process.stderr.write(`[prepare-keychain-addon] ✗ ${message}\n`);
  process.exit(1);
}

if (process.platform !== 'darwin') {
  log(`skipped on ${process.platform} (macOS only; other platforms keep safeStorage)`);
  process.exit(0);
}

function listFiles(dir) {
  return fs.readdirSync(dir, { withFileTypes: true }).flatMap((entry) => {
    const full = path.join(dir, entry.name);
    return entry.isDirectory() ? listFiles(full) : [full];
  });
}

function archsOf(file) {
  return execFileSync('lipo', ['-archs', file], { encoding: 'utf8' }).trim().split(/\s+/);
}

const sources = listFiles(srcDir).filter((f) => /\.(cc|cpp|mm)$/.test(f)).sort();
if (sources.length === 0) fail(`no sources in ${srcDir}`);
const inputs = [...listFiles(srcDir), ...listFiles(includeDir), __filename];
const newestInput = Math.max(...inputs.map((f) => fs.statSync(f).mtimeMs));

if (fs.existsSync(outFile) && fs.statSync(outFile).mtimeMs > newestInput) {
  const archs = archsOf(outFile);
  if (REQUIRED_ARCHS.every((a) => archs.includes(a))) {
    log(`up to date (${archs.join(' ')}): ${outFile}`);
    process.exit(0);
  }
  log(`existing build is missing an arch (${archs.join(' ')}); rebuilding`);
}

fs.mkdirSync(outDir, { recursive: true });
// Build under the final file name (the linker signature records it), in a
// temp folder, then move into place.
const tmpDir = path.join(outDir, `.tmp-${process.pid}`);
fs.mkdirSync(tmpDir, { recursive: true });
const tmpFile = path.join(tmpDir, 'keychain.node');
const args = [
  'clang++',
  '-std=c++17',
  '-O2',
  '-Wall',
  '-Wextra',
  '-Werror',
  '-fvisibility=hidden',
  '-mmacosx-version-min=11.0',
  ...REQUIRED_ARCHS.flatMap((a) => ['-arch', a]),
  '-I',
  includeDir,
  '-bundle',
  '-undefined',
  'dynamic_lookup',
  '-framework',
  'Security',
  '-framework',
  'CoreFoundation',
  '-o',
  tmpFile,
  ...sources,
];

log(`compiling ${sources.map((s) => path.relative(desktopDir, s)).join(', ')}`);
try {
  execFileSync('xcrun', args, { stdio: 'inherit' });
} catch (err) {
  fs.rmSync(tmpDir, { recursive: true, force: true });
  fail(`clang++ failed (${err.status ?? err.message}). Install the Xcode command line tools: xcode-select --install`);
}

const archs = archsOf(tmpFile);
if (!REQUIRED_ARCHS.every((a) => archs.includes(a))) {
  fs.rmSync(tmpDir, { recursive: true, force: true });
  fail(`built file has archs "${archs.join(' ')}", need ${REQUIRED_ARCHS.join(' + ')}`);
}
fs.renameSync(tmpFile, outFile);
fs.rmSync(tmpDir, { recursive: true, force: true });
log(`✓ built ${outFile} (${archs.join(' ')})`);
