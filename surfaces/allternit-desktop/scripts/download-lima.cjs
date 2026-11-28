/**
 * Downloads limactl for macOS and stages it for electron-builder.
 *
 * Usage:
 *   node scripts/download-lima.cjs                 # macOS host: Darwin limactl; other hosts: nothing
 *   node scripts/download-lima.cjs arm64           # Darwin arm64 (CI universal lipo)
 *   node scripts/download-lima.cjs x86_64 linux    # Linux x86_64, explicit only (not packaged)
 *
 * Windows and Linux builds do not bundle Lima (ALLTERNIT_PACK_OS=linux skips).
 */

const https = require('https');
const fs = require('fs');
const path = require('path');
const { execSync } = require('child_process');
const { withRetry, guardSocket } = require('./retry-download.cjs');

const LIMA_VERSION = '2.1.2';
const outDir = path.join(__dirname, '..', 'resources', 'lima');

const ARCH_MAP = {
  arm64: 'arm64',
  aarch64: 'aarch64',
  x64: 'x86_64',
  x86_64: 'x86_64',
};

function limaOsName(platform) {
  if (platform === 'darwin') return 'Darwin';
  if (platform === 'linux') return 'Linux';
  return null;
}

function limaArchName(platform, rawArch) {
  const mapped = ARCH_MAP[rawArch];
  if (!mapped) return null;
  if (platform === 'linux' && mapped === 'arm64') return 'aarch64';
  if (platform === 'darwin' && mapped === 'aarch64') return 'arm64';
  return mapped;
}

function binaryPathFor(platform) {
  const dir = platform === 'darwin' ? 'darwin' : 'linux';
  return path.join(outDir, dir, 'limactl');
}

function alreadyPresent(binaryPath, platform) {
  if (!fs.existsSync(binaryPath)) return false;
  if (platform === process.platform) {
    try {
      const ver = execSync(`"${binaryPath}" --version`, { encoding: 'utf8' }).trim();
      return ver.includes(LIMA_VERSION);
    } catch {
      return false;
    }
  }
  try {
    return fs.statSync(binaryPath).size > 1000;
  } catch {
    return false;
  }
}

function download(url, dest, cb) {
  const file = fs.createWriteStream(dest);
  guardSocket(https.get(url, (res) => {
    if (res.statusCode === 301 || res.statusCode === 302) {
      file.close();
      fs.unlinkSync(dest);
      return download(res.headers.location, dest, cb);
    }
    if (res.statusCode !== 200) {
      file.close();
      cb(new Error(`HTTP ${res.statusCode} for ${url}`));
      return;
    }
    res.pipe(file);
    file.on('finish', () => file.close(cb));
  }), url).on('error', (err) => {
    try { fs.unlinkSync(dest); } catch { /* ignore */ }
    cb(err);
  });
}

function extractLimactl(tarballPath, binaryPath) {
  const destDir = path.dirname(binaryPath);
  fs.mkdirSync(destDir, { recursive: true });
  try {
    execSync(
      `tar -xzf "${tarballPath}" --strip-components=2 -C "${destDir}" ./bin/limactl`,
      { stdio: 'pipe' }
    );
  } catch {
    execSync(
      `tar -xzf "${tarballPath}" --strip-components=1 -C "${destDir}" bin/limactl`,
      { stdio: 'pipe' }
    );
  }
  if (!fs.existsSync(binaryPath)) {
    throw new Error(`limactl was not extracted to ${binaryPath}`);
  }
  fs.chmodSync(binaryPath, 0o755);
  if (process.platform === 'darwin' && binaryPath.includes(`${path.sep}darwin${path.sep}`)) {
    try {
      execSync(`xattr -d com.apple.quarantine "${binaryPath}"`, { stdio: 'pipe' });
    } catch {
      /* attribute may already be absent */
    }
  }
}

function fetchOne(platform, rawArch) {
  return new Promise((resolve, reject) => {
    const osName = limaOsName(platform);
    const arch = limaArchName(platform, rawArch);
    if (!osName || !arch) {
      reject(new Error(`Unsupported Lima target ${platform}/${rawArch}`));
      return;
    }
    const binaryPath = binaryPathFor(platform);
    if (alreadyPresent(binaryPath, platform)) {
      console.log(`limactl ${LIMA_VERSION} already present at ${binaryPath} — skipping download`);
      resolve(binaryPath);
      return;
    }
    const tarballName = `lima-${LIMA_VERSION}-${osName}-${arch}.tar.gz`;
    const url = `https://github.com/lima-vm/lima/releases/download/v${LIMA_VERSION}/${tarballName}`;
    const tarballPath = path.join(outDir, tarballName);
    fs.mkdirSync(outDir, { recursive: true });
    console.log(`Downloading ${url}...`);
    withRetry(`Lima ${tarballName}`, () => new Promise((done, fail) => {
      download(url, tarballPath, (err) => (err ? fail(err) : done()));
    })).then(() => {
      try {
        extractLimactl(tarballPath, binaryPath);
        fs.unlinkSync(tarballPath);
        console.log(`limactl ${LIMA_VERSION} ready at ${binaryPath}`);
        resolve(binaryPath);
      } catch (e) {
        reject(e);
      }
    }, reject);
  });
}

// Lima is bundled on macOS only. Linux Desktop has no Lima VM feature, and the
// upstream Linux limactl needs glibc 2.34, above the Linux floor (2.27, see
// scripts/build-linux-sidecars.sh), so it is neither downloaded nor packaged there.
// Pass `x86_64 linux` explicitly to fetch it for local experiments.
function targets() {
  const osArg = process.argv[3] || process.env.ALLTERNIT_PACK_OS;
  const archArg = process.argv[2];
  if (osArg) {
    const platform = osArg === 'mac' || osArg === 'macos' ? 'darwin' : osArg;
    if (platform === 'win32' || platform === 'windows') return [];
    if (platform === 'linux' && !process.argv[3]) return [];
    return [{ platform, arch: archArg || process.arch }];
  }
  if (process.platform !== 'darwin') return [];
  return [{ platform: 'darwin', arch: archArg || process.arch }];
}

(async () => {
  fs.mkdirSync(path.join(outDir, 'darwin'), { recursive: true });
  fs.mkdirSync(path.join(outDir, 'linux'), { recursive: true });
  const planned = targets();
  if (planned.length === 0) {
    console.log('Lima is bundled on macOS only — skipping download');
    return;
  }
  for (const target of planned) {
    await fetchOne(target.platform, target.arch);
  }
})().catch((error) => {
  console.error('Lima download failed:', error.message || error);
  process.exit(1);
});
