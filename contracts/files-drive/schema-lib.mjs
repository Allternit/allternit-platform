// Shared loader + validator for the Files drive contract (contracts/files-drive/schema/*.json).
// Used by generate.mjs and by tools/files-stub. Node 20, no dependencies.
// Schemas use a fixed subset of JSON Schema 2020-12 (see SUPPORTED); the loader rejects any other keyword.
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const here = path.dirname(fileURLToPath(import.meta.url));

export function fail(msg) {
  console.error(`files-drive contract: ${msg}`);
  process.exit(1);
}

// ---------------------------------------------------------------- load

const SUPPORTED = new Set([
  '$schema', '$id', 'title', 'description', '$defs', 'type', 'properties', 'required', 'additionalProperties',
  'items', 'enum', 'const', 'pattern', 'minLength', 'maxLength', 'minimum', 'maximum', 'minItems', 'maxItems',
  '$ref', 'oneOf', 'x-discriminator', 'format',
]);

const schemaDir = path.join(here, 'schema');
export const files = fs.readdirSync(schemaDir).filter((f) => f.endsWith('.json')).sort();
export const docs = {};
export const defs = new Map(); // name -> { file, schema }
for (const f of files) {
  const doc = JSON.parse(fs.readFileSync(path.join(schemaDir, f), 'utf8'));
  docs[f] = doc;
  for (const [name, schema] of Object.entries(doc.$defs || {})) {
    if (defs.has(name)) fail(`type ${name} defined in ${defs.get(name).file} and ${f}`);
    if (!/^[A-Z][A-Za-z0-9]*$/.test(name)) fail(`type name ${name} must be PascalCase`);
    defs.set(name, { file: f, schema });
  }
}

function walk(s, where, fn) {
  if (!s || typeof s !== 'object') return;
  fn(s, where);
  if (s.properties) for (const [k, v] of Object.entries(s.properties)) walk(v, `${where}.${k}`, fn);
  if (s.items) walk(s.items, `${where}[]`, fn);
  if (s.additionalProperties && typeof s.additionalProperties === 'object') walk(s.additionalProperties, `${where}{}`, fn);
  if (s.oneOf) s.oneOf.forEach((v, i) => walk(v, `${where}|${i}`, fn));
}

function refName(file, ref) {
  const m = /^([a-z0-9-]+\.json)?#\/\$defs\/([A-Za-z0-9]+)$/.exec(ref);
  if (!m) fail(`${file}: unsupported $ref ${ref}`);
  const target = m[1] || file;
  const d = defs.get(m[2]);
  if (!d || d.file !== target) fail(`${file}: $ref ${ref} does not resolve`);
  return m[2];
}

for (const [name, { file, schema }] of defs) {
  walk(schema, `${file}#${name}`, (s, where) => {
    for (const k of Object.keys(s)) if (!SUPPORTED.has(k)) fail(`${where}: keyword "${k}" is not in the supported subset`);
    if (s.$ref) { s.__ref = refName(file, s.$ref); }
    if (s.type === 'object' && s.properties) {
      if (s.additionalProperties !== false) fail(`${where}: objects with properties must set additionalProperties: false`);
      for (const r of s.required || []) if (!(r in s.properties)) fail(`${where}: required ${r} is not a property`);
      for (const k of Object.keys(s.properties)) if (!/^[a-z][a-z0-9_]*$/.test(k)) fail(`${where}: property ${k} must be snake_case`);
    }
    if (s.oneOf && !s['x-discriminator']) fail(`${where}: oneOf needs x-discriminator`);
  });
}

// Unions: every member is an object whose discriminator property is a string const.
export const unionOf = new Map(); // variant name -> { union, tag, value }
for (const [name, { schema }] of defs) {
  if (!schema.oneOf) continue;
  const tag = schema['x-discriminator'];
  const seen = new Set();
  for (const m of schema.oneOf) {
    const v = defs.get(m.__ref)?.schema;
    if (!v || v.type !== 'object') fail(`${name}: union member must be an object $ref`);
    const p = v.properties[tag];
    if (!p || typeof p.const !== 'string' || !(v.required || []).includes(tag)) fail(`${name}: ${m.__ref}.${tag} must be a required string const`);
    if (seen.has(p.const)) fail(`${name}: duplicate tag ${p.const}`);
    seen.add(p.const);
    if (unionOf.has(m.__ref)) fail(`${m.__ref} is in two unions`);
    unionOf.set(m.__ref, { union: name, tag, value: p.const });
  }
}

// ---------------------------------------------------------------- validator (the supported subset)

const regexCache = new Map();
export function validate(s, v, at, errs) {
  if (s.$ref) return validate(defs.get(s.__ref).schema, v, at, errs);
  if (s.oneOf) {
    const ok = s.oneOf.filter((m) => { const e = []; validate(m, v, at, e); return e.length === 0; });
    if (ok.length !== 1) errs.push(`${at}: matches ${ok.length} of oneOf (need exactly 1)`);
    return;
  }
  const t = s.type;
  const isInt = typeof v === 'number' && Number.isInteger(v);
  const typeOk = { string: typeof v === 'string', integer: isInt, number: typeof v === 'number', boolean: typeof v === 'boolean',
    array: Array.isArray(v), object: v !== null && typeof v === 'object' && !Array.isArray(v) }[t];
  if (t && !typeOk) { errs.push(`${at}: expected ${t}`); return; }
  if ('const' in s && v !== s.const) errs.push(`${at}: must be ${JSON.stringify(s.const)}`);
  if (s.enum && !s.enum.includes(v)) errs.push(`${at}: ${JSON.stringify(v)} not in enum`);
  if (t === 'string') {
    const n = [...v].length;
    if (s.minLength !== undefined && n < s.minLength) errs.push(`${at}: shorter than ${s.minLength}`);
    if (s.maxLength !== undefined && n > s.maxLength) errs.push(`${at}: longer than ${s.maxLength}`);
    if (s.pattern) {
      if (!regexCache.has(s.pattern)) regexCache.set(s.pattern, new RegExp(s.pattern, 'u'));
      if (!regexCache.get(s.pattern).test(v)) errs.push(`${at}: ${JSON.stringify(v)} does not match ${s.pattern}`);
    }
  }
  if (t === 'integer' || t === 'number') {
    if (s.minimum !== undefined && v < s.minimum) errs.push(`${at}: below ${s.minimum}`);
    if (s.maximum !== undefined && v > s.maximum) errs.push(`${at}: above ${s.maximum}`);
  }
  if (t === 'array') {
    if (s.minItems !== undefined && v.length < s.minItems) errs.push(`${at}: fewer than ${s.minItems} items`);
    if (s.maxItems !== undefined && v.length > s.maxItems) errs.push(`${at}: more than ${s.maxItems} items`);
    if (s.items) v.forEach((x, i) => validate(s.items, x, `${at}[${i}]`, errs));
  }
  if (t === 'object') {
    for (const r of s.required || []) if (!(r in v)) errs.push(`${at}: missing ${r}`);
    for (const [k, x] of Object.entries(v)) {
      if (s.properties && k in s.properties) validate(s.properties[k], x, `${at}.${k}`, errs);
      else if (s.additionalProperties === false) errs.push(`${at}: unknown property ${k}`);
      else if (s.additionalProperties && typeof s.additionalProperties === 'object') validate(s.additionalProperties, x, `${at}.${k}`, errs);
    }
  }
}

/** Validate `value` against the named contract type. Returns a list of error strings (empty = valid). */
export function validateType(typeName, value) {
  const d = defs.get(typeName);
  if (!d) throw new Error(`unknown contract type ${typeName}`);
  const errs = [];
  validate(d.schema, value, typeName, errs);
  return errs;
}

