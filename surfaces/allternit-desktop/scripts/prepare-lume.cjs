/**
 * Download CUA Lume (Apple Virtualization helper) for packaged Desktop.
 *
 * The tarball is a 67-byte `lume` launcher plus `lume.app` (the real Mach-O).
 * allternit-api looks for Resources/bin/lume next to itself; the launcher
 * execs lume.app/Contents/MacOS/lume.
 *
 * Usage:
 *   node scripts/prepare-lume.cjs              # host arch (darwin only)
 *   node scripts/prepare-lume.cjs arm64 x64    # both Mac archs (release)
 *
 * Windows/Linux: no-op (Apple VF is macOS-only).
 */

'use strict';

const https = require('https');
const fs = require('fs');
const path = require('path');
const { execSync } = require('child_process');

const LUME_VERSION = 'v0.3.9';
const LUME_VER_SHORT = LUME_VERSION.replace(/^v/, '');
const desktopDir = path.resolve(__dirname, '..');
const outRoot = path.join(desktopDir, 'resources', 'lume');

const ARCH_MAP = {
  arm64: 'arm64',
  aarch64: 'arm64',
  x64: 'x64',
  x86_64: 'x64',
};

function download(url, dest) {
  return new Promise((resolve, reject) => {
    const file = fs.createWriteStream(dest);
    const get = (u) => {
      https.get(u, (res) => {
        if (res.statusCode === 301 || res.statusCode === 302) {
          file.close();
          fs.unlinkSync(dest);
          return download(res.headers.location, dest).then(resolve, reject);
        }
        if (res.statusCode !== 200) {
          file.close();
          reject(new Error(`HTTP ${res.statusCode} for ${u}`));
          return;
        }
        res.pipe(file);
        file.on('finish', () => file.close(resolve));
      }).on('error', (err) => {
        try { fs.unlinkSync(dest); } catch { /* ignore */ }
        reject(err);
      });
    };
    get(url);
  });
}

function realBinaryPath(archDir) {
  return path.join(archDir, 'lume.app', 'Contents', 'MacOS', 'lume');
}

function alreadyPresent(archDir) {
  const bin = realBinaryPath(archDir);
  try {
    return fs.existsSync(bin) && fs.statSync(bin).size > 1024 * 1024;
  } catch {
    return false;
  }
}

function extractTarball(tarball, archDir) {
  fs.mkdirSync(archDir, { recursive: true });
  execSync(`tar -xzf "${tarball}" -C "${archDir}"`, { stdio: 'pipe' });
  const launcher = path.join(archDir, 'lume');
  const real = realBinaryPath(archDir);
  if (!fs.existsSync(launcher) || !fs.existsSync(real)) {
    throw new Error(`Lume extract did not produce launcher+app in ${archDir}`);
  }
  fs.chmodSync(launcher, 0o755);
  fs.chmodSync(real, 0o755);
  try {
    execSync(`xattr -cr "${archDir}"`, { stdio: 'pipe' });
  } catch {
    /* quarantine may already be absent */
  }
}

async function fetchArch(arch) {
  const cuaArch = ARCH_MAP[arch];
  if (!cuaArch) throw new Error(`Unsupported Lume arch ${arch}`);
  const archDir = path.join(outRoot, cuaArch);
  if (alreadyPresent(archDir)) {
    console.log(`[prepare-lume] already present at ${realBinaryPath(archDir)}`);
    return;
  }
  const url =
    `https://github.com/trycua/cua/releases/download/lume-${LUME_VERSION}/lume-${LUME_VER_SHORT}-darwin-${cuaArch}.tar.gz`;
  const tarball = path.join(outRoot, `lume-${LUME_VER_SHORT}-darwin-${cuaArch}.tar.gz`);
  fs.mkdirSync(outRoot, { recursive: true });
  console.log(`[prepare-lume] downloading ${url}`);
  await download(url, tarball);
  extractTarball(tarball, archDir);
  fs.unlinkSync(tarball);
  console.log(`[prepare-lume] ready ${realBinaryPath(archDir)}`);
}

async function main() {
  if (process.platform === 'win32') {
    console.log('[prepare-lume] skip on Windows (Apple VF is macOS-only)');
    return;
  }
  if (process.platform !== 'darwin' && process.argv.length <= 2) {
    console.log(`[prepare-lume] skip on ${process.platform} unless arch args are passed`);
    return;
  }
  const args = process.argv.slice(2).filter(Boolean);
  // CUA lume-v0.3.9 ships darwin-arm64 only (no darwin-x64 asset). Default
  // matches this file's header: host arch. Pass `arm64 x64` for a release
  // that needs both — x64 will 404 until CUA publishes it.
  const archs = args.length > 0
    ? args
    : (process.platform === 'darwin' ? [process.arch === 'arm64' ? 'arm64' : 'x64'] : []);
  if (archs.length === 0) return;
  let staged = 0;
  for (const arch of archs) {
    try {
      await fetchArch(arch);
      staged += 1;
    } catch (err) {
      // CUA ships darwin-arm64 only; an x64 404 must not fail the release —
      // the x64 package goes without lume (same capability as pre-Lume
      // releases). arm64 failing still fails the build via staged === 0.
      console.warn(`[prepare-lume] WARN: ${arch} unavailable (${err.message}); continuing without it`);
    }
  }
  if (staged === 0) {
    throw new Error('no lume arch could be staged');
  }
  // Unpackaged allternit-api looks for lume next to itself in resources/bin.
  if (process.platform === 'darwin') {
    const host = process.arch === 'arm64' ? 'arm64' : 'x64';
    const src = path.join(outRoot, host);
    const binDir = path.join(desktopDir, 'resources', 'bin');
    if (alreadyPresent(src)) {
      fs.mkdirSync(binDir, { recursive: true });
      execSync(`ditto "${src}/." "${binDir}/"`);
      console.log(`[prepare-lume] copied host ${host} into ${binDir}`);
    }
  }
}

main().catch((err) => {
  console.error(`[prepare-lume] ${err.message}`);
  process.exit(1);
});
