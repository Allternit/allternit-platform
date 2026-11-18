#!/usr/bin/env node
// Converts iOS PhIcons (SVG) to Android VectorDrawables. Node 20, no dependencies.
//   node scripts/convert-icons.mjs            # write core/design/src/main/res/drawable/ph_<snake>.xml
//   node scripts/convert-icons.mjs --check    # exit 1 if any drawable differs from a fresh conversion
//
// Handles the subset the catalog uses: one <svg viewBox> holding <path d fill> elements. Anything
// else (transforms, strokes, gradients, other shapes, a different fill) is skipped and listed.
// DeckIcons are PDF only: no converter dependency is added, so they are listed as skipped.
import fs from 'node:fs';
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

fs.mkdirSync(outDir, { recursive: true });
for (const [dirName, prefix] of [['PhIcons', 'ph'], ['DeckIcons', 'deck']]) {
  const dir = path.join(xcassets, dirName);
  for (const set of fs.readdirSync(dir).filter((f) => f.endsWith('.imageset')).sort()) {
    const base = set.replace('.imageset', '').replace(new RegExp(`^${prefix}-`), '');
    const name = `${prefix}_${snake(base)}`;
    const files = fs.readdirSync(path.join(dir, set)).filter((f) => f !== 'Contents.json');
    const svg = files.find((f) => f.endsWith('.svg'));
    if (!svg) { skipped.push(`${dirName}/${set}: ${files.map((f) => path.extname(f)).join(',') || 'empty'} only (no SVG)`); continue; }
    const r = convertSvg(fs.readFileSync(path.join(dir, set, svg), 'utf8'));
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
if (skipped.length) console.log(`  e.g. ${skipped[0]}`);
if (check && stale.length) { console.error(`convert-icons: stale: ${stale.join(', ')}`); process.exit(1); }
