// Capture a UI transition as a frame strip so you can SEE the motion instead of guessing.
//
// usage:
//   node strip.mjs --url http://localhost:5173 --act "click:#open" [--ms 600] [--step 33]
//                  [--w 1280 --h 800] [--clip "#panel"] [--reduced] [--dark] [--out strip]
//
// --act is a ;-separated list run before the clock starts moving:
//   click:<sel>  hover:<sel>  focus:<sel>  press:<key>  type:<sel>=<text>  wait:<ms>  eval:<js>
// Prefix an action with @<ms>: to fire it DURING capture, e.g. "click:#open;@90:press:Escape".
// Time is frozen first (page.clock fakes rAF/Date/performance, so JS springs like
// framer-motion/motion step deterministically) and every CSS/WAAPI animation is seeked
// to the same elapsed time, so frame N is the same on every run.
// Output: <out>/f_000.png … plus <out>/sheet.png (tiled, frame time printed on each).
import fs from 'fs';
import path from 'path';
import { execSync } from 'child_process';
import { createRequire } from 'module';
import { fileURLToPath, pathToFileURL } from 'url';

// playwright-core: use the project's / global copy, else install it next to this script once.
const HERE = path.dirname(fileURLToPath(import.meta.url));
async function loadPlaywright() {
  try { return await import('playwright-core'); } catch {}
  const req = createRequire(path.join(HERE, 'package.json'));
  try { return await import(pathToFileURL(req.resolve('playwright-core')).href); } catch {}
  try {
    console.error('installing playwright-core@1 into ' + HERE + ' (first run only)…');
    if (!fs.existsSync(path.join(HERE, 'package.json'))) fs.writeFileSync(path.join(HERE, 'package.json'), '{"private":true,"type":"module"}\n');
    execSync('npm i playwright-core@1 --silent --no-fund --no-audit', { cwd: HERE, stdio: 'inherit' });
    return await import(pathToFileURL(createRequire(path.join(HERE, 'package.json')).resolve('playwright-core')).href);
  } catch {
    console.error(`playwright-core missing. Run: npm i --prefix "${HERE}" playwright-core@1`); process.exit(1);
  }
}
function findChrome() {
  if (process.env.CHROME) return process.env.CHROME;
  for (const f of ['/Applications/Google Chrome.app/Contents/MacOS/Google Chrome', '/Applications/Chromium.app/Contents/MacOS/Chromium',
    `${process.env.HOME}/Applications/Google Chrome.app/Contents/MacOS/Google Chrome`, 'C:\\Program Files\\Google\\Chrome\\Application\\chrome.exe'])
    if (fs.existsSync(f)) return f;
  for (const b of ['google-chrome', 'google-chrome-stable', 'chromium', 'chromium-browser']) {
    try { const p = execSync(`command -v ${b}`, { stdio: ['ignore', 'pipe', 'ignore'] }).toString().trim(); if (p) return p; } catch {}
  }
  console.error('Chrome/Chromium not found. Install Google Chrome or set CHROME=/path/to/chrome'); process.exit(1);
}

const a = process.argv.slice(2);
const opt = (k, d) => { const i = a.indexOf('--' + k); return i >= 0 ? a[i + 1] : d; };
const flag = k => a.includes('--' + k);
let url = opt('url'); const acts = (opt('act', '') || '').split(';').filter(Boolean);
const ms = +opt('ms', 600), step = +opt('step', 33), out = opt('out', 'strip');
const W = +opt('w', 1280), H = +opt('h', 800), clipSel = opt('clip');
if (!url) { console.error('need --url'); process.exit(1); }
if (!/^[a-z]+:/i.test(url)) url = pathToFileURL(path.resolve(url)).href;   // local file path

const pw = await loadPlaywright(), { chromium } = pw.chromium ? pw : pw.default;
const CHROME = findChrome();
const browser = await chromium.launch({ executablePath: CHROME });
const ctx = await browser.newContext({
  viewport: { width: W, height: H }, deviceScaleFactor: 1,
  reducedMotion: flag('reduced') ? 'reduce' : 'no-preference',
  colorScheme: flag('dark') ? 'dark' : 'light',
});
const p = await ctx.newPage();
let errors = 0;
p.on('pageerror', e => { errors++; console.log('PAGE ERROR', e.message); });
await p.clock.install();
await p.goto(url, { waitUntil: 'networkidle' });
await p.clock.runFor(1500); // let mount animations finish before the action

// Seek every CSS/WAAPI animation to its own elapsed time (first seen = its start).
const seek = e => p.evaluate(e => {
  for (const an of document.getAnimations()) {
    if (an.__s === undefined) an.__s = e;
    an.pause(); an.currentTime = e - an.__s;
  }
}, e);

// "@<ms>:<act>" runs during capture at that time (the interrupt test); the rest run before.
const timed = [], runAct = async act => {
  const i = act.indexOf(':'), kind = act.slice(0, i), arg = act.slice(i + 1);
  if (kind === 'wait') return p.clock.runFor(+arg);
  if (kind === 'eval') return p.evaluate(arg);
  if (kind === 'press') return p.keyboard.press(arg);
  if (kind === 'type') { const [s, t] = arg.split('='); return p.fill(s, t); }
  return p.locator(arg).first()[kind]({ noWaitAfter: true, timeout: 5000 });
};
for (const act of acts) {
  const m = act.match(/^@(\d+):(.*)$/);
  if (m) timed.push({ at: +m[1], act: m[2] }); else await runAct(act);
}

fs.rmSync(out, { recursive: true, force: true }); fs.mkdirSync(out, { recursive: true });
const clip = clipSel ? await p.locator(clipSel).first().boundingBox() : null;
let n = 0;
for (let t = 0; t <= ms; t += step, n++) {
  for (const x of timed) if (!x.done && t >= x.at) { x.done = true; await runAct(x.act); }
  await seek(t);
  await p.screenshot({ path: `${out}/f_${String(n).padStart(3, '0')}.png`, ...(clip ? { clip } : {}), animations: 'allow' });
  await p.clock.runFor(step);
}
// Contact sheet: tile the frames with their time stamped on each, rendered by the same browser.
const cols = Math.min(6, n), cellW = 360;
const cells = Array.from({ length: n }, (_, k) => {
  const src = 'data:image/png;base64,' + fs.readFileSync(`${out}/f_${String(k).padStart(3, '0')}.png`).toString('base64');
  return `<figure><img src="${src}"><figcaption>${k * step} ms</figcaption></figure>`;
}).join('');
const sp = await browser.newPage({ viewport: { width: cols * (cellW + 6) + 6, height: 100 } });
await sp.setContent(`<style>body{margin:0;padding:6px;background:#777;display:grid;gap:6px;grid-template-columns:repeat(${cols},${cellW}px);font:14px system-ui}
figure{margin:0;position:relative}img{width:100%;display:block}figcaption{position:absolute;left:4px;top:4px;background:#000a;color:#fff;padding:1px 6px;border-radius:4px}</style>${cells}`);
await sp.screenshot({ path: `${out}/sheet.png`, fullPage: true });
await browser.close();
console.log(`${n} frames, ${step} ms apart → ${out}/sheet.png${errors ? `  (${errors} page errors)` : ''}`);
