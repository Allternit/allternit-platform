#!/usr/bin/env node
// Render a viewer.html headless: a seed grid to look at, or a hi-res still.
//
// usage:
//   node render.mjs --viewer art/viewer.html --grid 3 --seed 1 --out grid.png [--size 1500]
//   node render.mjs --viewer art/viewer.html --still --seed 42 --size 4000 --out seed-42.png
//   node render.mjs --viewer art/viewer.html --check --seed 42      # same seed twice -> identical?
//   extra: --palette <i>   --p key=value (repeatable, sets a PARAMS slider)
//
// Uses $CHROME or a local Chrome/Chromium/Edge; installs playwright-core into
// $SHOT_CACHE or ~/.cache/gizzi-shot on first run. Prints page errors.
import { createRequire } from 'node:module';
import { execSync, execFileSync } from 'node:child_process';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { pathToFileURL } from 'node:url';

const PW_VERSION = '1.64.1';
const a = process.argv.slice(2);
const opt = (k, d) => { const i = a.indexOf('--' + k); return i >= 0 ? a[i + 1] : d; };
const all = k => a.flatMap((v, i) => (v === '--' + k ? [a[i + 1]] : []));
const flag = k => a.includes('--' + k);

const viewer = opt('viewer');
if (!viewer) { console.error('need --viewer <path to viewer.html>'); process.exit(1); }
const seed = opt('seed', '1'), out = opt('out', flag('still') ? `seed-${seed}.png` : 'grid.png');

async function loadPlaywright() {
  try { return await import('playwright-core'); } catch {}
  const dir = process.env.SHOT_CACHE || path.join(os.homedir(), '.cache', 'gizzi-shot');
  const req = createRequire(path.join(dir, 'package.json'));
  try { return req('playwright-core'); } catch {}
  console.error(`[render] first run: installing playwright-core@${PW_VERSION} into ${dir}`);
  fs.mkdirSync(dir, { recursive: true });
  if (!fs.existsSync(path.join(dir, 'package.json'))) fs.writeFileSync(path.join(dir, 'package.json'), '{"private":true}');
  execSync(`npm install --no-audit --no-fund --silent playwright-core@${PW_VERSION}`, { cwd: dir, stdio: 'inherit' });
  return req('playwright-core');
}
function findChrome() {
  if (process.env.CHROME) return process.env.CHROME;
  for (const p of [
    '/Applications/Google Chrome.app/Contents/MacOS/Google Chrome',
    '/Applications/Chromium.app/Contents/MacOS/Chromium',
    '/Applications/Microsoft Edge.app/Contents/MacOS/Microsoft Edge',
    path.join(os.homedir(), 'Applications/Google Chrome.app/Contents/MacOS/Google Chrome'),
    'C:\\Program Files\\Google\\Chrome\\Application\\chrome.exe',
  ]) if (fs.existsSync(p)) return p;
  for (const bin of ['google-chrome', 'google-chrome-stable', 'chromium', 'chromium-browser']) {
    try { const p = execFileSync('which', [bin], { encoding: 'utf8' }).trim(); if (p) return p; } catch {}
  }
}

const pw = await loadPlaywright();
const chromium = pw.chromium ?? pw.default.chromium;
let browser;
try { browser = await chromium.launch({ executablePath: findChrome() }); }
catch (e) { console.error('[render] no browser: set CHROME=/path/to/chrome or run: npx playwright-core install chromium'); process.exit(2); }

const hash = new URLSearchParams({ seed, ui: '0' });
if (opt('palette')) hash.set('palette', opt('palette'));
for (const kv of all('p')) { const [k, v] = kv.split('='); hash.set('p.' + k, v); }
const grid = +opt('grid', flag('still') || flag('check') ? 0 : 3);
const size = +opt('size', flag('still') ? 4000 : 1500);
if (grid) { hash.set('grid', grid); hash.set('size', size); }

const page = await browser.newPage({ viewport: { width: grid ? size : 1000, height: grid ? size : 1000 } });
let errors = 0;
page.on('pageerror', e => { errors++; console.log('PAGE ERROR', e.message); });
page.on('console', m => { if (m.type() === 'error') { errors++; console.log('CONSOLE', m.text()); } });
await page.goto(pathToFileURL(path.resolve(viewer)).href + '#' + hash);
await page.waitForFunction(() => window.ART && window.ART.ready, null, { timeout: 120000 });
fs.mkdirSync(path.dirname(path.resolve(out)), { recursive: true });

const t0 = Date.now();
if (flag('check')) {
  const [x, y] = await page.evaluate(s => [window.renderStill(s, 600), window.renderStill(s, 600)], seed);
  const z = await page.evaluate(s => window.renderStill(String(Number(s) + 1), 600), seed);
  console.log(x === y ? `[render] deterministic: seed ${seed} renders identically twice` : `[render] NOT DETERMINISTIC: seed ${seed} differs between runs (Math.random? time? unseeded noise?)`);
  if (x === z) console.log('[render] WARNING: seed and seed+1 are identical; the seed is not reaching draw()');
  if (x !== y) process.exitCode = 3;
} else if (grid) {
  await page.locator('#stage').screenshot({ path: out });
} else {
  const url = await page.evaluate(([s, n]) => window.renderStill(s, n), [seed, size]);
  fs.writeFileSync(out, Buffer.from(url.split(',')[1], 'base64'));
}
await browser.close();
if (!flag('check')) console.log(`[render] wrote ${path.resolve(out)} in ${((Date.now() - t0) / 1000).toFixed(1)}s${errors ? `, ${errors} errors` : ''}`);
