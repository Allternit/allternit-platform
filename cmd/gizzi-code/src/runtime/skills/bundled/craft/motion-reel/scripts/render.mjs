// usage (run from the project dir):
//   node render.mjs stills 0.5,2.2,4.1          → review/still_<t>.png   (look at these before a full render)
//   node render.mjs full [--fps 120] [--out frames] [--query 'k=v&k2=v2']  → <out>/f_00000.png ... (120 → 2-frame motion blur → 60fps)
import fs from 'fs';
import { findChrome } from './find_chrome.mjs';
let chromium;
try { ({ chromium } = await import('playwright-core')); }
catch { console.error('playwright-core missing. Run: npm i playwright-core@1   (in this project dir)'); process.exit(1); }
const args = process.argv.slice(2), mode = args[0];
const opt = (k, d) => { const i = args.indexOf('--' + k); return i >= 0 ? args[i + 1] : d; };
const CHROME = findChrome();
const browser = await chromium.launch({ executablePath: CHROME, args: ['--allow-file-access-from-files'] });
const q = opt('query', ''), outDir = opt('out', 'frames');   // --query 'theme=light&w=1080&h=1920&alpha=1' → read by scenes.js
const url = new URL('index.html', 'file://' + process.cwd() + '/').href + (q ? '?' + q : '') + '#render';
let errors = 0;
async function page() {
  const p = await browser.newPage({ viewport: { width: 1920, height: 1080 } });
  p.on('pageerror', e => { errors++; console.log('PAGE ERROR', e.message); });
  p.on('console', m => { if (m.type() === 'error') { errors++; console.log('CONSOLE ERROR', m.text()); } });
  await p.goto(url); await p.evaluate(() => window.ready); return p;
}
async function grab(p, t, file) {
  const data = await p.evaluate(t => { renderFrame(t); return document.getElementById('c').toDataURL('image/png'); }, t);
  fs.writeFileSync(file, Buffer.from(data.split(',')[1], 'base64'));
}
if (mode === 'stills') {
  fs.mkdirSync('review', { recursive: true });
  const p = await page();
  for (const t of args[1].split(',').map(Number)) await grab(p, t, `review/still_${t.toFixed(2).padStart(5, '0')}.png`);
  console.log('stills → review/');
} else if (mode === 'full') {
  const fps = +opt('fps', 120), workers = +opt('workers', 6);
  const dur = await (await page()).evaluate(() => window.REEL.duration);
  const n = Math.round(dur * fps); fs.rmSync(outDir, { recursive: true, force: true }); fs.mkdirSync(outDir);
  const t0 = Date.now();
  await Promise.all(Array.from({ length: workers }, async (_, w) => { const p = await page();
    for (let i = w; i < n; i += workers) await grab(p, i / fps, `${outDir}/f_${String(i).padStart(5, '0')}.png`); }));
  fs.writeFileSync(`${outDir}/fps.txt`, String(fps));
  console.log(`rendered ${n} frames @${fps}fps in ${(Date.now() - t0) / 1000}s`);
} else { console.log('mode: stills <t,t,...> | full [--fps 120]'); }
await browser.close();
if (errors) { console.log(`${errors} page errors — fix before trusting output`); process.exit(1); }
