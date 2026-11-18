#!/usr/bin/env node
// Converts iOS PhIcons (SVG) to Android VectorDrawables. Node 20, no dependencies.
//   node scripts/convert-icons.mjs            # write core/design/src/main/res/drawable/ph_<snake>.xml
//   node scripts/convert-icons.mjs --check    # exit 1 if any drawable differs from a fresh conversion
//
// Handles the subset the catalog uses: one <svg viewBox> holding <path d fill> elements. Anything
// else (transforms, strokes, gradients, other shapes, a different fill) is skipped and listed.
// DeckIcons are PDF only: they go through `pdftocairo -svg` (poppler, optional) and then convertDeckSvg,
// which handles coloured fill/stroke paths, fill/stroke opacity, no-op full-viewBox clips and the
// constant-alpha mask pattern pdftocairo emits for 70%-opacity groups. Gradients, glyph text and real
// clips are listed as skipped rather than converted wrongly. Without pdftocairo they are skipped.
import fs from 'node:fs';
import os from 'node:os';
import { spawnSync } from 'node:child_process';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const here = path.dirname(fileURLToPath(import.meta.url));
const android = path.resolve(here, '..');
const xcassets = path.resolve(android, '../ios/Assets.xcassets');
const outDir = path.join(android, 'core/design/src/main/res/drawable');
const check = process.argv.includes('--check');

const snake = (s) => s.replace(/[^A-Za-z0-9]+/g, '_').replace(/^_|_$/g, '').toLowerCase();
const converted = [], skipped = [], stale = [];

function convertSvg(svg) {
  const vb = svg.match(/viewBox="\s*([\d.]+)[ ,]+([\d.]+)[ ,]+([\d.]+)[ ,]+([\d.]+)\s*"/);
  if (!vb || vb[1] !== '0' || vb[2] !== '0') return { why: 'no 0-origin viewBox' };
  const body = svg.replace(/<\?xml[^>]*>|<!--[\s\S]*?-->/g, '');
  const tags = [...body.matchAll(/<([a-zA-Z]+)\b/g)].map((m) => m[1]);
  const other = tags.filter((t) => t !== 'svg' && t !== 'path');
  if (other.length) return { why: `unsupported element <${other[0]}>` };
  if (/\b(transform|stroke|opacity|style|clip-path|mask|fill-rule\s*=\s*"evenodd"[^>]*stroke)\b/.test(body.replace(/fill-rule/g, ''))) return { why: 'transform/stroke/opacity/style' };
  if (/url\(/.test(body)) return { why: 'gradient or reference' };
  const paths = [...body.matchAll(/<path\b([^>]*?)\/?>/g)].map((m) => m[1]);
  if (paths.length === 0) return { why: 'no path' };
  const out = [];
  for (const attrs of paths) {
    const d = attrs.match(/\bd="([^"]+)"/);
    if (!d) return { why: 'path without d' };
    const fill = attrs.match(/\bfill="([^"]*)"/)?.[1] ?? '#000000';
    if (!/^#000(000)?$|^black$/i.test(fill)) return { why: `fill ${fill} (icons are template-tinted black)` };
    const evenOdd = /fill-rule="evenodd"/.test(attrs) ? '\n        android:fillType="evenOdd"' : '';
    out.push(`    <path\n        android:fillColor="#FF000000"${evenOdd}\n        android:pathData="${d[1].replace(/\s+/g, ' ').trim()}" />`);
  }
  const w = vb[3], h = vb[4];
  return {
    xml: `<?xml version="1.0" encoding="utf-8"?>\n<!-- Converted from iOS Assets.xcassets by scripts/convert-icons.mjs; do not edit. -->\n` +
      `<vector xmlns:android="http://schemas.android.com/apk/res/android"\n    android:width="24dp"\n    android:height="24dp"\n    android:viewportWidth="${w}"\n    android:viewportHeight="${h}">\n${out.join('\n')}\n</vector>\n`,
  };
}

const hasCairo = spawnSync('pdftocairo', ['-v']).error === undefined;
const hex2 = (n) => Math.max(0, Math.min(255, Math.round(n))).toString(16).padStart(2, '0').toUpperCase();
const color = (v) => {
  const m = v.match(/^rgb\(\s*([\d.]+)%,\s*([\d.]+)%,\s*([\d.]+)%\)$/);
  return m ? '#FF' + [m[1], m[2], m[3]].map((x) => hex2(parseFloat(x) * 2.55)).join('') : null;
};
const num = (x) => String(Math.round(x * 1e4) / 1e4);
const attr = (a, n) => a.match(new RegExp(`(?:^|\\s)${n}="([^"]*)"`))?.[1];

function pathXml(a, extraAlpha) {
  const d = attr(a, 'd');
  if (!d) return { why: 'path without d' };
  const lines = [];
  const fill = attr(a, 'fill');
  if (fill && fill !== 'none') {
    const c = color(fill);
    if (!c) return { why: `fill ${fill}` };
    lines.push(`android:fillColor="${c}"`);
    const fa = parseFloat(attr(a, 'fill-opacity') ?? '1') * extraAlpha;
    if (fa < 0.9999) lines.push(`android:fillAlpha="${num(fa)}"`);
    if (attr(a, 'fill-rule') === 'evenodd') lines.push('android:fillType="evenOdd"');
  }
  const stroke = attr(a, 'stroke');
  if (stroke && stroke !== 'none') {
    const c = color(stroke);
    if (!c) return { why: `stroke ${stroke}` };
    lines.push(`android:strokeColor="${c}"`, `android:strokeWidth="${attr(a, 'stroke-width') ?? 1}"`);
    const sa = parseFloat(attr(a, 'stroke-opacity') ?? '1') * extraAlpha;
    if (sa < 0.9999) lines.push(`android:strokeAlpha="${num(sa)}"`);
    const cap = attr(a, 'stroke-linecap'), join = attr(a, 'stroke-linejoin'), ml = attr(a, 'stroke-miterlimit');
    if (cap && cap !== 'butt') lines.push(`android:strokeLineCap="${cap}"`);
    if (join && join !== 'miter') lines.push(`android:strokeLineJoin="${join}"`);
    if (ml && ml !== '4') lines.push(`android:strokeMiterLimit="${ml}"`);
  }
  if (!lines.length) return { why: 'path with no fill or stroke' };
  lines.push(`android:pathData="${d.replace(/\s+/g, ' ').trim()}"`);
  return { xml: `    <path\n        ${lines.join('\n        ')} />` };
}

function convertDeckSvg(svg) {
  const vb = svg.match(/viewBox="\s*0[ ,]+0[ ,]+([\d.]+)[ ,]+([\d.]+)\s*"/);
  if (!vb) return { why: 'no 0-origin viewBox' };
  const body = svg.replace(/<\?xml[^>]*>|<!--[\s\S]*?-->/g, '');
  if (/linearGradient|radialGradient/.test(body)) return { why: 'gradient fill (not converted)' };
  if (/id="glyph-/.test(body)) return { why: 'text glyphs (not converted)' };
  const defs = body.match(/<defs>([\s\S]*?)<\/defs>/)?.[1] ?? '';
  const main = body.replace(/<defs>[\s\S]*?<\/defs>/, '');
  // clips must be a no-op rect covering the whole viewBox
  for (const c of defs.matchAll(/<clipPath\b[^>]*>([\s\S]*?)<\/clipPath>/g)) {
    const r = c[1].match(/<rect\b([^>]*)\/>/);
    if (!r || +attr(r[1], 'x') > 0 || +attr(r[1], 'y') > 0 || +attr(r[1], 'width') < +vb[1] || +attr(r[1], 'height') < +vb[2]) return { why: 'real clip path (not converted)' };
  }
  const masks = {};
  for (const m of defs.matchAll(/<mask id="([^"]+)">([\s\S]*?)<\/mask>/g)) {
    const r = m[2].match(/<rect\b([^>]*)\/>/);
    if (!r || !/filter-remove-color/.test(m[2]) || !/^rgb\(0%, 0%, 0%\)$/.test(attr(r[1], 'fill') ?? '')) return { why: 'non-constant mask (not converted)' };
    masks[m[1]] = parseFloat(attr(r[1], 'fill-opacity') ?? '1');
  }
  const sources = {};
  for (const g of defs.matchAll(/<g id="(source-[^"]+)"[^>]*>([\s\S]*?)<\/g>/g)) sources[g[1]] = [...g[2].matchAll(/<path\b([^>]*?)\/?>/g)].map((x) => x[1]);
  const out = [];
  const add = (a, alpha) => { const r = pathXml(a, alpha); if (r.xml) out.push(r.xml); return r.why; };
  // walk top-level elements in order
  for (const el of main.matchAll(/<path\b([^>]*?)\/>|<g\b([^>]*)>\s*<use xlink:href="#([^"]+)"\/>\s*<\/g>/g)) {
    if (el[1] !== undefined) { const w = add(el[1], 1); if (w) return { why: w }; continue; }
    const mk = attr(el[2], 'mask')?.match(/url\(#([^)]+)\)/)?.[1];
    if (!mk || !(mk in masks) || !sources[el[3]] || /transform/.test(el[2])) return { why: 'unsupported group/use (not converted)' };
    for (const a of sources[el[3]]) { const w = add(a, masks[mk]); if (w) return { why: w }; }
  }
  const left = main.replace(/<svg\b[^>]*>|<\/svg>|<path\b[^>]*?\/>|<g\b[^>]*>\s*<use [^>]*\/>\s*<\/g>/g, '').trim();
  if (left || /transform/.test(main)) return { why: 'unsupported element or transform (not converted)' };
  if (!out.length) return { why: 'no path' };
  return { xml: `<?xml version="1.0" encoding="utf-8"?>\n<!-- Converted from iOS Assets.xcassets by scripts/convert-icons.mjs; do not edit. -->\n` +
    `<vector xmlns:android="http://schemas.android.com/apk/res/android"\n    android:width="${vb[1]}dp"\n    android:height="${vb[2]}dp"\n    android:viewportWidth="${vb[1]}"\n    android:viewportHeight="${vb[2]}">\n${out.join('\n')}\n</vector>\n` };
}

function pdfToSvg(pdfPath) {
  const tmp = fs.mkdtempSync(path.join(os.tmpdir(), 'deckicon-'));
  const out = path.join(tmp, 'i.svg');
  const r = spawnSync('pdftocairo', ['-svg', pdfPath, out]);
  const svg = r.status === 0 && fs.existsSync(out) ? fs.readFileSync(out, 'utf8') : null;
  fs.rmSync(tmp, { recursive: true, force: true });
  return svg;
}

fs.mkdirSync(outDir, { recursive: true });
for (const [dirName, prefix] of [['PhIcons', 'ph'], ['DeckIcons', 'deck']]) {
  const dir = path.join(xcassets, dirName);
  for (const set of fs.readdirSync(dir).filter((f) => f.endsWith('.imageset')).sort()) {
    const base = set.replace('.imageset', '').replace(new RegExp(`^${prefix}-`), '');
    const name = `${prefix}_${snake(base)}`;
    const files = fs.readdirSync(path.join(dir, set)).filter((f) => f !== 'Contents.json');
    const svg = files.find((f) => f.endsWith('.svg'));
    const pdf = files.find((f) => f.endsWith('.pdf'));
    let r;
    if (svg) r = convertSvg(fs.readFileSync(path.join(dir, set, svg), 'utf8'));
    else if (pdf) {
      if (!hasCairo) { skipped.push(`${dirName}/${set}: PDF only and pdftocairo (poppler) is not installed: brew install poppler`); continue; }
      const text = pdfToSvg(path.join(dir, set, pdf));
      r = text ? convertDeckSvg(text) : { why: 'pdftocairo failed' };
    } else { skipped.push(`${dirName}/${set}: ${files.map((f) => path.extname(f)).join(',') || 'empty'} only (no SVG or PDF)`); continue; }
    if (!r.xml) { skipped.push(`${dirName}/${set}: ${r.why}`); continue; }
    const file = path.join(outDir, `${name}.xml`);
    if (check) {
      if (!fs.existsSync(file) || fs.readFileSync(file, 'utf8') !== r.xml) stale.push(name);
    } else fs.writeFileSync(file, r.xml);
    converted.push(name);
  }
}
const byDir = (p) => skipped.filter((s) => s.startsWith(p)).length;
console.log(`convert-icons: converted ${converted.length}, skipped ${skipped.length} (PhIcons ${byDir('PhIcons')}, DeckIcons ${byDir('DeckIcons')})`);
for (const k of skipped) console.log(`  skipped ${k}`);
if (check && stale.length) { console.error(`convert-icons: stale: ${stale.join(', ')}`); process.exit(1); }
