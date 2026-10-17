#!/usr/bin/env node
/**
 * Re-checks every store id in src/main/phone-app-catalog.json against the
 * live stores: the App Store lookup API (US) for iPhone bundle ids, Google
 * Play for Android packages. Run it after editing the catalog:
 *
 *   node scripts/verify-phone-app-catalog.mjs [--only id,id]
 *
 * Exits 1 when an id is not found. Entries with a `note` explain known
 * exceptions and are reported, not failed. Apple rate-limits bursts, so it
 * waits between lookups and retries on 403/429.
 */
import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import path from 'node:path';

const here = path.dirname(fileURLToPath(import.meta.url));
const { apps } = JSON.parse(readFileSync(path.join(here, '../src/main/phone-app-catalog.json'), 'utf8'));
const only = process.argv.includes('--only') ? new Set(process.argv[process.argv.indexOf('--only') + 1].split(',')) : null;
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

async function status(url, tries = 4) {
  for (let i = 0; i < tries; i++) {
    const res = await fetch(url, { headers: { 'user-agent': 'Mozilla/5.0' } }).catch(() => null);
    if (res && res.status !== 403 && res.status !== 429) return res;
    await sleep(8000);
  }
  return null;
}

let failed = 0;
for (const app of apps) {
  if (only && !only.has(app.id)) continue;
  const missing = [];
  for (const bundle of app.ios) {
    const res = await status(`https://itunes.apple.com/lookup?country=us&bundleId=${encodeURIComponent(bundle)}`);
    const found = res && res.ok && (await res.json()).resultCount > 0;
    if (!found) missing.push(`ios ${bundle}`);
    await sleep(1500);
  }
  for (const pkg of app.android) {
    const res = await status(`https://play.google.com/store/apps/details?id=${encodeURIComponent(pkg)}&hl=en&gl=US`);
    if (!res || res.status !== 200) missing.push(`android ${pkg}`);
  }
  if (missing.length === 0) console.log(`ok    ${app.id}`);
  else if (app.note) console.log(`note  ${app.id}: ${missing.join(', ')} — ${app.note}`);
  else {
    failed++;
    console.log(`FAIL  ${app.id}: ${missing.join(', ')}`);
  }
}
process.exit(failed ? 1 : 0);
