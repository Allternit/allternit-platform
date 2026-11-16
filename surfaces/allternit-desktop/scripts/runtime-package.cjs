#!/usr/bin/env node
/**
 * Runtime packages: ship allternit-api, gizzi-code and the platform screens
 * without a new Desktop app build (see src/main/runtime-package.ts).
 *
 *   node scripts/runtime-package.cjs stamp     write resources/runtime.json (build number + bundled file list)
 *   node scripts/runtime-package.cjs pack      write the feed for resources/{bin,platform,meeting-bot} + model-training → release/runtime/feed/
 *   node scripts/runtime-package.cjs publish   show what would upload; add --confirm to upload to R2
 *   node scripts/runtime-package.cjs prune     plan old-version cleanup (--keep N, default 3); --confirm deletes
 *
 * Feed layout (bucket allternit-runtime, served at runtime.allternit.com):
 *   objects/<sha256>.gz                          each file once, gzip, shared by every version and platform
 *   stable/<platform>/manifests/<version>.json   signed list of files {sha256, size} (+ .sig)
 *   stable/<platform>/latest.json                signed pointer to the newest manifest (+ .sig)
 * Clients copy files they already have (same hash) and download only the rest,
 * so an update that changes allternit-api downloads allternit-api, not the
 * whole 600 MB runtime.
 *
 * Build the parts first without packaging the app:
 *   scripts/build-desktop.sh --skip-electron && (cd surfaces/allternit-desktop &&
 *     npm run prepare:platform-static && npm run prepare:api-binary && npm run prepare:meeting-bot)
 *
 * Signing key: ALLTERNIT_RUNTIME_SIGNING_KEY or ~/.config/allternit/runtime-signing-ed25519.pem
 * (Ed25519, PKCS#8 PEM). Its public half is RUNTIME_PUBLIC_KEY in runtime-package.ts.
 * Upload: Cloudflare REST API with the wrangler login (`wrangler auth token`).
 */
const crypto = require('node:crypto');
const fs = require('node:fs');
const os = require('node:os');
const path = require('node:path');
const zlib = require('node:zlib');
const { execFileSync, spawnSync } = require('node:child_process');
const https = require('node:https');

const DESKTOP_DIR = path.resolve(__dirname, '..');
const REPO_ROOT = path.resolve(DESKTOP_DIR, '..', '..');
// ALLTERNIT_RUNTIME_RESOURCES packs another build's staged resources (e.g. a build worktree).
const RESOURCES = process.env.ALLTERNIT_RUNTIME_RESOURCES ? path.resolve(process.env.ALLTERNIT_RUNTIME_RESOURCES) : path.join(DESKTOP_DIR, 'resources');
const STAMP = path.join(RESOURCES, 'runtime.json');
// ALLTERNIT_RUNTIME_PLATFORM packs another platform's staged build, e.g. `linux-x64`
// from binaries built on a Linux host (cloud computers run the Linux app, which
// polls stable/linux-x64). The binaries must already be that platform's.
const PLATFORM = process.env.ALLTERNIT_RUNTIME_PLATFORM || `${process.platform}-${process.arch}`;
const TARGET_OS = PLATFORM.split('-')[0];
if (!['darwin', 'linux', 'win32'].includes(TARGET_OS)) {
  console.error(`✗ unknown runtime platform ${PLATFORM}`);
  process.exit(1);
}
const FEED = path.join(DESKTOP_DIR, 'release', 'runtime', 'feed');
const BUCKET = process.env.ALLTERNIT_RUNTIME_BUCKET || 'allternit-runtime';
const ACCOUNT = process.env.CLOUDFLARE_ACCOUNT_ID || '7cd19487307235aedc039d3a64ad7039';
const EXE = TARGET_OS === 'win32' ? '.exe' : '';
// allternit-factory (the Factory engine) rides with gizzi-code so a runtime
// update moves both together.
// allternit-files-mount (Files drive mount + big uploads) rides with gizzi-code
// too; macOS and Linux only (Windows uses the cfapi helper).
const BINARIES = [
  `allternit-api${EXE}`,
  `gizzi-code${EXE}`,
  `allternit-factory${EXE}`,
  ...(TARGET_OS === 'win32' ? [] : ['allternit-files-mount']),
];

const die = (msg) => { console.error(`✗ ${msg}`); process.exit(1); };
const sha256 = (buf) => crypto.createHash('sha256').update(buf).digest('hex');
const mb = (n) => `${(n / 1e6).toFixed(1)} MB`;

function git(dir, ...args) {
  try { return execFileSync('git', ['-C', dir, ...args], { encoding: 'utf8' }).trim(); } catch { return ''; }
}

function shellApi() {
  const src = fs.readFileSync(path.join(DESKTOP_DIR, 'src', 'main', 'runtime-package.ts'), 'utf8');
  const m = src.match(/export const SHELL_API = (\d+);/);
  if (!m) die('SHELL_API not found in runtime-package.ts');
  return Number(m[1]);
}

function walk(dir, base = dir, out = []) {
  for (const e of fs.readdirSync(dir, { withFileTypes: true })) {
    const abs = path.join(dir, e.name);
    if (e.isDirectory()) walk(abs, base, out);
    else if (e.isFile()) out.push(path.relative(base, abs).split(path.sep).join('/'));
  }
  return out;
}

// The Meetings bot (scripts/prepare-meeting-bot.cjs): resources/meeting-bot plus this
// platform's rtc-node binding from resources/meeting-bot-native/<platform>/, listed where
// the app has it after electron-builder's overlay (meeting-bot/node_modules/@livekit/…).
const BOT_DIR = path.join(RESOURCES, 'meeting-bot');
const BOT_NATIVE_DIR = path.join(RESOURCES, 'meeting-bot-native', PLATFORM);
const BOT_RUNNER = 'meeting-bot/dist/runner.js';

/** Manifest path -> source file for the Meetings bot; null when it is not staged for PLATFORM. */
let botSources;
function meetingBotSources() {
  if (botSources !== undefined) return botSources;
  botSources = null;
  if (!fs.existsSync(path.join(BOT_DIR, 'dist', 'runner.js')) || !fs.existsSync(BOT_NATIVE_DIR)) return null;
  const out = new Map();
  for (const rel of walk(BOT_DIR)) out.set(`meeting-bot/${rel}`, path.join(BOT_DIR, rel));
  for (const rel of walk(BOT_NATIVE_DIR)) out.set(`meeting-bot/node_modules/${rel}`, path.join(BOT_NATIVE_DIR, rel));
  botSources = out;
  return out;
}

// Allternit Tune (services/model-training), source only: the Studio host in
// allternit-api installs the engine's venv from it. Same files as the
// electron-builder extraResources `model-training` entry, so the bundled list
// matches what the app carries. Read from the checkout the staged resources
// belong to (ALLTERNIT_RUNTIME_RESOURCES = <worktree>/surfaces/allternit-desktop/resources).
const TUNE_DIR = [
  path.resolve(RESOURCES, '..', '..', '..', 'services', 'model-training'),
  path.join(REPO_ROOT, 'services', 'model-training'),
].find((dir) => fs.existsSync(path.join(dir, 'requirements.txt')));
const TUNE_REQUIREMENTS = 'model-training/requirements.txt';
const TUNE_SKIP = /^(tests|tools|\.venv|\.scratch|\.pytest_cache)\/|(^|\/)__pycache__\/|\.egg-info\/|^\.gitignore$|^requirements-dev\.txt$/;

/** Manifest path -> source file for Allternit Tune; empty when the checkout has none. */
function tuneSources() {
  if (!TUNE_DIR) return new Map();
  return new Map(walk(TUNE_DIR).filter((rel) => !TUNE_SKIP.test(rel)).map((rel) => [`model-training/${rel}`, path.join(TUNE_DIR, rel)]));
}

/** Where a manifest path's bytes come from. */
function sourceOf(rel) {
  if (rel.startsWith('model-training/')) return tuneSources().get(rel) ?? path.join(RESOURCES, rel);
  return rel.startsWith('meeting-bot/') ? meetingBotSources()?.get(rel) ?? path.join(RESOURCES, rel) : path.join(RESOURCES, rel);
}

/** The runtime's files under resources/, as manifest entries. Null when not built yet. */
function runtimeFiles() {
  if (!BINARIES.every((b) => fs.existsSync(path.join(RESOURCES, 'bin', b)))) return null;
  if (!fs.existsSync(path.join(RESOURCES, 'platform', 'index.html'))) return null;
  const sources = new Map([
    ...BINARIES.map((b) => [`bin/${b}`, path.join(RESOURCES, 'bin', b)]),
    ...walk(path.join(RESOURCES, 'platform')).map((r) => [`platform/${r}`, path.join(RESOURCES, 'platform', r)]),
    ...(meetingBotSources() ?? []),
    ...tuneSources(),
  ]);
  const files = {};
  for (const rel of [...sources.keys()].sort()) {
    const buf = fs.readFileSync(sources.get(rel));
    files[rel] = { sha256: sha256(buf), size: buf.length };
  }
  return files;
}

/**
 * The UI commit the staged screens were built from. prepare-platform-static writes
 * resources/platform/ui-source.json, so the stamp still knows the UI commit when
 * ALLTERNIT_AI_PATH is only set for the screen build (2026-10-03: a stamp without it
 * ordered by the older platform commit and came out lower than the live package).
 */
function stagedUiSource() {
  try {
    const src = JSON.parse(fs.readFileSync(path.join(RESOURCES, 'platform', 'ui-source.json'), 'utf8'));
    return src && src.commit && src.path ? src : null;
  } catch {
    return null;
  }
}

/**
 * Sign the Mac binaries with the Developer ID, under the same identifiers as the
 * app bundle. A linker's ad-hoc signature is a new identity on every publish, so
 * macOS asked again for Keychain access after each runtime update, once per
 * binary. This has to run before stamp and pack hash the files.
 */
function signMacBinaries() {
  if (!PLATFORM.startsWith('darwin') || process.platform !== 'darwin') return;
  const identities = execFileSync('security', ['find-identity', '-v', '-p', 'codesigning'], { encoding: 'utf8' });
  const developerId = identities.match(/"(Developer ID Application: [^"]+)"/);
  if (!developerId) {
    if (process.env.ALLTERNIT_RUNTIME_ALLOW_ADHOC === '1') return;
    die('no Developer ID Application certificate: an ad-hoc runtime brings the Keychain prompts back (ALLTERNIT_RUNTIME_ALLOW_ADHOC=1 for a local test)');
  }
  const entitlements = path.join(DESKTOP_DIR, 'build', 'entitlements.mac.plist');
  for (const binary of BINARIES) {
    const file = path.join(RESOURCES, 'bin', binary);
    if (!fs.existsSync(file)) continue;
    // codesign reports on stderr.
    const details = spawnSync('codesign', ['-dvv', file], { encoding: 'utf8' }).stderr || '';
    if (details.includes(`Identifier=${binary}\n`) && details.includes(`Authority=${developerId[1]}`)) continue;
    execFileSync('codesign', [
      '--force', '--options', 'runtime', '--timestamp', '--identifier', binary,
      '--entitlements', entitlements, '--sign', developerId[1], file,
    ], { stdio: 'inherit' });
    console.log(`✓ signed ${binary}`);
  }
  // The Meetings bot's native modules (the rtc-node binding): Developer ID like the app signs
  // its own native code, so a runtime update does not ship an unsigned dylib into a hardened app.
  for (const [rel, file] of meetingBotSources() ?? []) {
    if (!rel.endsWith('.node')) continue;
    const details = spawnSync('codesign', ['-dvv', file], { encoding: 'utf8' }).stderr || '';
    if (details.includes(`Authority=${developerId[1]}`)) continue;
    execFileSync('codesign', ['--force', '--options', 'runtime', '--timestamp', '--sign', developerId[1], file], { stdio: 'inherit' });
    console.log(`✓ signed ${rel}`);
  }
}

function stamp() {
  signMacBinaries();
  const staged = stagedUiSource();
  const aiDir = process.env.ALLTERNIT_AI_PATH || staged?.path;
  const aiRef = process.env.ALLTERNIT_AI_PATH ? 'HEAD' : staged?.commit;
  const times = [Number(git(REPO_ROOT, 'log', '-1', '--format=%ct')) || 0];
  if (aiDir) times.push(Number(git(aiDir, 'log', '-1', '--format=%ct', aiRef || 'HEAD')) || 0);
  // Commit time orders builds; the stamp time breaks ties so a rebuild of the same commits still wins.
  const build = Math.max(...times, 0) * 1000 + (Math.floor(Date.now() / 1000) % 1000);
  const d = new Date(Math.max(...times) * 1000 || Date.now()).toISOString();
  const sha = git(REPO_ROOT, 'rev-parse', '--short=7', 'HEAD') || 'local';
  const version = `r${d.slice(0, 10).replace(/-/g, '')}.${d.slice(11, 16).replace(':', '')}-${sha}-${build % 1000}`;
  // The bundled file list lets a runtime update copy unchanged files from the app instead of downloading them.
  const files = runtimeFiles();
  if (!files) console.warn('⚠ runtime parts not staged yet — runtime.json has no file list (updates download every file)');
  else if (!files[BOT_RUNNER]) console.warn(`⚠ meeting-bot not staged for ${PLATFORM} — run node scripts/prepare-meeting-bot.cjs (the bundled list has no Meetings bot)`);
  const info = {
    version, build, shellApi: shellApi(),
    platformSha: git(REPO_ROOT, 'rev-parse', 'HEAD') || null,
    // Uncommitted platform changes would ship without being on main; publish refuses them.
    platformDirty: git(REPO_ROOT, 'status', '--porcelain', '--untracked-files=no').split('\n').filter(Boolean).length,
    aiSha: aiDir ? git(aiDir, 'rev-parse', aiRef || 'HEAD') || null : null,
    ...(files ? { files } : {}),
  };
  fs.mkdirSync(RESOURCES, { recursive: true });
  fs.writeFileSync(STAMP, JSON.stringify(info, null, 2));
  console.log(`✓ runtime stamp ${version} (build ${build}${files ? `, ${Object.keys(files).length} files` : ''}) → ${path.relative(REPO_ROOT, STAMP)}`);
  return info;
}

function signingKey() {
  const file = process.env.ALLTERNIT_RUNTIME_SIGNING_KEY || path.join(os.homedir(), '.config', 'allternit', 'runtime-signing-ed25519.pem');
  if (!fs.existsSync(file)) die(`signing key not found at ${file}`);
  return crypto.createPrivateKey(fs.readFileSync(file));
}

function pack() {
  signMacBinaries();
  const info = fs.existsSync(STAMP) ? JSON.parse(fs.readFileSync(STAMP, 'utf8')) : stamp();
  const files = runtimeFiles();
  if (!files) die(`resources/bin/{${BINARIES.join(',')}} or resources/platform missing — build them first`);
  // Cloud computers and Desktop run the Meetings bot from the runtime package; never publish one without it.
  if (!files[BOT_RUNNER]) die(`${BOT_RUNNER} or meeting-bot-native/${PLATFORM} missing — run node scripts/prepare-meeting-bot.cjs (ALLTERNIT_RUNTIME_PLATFORM / ALLTERNIT_RUNTIME_RESOURCES pick the target)`);
  // Studio installs Allternit Tune from the runtime's model-training/ (allternit-api engines.rs).
  if (!files[TUNE_REQUIREMENTS]) die(`${TUNE_REQUIREMENTS} missing — services/model-training is not in this checkout`);
  const key = signingKey();
  const objectsDir = path.join(FEED, 'objects');
  const platformDir = path.join(FEED, 'stable', PLATFORM);
  fs.mkdirSync(objectsDir, { recursive: true });
  fs.mkdirSync(path.join(platformDir, 'manifests'), { recursive: true });

  let written = 0;
  for (const [rel, f] of Object.entries(files)) {
    const obj = path.join(objectsDir, `${f.sha256}.gz`);
    if (fs.existsSync(obj)) continue;
    fs.writeFileSync(obj, zlib.gzipSync(fs.readFileSync(sourceOf(rel)), { level: 9 }));
    written += 1;
  }
  const manifest = Buffer.from(JSON.stringify({
    version: info.version, build: info.build, platform: PLATFORM, shellApi: info.shellApi,
    platformSha: info.platformSha, platformDirty: info.platformDirty, aiSha: info.aiSha, files,
  }, null, 2));
  const manifestRel = `manifests/${info.version}.json`;
  fs.writeFileSync(path.join(platformDir, manifestRel), manifest);
  fs.writeFileSync(path.join(platformDir, `${manifestRel}.sig`), crypto.sign(null, manifest, key).toString('base64'));
  const latest = Buffer.from(JSON.stringify({
    format: 2, version: info.version, build: info.build, shellApi: info.shellApi,
    manifest: manifestRel, manifestSha256: sha256(manifest),
  }, null, 2));
  fs.writeFileSync(path.join(platformDir, 'latest.json'), latest);
  fs.writeFileSync(path.join(platformDir, 'latest.json.sig'), crypto.sign(null, latest, key).toString('base64'));
  console.log(`✓ runtime ${info.version}: ${Object.keys(files).length} files, ${written} new objects → ${path.relative(REPO_ROOT, FEED)}`);
}

function cloudflareToken() {
  if (process.env.CLOUDFLARE_API_TOKEN) return process.env.CLOUDFLARE_API_TOKEN;
  const out = execFileSync('npx', ['-y', 'wrangler@4', 'auth', 'token'], { encoding: 'utf8', cwd: os.tmpdir() }).trim().split('\n');
  return out[out.length - 1].trim();
}

// The wrangler OAuth token lasts about an hour, shorter than a full base upload. Every upload
// worker shares this one token; the first to get a 401 re-runs `wrangler auth token` (which
// refreshes it) and the others pick up the new value.
const auth = { token: '' };
function refreshToken(stale) {
  if (auth.token === stale) auth.token = cloudflareToken();
  return auth.token;
}

// Uploads go over HTTP/1.1 keep-alive, not fetch: Node's fetch multiplexes every PUT onto one
// HTTP/2 session, and once that session breaks (ERR_HTTP2_INVALID_SESSION) every retry reuses it.
const agent = new https.Agent({ keepAlive: true, maxSockets: 64 });
function put(url, method, headers, body) {
  return new Promise((resolve, reject) => {
    const req = https.request(url, { method, headers: { ...headers, ...(body ? { 'Content-Length': body.length } : {}) }, agent, timeout: 120_000 }, (res) => {
      const chunks = [];
      res.on('data', (c) => chunks.push(c));
      res.on('end', () => {
        const text = Buffer.concat(chunks).toString('utf8');
        resolve({ ok: res.statusCode >= 200 && res.statusCode < 300, status: res.statusCode, headers: { get: (k) => res.headers[k.toLowerCase()] ?? null }, text: async () => text });
      });
      res.on('error', reject);
    });
    req.on('timeout', () => req.destroy(new Error('request timed out')));
    req.on('error', reject);
    req.end(body);
  });
}

// Prune makes thousands of small requests; the account allows ~1200 / 5 min, so it paces itself.
const pace = { on: false, next: 0, gapMs: 300 };
async function gate() {
  if (!pace.on) return;
  const at = Math.max(Date.now(), pace.next);
  pace.next = at + pace.gapMs;
  if (at > Date.now()) await new Promise((r) => setTimeout(r, at - Date.now()));
}

async function r2(method, key, body, contentType, cacheControl) {
  const url = `https://api.cloudflare.com/client/v4/accounts/${ACCOUNT}/r2/buckets/${BUCKET}/objects/${key}`;
  for (let attempt = 1, refreshed = false; ; attempt++) {
    const token = auth.token;
    let res;
    await gate();
    try {
      res = await put(url, method, { Authorization: `Bearer ${token}`, ...(contentType ? { 'Content-Type': contentType } : {}), ...(cacheControl ? { 'Cache-Control': cacheControl } : {}) }, body);
    } catch (e) {
      // A dropped connection (network blip, laptop sleep) is retried like a 5xx.
      if (attempt >= 6) throw new Error(`${method} ${key}: ${e.code || e.cause?.code || e.message}`);
      await new Promise((r) => setTimeout(r, attempt * 5000));
      continue;
    }
    if (res.ok) return res;
    if (res.status === 401 && !refreshed && !process.env.CLOUDFLARE_API_TOKEN) {
      refreshed = true;
      refreshToken(token);
      continue;
    }
    if (res.status === 429 && attempt < 12) {
      // Cloudflare API rate limit (~1200 requests / 5 min): wait it out instead of dying.
      const after = Number(res.headers.get('retry-after')) || 0;
      await new Promise((r) => setTimeout(r, Math.max(after * 1000, attempt * 10_000)));
      continue;
    }
    if (attempt >= 4 || res.status < 500) throw new Error(`${method} ${key}: ${res.status} ${await res.text()}`);
    await new Promise((r) => setTimeout(r, attempt * 2000));
  }
}

async function remoteObjects() {
  const keys = new Set();
  let cursor = '';
  for (;;) {
    const url = `https://api.cloudflare.com/client/v4/accounts/${ACCOUNT}/r2/buckets/${BUCKET}/objects?prefix=objects/&per_page=1000${cursor ? `&cursor=${encodeURIComponent(cursor)}` : ''}`;
    const res = await fetch(url, { headers: { Authorization: `Bearer ${auth.token}` } });
    if (!res.ok) throw new Error(`list objects: ${res.status} ${await res.text()}`);
    const body = await res.json();
    for (const o of body.result ?? []) keys.add(o.key);
    cursor = body.result_info?.cursor ?? '';
    if (!body.result_info?.is_truncated || !cursor) return keys;
  }
}

/**
 * A published runtime must be built from merged, current code: the platform commit and the
 * workspace UI commit in the manifest must each be the tip of their origin/main. A package
 * from a stale or branch checkout silently ships without merged work (2026-10-21, Eoj: make
 * sure no agent deploys that way). In CI, being behind only means main moved during the run,
 * so it is a warning there; a commit that is not on main at all always fails.
 * ALLTERNIT_ALLOW_UNMERGED_RELEASE=1 overrides (Eoj's call only).
 */
function requireMergedSources(manifest) {
  if (process.env.ALLTERNIT_ALLOW_UNMERGED_RELEASE === '1') {
    console.warn('! ALLTERNIT_ALLOW_UNMERGED_RELEASE=1: skipping the merged-main check');
    return;
  }
  const inCi = process.env.GITHUB_ACTIONS === 'true' || process.env.CI === 'true';
  const problems = [];
  const check = (repo, sha) => {
    if (!sha) return problems.push(`${repo}: the manifest has no commit; re-stamp and re-pack`);
    let cmp;
    try {
      cmp = JSON.parse(execFileSync('gh', ['api', `repos/${repo}/compare/${sha}...main`, '--jq', '{status, ahead_by, behind_by}'], { encoding: 'utf8', stdio: ['ignore', 'pipe', 'pipe'], maxBuffer: 64 * 1024 * 1024 }));
    } catch (e) {
      return problems.push(`${repo}: could not compare ${sha.slice(0, 9)} with main (${String(e.message).split('\n')[0]})`);
    }
    if (cmp.status !== 'identical' && cmp.status !== 'ahead') {
      problems.push(`${repo}: ${sha.slice(0, 9)} is not on main (${cmp.status}, ${cmp.behind_by} commit(s) only on the build)`);
    } else if (cmp.ahead_by > 0) {
      const msg = `${repo}: ${sha.slice(0, 9)} is ${cmp.ahead_by} commit(s) behind main`;
      if (inCi) console.warn(`note (CI): ${msg}; main moved during this run`);
      else problems.push(msg);
    }
  };
  if (manifest.platformDirty) problems.push(`Allternit/allternit-platform: built with ${manifest.platformDirty} uncommitted change(s)`);
  if (manifest.platformDirty === undefined) problems.push('the manifest predates the dirty-tree check; re-stamp and re-pack');
  check('Allternit/allternit-platform', manifest.platformSha);
  check('Allternit/allternit-ai', manifest.aiSha);
  if (problems.length) {
    die(`refusing to publish a runtime that is not built from current main:\n  ${problems.join('\n  ')}\n  Build from clean worktrees on origin/main (platform and ALLTERNIT_AI_PATH), then stamp, pack and publish again.`);
  }
}

async function publish(confirm) {
  const platformDir = path.join(FEED, 'stable', PLATFORM);
  const latestPath = path.join(platformDir, 'latest.json');
  if (!fs.existsSync(latestPath)) die('nothing packed — run pack first');
  const latest = JSON.parse(fs.readFileSync(latestPath, 'utf8'));
  const manifest = JSON.parse(fs.readFileSync(path.join(platformDir, latest.manifest), 'utf8'));
  // The app only takes a package with a higher build than the one it runs, so publishing a
  // lower build moves the pointer but no client ever installs it. Refuse that unless forced.
  const live = await fetch(`https://runtime.allternit.com/stable/${PLATFORM}/latest.json`, { cache: 'no-store' })
    .then((r) => (r.ok ? r.json() : null)).catch(() => null);
  if (live && Number(live.build) >= Number(latest.build) && !process.env.ALLTERNIT_RUNTIME_ALLOW_OLDER) {
    die(`live package ${live.version} (build ${live.build}) is not older than ${latest.version} (build ${latest.build}); clients would ignore this publish. Re-stamp from newer commits, or set ALLTERNIT_RUNTIME_ALLOW_OLDER=1 to roll the pointer back on purpose.`);
  }
  requireMergedSources(manifest);
  auth.token = cloudflareToken();
  const have = await remoteObjects();
  const needed = [...new Set(Object.values(manifest.files).map((f) => `objects/${f.sha256}.gz`))].filter((k) => !have.has(k));
  const bytes = needed.reduce((n, k) => n + fs.statSync(path.join(FEED, k)).size, 0);
  console.log(`Runtime ${latest.version} (${PLATFORM}): ${Object.keys(manifest.files).length} files, ${needed.length} new objects to upload (${mb(bytes)}), ${have.size} already in r2://${BUCKET}`);
  if (!confirm) {
    console.log('Dry run. Re-run with --confirm to upload.');
    return;
  }
  let done = 0;
  const queue = [...needed];
  const worker = async () => {
    for (let key = queue.shift(); key; key = queue.shift()) {
      await r2('PUT', key, fs.readFileSync(path.join(FEED, key)), 'application/gzip', 'public, max-age=31536000, immutable');
      done += 1;
      if (done % 500 === 0) console.log(`  ${done}/${needed.length} objects`);
    }
  };
  // Each PUT is a separate Cloudflare API request, so throughput is set by latency, not bandwidth;
  // the account allows ~1200 API requests per 5 minutes.
  const workers = Math.max(1, Number(process.env.ALLTERNIT_RUNTIME_UPLOAD_WORKERS) || 16);
  await Promise.all(Array.from({ length: workers }, worker));
  // Manifest before the pointer, pointer last: a client never sees latest.json naming something missing.
  for (const rel of [latest.manifest, `${latest.manifest}.sig`, 'latest.json.sig', 'latest.json']) {
    const immutable = rel.startsWith('manifests/');
    await r2('PUT', `stable/${PLATFORM}/${rel}`, fs.readFileSync(path.join(platformDir, rel)),
      rel.endsWith('.json') ? 'application/json' : 'text/plain', immutable ? 'public, max-age=31536000, immutable' : 'no-store');
  }
  console.log(`✓ published ${latest.version} → https://runtime.allternit.com/stable/${PLATFORM}/latest.json`);
  console.log('Run `node surfaces/allternit-desktop/scripts/runtime-package.cjs prune --confirm` to delete old versions (keeps 3).');
}

/** Order of a manifest for "newest": its build number, else the rNNNNNNNN.NNNN timestamp in its version. */
function manifestOrder(m, key) {
  if (Number.isFinite(Number(m?.build)) && m?.build !== undefined && m?.build !== null) return [1, Number(m.build), ''];
  const v = String(m?.version ?? key.split('/').pop().replace(/\.json$/, ''));
  const t = v.match(/r(\d{8})\.(\d+)/);
  return t ? [0, Number(t[1]) * 1e6 + Number(t[2]), ''] : [0, 0, v];
}
const cmpOrder = (a, b) => a[0] - b[0] || a[1] - b[1] || a[2].localeCompare(b[2]);

/**
 * Pure keep/delete decision. Objects are content-addressed and shared by every version and platform,
 * so an object goes only when NO kept manifest on ANY platform references it.
 * @param {{platforms: Record<string, {latest: string, manifests: Record<string, object>}>, objects: Set<string>|string[], keep: number}} input
 *   manifests keyed by manifest file name (e.g. "r2026.json"), latest = the name latest.json points at.
 * @returns {{keepManifests: string[], dropManifests: string[], keepObjects: Set<string>, dropObjects: string[], missing: string[]}}
 */
function planPrune({ platforms, objects, keep }) {
  const have = new Set(objects);
  const keepManifests = new Set();
  const dropManifests = [];
  for (const [plat, { latest, manifests }] of Object.entries(platforms)) {
    const names = Object.keys(manifests).sort((a, b) => cmpOrder(manifestOrder(manifests[a], a), manifestOrder(manifests[b], b)));
    const kept = new Set(names.slice(-Math.max(1, keep)));
    if (latest) kept.add(latest);
    for (const n of names) {
      const key = `stable/${plat}/manifests/${n}`;
      if (kept.has(n)) keepManifests.add(key); else dropManifests.push(key);
    }
  }
  const keepObjects = new Set();
  for (const [plat, { manifests }] of Object.entries(platforms)) {
    for (const [n, m] of Object.entries(manifests)) {
      if (!keepManifests.has(`stable/${plat}/manifests/${n}`)) continue;
      for (const f of Object.values(m.files ?? {})) keepObjects.add(`objects/${f.sha256}.gz`);
    }
  }
  return {
    keepManifests: [...keepManifests].sort(),
    dropManifests,
    keepObjects,
    dropObjects: [...have].filter((k) => !keepObjects.has(k)),
    missing: [...keepObjects].filter((k) => !have.has(k)),
  };
}

async function listAll(prefix) {
  const out = [];
  let cursor = '';
  for (;;) {
    const url = `https://api.cloudflare.com/client/v4/accounts/${ACCOUNT}/r2/buckets/${BUCKET}/objects?prefix=${encodeURIComponent(prefix)}&per_page=1000${cursor ? `&cursor=${encodeURIComponent(cursor)}` : ''}`;
    let body;
    for (let attempt = 1; ; attempt++) {
      await gate();
      const res = await put(url, 'GET', { Authorization: `Bearer ${auth.token}` });
      if (res.ok) { body = JSON.parse(await res.text()); break; }
      if (res.status === 401 && attempt === 1 && !process.env.CLOUDFLARE_API_TOKEN) { refreshToken(auth.token); continue; }
      if ((res.status === 429 || res.status >= 500) && attempt < 8) { await new Promise((r) => setTimeout(r, attempt * 10_000)); continue; }
      throw new Error(`list ${prefix}: ${res.status} ${await res.text()}`);
    }
    for (const o of body.result ?? []) out.push({ key: o.key, size: Number(o.size ?? 0) });
    cursor = body.result_info?.cursor ?? '';
    if (!body.result_info?.is_truncated || !cursor) return out;
  }
}

async function prune(confirm, keep) {
  auth.token = cloudflareToken();
  pace.on = true;
  const stable = await listAll('stable/');
  const objs = await listAll('objects/');
  const sizes = new Map(objs.map((o) => [o.key, o.size]));
  const stableKeys = new Set(stable.map((o) => o.key));
  const plats = [...new Set(stable.map((o) => o.key.split('/')[1]))].sort();
  const platforms = {};
  for (const plat of plats) {
    const latestKey = `stable/${plat}/latest.json`;
    if (!stableKeys.has(latestKey)) { console.log(`skip ${plat}: no latest.json`); continue; }
    const latest = JSON.parse(await (await r2('GET', latestKey)).text());
    const manifests = {};
    for (const o of stable) {
      const m = o.key.match(new RegExp(`^stable/${plat}/manifests/([^/]+\\.json)$`));
      if (m) manifests[m[1]] = JSON.parse(await (await r2('GET', o.key)).text());
    }
    platforms[plat] = { latest: String(latest.manifest).replace(/^manifests\//, ''), manifests };
  }
  const plan = planPrune({ platforms, objects: sizes.keys(), keep });
  const dropManifestFiles = plan.dropManifests.flatMap((k) => [k, `${k}.sig`].filter((x) => stableKeys.has(x)));
  const gb = (n) => `${(n / 1e9).toFixed(2)} GB`;
  for (const [plat, { latest, manifests }] of Object.entries(platforms)) {
    const kept = plan.keepManifests.filter((k) => k.startsWith(`stable/${plat}/`)).map((k) => k.split('/').pop());
    console.log(`${plat}: ${Object.keys(manifests).length} manifests, latest -> ${latest}, keep ${kept.length}: ${kept.join(', ')}`);
  }
  const dropBytes = plan.dropObjects.reduce((n, k) => n + sizes.get(k), 0);
  console.log(`objects: ${sizes.size} (${gb([...sizes.values()].reduce((a, b) => a + b, 0))}); referenced by kept manifests: ${plan.keepObjects.size}; missing: ${plan.missing.length}`);
  console.log(`delete: ${plan.dropObjects.length} objects (${gb(dropBytes)}) + ${dropManifestFiles.length} old manifest files`);
  if (plan.missing.length) die(`refusing: kept manifests reference ${plan.missing.length} missing object(s), e.g. ${plan.missing[0]}; nothing deleted`);
  if (!confirm) { console.log('Dry run. Re-run with --confirm to delete.'); return; }
  const queue = [...plan.dropObjects, ...dropManifestFiles];
  let done = 0;
  const worker = async () => {
    for (let k = queue.shift(); k; k = queue.shift()) {
      await r2('DELETE', k);
      if (++done % 1000 === 0) console.log(`  deleted ${done}`);
    }
  };
  await Promise.all(Array.from({ length: 6 }, worker));
  console.log(`✓ deleted ${done} files`);
}

module.exports = { planPrune, manifestOrder };

if (require.main === module) {
  const [cmd, ...rest] = process.argv.slice(2);
  if (cmd === 'stamp') stamp();
  else if (cmd === 'pack') pack();
  else if (cmd === 'publish') publish(rest.includes('--confirm')).catch((e) => die(e.message));
  else if (cmd === 'prune') {
    const i = rest.indexOf('--keep');
    const keep = i >= 0 ? Number(rest[i + 1]) : 3;
    if (!Number.isInteger(keep) || keep < 1) die('--keep needs a whole number >= 1');
    prune(rest.includes('--confirm'), keep).catch((e) => die(e.message));
  } else die('usage: runtime-package.cjs stamp | pack | publish [--confirm] | prune [--keep N] [--confirm]');
}
