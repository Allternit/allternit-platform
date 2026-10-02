#!/usr/bin/env node
/**
 * Runtime packages: ship allternit-api, gizzi-code and the platform screens
 * without a new Desktop app build (see src/main/runtime-package.ts).
 *
 *   node scripts/runtime-package.cjs stamp     write resources/runtime.json (build number + bundled file list)
 *   node scripts/runtime-package.cjs pack      write the feed for resources/{bin,platform} → release/runtime/feed/
 *   node scripts/runtime-package.cjs publish   show what would upload; add --confirm to upload to R2
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
 *     npm run prepare:platform-static && npm run prepare:api-binary)
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
const { execFileSync } = require('node:child_process');
const https = require('node:https');

const DESKTOP_DIR = path.resolve(__dirname, '..');
const REPO_ROOT = path.resolve(DESKTOP_DIR, '..', '..');
// ALLTERNIT_RUNTIME_RESOURCES packs another build's staged resources (e.g. a build worktree).
const RESOURCES = process.env.ALLTERNIT_RUNTIME_RESOURCES ? path.resolve(process.env.ALLTERNIT_RUNTIME_RESOURCES) : path.join(DESKTOP_DIR, 'resources');
const STAMP = path.join(RESOURCES, 'runtime.json');
const PLATFORM = `${process.platform}-${process.arch}`;
const FEED = path.join(DESKTOP_DIR, 'release', 'runtime', 'feed');
const BUCKET = process.env.ALLTERNIT_RUNTIME_BUCKET || 'allternit-runtime';
const ACCOUNT = process.env.CLOUDFLARE_ACCOUNT_ID || '7cd19487307235aedc039d3a64ad7039';
const EXE = process.platform === 'win32' ? '.exe' : '';
const BINARIES = [`allternit-api${EXE}`, `gizzi-code${EXE}`];

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

/** The runtime's files under resources/, as manifest entries. Null when not built yet. */
function runtimeFiles() {
  if (!BINARIES.every((b) => fs.existsSync(path.join(RESOURCES, 'bin', b)))) return null;
  if (!fs.existsSync(path.join(RESOURCES, 'platform', 'index.html'))) return null;
  const rels = [...BINARIES.map((b) => `bin/${b}`), ...walk(path.join(RESOURCES, 'platform')).map((r) => `platform/${r}`)];
  const files = {};
  for (const rel of rels.sort()) {
    const buf = fs.readFileSync(path.join(RESOURCES, rel));
    files[rel] = { sha256: sha256(buf), size: buf.length };
  }
  return files;
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
  // The bundled file list lets a runtime update copy unchanged files from the app instead of downloading them.
  const files = runtimeFiles();
  if (!files) console.warn('⚠ runtime parts not staged yet — runtime.json has no file list (updates download every file)');
  const info = {
    version, build, shellApi: shellApi(),
    platformSha: git(REPO_ROOT, 'rev-parse', 'HEAD') || null,
    aiSha: aiDir ? git(aiDir, 'rev-parse', 'HEAD') || null : null,
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
  const info = fs.existsSync(STAMP) ? JSON.parse(fs.readFileSync(STAMP, 'utf8')) : stamp();
  const files = runtimeFiles();
  if (!files) die('resources/bin/{allternit-api,gizzi-code} or resources/platform missing — build them first');
  const key = signingKey();
  const objectsDir = path.join(FEED, 'objects');
  const platformDir = path.join(FEED, 'stable', PLATFORM);
  fs.mkdirSync(objectsDir, { recursive: true });
  fs.mkdirSync(path.join(platformDir, 'manifests'), { recursive: true });

  let written = 0;
  for (const [rel, f] of Object.entries(files)) {
    const obj = path.join(objectsDir, `${f.sha256}.gz`);
    if (fs.existsSync(obj)) continue;
    fs.writeFileSync(obj, zlib.gzipSync(fs.readFileSync(path.join(RESOURCES, rel)), { level: 9 }));
    written += 1;
  }
  const manifest = Buffer.from(JSON.stringify({
    version: info.version, build: info.build, platform: PLATFORM, shellApi: info.shellApi,
    platformSha: info.platformSha, aiSha: info.aiSha, files,
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

async function r2(method, key, body, contentType, cacheControl) {
  const url = `https://api.cloudflare.com/client/v4/accounts/${ACCOUNT}/r2/buckets/${BUCKET}/objects/${key}`;
  for (let attempt = 1, refreshed = false; ; attempt++) {
    const token = auth.token;
    let res;
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

async function publish(confirm) {
  const platformDir = path.join(FEED, 'stable', PLATFORM);
  const latestPath = path.join(platformDir, 'latest.json');
  if (!fs.existsSync(latestPath)) die('nothing packed — run pack first');
  const latest = JSON.parse(fs.readFileSync(latestPath, 'utf8'));
  const manifest = JSON.parse(fs.readFileSync(path.join(platformDir, latest.manifest), 'utf8'));
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
}

const [cmd, ...rest] = process.argv.slice(2);
if (cmd === 'stamp') stamp();
else if (cmd === 'pack') pack();
else if (cmd === 'publish') publish(rest.includes('--confirm')).catch((e) => die(e.message));
else die('usage: runtime-package.cjs stamp | pack | publish [--confirm]');
