#!/usr/bin/env node
// Generates the Android design tokens from the iOS design system (the source of truth).
// Node 20, no dependencies.
//
//   node contracts/design-tokens/generate.mjs           # write the generated file
//   node contracts/design-tokens/generate.mjs --check   # exit 1 if the generated file is stale
//
// Reads:
//   surfaces/allternit-mobile/ios/Assets.xcassets/*.colorset/Contents.json   (light + dark)
//   surfaces/allternit-mobile/ios/Core/DesignSystem/Color+Theme.swift        (radii, glass, static colors)
//   surfaces/allternit-mobile/ios/Core/DesignSystem/ModeTheme.swift          (mode accents, icons, agent tiles)
// Writes (never edit by hand):
//   surfaces/allternit-mobile/android/core/design/src/main/java/com/allternit/core/design/AllternitTokens.gen.kt
//
// A value that is not a literal fails loudly, naming it, instead of being guessed.
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const here = path.dirname(fileURLToPath(import.meta.url));
const repo = path.resolve(here, '../..');
const mobile = path.join(repo, 'surfaces/allternit-mobile');
const assets = path.join(mobile, 'ios/Assets.xcassets');
const themeSwift = path.join(mobile, 'ios/Core/DesignSystem/Color+Theme.swift');
const modeSwift = path.join(mobile, 'ios/Core/DesignSystem/ModeTheme.swift');
const outFile = path.join(mobile, 'android/core/design/src/main/java/com/allternit/core/design/AllternitTokens.gen.kt');
const check = process.argv.includes('--check');

const fail = (msg) => { console.error(`design-tokens: ${msg}`); process.exit(1); };
const lowerFirst = (s) => s[0].toLowerCase() + s.slice(1);
const hex2 = (n) => n.toString(16).toUpperCase().padStart(2, '0');

// ---- colorsets ------------------------------------------------------------------------------
function component(raw, name, set) {
  const s = String(raw).trim();
  if (/^0x[0-9a-f]+$/i.test(s)) return parseInt(s, 16);       // hex byte: 0xFF
  if (/^#[0-9a-f]{1,2}$/i.test(s)) return parseInt(s.slice(1), 16);
  if (/^\d+\.\d*$|^\.\d+$/.test(s)) {                          // float 0-1: 1.000
    const f = parseFloat(s);
    if (f > 1) fail(`colorset ${set}: ${name}="${s}" is a float above 1`);
    return Math.round(f * 255);
  }
  if (/^\d+$/.test(s)) {                                       // integer 0-255
    const n = parseInt(s, 10);
    if (n > 255) fail(`colorset ${set}: ${name}="${s}" is above 255`);
    return n;
  }
  return fail(`colorset ${set}: cannot parse ${name}="${s}"`);
}
function argb(color, set) {
  const c = color.components;
  if (!c) fail(`colorset ${set}: entry has no components`);
  let r, g, b;
  if ('white' in c) { r = g = b = component(c.white, 'white', set); }
  else { r = component(c.red, 'red', set); g = component(c.green, 'green', set); b = component(c.blue, 'blue', set); }
  const a = 'alpha' in c ? component(c.alpha, 'alpha', set) : 255;
  return `0x${hex2(a)}${hex2(r)}${hex2(g)}${hex2(b)}`;
}
const colorsets = fs.readdirSync(assets).filter((f) => f.endsWith('.colorset')).sort().map((dir) => {
  const set = dir.replace('.colorset', '');
  const json = JSON.parse(fs.readFileSync(path.join(assets, dir, 'Contents.json'), 'utf8'));
  let light = null, dark = null;
  for (const e of json.colors ?? []) {
    const ap = e.appearances ?? [];
    const isDark = ap.some((a) => a.appearance === 'luminosity' && a.value === 'dark');
    const other = ap.some((a) => !(a.appearance === 'luminosity' && a.value === 'dark'));
    if (other) continue; // high-contrast etc: not used
    if (isDark) dark = argb(e.color, set); else light = argb(e.color, set);
  }
  if (!light) fail(`colorset ${set}: no light (default) color`);
  return { set, prop: lowerFirst(set), light, dark: dark ?? light };
});
if (colorsets.length === 0) fail('no colorsets found');

// ---- Swift literals -------------------------------------------------------------------------
const theme = fs.readFileSync(themeSwift, 'utf8');
const mode = fs.readFileSync(modeSwift, 'utf8');

const radii = [...theme.matchAll(/static let radius(\w+): CGFloat = ([\d.]+)/g)].map((m) => ({ name: m[1], v: m[2] }));
if (radii.length === 0) fail('Color+Theme.swift: no radius constants found');

const statics = [...theme.matchAll(/static let (\w+) = Color\(hex: "#([0-9A-Fa-f]{6})"\)/g)]
  .map((m) => ({ name: m[1], argb: `0xFF${m[2].toUpperCase()}` }));
const status = statics.filter((s) => s.name.startsWith('status'));
const accents = Object.fromEntries(statics.filter((s) => s.name.startsWith('accent')).map((s) => [s.name, s.argb]));
for (const n of ['statusSuccess', 'statusWarning', 'statusInfo', 'statusError', 'accentChat', 'accentCowork', 'accentCode', 'accentBrowser']) {
  if (!statics.find((s) => s.name === n)) fail(`Color+Theme.swift: static ${n} is missing or not a hex literal`);
}

const glass = [];
for (const m of theme.matchAll(/static let (glassBg\w*|border\w+|shadow\w+) = (.+)$/gm)) {
  const t = m[2].trim().match(/^(?:Color\("(\w+)"\)|Color\.(black|white))\.opacity\(([\d.]+)\)$/);
  if (t) glass.push({ name: m[1], token: t[1] ? lowerFirst(t[1]) : t[2], alpha: t[3] });
  else if (/^Color\("\w+"\)$/.test(m[2].trim()) === false) fail(`Color+Theme.swift: ${m[1]} = ${m[2].trim()} is not a literal tint`);
}
if (!glass.find((g) => g.name === 'glassBg')) fail('Color+Theme.swift: glassBg not found');

function block(src, header, file) {
  const i = src.search(header);
  if (i < 0) fail(`${file}: ${header} not found`);
  const open = src.indexOf('{', src.indexOf('{', i) + 0);
  let depth = 0, j = open;
  for (; j < src.length; j++) { if (src[j] === '{') depth++; else if (src[j] === '}' && --depth === 0) break; }
  return src.slice(open, j);
}
const cases = (b) => Object.fromEntries([...b.matchAll(/case \.(\w+): return (.+)$/gm)].map((m) => [m[1], m[2].trim()]));
const resolveColor = (expr, what) => {
  let m;
  if ((m = expr.match(/^Theme\.(\w+)$/))) {
    const hit = statics.find((s) => s.name === m[1]);
    return hit ? { argb: hit.argb } : fail(`ModeTheme.swift: ${what} -> Theme.${m[1]} is not a static hex color`);
  }
  if ((m = expr.match(/^Color\(hex: "#([0-9A-Fa-f]{6})"\)$/))) return { argb: `0xFF${m[1].toUpperCase()}` };
  if ((m = expr.match(/^Color\("(\w+)"\)$/))) return { token: lowerFirst(m[1]) };
  return fail(`ModeTheme.swift: ${what} = ${expr} is not a literal`);
};
const str = (expr, what) => {
  const m = expr.match(/^"([^"]*)"$/);
  return m ? m[1] : (expr === 'nil' ? null : fail(`ModeTheme.swift: ${what} = ${expr} is not a string literal`));
};

const modeTheme = block(mode, /struct ModeTheme/, 'ModeTheme.swift');
const accentCases = cases(block(modeTheme, /var accent: Color/, 'ModeTheme.swift'));
const iconCases = cases(block(modeTheme, /var icon: String/, 'ModeTheme.swift'));
const softM = mode.match(/var accentSoft: Color \{ accent\.opacity\(([\d.]+)\) \}/);
const glowM = mode.match(/var accentGlow: Color \{ accent\.opacity\(([\d.]+)\) \}/);
if (!softM || !glowM) fail('ModeTheme.swift: accentSoft/accentGlow opacity literals not found');
const modes = Object.keys(accentCases).map((k) => ({
  name: k, color: resolveColor(accentCases[k], `accent ${k}`).argb ?? fail(`ModeTheme.swift: accent ${k} is adaptive`),
  icon: str(iconCases[k], `icon ${k}`),
}));

const tileBlock = block(mode, /enum AgentModeTile/, 'ModeTheme.swift');
const tileOrder = [...tileBlock.matchAll(/^\s{4}case (\w+)$/gm)].map((m) => m[1]);
const tLabel = cases(block(tileBlock, /var label: String/, 'ModeTheme.swift'));
const tColor = cases(block(tileBlock, /var color: Color/, 'ModeTheme.swift'));
const tIcon = cases(block(tileBlock, /var icon: String/, 'ModeTheme.swift'));
const tDeck = cases(block(tileBlock, /var deckIcon: String\?/, 'ModeTheme.swift'));
const tiles = tileOrder.map((k) => {
  const c = resolveColor(tColor[k] ?? fail(`ModeTheme.swift: tile ${k} has no color`), `tile ${k} color`);
  return { name: k, label: str(tLabel[k], `tile ${k} label`), icon: str(tIcon[k], `tile ${k} icon`), deck: str(tDeck[k], `tile ${k} deckIcon`), ...c };
});

// ---- emit -----------------------------------------------------------------------------------
const cap = (s) => s[0].toUpperCase() + s.slice(1);
const q = (s) => (s === null ? 'null' : `"${s}"`);
const L = [];
L.push(
  '// GENERATED by contracts/design-tokens/generate.mjs — do not edit.',
  '// Source: surfaces/allternit-mobile/ios (Assets.xcassets colorsets, Color+Theme.swift, ModeTheme.swift).',
  '@file:Suppress("unused")',
  '',
  'package com.allternit.core.design',
  '',
  'import androidx.compose.runtime.Immutable',
  'import androidx.compose.ui.graphics.Color',
  'import androidx.compose.ui.unit.Dp',
  'import androidx.compose.ui.unit.dp',
  '',
  '/** One value per iOS colorset, by colorset name (lower camel case). */',
  '@Immutable',
  'data class AllternitColorScheme(',
  ...colorsets.map((c) => `    val ${c.prop}: Color,`),
  ') {',
  '    /** Colorset name to color, for tests and debug screens. */',
  '    fun asMap(): Map<String, Color> = mapOf(',
  ...colorsets.map((c) => `        ${q(c.set)} to ${c.prop},`),
  '    )',
  '}',
  '',
  'object AllternitColors {',
  '    val names: List<String> = listOf(' + colorsets.map((c) => q(c.set)).join(', ') + ')',
  '',
);
for (const [label, key] of [['Light', 'light'], ['Dark', 'dark']]) {
  L.push(`    val ${label} = AllternitColorScheme(`, ...colorsets.map((c) => `        ${c.prop} = Color(${c[key]}),`), '    )', '');
}
L.pop();
L.push('}', '', '/** Non-adaptive hues (the same in light and dark). */', 'object AllternitStatic {');
for (const s of statics) L.push(`    val ${cap(s.name)} = Color(${s.argb})`);
L.push('}', '', '/** Corner radius scale (iOS Theme.radius*). */', 'object AllternitRadii {');
for (const r of radii) L.push(`    val ${r.name}: Dp = ${r.v}.dp`);
L.push('}', '', '/** A colorset token (or "black"/"white") at an opacity. */', 'data class AllternitTint(val token: String, val alpha: Float)', '',
  '/** Translucent fills and hairlines (iOS Theme.glassBg, borderWarm*, shadowGlow). */', 'object AllternitGlass {');
for (const g of glass) L.push(`    val ${cap(g.name)} = AllternitTint(${q(g.token)}, ${g.alpha}f)`);
L.push('}', '', '/** A chat mode: accent plus the SF Symbol iOS uses (Android maps it to a Phosphor icon). */',
  'data class ModeAccent(val accent: Color, val sfSymbol: String)', '',
  '/** An agent-mode tile. [color] is null when the tile uses the adaptive colorset named by [colorToken]. */',
  'data class AgentTile(val slug: String, val label: String, val color: Color?, val colorToken: String?, val sfSymbol: String, val deckIcon: String?)', '',
  'object ModeAccents {',
  `    const val SOFT_ALPHA = ${softM[1]}f`,
  `    const val GLOW_ALPHA = ${glowM[1]}f`,
  '');
for (const m of modes) L.push(`    val ${cap(m.name)} = ModeAccent(Color(${m.color}), ${q(m.icon)})`);
L.push('', '    val all: List<ModeAccent> = listOf(' + modes.map((m) => cap(m.name)).join(', ') + ')', '', '    val agentTiles: List<AgentTile> = listOf(');
for (const t of tiles) L.push(`        AgentTile(${q(t.name)}, ${q(t.label)}, ${t.argb ? `Color(${t.argb})` : 'null'}, ${q(t.token ?? null)}, ${q(t.icon)}, ${q(t.deck)}),`);
L.push('    )', '}', '');
const out = L.join('\n');

if (check) {
  const cur = fs.existsSync(outFile) ? fs.readFileSync(outFile, 'utf8') : '';
  if (cur !== out) fail(`${path.relative(repo, outFile)} is stale; run: node contracts/design-tokens/generate.mjs`);
  console.log(`design-tokens: ok (${colorsets.length} colors x light+dark, ${radii.length} radii, ${glass.length} glass, ${modes.length} modes, ${tiles.length} tiles)`);
} else {
  fs.mkdirSync(path.dirname(outFile), { recursive: true });
  fs.writeFileSync(outFile, out);
  console.log(`design-tokens: wrote ${path.relative(repo, outFile)} (${colorsets.length} colors x light+dark, ${radii.length} radii, ${glass.length} glass, ${modes.length} modes, ${tiles.length} tiles)`);
}
