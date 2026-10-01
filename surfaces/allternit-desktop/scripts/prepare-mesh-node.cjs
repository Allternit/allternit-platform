/**
 * Stage the mesh-node tsnet sidecar into resources/bin/ so electron-builder
 * packs it (extraResources resources/bin/ → Contents/Resources/bin/, where
 * src/main/mesh-manager.ts resolves it in packaged builds).
 *
 * mesh-node (infrastructure/mesh/tsnet-ios/cmd/mesh-node) joins the Allternit
 * tailnet in pure userspace and reverse-proxies one tailnet target per
 * process — it's what gives the desktop app reachability to 100.64.0.0/10
 * gizzi instances without a system VPN.
 *
 * Acquisition order (first success wins):
 *   1. Copy from the repo vendor tree (cmd/gizzi-code/vendor/mesh-node/
 *      <platform>-<arch>/mesh-node) — same binary gizzi-code's mesh.ts uses —
 *      when it is at least as new as the mesh-node source.
 *   2. Build from source via infrastructure/mesh/tsnet-ios/build-sidecar.sh
 *      (darwin-arm64 / linux-x64), or an equivalent `go build` for win32-x64
 *      (mirrors the GOOS/GOARCH mapping in release-gizzi-code.yml). Requires
 *      Go on PATH.
 *   3. Download the latest gizzi-code GitHub release asset (the v0.2.2+
 *      tarballs/zips ship mesh-node next to gizzi-code) and extract just
 *      mesh-node.
 *
 * Idempotent: if resources/bin/mesh-node[.exe] already exists and is at least
 * as new as the mesh-node source, does nothing. Older binaries (staged or
 * vendored) are rebuilt when Go is available, so source fixes actually ship
 * instead of a build machine reusing its first binary forever.
 */

const fs = require('node:fs');
const https = require('node:https');
const os = require('node:os');
const path = require('node:path');
const { spawnSync } = require('node:child_process');

const REPO_ROOT = path.resolve(__dirname, '..', '..', '..');
const BINARY_NAME = process.platform === 'win32' ? 'mesh-node.exe' : 'mesh-node';
// Vendor directory naming matches build-sidecar.sh (`${platform}-${arch}`,
// Node-style x64) — the same convention gizzi-code's mesh.ts discovery uses.
const PLATFORM_ARCH = `${process.platform}-${process.arch}`;
const OUTPUT_DIR = path.resolve(__dirname, '..', 'resources', 'bin');
const OUTPUT = path.join(OUTPUT_DIR, BINARY_NAME);
const VENDOR_BINARY = path.join(
  REPO_ROOT, 'cmd', 'gizzi-code', 'vendor', 'mesh-node', PLATFORM_ARCH, BINARY_NAME,
);
const SIDECAR_DIR = path.join(REPO_ROOT, 'infrastructure', 'mesh', 'tsnet-ios');

const GITHUB_REPO = 'Gizziio/allternit-platform';
// Asset naming from release-gizzi-code.yml (mesh-node shipped since v0.2.2).
const RELEASE_ASSET = {
  'darwin-arm64': { pattern: /-darwin-arm64\.tar\.gz$/, member: 'mesh-node' },
  'linux-x64': { pattern: /-linux-x64\.tar\.gz$/, member: 'mesh-node' },
  'win32-x64': { pattern: /-windows-x64\.zip$/, member: 'mesh-node.exe' },
};

/** Newest mtime of the mesh-node Go sources (0 when they aren't in this checkout). */
function newestSourceMtime(dir = SIDECAR_DIR) {
  if (!fs.existsSync(dir)) return 0;
  let newest = 0;
  for (const entry of fs.readdirSync(dir, { withFileTypes: true })) {
    const full = path.join(dir, entry.name);
    if (entry.isDirectory()) {
      newest = Math.max(newest, newestSourceMtime(full));
    } else if (/\.go$|^go\.(mod|sum)$/.test(entry.name)) {
      newest = Math.max(newest, fs.statSync(full).mtimeMs);
    }
  }
  return newest;
}

/** True when `binary` exists and isn't older than the mesh-node source. */
function isCurrent(binary, sourceMtime) {
  return fs.existsSync(binary) && fs.statSync(binary).mtimeMs >= sourceMtime;
}

function run(command, args, options = {}) {
  const result = spawnSync(command, args, { stdio: 'inherit', ...options });
  return result.status === 0;
}

function stage(binary) {
  fs.copyFileSync(binary, OUTPUT);
  fs.chmodSync(OUTPUT, 0o755);
}

function stageFromVendor(sourceMtime) {
  if (!isCurrent(VENDOR_BINARY, sourceMtime)) return false;
  stage(VENDOR_BINARY);
  console.log(`Staged mesh-node from repo vendor tree (${path.relative(REPO_ROOT, VENDOR_BINARY)}).`);
  return true;
}

function stageFromBuild() {
  if (!run('go', ['version'], { stdio: 'ignore' })) return false;
  if (PLATFORM_ARCH === 'win32-x64') {
    // build-sidecar.sh only covers the unix targets; the win32 GOOS/GOARCH
    // mapping mirrors the mesh-node step in release-gizzi-code.yml.
    const env = { ...process.env, CGO_ENABLED: '0', GOOS: 'windows', GOARCH: 'amd64' };
    fs.mkdirSync(path.dirname(VENDOR_BINARY), { recursive: true });
    if (!run('go', ['build', '-ldflags=-s -w', '-o', VENDOR_BINARY, './cmd/mesh-node'], { cwd: SIDECAR_DIR, env })) {
      return false;
    }
  } else if (PLATFORM_ARCH === 'darwin-arm64' || PLATFORM_ARCH === 'linux-x64') {
    if (!run('bash', ['build-sidecar.sh'], { cwd: SIDECAR_DIR })) return false;
  } else {
    return false;
  }
  if (!fs.existsSync(VENDOR_BINARY)) return false;
  stage(VENDOR_BINARY);
  console.log(`Built mesh-node from source (infrastructure/mesh/tsnet-ios) for ${PLATFORM_ARCH}.`);
  return true;
}

function request(url, redirects = 0) {
  if (redirects > 5) return Promise.reject(new Error('Too many redirects'));
  return new Promise((resolve, reject) => {
    https.get(url, { headers: { 'User-Agent': 'allternit-desktop-packager' } }, (response) => {
      if (response.statusCode >= 300 && response.statusCode < 400 && response.headers.location) {
        response.resume();
        request(response.headers.location, redirects + 1).then(resolve, reject);
        return;
      }
      if (response.statusCode !== 200) {
        response.resume();
        reject(new Error(`Request to ${url} failed with HTTP ${response.statusCode}`));
        return;
      }
      resolve(response);
    }).on('error', reject);
  });
}

async function download(url, destination) {
  const response = await request(url);
  const file = fs.createWriteStream(destination, { mode: 0o600 });
  response.pipe(file);
  return new Promise((resolve, reject) => {
    file.on('finish', () => file.close(resolve));
    file.on('error', reject);
  });
}

async function stageFromDownload() {
  const spec = RELEASE_ASSET[PLATFORM_ARCH];
  if (!spec) return false;
  const response = await request(`https://api.github.com/repos/${GITHUB_REPO}/releases?per_page=20`);
  const releases = JSON.parse(await new Promise((resolve, reject) => {
    let body = '';
    response.on('data', (chunk) => { body += chunk; });
    response.on('end', () => resolve(body));
    response.on('error', reject);
  }));
  // Releases come back newest-first; the first gizzi-code tag is the latest
  // sidecar build (v0.2.2+ all ship mesh-node in the asset archive).
  for (const release of releases) {
    if (!release.tag_name || !release.tag_name.startsWith('gizzi-code/')) continue;
    const asset = (release.assets || []).find((a) => spec.pattern.test(a.name));
    if (!asset) continue;
    const tempDir = fs.mkdtempSync(path.join(os.tmpdir(), 'allternit-mesh-node-'));
    try {
      const archive = path.join(tempDir, asset.name);
      await download(asset.browser_download_url, archive);
      // tar handles .tar.gz everywhere and .zip via the bsdtar shipped with
      // macOS and Windows 10+ — no extra tooling needed on any CI runner.
      if (!run('tar', ['-xf', archive, '-C', tempDir, spec.member], { stdio: 'inherit' })) {
        throw new Error(`Unable to extract ${spec.member} from ${asset.name}`);
      }
      stage(path.join(tempDir, spec.member));
      console.log(`Staged mesh-node from ${release.tag_name} release asset ${asset.name}.`);
      return true;
    } finally {
      fs.rmSync(tempDir, { recursive: true, force: true });
    }
  }
  return false;
}

function stageStaleVendor() {
  if (!fs.existsSync(VENDOR_BINARY)) return false;
  stage(VENDOR_BINARY);
  console.warn('Staged an older vendored mesh-node: its source changed since, and Go is not available to rebuild it.');
  return true;
}

(async () => {
  const sourceMtime = newestSourceMtime();
  if (isCurrent(OUTPUT, sourceMtime)) {
    console.log(`mesh-node already staged at ${path.relative(REPO_ROOT, OUTPUT)}; skipping.`);
    return;
  }
  fs.mkdirSync(OUTPUT_DIR, { recursive: true });
  if (stageFromVendor(sourceMtime) || stageFromBuild() || stageStaleVendor() || await stageFromDownload()) return;
  console.error(
    `Unable to stage mesh-node for ${PLATFORM_ARCH}: not in the repo vendor tree, ` +
    'Go is not available to build it, and no gizzi-code release asset matched.',
  );
  process.exitCode = 1;
})().catch((error) => {
  console.error(error);
  process.exitCode = 1;
});
