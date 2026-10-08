const crypto = require('node:crypto');
const fs = require('node:fs');
const https = require('node:https');
const os = require('node:os');
const path = require('node:path');
const { spawnSync } = require('node:child_process');

const VERSION = '0.34.0';
const RELEASE = `https://github.com/trycua/cua/releases/download/cua-driver-rs-v${VERSION}`;
const outputRoot = path.resolve(__dirname, '..', 'resources', 'computer-use');

const ASSETS = {
  darwin: {
    asset: `cua-driver-rs-${VERSION}-darwin-universal-binary.tar.gz`,
    sha256: '940dc008e0f7c5d217d14c0f247d1ebab91b1bac965f4a649d19e8c789bdfd81',
    binary: 'cua-driver',
  },
  linux: {
    asset: `cua-driver-rs-${VERSION}-linux-x86_64-binary.tar.gz`,
    sha256: '629ac96eff829d4dfd5cf221f3f2165c2d813aed91e5efb7b20777a741cd70a7',
    binary: 'cua-driver',
  },
  win32: {
    asset: `cua-driver-rs-${VERSION}-windows-x86_64-binary.zip`,
    sha256: 'bcc520e50861c7092cf775846fec76ae386d7dcd6b5b408608b0ea4423a8b888',
    binary: 'cua-driver.exe',
  },
};

function sha256File(file) {
  return crypto.createHash('sha256').update(fs.readFileSync(file)).digest('hex');
}

function packTargets() {
  const requested = process.env.ALLTERNIT_PACK_OS;
  if (requested) {
    if (!ASSETS[requested]) {
      throw new Error(`Unsupported ALLTERNIT_PACK_OS=${requested} (expected darwin, linux, or win32)`);
    }
    return [requested];
  }
  if (process.env.ALLTERNIT_PACK_ALL === '1' || process.platform === 'darwin') {
    return ['darwin', 'linux', 'win32'];
  }
  if (!ASSETS[process.platform]) {
    console.log(`Cua Driver bundle preparation skipped: no asset for ${process.platform}.`);
    return [];
  }
  return [process.platform];
}

function download(url, destination, redirects = 0) {
  if (redirects > 5) return Promise.reject(new Error('Too many redirects while downloading Cua Driver'));
  return new Promise((resolve, reject) => {
    https.get(url, { headers: { 'User-Agent': 'allternit-desktop-packager' } }, (response) => {
      if (response.statusCode >= 300 && response.statusCode < 400 && response.headers.location) {
        response.resume();
        download(response.headers.location, destination, redirects + 1).then(resolve, reject);
        return;
      }
      if (response.statusCode !== 200) {
        response.resume();
        reject(new Error(`Cua Driver download failed with HTTP ${response.statusCode}`));
        return;
      }
      const file = fs.createWriteStream(destination, { mode: 0o600 });
      response.pipe(file);
      file.on('finish', () => file.close(resolve));
      file.on('error', reject);
    }).on('error', reject);
  });
}

function extractArchive(archive, tempDir) {
  if (archive.endsWith('.zip')) {
    const unzip = spawnSync('unzip', ['-o', archive, '-d', tempDir], { encoding: 'utf8' });
    if (unzip.status === 0) return;
    const tar = spawnSync('tar', ['-xf', archive, '-C', tempDir], { encoding: 'utf8' });
    if (tar.status !== 0) {
      throw new Error(unzip.stderr || tar.stderr || 'Unable to extract Cua Driver zip');
    }
    return;
  }
  const extract = spawnSync('tar', ['-xzf', archive, '-C', tempDir], { encoding: 'utf8' });
  if (extract.status !== 0) throw new Error(extract.stderr || 'Unable to extract Cua Driver');
}

function findBinary(tempDir, binaryName) {
  return fs.readdirSync(tempDir, { recursive: true })
    .map((entry) => path.join(tempDir, entry))
    .find((entry) => path.basename(entry) === binaryName && fs.statSync(entry).isFile());
}

async function prepareTarget(platform) {
  const spec = ASSETS[platform];
  const platformDir = path.join(outputRoot, platform);
  const output = path.join(platformDir, spec.binary);
  fs.mkdirSync(platformDir, { recursive: true });

  const versionPath = path.join(outputRoot, 'VERSION.json');
  if (fs.existsSync(output) && fs.existsSync(versionPath)) {
    try {
      const existing = JSON.parse(fs.readFileSync(versionPath, 'utf8'));
      const entry = existing.platforms?.[platform] || {};
      const recorded = entry.sha256 || (platform === 'darwin' ? existing.sha256 : '');
      // The archive checksum alone can't prove the staged binary is that
      // release (a stale binary next to a bumped VERSION.json would pass), so
      // the binary's own hash, recorded when it was staged, must match too.
      if (existing.version === VERSION && recorded === spec.sha256 && entry.binarySha256 === sha256File(output)) {
        console.log(`Cua Driver ${VERSION} already present for ${platform} at ${output}`);
        return { ...spec, binarySha256: entry.binarySha256 };
      }
    } catch {
      /* re-download */
    }
  }

  const url = `${RELEASE}/${spec.asset}`;
  const tempDir = fs.mkdtempSync(path.join(os.tmpdir(), `allternit-cua-driver-${platform}-`));
  const archive = path.join(tempDir, spec.asset);
  try {
    await download(url, archive);
    const actual = crypto.createHash('sha256').update(fs.readFileSync(archive)).digest('hex');
    if (actual !== spec.sha256) {
      throw new Error(`Cua Driver checksum mismatch for ${platform}: expected ${spec.sha256}, got ${actual}`);
    }
    extractArchive(archive, tempDir);
    const candidate = findBinary(tempDir, spec.binary);
    if (!candidate) throw new Error(`Cua Driver binary ${spec.binary} was not present in the ${platform} archive`);
    fs.copyFileSync(candidate, output);
    if (platform !== 'win32') fs.chmodSync(output, 0o755);
    console.log(`Prepared embedded Cua Driver ${VERSION} for ${platform} at ${output}`);
    return { ...spec, binarySha256: sha256File(output) };
  } finally {
    fs.rmSync(tempDir, { recursive: true, force: true });
  }
}

(async () => {
  const targets = packTargets();
  if (targets.length === 0) return;
  fs.mkdirSync(outputRoot, { recursive: true });
  const versionPath = path.join(outputRoot, 'VERSION.json');
  let previous = {};
  try {
    previous = JSON.parse(fs.readFileSync(versionPath, 'utf8'));
  } catch {
    /* first run */
  }
  const prepared = {};
  for (const platform of targets) {
    prepared[platform] = await prepareTarget(platform);
  }
  const darwin = prepared.darwin || ASSETS.darwin;
  fs.writeFileSync(path.join(outputRoot, 'VERSION.json'), JSON.stringify({
    version: VERSION,
    asset: darwin.asset,
    sha256: darwin.sha256,
    source: `${RELEASE}/${darwin.asset}`,
    platforms: Object.fromEntries(
      Object.entries(ASSETS).map(([platform, spec]) => [platform, {
        asset: spec.asset,
        sha256: spec.sha256,
        binary: spec.binary,
        source: `${RELEASE}/${spec.asset}`,
        // Hash of the staged binary; kept for platforms not staged this run
        // only when that platform was already staged at this version.
        ...(prepared[platform]?.binarySha256
          ? { binarySha256: prepared[platform].binarySha256 }
          : previous.version === VERSION && previous.platforms?.[platform]?.binarySha256
            ? { binarySha256: previous.platforms[platform].binarySha256 }
            : {}),
      }])
    ),
  }, null, 2) + '\n');
})().catch((error) => {
  console.error(error);
  process.exitCode = 1;
});
