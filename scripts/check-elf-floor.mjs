#!/usr/bin/env node
// check-elf-floor.mjs - fail if any ELF under --dir needs a GLIBC newer than the floor.
//
// Method: for every file whose first 4 bytes are 0x7f 'E' 'L' 'F', parse the ELF64
// little-endian section headers, find the SHT_GNU_verneed section (.gnu.version_r)
// and walk its Verdef-needed entries (Elf64_Verneed / Elf64_Vernaux), reading each
// version name from the linked string table. That is exactly the list the dynamic
// loader enforces (GLIBC_x.y, GLIBCXX_3.4.z, CXXABI_..., GCC_...). GLIBC_PRIVATE is
// ignored. If an ELF has no section headers (stripped/odd) or is not 64-bit LE, we
// fall back to a byte-scan for GLIBC_2.N / GLIBCXX_3.4.N strings and mark the row
// "scan". Statically linked ELFs have no verneed and report "-".
//
// The floor default is read from GLIBC_FLOOR in scripts/build-linux-sidecars.sh
// (the single source of truth); --floor overrides it.
//
// Flags: --dir <path> | --files <list> [--floor 2.31] [--json] [--allowlist <file>] [--glibcxx-floor 3.4.28]
//        --print-floor   print the floor from build-linux-sidecars.sh and exit
// Allowlist (default scripts/elf-floor-allowlist.json, empty): [{"path":"suffix/of/file","reason":"why"}]
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const here = path.dirname(fileURLToPath(import.meta.url));

function readFloorConst() {
  const txt = fs.readFileSync(path.join(here, 'build-linux-sidecars.sh'), 'utf8');
  const m = txt.match(/^GLIBC_FLOOR=["']?(\d+\.\d+)["']?/m);
  if (!m) throw new Error('GLIBC_FLOOR not found in build-linux-sidecars.sh');
  return m[1];
}

const args = process.argv.slice(2);
const opt = (n, d) => { const i = args.indexOf(n); return i >= 0 ? args[i + 1] : d; };
if (args.includes('--print-floor')) { console.log(readFloorConst()); process.exit(0); }
const dir = opt('--dir');
// --files <list>: check exactly these files (one absolute path per line) instead of walking --dir.
const filesList = opt('--files');
if (!dir && !filesList) { console.error('usage: check-elf-floor.mjs (--dir <path> | --files <list>) [--floor 2.31] [--json] [--allowlist f] [--glibcxx-floor 3.4.28]'); process.exit(2); }
const floor = opt('--floor', readFloorConst());
const cxxFloor = opt('--glibcxx-floor', null);
const asJson = args.includes('--json');
const allowFile = opt('--allowlist', path.join(here, 'elf-floor-allowlist.json'));

let allow = [];
if (fs.existsSync(allowFile)) {
  allow = JSON.parse(fs.readFileSync(allowFile, 'utf8'));
  for (const e of allow) if (!e.path || !e.reason) { console.error(`allowlist entry needs path and reason: ${JSON.stringify(e)}`); process.exit(2); }
}

const vkey = (v) => v.split('.').map(Number);
const cmp = (a, b) => { const x = vkey(a), y = vkey(b); for (let i = 0; i < Math.max(x.length, y.length); i++) { const d = (x[i] || 0) - (y[i] || 0); if (d) return d; } return 0; };
const maxOf = (list) => list.reduce((m, v) => (m === null || cmp(v, m) > 0 ? v : m), null);

function* walk(d) {
  for (const ent of fs.readdirSync(d, { withFileTypes: true })) {
    const p = path.join(d, ent.name);
    if (ent.isSymbolicLink()) continue;
    if (ent.isDirectory()) yield* walk(p);
    else if (ent.isFile()) yield p;
  }
}

function isElf(p) {
  let fd; try { fd = fs.openSync(p, 'r'); const b = Buffer.alloc(4); const n = fs.readSync(fd, b, 0, 4, 0); return n === 4 && b[0] === 0x7f && b[1] === 0x45 && b[2] === 0x4c && b[3] === 0x46; }
  catch { return false; } finally { if (fd !== undefined) fs.closeSync(fd); }
}

function cstr(buf, off) { let e = off; while (e < buf.length && buf[e] !== 0) e++; return buf.toString('latin1', off, e); }

function verneed(buf) {
  if (buf[4] !== 2 || buf[5] !== 1) return null; // ELF64 little-endian only
  const shoff = Number(buf.readBigUInt64LE(0x28));
  const shentsize = buf.readUInt16LE(0x3a), shnum = buf.readUInt16LE(0x3c);
  if (!shoff || !shnum || shoff + shentsize * shnum > buf.length) return null;
  const sh = (i) => { const o = shoff + i * shentsize; return { type: buf.readUInt32LE(o + 4), off: Number(buf.readBigUInt64LE(o + 0x18)), size: Number(buf.readBigUInt64LE(o + 0x20)), link: buf.readUInt32LE(o + 0x28) }; };
  const out = [];
  for (let i = 0; i < shnum; i++) {
    const s = sh(i);
    if (s.type !== 0x6ffffffe) continue; // SHT_GNU_verneed
    const str = sh(s.link);
    let o = s.off;
    for (;;) {
      const cnt = buf.readUInt16LE(o + 2), file = cstr(buf, str.off + buf.readUInt32LE(o + 4));
      let a = o + buf.readUInt32LE(o + 8);
      for (let k = 0; k < cnt; k++) {
        out.push({ file, name: cstr(buf, str.off + buf.readUInt32LE(a + 8)) });
        a += buf.readUInt32LE(a + 12);
      }
      const next = buf.readUInt32LE(o + 12);
      if (!next) break; o += next;
    }
  }
  return out;
}

function scan(buf) {
  const txt = buf.toString('latin1');
  const names = new Set([...txt.matchAll(/GLIBC(?:XX)?_\d+(?:\.\d+)+/g)].map((m) => m[0]));
  return [...names].map((name) => ({ file: '?', name }));
}

const rows = [];
const targets = filesList
  ? fs.readFileSync(filesList, 'utf8').split('\n').map((l) => l.trim()).filter(Boolean)
  : [...walk(dir)];
for (const p of targets) {
  if (!isElf(p)) continue;
  const rel = dir ? path.relative(dir, p) : p;
  const buf = fs.readFileSync(p);
  let method = 'verneed', ents = null;
  try { ents = verneed(buf); } catch { ents = null; }
  if (!ents) { method = 'scan'; ents = scan(buf); }
  const glibc = [], cxx = [];
  for (const { name } of ents) {
    let m;
    if ((m = name.match(/^GLIBC_(\d+(?:\.\d+)+)$/))) glibc.push(m[1]);
    else if ((m = name.match(/^GLIBCXX_(\d+(?:\.\d+)+)$/))) cxx.push(m[1]);
  }
  const g = maxOf(glibc), x = maxOf(cxx);
  const al = allow.find((e) => rel === e.path || rel.endsWith(e.path));
  let status = 'ok';
  if (g && cmp(g, floor) > 0) status = al ? 'allowed' : 'FAIL';
  else if (cxxFloor && x && cmp(x, cxxFloor) > 0) status = al ? 'allowed' : 'FAIL';
  rows.push({ file: rel, glibc: g, glibcxx: x, method, status, reason: al ? al.reason : undefined });
}
rows.sort((a, b) => a.file.localeCompare(b.file));
const fails = rows.filter((r) => r.status === 'FAIL');

if (asJson) {
  console.log(JSON.stringify({ floor, dir, checked: rows.length, failed: fails.length, rows }, null, 2));
} else {
  const w = Math.min(70, Math.max(10, ...rows.map((r) => r.file.length)));
  console.log(`ELF floor check: GLIBC <= ${floor}${cxxFloor ? `, GLIBCXX <= ${cxxFloor}` : ''}  (${rows.length} ELF files in ${dir || filesList})`);
  console.log(`${'FILE'.padEnd(w)}  GLIBC   GLIBCXX   METHOD   STATUS`);
  for (const r of rows) console.log(`${r.file.slice(-w).padEnd(w)}  ${(r.glibc || '-').padEnd(6)}  ${(r.glibcxx || '-').padEnd(8)}  ${r.method.padEnd(7)}  ${r.status}${r.reason ? ' (' + r.reason + ')' : ''}`);
  console.log(fails.length ? `\nFAIL: ${fails.length} ELF file(s) need GLIBC above ${floor}` : `\nPASS: every ELF needs GLIBC <= ${floor}`);
}
process.exit(fails.length ? 1 : 0);
