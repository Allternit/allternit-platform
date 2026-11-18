#!/usr/bin/env node
// Maps the iOS AppLogos imagesets to WebP assets plus a slug index. Node 20, no dependencies;
// needs `cwebp` (brew install webp) unless --dry-run.
//   node scripts/convert-logos.mjs --dry-run   # counts and total source size, writes nothing
//   node scripts/convert-logos.mjs             # core/design/src/main/assets/logos/<slug>.webp + index.json
// Not run in the design-tokens PR (about 1,250 files); run it in the PR that wires Coil.
import fs from 'node:fs';
import path from 'node:path';
import { execFileSync } from 'node:child_process';
import { fileURLToPath } from 'node:url';

const here = path.dirname(fileURLToPath(import.meta.url));
const android = path.resolve(here, '..');
const src = path.resolve(android, '../ios/Assets.xcassets/AppLogos');
const outDir = path.join(android, 'core/design/src/main/assets/logos');
const dry = process.argv.includes('--dry-run');

const logos = [];
let bytes = 0;
for (const set of fs.readdirSync(src).filter((f) => f.endsWith('.imageset')).sort()) {
  const json = JSON.parse(fs.readFileSync(path.join(src, set, 'Contents.json'), 'utf8'));
  const file = (json.images ?? []).map((i) => i.filename).find(Boolean);
  if (!file) { console.error(`convert-logos: ${set} has no image file`); process.exit(1); }
  const from = path.join(src, set, file);
  const slug = set.replace(/\.imageset$/, '').replace(/^applogo-/, '');
  bytes += fs.statSync(from).size;
  logos.push({ slug, from });
}
const dup = logos.map((l) => l.slug).filter((s, i, a) => a.indexOf(s) !== i);
if (dup.length) { console.error(`convert-logos: duplicate slugs: ${dup.join(', ')}`); process.exit(1); }

if (dry) {
  console.log(`convert-logos (dry run): ${logos.length} logos, ${(bytes / 1048576).toFixed(1)} MB source PNG, ${fs.existsSync('/opt/homebrew/bin/cwebp') || hasCwebp() ? 'cwebp found' : 'cwebp NOT installed'}`);
  process.exit(0);
}
function hasCwebp() { try { execFileSync('cwebp', ['-version'], { stdio: 'ignore' }); return true; } catch { return false; } }
if (!hasCwebp()) { console.error('convert-logos: cwebp not found (brew install webp)'); process.exit(1); }
fs.mkdirSync(outDir, { recursive: true });
let outBytes = 0;
for (const { slug, from } of logos) {
  const to = path.join(outDir, `${slug}.webp`);
  execFileSync('cwebp', ['-quiet', '-q', '85', '-alpha_q', '100', from, '-o', to]);
  outBytes += fs.statSync(to).size;
}
fs.writeFileSync(path.join(outDir, 'index.json'), JSON.stringify({ count: logos.length, logos: logos.map((l) => l.slug) }, null, 1) + '\n');
console.log(`convert-logos: wrote ${logos.length} WebP (${(outBytes / 1048576).toFixed(1)} MB) and index.json`);
