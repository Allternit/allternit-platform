#!/usr/bin/env node
// gen-block-glyphs.mjs: generates the A:// block-type glyph tables for Swift, Kotlin and
// Gizzi TS from the brand master glyphs.json, and checks web block-type.ts against it.
// Node 20+, no dependencies. Plan: handoffs/mobile-refresh-2026-11-23 decision D7.
//
// Usage:
//   node gen-block-glyphs.mjs --swift <out.swift>
//   node gen-block-glyphs.mjs --kotlin <out.kt> [--package com.allternit.core.design]
//   node gen-block-glyphs.mjs --ts <out.ts>
//   node gen-block-glyphs.mjs --check <allternit-ai/src/components/brand/block-type.ts>
//   Any flag combination works in one run; add --master <glyphs.json> to override the master.
//   --check exits 1 with a diff when web and master differ, 0 when they match.
import { readFileSync, writeFileSync, mkdirSync } from 'node:fs';
import { dirname, resolve } from 'node:path';

const DEFAULT_MASTER =
  '/Users/joe/Desktop/Allternit/Allternit Assets/Brand/Allternit/block-type/glyphs.json';
const HELP = `gen-block-glyphs.mjs: generate block-type glyph tables from the master glyphs.json
  --master <path>    master glyphs.json (default: ${DEFAULT_MASTER})
  --swift <out>      write a Swift file (enum BlockGlyphs)
  --kotlin <out>     write a Kotlin file (object BlockGlyphs)
  --package <pkg>    Kotlin package (default com.allternit.core.design)
  --ts <out>         write a TS module (BLOCK_GLYPHS, BLOCK_PUNCT) for Gizzi
  --check <file>     compare web block-type.ts with the master; exit 1 on mismatch
  --help             this text`;

const args = process.argv.slice(2);
const opt = {};
for (let i = 0; i < args.length; i++) {
  const a = args[i];
  if (a === '--help' || a === '-h') { console.log(HELP); process.exit(0); }
  if (!['--master', '--swift', '--kotlin', '--package', '--ts', '--check'].includes(a)) {
    console.error(`unknown flag ${a}\n${HELP}`); process.exit(2);
  }
  if (i + 1 >= args.length) { console.error(`${a} needs a value`); process.exit(2); }
  opt[a.slice(2)] = args[++i];
}
if (!opt.swift && !opt.kotlin && !opt.ts && !opt.check) { console.log(HELP); process.exit(2); }

const masterPath = resolve(opt.master ?? DEFAULT_MASTER);
const master = JSON.parse(readFileSync(masterPath, 'utf8'));
const g = master.grid;
const byCode = (a, b) => (a.codePointAt(0) - b.codePointAt(0));
const sortCells = (cells) => cells.map(([x, y]) => [x, y]).sort((a, b) => a[1] - b[1] || a[0] - b[0]);
const glyphs = Object.fromEntries(
  Object.keys(master.glyphs).sort(byCode).map((k) => [k, sortCells(master.glyphs[k].cells)]));
const punct = Object.fromEntries(
  Object.keys(master.punctuation).sort(byCode).map((k) => [
    k, { w: master.punctuation[k].w, cells: sortCells(master.punctuation[k].cells) }]));

const cmd = `node surfaces/allternit-mobile/scripts/gen-block-glyphs.mjs ${args.join(' ')}`;
const header = (c) =>
  `${c} GENERATED FILE, DO NOT EDIT.\n${c} Master: ${masterPath}\n${c} Regenerate: ${cmd}`;
const lit = (ch) => JSON.stringify(ch).replace(/[^\x20-\x7e]/g, (m) => '\\u{' + m.codePointAt(0).toString(16) + '}');
const swiftLit = (ch) => (ch === "'" ? `"'"` : lit(ch));
const kotlinLit = (ch) => (ch === "'" ? "'\\''" : `'${ch === '\\' ? '\\\\' : ch.codePointAt(0) > 126 ? '\\u' + ch.codePointAt(0).toString(16).padStart(4, '0') : ch}'`);
const pairs = (cells, f) => cells.map(([x, y]) => f(x, y)).join(', ');
const num = (n) => String(n);

function write(path, text) {
  mkdirSync(dirname(resolve(path)), { recursive: true });
  writeFileSync(path, text);
  console.log(`wrote ${path}`);
}

if (opt.swift) {
  let s = `${header('//')}\n\nimport Foundation\n\nenum BlockGlyphs {\n`;
  s += `    static let cell: Double = ${num(g.cell)}\n    static let block: Double = ${num(g.block)}\n    static let radius: Double = ${num(g.radius)}\n`;
  s += `    static let rows = ${num(g.rows)}\n    static let pitch = ${num(g.pitch)}\n    static let space = ${num(g.space)}\n\n`;
  s += `    /// Cells are (column, row) on a 5-column, ${g.rows}-row grid.\n    static let glyphs: [Character: [(Int, Int)]] = [\n`;
  for (const [k, c] of Object.entries(glyphs)) s += `        ${swiftLit(k)}: [${pairs(c, (x, y) => `(${x}, ${y})`)}],\n`;
  s += `    ]\n\n    /// Punctuation: w is the width in columns.\n    static let punct: [Character: (w: Int, cells: [(Int, Int)])] = [\n`;
  for (const [k, p] of Object.entries(punct)) s += `        ${swiftLit(k)}: (w: ${p.w}, cells: [${pairs(p.cells, (x, y) => `(${x}, ${y})`)}]),\n`;
  s += `    ]\n}\n`;
  write(opt.swift, s);
}

if (opt.kotlin) {
  const pkg = opt.package ?? 'com.allternit.core.design';
  let s = `${header('//')}\n\npackage ${pkg}\n\nobject BlockGlyphs {\n`;
  s += `    const val CELL = ${num(g.cell)}f\n    const val BLOCK = ${num(g.block)}f\n    const val RADIUS = ${num(g.radius)}f\n`;
  s += `    const val ROWS = ${num(g.rows)}\n    const val PITCH = ${num(g.pitch)}\n    const val SPACE = ${num(g.space)}\n\n`;
  s += `    /** Cells are (column, row) pairs. */\n    val glyphs: Map<Char, List<Pair<Int, Int>>> = mapOf(\n`;
  for (const [k, c] of Object.entries(glyphs)) s += `        ${kotlinLit(k)} to listOf(${pairs(c, (x, y) => `${x} to ${y}`)}),\n`;
  s += `    )\n\n    /** Punctuation: w is the width in columns. */\n    class Punct(val w: Int, val cells: List<Pair<Int, Int>>)\n\n    val punct: Map<Char, Punct> = mapOf(\n`;
  for (const [k, p] of Object.entries(punct)) s += `        ${kotlinLit(k)} to Punct(${p.w}, listOf(${pairs(p.cells, (x, y) => `${x} to ${y}`)})),\n`;
  s += `    )\n}\n`;
  write(opt.kotlin, s);
}

if (opt.ts) {
  let s = `${header('//')}\n\nexport type BlockCell = readonly [number, number];\n\n`;
  s += `export const BLOCK_CELL = ${num(g.cell)};\nexport const BLOCK_SIZE = ${num(g.block)};\nexport const BLOCK_RADIUS = ${num(g.radius)};\n`;
  s += `export const BLOCK_ROWS = ${num(g.rows)};\nexport const BLOCK_PITCH = ${num(g.pitch)};\nexport const BLOCK_SPACE = ${num(g.space)};\n\n`;
  s += `export const BLOCK_GLYPHS: Record<string, ReadonlyArray<BlockCell>> = {\n`;
  for (const [k, c] of Object.entries(glyphs)) s += `  ${JSON.stringify(k)}: [${pairs(c, (x, y) => `[${x}, ${y}]`)}],\n`;
  s += `};\n\nexport const BLOCK_PUNCT: Record<string, { w: number; cells: ReadonlyArray<BlockCell> }> = {\n`;
  for (const [k, p] of Object.entries(punct)) s += `  ${lit(k)}: { w: ${p.w}, cells: [${pairs(p.cells, (x, y) => `[${x}, ${y}]`)}] },\n`;
  s += `};\n`;
  write(opt.ts, s);
}

// ---- --check: eval-free parse of web block-type.ts ----
function extractObject(src, name) {
  const m = new RegExp(`export const ${name}\\b[^=]*=\\s*\\{`).exec(src);
  if (!m) throw new Error(`${name} not found`);
  let i = m.index + m[0].length, depth = 1, inStr = null;
  const start = i;
  for (; i < src.length && depth > 0; i++) {
    const c = src[i];
    if (inStr) { if (c === '\\') i++; else if (c === inStr) inStr = null; continue; }
    if (c === '"' || c === "'") inStr = c;
    else if (c === '{') depth++;
    else if (c === '}') depth--;
  }
  return src.slice(start, i - 1);
}
function unquote(raw) {
  const q = raw[0];
  if (q !== '"' && q !== "'") return raw;
  return raw.slice(1, -1).replace(/\\u\{([0-9a-fA-F]+)\}|\\u([0-9a-fA-F]{4})/g, (_, a, b) => String.fromCodePoint(parseInt(a || b, 16))).replace(/\\(.)/g, '$1');
}
const KEY = String.raw`("(?:\\.|[^"\\])*"|'(?:\\.|[^'\\])*'|[A-Za-z0-9_]+)`;
const CELLS = String.raw`\[((?:\s*\[\s*\d+\s*,\s*\d+\s*\]\s*,?)*)\s*\]`;
const parseCells = (t) => [...t.matchAll(/\[\s*(\d+)\s*,\s*(\d+)\s*\]/g)].map((m) => [+m[1], +m[2]]);
function parseWeb(src) {
  const gl = {}, pu = {};
  for (const m of extractObject(src, 'BLOCK_GLYPHS').matchAll(new RegExp(`${KEY}\\s*:\\s*${CELLS}`, 'g')))
    gl[unquote(m[1])] = parseCells(m[2]);
  for (const m of extractObject(src, 'BLOCK_PUNCT').matchAll(new RegExp(`${KEY}\\s*:\\s*\\{\\s*w\\s*:\\s*(\\d+)\\s*,\\s*cells\\s*:\\s*${CELLS}\\s*\\}`, 'g')))
    pu[unquote(m[1])] = { w: +m[2], cells: parseCells(m[3]) };
  return { gl, pu };
}
const key = (c) => sortCells(c).map(([x, y]) => `${x},${y}`).join(' ');

if (opt.check) {
  const { gl, pu } = parseWeb(readFileSync(opt.check, 'utf8'));
  const diffs = [];
  const cmp = (kind, want, got, show) => {
    for (const k of Object.keys(want)) {
      if (!(k in got)) diffs.push(`${kind} ${JSON.stringify(k)}: missing in web`);
      else if (show(want[k]) !== show(got[k])) diffs.push(`${kind} ${JSON.stringify(k)}: master [${show(want[k])}] web [${show(got[k])}]`);
    }
    for (const k of Object.keys(got)) if (!(k in want)) diffs.push(`${kind} ${JSON.stringify(k)}: missing in master`);
  };
  cmp('glyph', glyphs, gl, key);
  cmp('punct', punct, pu, (p) => `w=${p.w} ${key(p.cells)}`);
  const gotC = Object.keys(gl).length + Object.keys(pu).length;
  if (diffs.length) {
    console.error(`MISMATCH: master has ${Object.keys(glyphs).length} glyphs + ${Object.keys(punct).length} punct; web has ${Object.keys(gl).length} + ${Object.keys(pu).length} (${gotC} parsed)`);
    for (const d of diffs) console.error('  ' + d);
    process.exit(1);
  }
  console.log(`OK: ${Object.keys(glyphs).length} glyphs + ${Object.keys(punct).length} punctuation match cell for cell`);
}
