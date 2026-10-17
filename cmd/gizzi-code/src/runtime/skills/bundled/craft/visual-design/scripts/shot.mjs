#!/usr/bin/env node
// Screenshot a page so you can look at it before calling the design done.
//
// usage:
//   node shot.mjs --url <url|file> --out <png> [--w 1440 --h 900] [--phone] [--dark]
//                 [--full] [--wait 800] [--sheet]
//
//   --phone   390x844 viewport, touch, 2x pixels (overrides --w/--h)
//   --dark    prefers-color-scheme: dark
//   --full    capture the full scroll height instead of one viewport
//   --wait    extra ms after load (fonts, entrance animation)
//   --sheet   one PNG with desktop (1440x900) and phone (390x844) side by side,
//             light on top, dark below when --dark is also given
//
// Also prints: page errors, failed requests, and horizontal overflow (page wider
// than the viewport), which is the most common phone-width bug.
//
// Browser: $CHROME, else a local Chrome/Chromium/Edge, else Playwright's own
// Chromium if installed. playwright-core installs itself into a cache dir on
// first use ($SHOT_CACHE or ~/.cache/gizzi-shot); nothing is written next to
// this script.
import { createRequire } from 'node:module';
import { execSync, execFileSync } from 'node:child_process';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { pathToFileURL } from 'node:url';

const PW_VERSION = '1.64.1';
const a = process.argv.slice(2);
const opt = (k, d) => { const i = a.indexOf('--' + k); return i >= 0 && a[i + 1] && !a[i + 1].startsWith('--') ? a[i + 1] : d; };
const flag = k => a.includes('--' + k);

let url = opt('url');
const out = opt('out', 'shot.png');
if (!url) { console.error('need --url <url|file>'); process.exit(1); }
if (!/^[a-z]+:\/\//i.test(url)) url = pathToFileURL(path.resolve(url)).href;

async function loadPlaywright() {
  try { return (await import('playwright-core')).default ?? (await import('playwright-core')); } catch {}
  const dir = process.env.SHOT_CACHE || path.join(os.homedir(), '.cache', 'gizzi-shot');
  const req = createRequire(path.join(dir, 'package.json'));
  try { return req('playwright-core'); } catch {}
  console.error(`[shot] first run: installing playwright-core@${PW_VERSION} into ${dir}`);
  fs.mkdirSync(dir, { recursive: true });
  if (!fs.existsSync(path.join(dir, 'package.json'))) fs.writeFileSync(path.join(dir, 'package.json'), '{"private":true}');
  execSync(`npm install --no-audit --no-fund --silent playwright-core@${PW_VERSION}`, { cwd: dir, stdio: 'inherit' });
  return req('playwright-core');
}

function findChrome() {
  if (process.env.CHROME) return process.env.CHROME;
  const mac = [
    '/Applications/Google Chrome.app/Contents/MacOS/Google Chrome',
    '/Applications/Chromium.app/Contents/MacOS/Chromium',
    '/Applications/Microsoft Edge.app/Contents/MacOS/Microsoft Edge',
    path.join(os.homedir(), 'Applications/Google Chrome.app/Contents/MacOS/Google Chrome'),
  ];
  const win = [
    'C:\\Program Files\\Google\\Chrome\\Application\\chrome.exe',
    'C:\\Program Files (x86)\\Google\\Chrome\\Application\\chrome.exe',
  ];
  for (const p of [...mac, ...win]) if (fs.existsSync(p)) return p;
  for (const bin of ['google-chrome', 'google-chrome-stable', 'chromium', 'chromium-browser', 'microsoft-edge']) {
    try { const p = execFileSync('which', [bin], { encoding: 'utf8' }).trim(); if (p) return p; } catch {}
  }
  return undefined; // fall back to Playwright's bundled Chromium, if installed
}

const { chromium } = await loadPlaywright();
const executablePath = findChrome();
let browser;
try { browser = await chromium.launch({ executablePath }); }
catch (e) {
  console.error('[shot] could not start a browser. Set CHROME=/path/to/chrome, or run: npx playwright-core install chromium');
  console.error(String(e.message || e).split('\n')[0]);
  process.exit(2);
}

const DESK = { width: +opt('w', 1440), height: +opt('h', 900) };
const PHONE = { width: 390, height: 844 };
const wait = +opt('wait', 600);

async function capture({ phone, dark, full }) {
  const ctx = await browser.newContext({
    viewport: phone ? PHONE : DESK,
    deviceScaleFactor: phone ? 2 : 1,
    isMobile: !!phone, hasTouch: !!phone,
    colorScheme: dark ? 'dark' : 'light',
  });
  const p = await ctx.newPage();
  const tag = `${phone ? 'phone' : 'desktop'}${dark ? '/dark' : ''}`;
  p.on('pageerror', e => console.log(`[${tag}] PAGE ERROR ${e.message}`));
  p.on('requestfailed', r => console.log(`[${tag}] FAILED ${r.url()} ${r.failure()?.errorText ?? ''}`));
  try { await p.goto(url, { waitUntil: 'networkidle', timeout: 30000 }); }
  catch { await p.goto(url, { waitUntil: 'load', timeout: 30000 }); }
  await p.evaluate(() => document.fonts && document.fonts.ready);
  await p.waitForTimeout(wait);
  const over = await p.evaluate(() => {
    const vw = document.documentElement.clientWidth, sw = document.documentElement.scrollWidth;
    if (sw <= vw + 1) return null;
    const wide = [...document.querySelectorAll('body *')].filter(el => el.getBoundingClientRect().right > vw + 1)
      .slice(0, 5).map(el => el.tagName.toLowerCase() + (el.id ? '#' + el.id : '') + (el.className && typeof el.className === 'string' ? '.' + el.className.trim().split(/\s+/)[0] : ''));
    return { vw, sw, wide };
  });
  if (over) console.log(`[${tag}] HORIZONTAL OVERFLOW: page ${over.sw}px > viewport ${over.vw}px; widest: ${over.wide.join(', ')}`);
  const buf = await p.screenshot({ fullPage: !!full });
  await ctx.close();
  return buf;
}

async function sheet(rows) {
  // Lay the captures out in a blank page and screenshot that: no image library needed.
  const ctx = await browser.newContext({ viewport: { width: 1200, height: 800 }, deviceScaleFactor: 1 });
  const p = await ctx.newPage();
  const cells = rows.map(r => `<div class="row">${r.map(c => `<figure><img src="data:image/png;base64,${c.buf.toString('base64')}" style="width:${c.w}px"><figcaption>${c.label}</figcaption></figure>`).join('')}</div>`).join('');
  await p.setContent(`<!doctype html><style>
    body{margin:0;padding:24px;background:#d9d9de;font:13px/1.3 ui-monospace,Menlo,monospace;color:#222;display:inline-block}
    .row{display:flex;gap:24px;align-items:flex-start;margin-bottom:24px}
    figure{margin:0} img{display:block;box-shadow:0 1px 4px rgba(0,0,0,.25)} figcaption{margin-top:6px}
  </style>${cells}`);
  await p.evaluate(() => Promise.all([...document.images].map(i => i.decode())));
  const el = await p.$('body');
  const buf = await el.screenshot();
  await ctx.close();
  return buf;
}

fs.mkdirSync(path.dirname(path.resolve(out)), { recursive: true });
const full = flag('full');
if (flag('sheet')) {
  const schemes = flag('dark') ? [false, true] : [false];
  const rows = [];
  for (const dark of schemes) {
    rows.push([
      { buf: await capture({ phone: false, dark, full }), w: Math.round(DESK.width * 0.6), label: `desktop ${DESK.width}x${DESK.height}${dark ? ' dark' : ''}` },
      { buf: await capture({ phone: true, dark, full }), w: PHONE.width, label: `phone ${PHONE.width}x${PHONE.height}${dark ? ' dark' : ''}` },
    ]);
  }
  fs.writeFileSync(out, await sheet(rows));
} else {
  fs.writeFileSync(out, await capture({ phone: flag('phone'), dark: flag('dark'), full }));
}
await browser.close();
console.log(`[shot] wrote ${path.resolve(out)}`);
