#!/usr/bin/env node
// Fails if a PR changes fabric-session PWA assets without bumping the
// service worker CACHE_NAME. A stale CACHE_NAME strands installed PWAs on
// old cached assets (this blanked the PWA for a user after a deploy).
import { execSync } from 'node:child_process';

import { existsSync } from 'node:fs';

const SW_PATH = 'public/fabric-session-service-worker.js';
const WATCH_PATHS = [
  SW_PATH,
  'fabric-session.html',
  'public/fabric-session.webmanifest',
  'public/fabric-session-icon-192.png',
  'public/fabric-session-icon-512.png',
  'public/fabric-session-splash-1170x2532.png',
  'src/fabric-session/',
];

if (!existsSync(SW_PATH)) {
  console.log('Fabric Session SW lives in Allternit/allternit-ai; skipping this public-repo guard.');
  process.exit(0);
}

function sh(cmd) {
  return execSync(cmd, { encoding: 'utf8', stdio: ['ignore', 'pipe', 'pipe'] }).trim();
}

// The actions/checkout checkout has no local branch refs (only origin/*), so
// retry a ref that doesn't exist locally with an origin/ prefix.
function resolveRef(ref) {
  try {
    sh(`git rev-parse --verify --quiet ${ref}`);
    return ref;
  } catch {
    try {
      sh(`git rev-parse --verify --quiet origin/${ref}`);
      return `origin/${ref}`;
    } catch {
      return ref; // let the downstream git call fail with its own error
    }
  }
}

function cacheNameAt(ref) {
  try {
    const body = sh(`git show ${ref}:${SW_PATH}`);
    const m = body.match(/const\s+CACHE_NAME\s*=\s*['"]([^'"]+)['"]/);
    return m ? m[1] : null;
  } catch {
    return null; // file absent at that ref
  }
}

const args = process.argv.slice(2);
const base = resolveRef(args[args.indexOf('--base') + 1] || process.env.GITHUB_BASE_REF);
const head = resolveRef(args[args.indexOf('--head') + 1] || process.env.GITHUB_HEAD_REF || 'HEAD');
if (!base) {
  console.error('check-sw-cache-bump: no base ref (pass --base <ref> or set GITHUB_BASE_REF)');
  process.exit(2);
}

const changed = sh(`git diff --name-only ${base}...${head}`).split('\n').filter(Boolean);
const hit = changed.filter((p) => WATCH_PATHS.some((w) => (w.endsWith('/') ? p.startsWith(w) : p === w)));
if (hit.length === 0) {
  console.log(`check-sw-cache-bump: no fabric-session watch paths changed (${changed.length} files changed) — OK`);
  process.exit(0);
}

const baseName = cacheNameAt(base);
const headName = cacheNameAt(head);
if (baseName && headName && baseName !== headName) {
  console.log(`check-sw-cache-bump: CACHE_NAME bumped ${baseName} -> ${headName} — OK`);
  process.exit(0);
}
if (!baseName && headName) {
  console.log(`check-sw-cache-bump: service worker added with CACHE_NAME ${headName} — OK`);
  process.exit(0);
}
console.error(`check-sw-cache-bump: fabric-session assets changed without a CACHE_NAME bump:\n  ${hit.join('\n  ')}`);
console.error(`  CACHE_NAME at base (${base}): ${baseName ?? '<absent>'}`);
console.error(`  CACHE_NAME at head (${head}): ${headName ?? '<absent>'}`);
console.error('  Bump the CACHE_NAME constant in ' + SW_PATH + ' so installed PWAs discard stale caches.');
process.exit(1);
