#!/usr/bin/env node
// Rebuilds the member input schemas of allternit-computer-v1.json and
// allternit-browser-v1.json from the published @anthropic-ai/sdk typings, so
// member names and input fields stay identical to computer_toolset_20260801 /
// browser_toolset_20260801. Allternit metadata (risk, needs_confirm, scaling)
// lives in META below; the SDK only supplies names, fields and descriptions.
//
//   node contracts/computer-toolset/tools/import-anthropic-sdk.mjs <path-to-node_modules/@anthropic-ai/sdk>
//
// Run it only when Anthropic ships a new toolset version, then run
// `node contracts/computer-toolset/generate.mjs` to refresh the generated types.
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const here = path.dirname(fileURLToPath(import.meta.url));
const root = path.resolve(here, '..');
const sdkDir = process.argv[2];
if (!sdkDir) {
  console.error('usage: import-anthropic-sdk.mjs <path to @anthropic-ai/sdk>');
  process.exit(2);
}
const sdkVersion = JSON.parse(fs.readFileSync(path.join(sdkDir, 'package.json'), 'utf8')).version;
const dts = fs.readFileSync(path.join(sdkDir, 'resources/beta/messages/messages.d.ts'), 'utf8');

const clean = (doc) =>
  doc
    .replace(/^\/\*\*|\*\/$/g, '')
    .split('\n')
    .map((l) => l.replace(/^\s*\*\s?/, '').trim())
    .filter(Boolean)
    .join(' ')
    .trim();

function interfaceBody(name) {
  const re = new RegExp(`(/\\*\\*(?:(?!\\*/)[\\s\\S])*\\*/)?\\s*export interface ${name} \\{([\\s\\S]*?)\\n\\}`);
  const m = dts.match(re);
  if (!m) throw new Error(`interface ${name} not found in SDK typings`);
  return { doc: m[1] ? clean(m[1]) : '', body: m[2] };
}
function typeAlias(name) {
  const m = dts.match(new RegExp(`export type ${name} = ([^;]+);`));
  if (!m) throw new Error(`type ${name} not found`);
  return m[1].trim();
}

function parseFields(body) {
  const fields = [];
  const re = /(\/\*\*(?:(?!\*\/)[\s\S])*\*\/)?\s*(\w+)(\??):\s*([^;]+);/g;
  let m;
  while ((m = re.exec(body))) {
    fields.push({ name: m[2], optional: m[3] === '?', type: m[4].trim(), doc: m[1] ? clean(m[1]) : '' });
  }
  return fields;
}

const ARRAY_LEN = { coordinate: 2, start_coordinate: 2, region: 4 };

function tsToSchema(ts, fieldName) {
  ts = ts.replace(/\s*\|\s*null$/, '').trim();
  if (ts === 'string') return { type: 'string' };
  if (ts === 'number') return { type: 'number' };
  if (ts === 'boolean') return { type: 'boolean' };
  if (/^'[^']*'$/.test(ts)) return { type: 'string', const: ts.slice(1, -1) };
  if (ts === 'Array<number>') {
    const n = ARRAY_LEN[fieldName];
    return { type: 'array', items: { type: 'number' }, ...(n ? { minItems: n, maxItems: n } : {}) };
  }
  if (ts === 'Array<string>') return { type: 'array', items: { type: 'string' } };
  if (/^Beta\w+$/.test(ts)) {
    if (/Target$/.test(ts) && dts.includes(`export interface ${ts} {`)) return objectSchema(ts);
    const alias = typeAlias(ts);
    const parts = alias.split('|').map((p) => p.trim());
    if (parts.every((p) => /^'[^']*'$/.test(p))) return { type: 'string', enum: parts.map((p) => p.slice(1, -1)) };
    return { anyOf: parts.map((p) => tsToSchema(p, fieldName)) };
  }
  throw new Error(`unmapped TS type ${ts}`);
}

function objectSchema(iface) {
  const { doc, body } = interfaceBody(iface);
  const properties = {};
  const required = [];
  for (const f of parseFields(body)) {
    const s = tsToSchema(f.type, f.name);
    if (f.doc) s.description = f.doc;
    properties[f.name] = s;
    if (!f.optional) required.push(f.name);
  }
  const out = { type: 'object', properties, required, additionalProperties: false };
  if (doc) out.description = doc;
  return out;
}

const pascal = (s) => s.split('_').map((w) => w[0].toUpperCase() + w.slice(1)).join('');

function members(prefix, names) {
  return names.map((name) => {
    const s = objectSchema(`${prefix}${pascal(name)}Input`);
    const description = s.description || '';
    delete s.description;
    return { name, description, input_schema: s };
  });
}

function sdkNames(alias) {
  return typeAlias(alias)
    .split('|')
    .map((p) => p.trim().slice(1, -1));
}

for (const [file, prefix, alias] of [
  ['allternit-computer-v1.json', 'BetaComputer', 'BetaComputerMemberName'],
  ['allternit-browser-v1.json', 'BetaBrowser', 'BetaBrowserMemberName'],
]) {
  const p = path.join(root, file);
  const contract = JSON.parse(fs.readFileSync(p, 'utf8'));
  const sdk = members(prefix, sdkNames(alias));
  const sdkSet = new Set(sdk.map((m) => m.name));
  const ours = new Set(contract.members.map((m) => m.name));
  const missing = [...sdkSet].filter((n) => !ours.has(n));
  const extra = [...ours].filter((n) => !sdkSet.has(n));
  if (missing.length || extra.length) {
    console.error(`${file}: member set differs from SDK ${sdkVersion}: missing=${missing} extra=${extra}. Add metadata rows by hand first.`);
    process.exit(1);
  }
  const byName = Object.fromEntries(sdk.map((m) => [m.name, m]));
  contract.upstream.sdk_version = sdkVersion;
  contract.members = contract.members.map((m) => ({ ...m, description: byName[m.name].description, input_schema: byName[m.name].input_schema }));
  fs.writeFileSync(p, JSON.stringify(contract, null, 2) + '\n');
  console.log(`${file}: ${contract.members.length} members refreshed from @anthropic-ai/sdk ${sdkVersion}`);
}
