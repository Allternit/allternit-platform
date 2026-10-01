#!/usr/bin/env node
/**
 * Runtime packages: ship allternit-api, gizzi-code and the platform screens
 * without a new Desktop app build (see src/main/runtime-package.ts).
 *
 *   node scripts/runtime-package.cjs stamp     write resources/runtime.json (build number for this checkout)
 *   node scripts/runtime-package.cjs pack      package resources/{bin,platform} → release/runtime/stable/<platform>/
 *   node scripts/runtime-package.cjs publish   print the upload commands; add --confirm to upload to R2
 *
 * Build the parts first without packaging the app:
 *   scripts/build-desktop.sh --skip-electron && (cd surfaces/allternit-desktop &&
 *     npm run prepare:platform-static && npm run prepare:api-binary)
 *
 * Signing key: ALLTERNIT_RUNTIME_SIGNING_KEY or ~/.config/allternit/runtime-signing-ed25519.pem
 * (Ed25519, PKCS#8 PEM). Its public half is RUNTIME_PUBLIC_KEY in runtime-package.ts.
 */
const crypto = require('node:crypto');
const fs = require('node:fs');
const os = require('node:os');
const path = require('node:path');
const { execFileSync } = require('node:child_process');

const DESKTOP_DIR = path.resolve(__dirname, '..');
const REPO_ROOT = path.resolve(DESKTOP_DIR, '..', '..');
const RESOURCES = path.join(DESKTOP_DIR, 'resources');
const STAMP = path.join(RESOURCES, 'runtime.json');
const PLATFORM = `${process.platform}-${process.arch}`;
const OUT = path.join(DESKTOP_DIR, 'release', 'runtime', 'stable', PLATFORM);
const BUCKET = process.env.ALLTERNIT_RUNTIME_BUCKET || 'allternit-runtime';
const EXE = process.platform === 'win32' ? '.exe' : '';
const BINARIES = [`allternit-api${EXE}`, `gizzi-code${EXE}`];

const die = (msg) => { console.error(`✗ ${msg}`); process.exit(1); };
const sha256 = (file) => crypto.createHash('sha256').update(fs.readFileSync(file)).digest('hex');

function git(dir, ...args) {
  try { return execFileSync('git', ['-C', dir, ...args], { encoding: 'utf8' }).trim(); } catch { return ''; }
}

function shellApi() {
  const src = fs.readFileSync(path.join(DESKTOP_DIR, 'src', 'main', 'runtime-package.ts'), 'utf8');
  const m = src.match(/export const SHELL_API = (\d+);/);
  if (!m) die('SHELL_API not found in runtime-package.ts');
  return Number(m[1]);
}

function stamp() {
  const aiDir = process.env.ALLTERNIT_AI_PATH;
  const times = [Number(git(REPO_ROOT, 'log', '-1', '--format=%ct')) || 0];
  if (aiDir) times.push(Number(git(aiDir, 'log', '-1', '--format=%ct')) || 0);
  // Commit time orders builds; the stamp time breaks ties so a rebuild of the same commits still wins.
  const build = Math.max(...times, 0) * 1000 + (Math.floor(Date.now() / 1000) % 1000);
  const d = new Date(Math.max(...times) * 1000 || Date.now()).toISOString();
  const sha = git(REPO_ROOT, 'rev-parse', '--short=7', 'HEAD') || 'local';
  const version = `r${d.slice(0, 10).replace(/-/g, '')}.${d.slice(11, 16).replace(':', '')}-${sha}-${build % 1000}`;
  const info = {
    version, build, shellApi: shellApi(),
    platformSha: git(REPO_ROOT, 'rev-parse', 'HEAD') || null,
    aiSha: aiDir ? git(aiDir, 'rev-parse', 'HEAD') || null : null,
  };
  fs.mkdirSync(RESOURCES, { recursive: true });
  fs.writeFileSync(STAMP, JSON.stringify(info, null, 2));
  console.log(`✓ runtime stamp ${version} (build ${build}) → ${path.relative(REPO_ROOT, STAMP)}`);
  return info;
}

function walk(dir, base = dir, out = []) {
  for (const e of fs.readdirSync(dir, { withFileTypes: true })) {
    const abs = path.join(dir, e.name);
    if (e.isDirectory()) walk(abs, base, out);
    else if (e.isFile()) out.push(path.relative(base, abs).split(path.sep).join('/'));
  }
  return out;
}

function signingKey() {
  const file = process.env.ALLTERNIT_RUNTIME_SIGNING_KEY || path.join(os.homedir(), '.config', 'allternit', 'runtime-signing-ed25519.pem');
  if (!fs.existsSync(file)) die(`signing key not found at ${file}`);
  return crypto.createPrivateKey(fs.readFileSync(file));
}

function pack() {
  const info = fs.existsSync(STAMP) ? JSON.parse(fs.readFileSync(STAMP, 'utf8')) : stamp();
  for (const b of BINARIES) {
    if (!fs.existsSync(path.join(RESOURCES, 'bin', b))) die(`resources/bin/${b} missing — build it first`);
  }
  if (!fs.existsSync(path.join(RESOURCES, 'platform', 'index.html'))) die('resources/platform missing — run npm run prepare:platform-static');

  const key = signingKey();
  const staging = fs.mkdtempSync(path.join(os.tmpdir(), 'allternit-runtime-'));
  try {
    fs.mkdirSync(path.join(staging, 'bin'));
    for (const b of BINARIES) fs.copyFileSync(path.join(RESOURCES, 'bin', b), path.join(staging, 'bin', b));
    fs.cpSync(path.join(RESOURCES, 'platform'), path.join(staging, 'platform'), { recursive: true });

    const files = {};
    for (const rel of walk(staging)) {
      const abs = path.join(staging, rel);
      files[rel] = { sha256: sha256(abs), size: fs.statSync(abs).size };
    }
    const manifest = Buffer.from(JSON.stringify({
      version: info.version, build: info.build, platform: PLATFORM, shellApi: info.shellApi,
      platformSha: info.platformSha, aiSha: info.aiSha, files,
    }, null, 2));
    fs.writeFileSync(path.join(staging, 'manifest.json'), manifest);
    fs.writeFileSync(path.join(staging, 'manifest.sig'), crypto.sign(null, manifest, key).toString('base64'));

    fs.rmSync(OUT, { recursive: true, force: true });
    fs.mkdirSync(OUT, { recursive: true });
    const archive = path.join(OUT, `${info.version}.tar.gz`);
    execFileSync('tar', ['-czf', archive, '-C', staging, '.']);
    const latest = Buffer.from(JSON.stringify({
      version: info.version, build: info.build, shellApi: info.shellApi,
      url: `${info.version}.tar.gz`, sha256: sha256(archive), size: fs.statSync(archive).size,
    }, null, 2));
    fs.writeFileSync(path.join(OUT, 'latest.json'), latest);
    fs.writeFileSync(path.join(OUT, 'latest.json.sig'), crypto.sign(null, latest, key).toString('base64'));
    const mb = (fs.statSync(archive).size / 1e6).toFixed(1);
    console.log(`✓ runtime package ${info.version} (${Object.keys(files).length} files, ${mb} MB) → ${path.relative(REPO_ROOT, OUT)}`);
  } finally {
    fs.rmSync(staging, { recursive: true, force: true });
  }
}

function publish(confirm) {
  const latestPath = path.join(OUT, 'latest.json');
  if (!fs.existsSync(latestPath)) die('nothing packed — run pack first');
  const latest = JSON.parse(fs.readFileSync(latestPath, 'utf8'));
  // The archive goes up first and latest.json last, so clients never see a pointer to a missing file.
  const uploads = [latest.url, 'latest.json.sig', 'latest.json'].map((name) => [
    'npx', 'wrangler', 'r2', 'object', 'put', `${BUCKET}/stable/${PLATFORM}/${name}`,
    '--file', path.join(OUT, name), '--remote',
    ...(name.startsWith('latest') ? ['--cache-control', 'no-store'] : ['--cache-control', 'public, max-age=31536000, immutable']),
  ]);
  console.log(`Runtime ${latest.version} → r2://${BUCKET}/stable/${PLATFORM}/ (${(latest.size / 1e6).toFixed(1)} MB)`);
  for (const cmd of uploads) {
    console.log(`  ${cmd.join(' ')}`);
    if (confirm) execFileSync(cmd[0], cmd.slice(1), { stdio: 'inherit', cwd: REPO_ROOT });
  }
  if (!confirm) console.log('Dry run. Re-run with --confirm to upload.');
}

const [cmd, ...rest] = process.argv.slice(2);
if (cmd === 'stamp') stamp();
else if (cmd === 'pack') pack();
else if (cmd === 'publish') publish(rest.includes('--confirm'));
else die('usage: runtime-package.cjs stamp | pack | publish [--confirm]');
