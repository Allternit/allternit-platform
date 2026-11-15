#!/usr/bin/env node
// Generates Rust, TypeScript, Swift and Kotlin types from the Files drive contract
// (contracts/files-drive/schema/*.json). Node 20, no dependencies.
//
//   node contracts/files-drive/generate.mjs                 # write the generated files
//   node contracts/files-drive/generate.mjs --check         # exit 1 if any generated file is stale
//   node contracts/files-drive/generate.mjs --test          # validate every fixture against its schema
//   node contracts/files-drive/generate.mjs --harness DIR   # write TS + Swift fixture checks into DIR (scratch)
//
// Outputs (never edit them by hand):
//   platform/types/allternit-files-core/src/generated.rs
//   platform/types/allternit-files-core/tests/fixtures.rs
//   contracts/files-drive/gen/files-drive.gen.ts
//   contracts/files-drive/gen/FilesDriveContract.gen.swift
//   contracts/files-drive/gen/FilesDriveContract.gen.kt
//
// Schemas use a fixed subset of JSON Schema 2020-12 (see SUPPORTED). The loader rejects any other
// keyword, so a fixture can never pass because the validator skipped a rule.
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const here = path.dirname(fileURLToPath(import.meta.url));
const repo = path.resolve(here, '../..');
const argv = process.argv.slice(2);
const args = new Set(argv);

import { fail, files, docs, defs, unionOf, validate } from './schema-lib.mjs';

const fixtureDir = path.join(here, 'fixtures');
const fixtures = fs.readdirSync(fixtureDir).filter((f) => f.endsWith('.json')).sort().map((f) => {
  const fx = JSON.parse(fs.readFileSync(path.join(fixtureDir, f), 'utf8'));
  if (!defs.has(fx.type)) fail(`fixture ${f}: unknown type ${fx.type}`);
  if (typeof fx.valid !== 'boolean' || typeof fx.case !== 'string' || !('message' in fx)) fail(`fixture ${f}: needs type, valid, case, message`);
  return { file: f, ...fx };
});

if (args.has('--test')) {
  let bad = 0;
  for (const fx of fixtures) {
    const errs = [];
    validate(defs.get(fx.type).schema, fx.message, fx.type, errs);
    if (fx.valid && errs.length) { bad++; console.error(`FAIL ${fx.file} (${fx.type}) should be valid:\n  ${errs.join('\n  ')}`); }
    if (!fx.valid && !errs.length) { bad++; console.error(`FAIL ${fx.file} (${fx.type}) should be rejected but passed`); }
  }
  const nValid = fixtures.filter((f) => f.valid).length;
  if (bad) { console.error(`files-drive fixtures: ${bad} failed of ${fixtures.length}`); process.exit(1); }
  console.log(`files-drive fixtures: ${fixtures.length} passed (${nValid} valid, ${fixtures.length - nValid} rejected as expected; ${defs.size} types in ${files.length} schemas)`);
  process.exit(0);
}

// ---------------------------------------------------------------- shared model

const pascal = (s) => s.split(/[^A-Za-z0-9]+/).filter(Boolean).map((w) => w[0].toUpperCase() + w.slice(1)).join('');
const camel = (s) => { const p = pascal(s); return p[0].toLowerCase() + p.slice(1); };
const upperSnake = (s) => s.replace(/[^A-Za-z0-9]+/g, '_').replace(/([a-z0-9])([A-Z])/g, '$1_$2').toUpperCase();

// Hoist inline enums to named types: Parent + Field.
const model = []; // { name, kind: struct|enum|union|alias, ... }
const byName = new Map();
function add(t) { if (byName.has(t.name)) fail(`generated name clash: ${t.name}`); byName.set(t.name, t); model.push(t); }

function fieldType(s, owner, field) {
  if (s.__ref) return { k: 'ref', name: s.__ref };
  if (s.enum) { const name = owner + pascal(field); add({ name, kind: 'enum', values: s.enum, doc: s.description }); return { k: 'ref', name }; }
  if (typeof s.const === 'string') return { k: 'const', value: s.const };
  switch (s.type) {
    case 'string': return { k: 'string' };
    case 'boolean': return { k: 'bool' };
    case 'number': return { k: 'f64' };
    case 'integer': return { k: s.minimum !== undefined && s.minimum >= 0 ? 'u64' : 'i64' };
    case 'array': return { k: 'array', of: fieldType(s.items, owner, field) };
    case 'object':
      if (s.properties) fail(`${owner}.${field}: inline objects are not supported; use a $def`);
      return { k: 'map', of: fieldType(s.additionalProperties, owner, field) };
    default: fail(`${owner}.${field}: cannot map schema`);
  }
}

for (const [name, { schema }] of defs) {
  if (schema.enum) add({ name, kind: 'enum', values: schema.enum, doc: schema.description });
  else if (schema.oneOf) add({ name, kind: 'union', tag: schema['x-discriminator'], members: schema.oneOf.map((m) => m.__ref), doc: schema.description });
  else if (schema.type === 'object' && schema.properties) {
    const req = new Set(schema.required || []);
    const t = { name, kind: 'struct', doc: schema.description, fields: [] };
    add(t);
    for (const [k, v] of Object.entries(schema.properties)) {
      t.fields.push({ wire: k, type: fieldType(v, name, k), optional: !req.has(k), doc: v.description });
    }
  } else add({ name, kind: 'alias', type: fieldType(schema, name, ''), doc: schema.description });
}
model.sort((a, b) => a.name.localeCompare(b.name));
const isTag = (t, f) => unionOf.has(t.name) && unionOf.get(t.name).tag === f.wire;

const HEADER = [
  'GENERATED by contracts/files-drive/generate.mjs from contracts/files-drive/schema/*.json.',
  'Do not edit. Change the schema and run: node contracts/files-drive/generate.mjs',
];
const docLines = (doc, prefix) => (doc ? doc.split('\n').map((l) => `${prefix}${l}`) : []);
const jstr = (s) => JSON.stringify(s);

// ---------------------------------------------------------------- Rust

function rustTy(t) {
  switch (t.k) {
    case 'ref': return t.name;
    case 'const': case 'string': return 'String';
    case 'bool': return 'bool';
    case 'f64': return 'f64';
    case 'u64': return 'u64';
    case 'i64': return 'i64';
    case 'array': return `Vec<${rustTy(t.of)}>`;
    case 'map': return `std::collections::BTreeMap<String, ${rustTy(t.of)}>`;
  }
}
const RUST_KW = new Set(['type', 'ref', 'move', 'match', 'self', 'struct', 'enum', 'fn', 'impl', 'mod', 'use', 'where', 'loop', 'in', 'as', 'box']);
function rust() {
  const o = HEADER.map((l) => `// ${l}`);
  o.push('#![allow(clippy::all)]', '', 'use serde::{Deserialize, Serialize};', '');
  for (const t of model) {
    o.push(...docLines(t.doc, '/// '));
    if (t.kind === 'alias') o.push(`pub type ${t.name} = ${rustTy(t.type)};`);
    else if (t.kind === 'enum') {
      o.push('#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]', `pub enum ${t.name} {`);
      for (const v of t.values) o.push(`    #[serde(rename = ${jstr(v)})]`, `    ${pascal(v)},`);
      o.push('}', '', `impl ${t.name} {`, '    pub const ALL: &\'static [Self] = &[' + t.values.map((v) => `Self::${pascal(v)}`).join(', ') + '];',
        '    pub fn as_str(&self) -> &\'static str {', '        match self {',
        ...t.values.map((v) => `            Self::${pascal(v)} => ${jstr(v)},`), '        }', '    }', '}');
    } else if (t.kind === 'union') {
      o.push('#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]', `#[serde(tag = ${jstr(t.tag)})]`, `pub enum ${t.name} {`);
      for (const m of t.members) o.push(`    #[serde(rename = ${jstr(unionOf.get(m).value)})]`, `    ${m}(${m}),`);
      o.push('}');
    } else {
      if (unionOf.has(t.name)) o.push(`/// Variant of [\`${unionOf.get(t.name).union}\`]; the \`${unionOf.get(t.name).tag}\` field is carried by the enum.`);
      o.push('#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]', `pub struct ${t.name} {`);
      for (const f of t.fields) {
        if (isTag(t, f)) continue;
        o.push(...docLines(f.doc, '    /// '));
        const id = RUST_KW.has(f.wire) ? `r#${f.wire}` : f.wire;
        if (f.optional) o.push('    #[serde(default, skip_serializing_if = "Option::is_none")]', `    pub ${id}: Option<${rustTy(f.type)}>,`);
        else o.push(`    pub ${id}: ${rustTy(f.type)},`);
      }
      o.push('}');
    }
    o.push('');
  }
  return o.join('\n');
}

function rustTests() {
  const o = HEADER.map((l) => `// ${l}`);
  o.push('//! Every valid fixture must deserialize into its generated type and serialize back to the same JSON.', '',
    'use allternit_files_core::*;', '',
    'fn round_trip<T: serde::de::DeserializeOwned + serde::Serialize>(name: &str, raw: &str) {',
    '    let fx: serde_json::Value = serde_json::from_str(raw).expect(name);',
    '    let msg = fx["message"].clone();',
    '    let typed: T = serde_json::from_value(msg.clone()).unwrap_or_else(|e| panic!("{name}: {e}"));',
    '    let back = serde_json::to_value(&typed).unwrap();',
    '    assert_eq!(back, msg, "{name}: round trip changed the message");',
    '}', '', '#[test]', 'fn fixtures_round_trip() {');
  for (const fx of fixtures.filter((f) => f.valid)) {
    const ty = unionOf.has(fx.type) ? unionOf.get(fx.type).union : fx.type;
    o.push(`    round_trip::<${ty}>(${jstr(fx.file)}, include_str!("../../../../contracts/files-drive/fixtures/${fx.file}"));`);
  }
  o.push('}', '');
  return o.join('\n');
}

// ---------------------------------------------------------------- TypeScript

function tsTy(t) {
  switch (t.k) {
    case 'ref': return t.name;
    case 'const': return jstr(t.value);
    case 'string': return 'string';
    case 'bool': return 'boolean';
    case 'f64': case 'u64': case 'i64': return 'number';
    case 'array': return `${tsTy(t.of).includes('|') ? `(${tsTy(t.of)})` : tsTy(t.of)}[]`;
    case 'map': return `Record<string, ${tsTy(t.of)}>`;
  }
}
function ts() {
  const o = HEADER.map((l) => `// ${l}`);
  o.push('/* eslint-disable */', '');
  for (const t of model) {
    if (t.doc) o.push('/**', ...docLines(t.doc, ' * '), ' */');
    if (t.kind === 'alias') o.push(`export type ${t.name} = ${tsTy(t.type)};`);
    else if (t.kind === 'enum') {
      o.push(`export type ${t.name} = ${t.values.map(jstr).join(' | ')};`);
      o.push(`export const ${upperSnake(t.name)}_VALUES: readonly ${t.name}[] = [${t.values.map(jstr).join(', ')}];`);
    } else if (t.kind === 'union') o.push(`export type ${t.name} = ${t.members.join(' | ')};`);
    else {
      o.push(`export interface ${t.name} {`);
      for (const f of t.fields) {
        if (f.doc) o.push(`  /** ${f.doc.replace(/\*\//g, '* /')} */`);
        o.push(`  ${f.wire}${f.optional ? '?' : ''}: ${tsTy(f.type)};`);
      }
      o.push('}');
    }
    o.push('');
  }
  return o.join('\n');
}

// ---------------------------------------------------------------- Swift

const SWIFT_KW = new Set(['default', 'protocol', 'extension', 'case', 'in', 'is', 'as', 'class', 'func', 'import', 'let', 'var', 'return', 'self', 'static', 'struct', 'enum', 'internal', 'public', 'private', 'operator', 'repeat', 'where', 'while', 'for', 'if', 'else', 'switch', 'break', 'continue', 'do', 'try', 'throw', 'nil', 'true', 'false', 'super', 'init', 'Type', 'deinit', 'subscript', 'typealias', 'fallthrough', 'guard', 'defer', 'inout', 'associatedtype', 'rethrows', 'throws', 'Any']);
const sid = (n) => (SWIFT_KW.has(n) ? `\`${n}\`` : n);
function swTy(t) {
  switch (t.k) {
    case 'ref': return t.name;
    case 'const': case 'string': return 'String';
    case 'bool': return 'Bool';
    case 'f64': return 'Double';
    case 'u64': return 'UInt64';
    case 'i64': return 'Int64';
    case 'array': return `[${swTy(t.of)}]`;
    case 'map': return `[String: ${swTy(t.of)}]`;
  }
}
function swift() {
  const o = HEADER.map((l) => `// ${l}`);
  o.push('', 'import Foundation', '');
  for (const t of model) {
    o.push(...docLines(t.doc, '/// '));
    if (t.kind === 'alias') o.push(`public typealias ${t.name} = ${swTy(t.type)}`);
    else if (t.kind === 'enum') {
      o.push(`public enum ${t.name}: String, Codable, Hashable, Sendable, CaseIterable {`);
      for (const v of t.values) o.push(`    case ${sid(camel(v))} = ${jstr(v)}`);
      o.push('}');
    } else if (t.kind === 'union') {
      o.push(`public enum ${t.name}: Codable, Hashable, Sendable {`);
      for (const m of t.members) o.push(`    case ${sid(camel(m))}(${m})`);
      o.push('', '    private enum TagKey: String, CodingKey { case tag = ' + jstr(t.tag) + ' }', '',
        '    public init(from decoder: Decoder) throws {',
        '        let c = try decoder.container(keyedBy: TagKey.self)',
        '        let tag = try c.decode(String.self, forKey: .tag)',
        '        switch tag {');
      for (const m of t.members) o.push(`        case ${jstr(unionOf.get(m).value)}: self = .${sid(camel(m))}(try ${m}(from: decoder))`);
      o.push('        default: throw DecodingError.dataCorruptedError(forKey: .tag, in: c, debugDescription: "unknown ' + t.tag + ' \\(tag)")',
        '        }', '    }', '',
        '    public func encode(to encoder: Encoder) throws {',
        '        var c = encoder.container(keyedBy: TagKey.self)',
        '        switch self {');
      for (const m of t.members) o.push(`        case .${sid(camel(m))}(let v): try c.encode(${jstr(unionOf.get(m).value)}, forKey: .tag); try v.encode(to: encoder)`);
      o.push('        }', '    }', '}');
    } else {
      if (unionOf.has(t.name)) o.push(`/// Variant of \`${unionOf.get(t.name).union}\`; the "${unionOf.get(t.name).tag}" field is carried by the enum.`);
      const fs_ = t.fields.filter((f) => !isTag(t, f));
      o.push(`public struct ${t.name}: Codable, Hashable, Sendable {`);
      for (const f of fs_) {
        o.push(...docLines(f.doc, '    /// '));
        o.push(`    public var ${sid(camel(f.wire))}: ${swTy(f.type)}${f.optional ? '?' : ''}`);
      }
      o.push('', `    public init(${fs_.map((f) => `${sid(camel(f.wire))}: ${swTy(f.type)}${f.optional ? '? = nil' : ''}`).join(', ')}) {`);
      for (const f of fs_) o.push(`        self.${camel(f.wire)} = ${sid(camel(f.wire))}`);
      o.push('    }');
      if (fs_.length) {
        o.push('', '    enum CodingKeys: String, CodingKey {');
        for (const f of fs_) o.push(`        case ${sid(camel(f.wire))}${camel(f.wire) === f.wire ? '' : ` = ${jstr(f.wire)}`}`);
        o.push('    }');
      }
      o.push('}');
    }
    o.push('');
  }
  return o.join('\n');
}

// ---------------------------------------------------------------- Kotlin

const KT_KW = new Set(['as', 'break', 'class', 'continue', 'do', 'else', 'false', 'for', 'fun', 'if', 'in', 'interface', 'is', 'null', 'object', 'package', 'return', 'super', 'this', 'throw', 'true', 'try', 'typealias', 'typeof', 'val', 'var', 'when', 'while']);
const kid = (n) => (KT_KW.has(n) ? `\`${n}\`` : n);
function ktTy(t) {
  switch (t.k) {
    case 'ref': return t.name;
    case 'const': case 'string': return 'String';
    case 'bool': return 'Boolean';
    case 'f64': return 'Double';
    case 'u64': case 'i64': return 'Long';
    case 'array': return `List<${ktTy(t.of)}>`;
    case 'map': return `Map<String, ${ktTy(t.of)}>`;
  }
}
function kotlin() {
  const o = HEADER.map((l) => `// ${l}`);
  o.push('', 'package com.allternit.files.contract', '',
    '/** JSON field name on the wire (snake_case). Stdlib only: the Android app has no serialization library. */',
    '@Target(AnnotationTarget.VALUE_PARAMETER, AnnotationTarget.PROPERTY)',
    '@Retention(AnnotationRetention.RUNTIME)',
    'annotation class Wire(val name: String)', '');
  for (const t of model) {
    if (t.doc) o.push('/**', ...docLines(t.doc.replace(/\*\//g, '* /'), ' * '), ' */');
    if (t.kind === 'alias') o.push(`typealias ${t.name} = ${ktTy(t.type)}`);
    else if (t.kind === 'enum') {
      o.push(`enum class ${t.name}(val wire: String) {`);
      o.push(t.values.map((v) => `    ${upperSnake(v)}(${jstr(v)})`).join(',\n') + ';');
      o.push('', '    companion object {', `        fun fromWire(wire: String): ${t.name}? = values().firstOrNull { it.wire == wire }`, '    }', '}');
    } else if (t.kind === 'union') {
      o.push(`sealed interface ${t.name} {`, `    /** Value of the "${t.tag}" field. */`, '    val wireTag: String', '}');
    } else {
      const u = unionOf.get(t.name);
      const fs_ = t.fields.filter((f) => !isTag(t, f));
      o.push(`data class ${t.name}(`);
      o.push(fs_.map((f) => `    @Wire(${jstr(f.wire)}) val ${kid(camel(f.wire))}: ${ktTy(f.type)}${f.optional ? '? = null' : ''}`).join(',\n'));
      if (u) o.push(`) : ${u.union} {`, `    override val wireTag: String get() = ${jstr(u.value)}`, '}');
      else o.push(')');
    }
    o.push('');
  }
  return o.join('\n');
}

// ---------------------------------------------------------------- harness (scratch only)

if (args.has('--harness')) {
  const dir = path.resolve(argv[argv.indexOf('--harness') + 1] || fail('--harness needs a directory'));
  fs.mkdirSync(dir, { recursive: true });
  const valid = fixtures.filter((f) => f.valid);
  // TS: every valid fixture literal must type-check as its generated type.
  const tsLines = [`import type * as C from ${jstr(path.join(here, 'gen/files-drive.gen.js'))};`, ''];
  valid.forEach((fx, i) => {
    const ty = unionOf.has(fx.type) ? unionOf.get(fx.type).union : fx.type;
    tsLines.push(`export const f${i}: C.${ty} = ${JSON.stringify(fx.message)};`);
  });
  fs.writeFileSync(path.join(dir, 'fixtures.check.ts'), tsLines.join('\n') + '\n');
  // Swift: decode every valid fixture, encode it back, compare as JSON objects.
  const sw = ['import Foundation', '', 'func check<T: Codable>(_ t: T.Type, _ name: String, _ json: String) -> Bool {',
    '    let data = json.data(using: .utf8)!',
    '    do {',
    '        let v = try JSONDecoder().decode(T.self, from: data)',
    '        let back = try JSONEncoder().encode(v)',
    '        let a = try JSONSerialization.jsonObject(with: data) as! NSDictionary',
    '        let b = try JSONSerialization.jsonObject(with: back) as! NSDictionary',
    '        if a != b { print("FAIL \\(name): round trip changed the message"); return false }',
    '        return true',
    '    } catch { print("FAIL \\(name): \\(error)"); return false }',
    '}', '', 'var ok = 0', 'var bad = 0'];
  for (const fx of valid) {
    const ty = unionOf.has(fx.type) ? unionOf.get(fx.type).union : fx.type;
    sw.push(`if check(${ty}.self, ${jstr(fx.file)}, ${jstr(JSON.stringify(fx.message))}) { ok += 1 } else { bad += 1 }`);
  }
  sw.push('print("swift fixtures: \\(ok) round-tripped, \\(bad) failed")', 'exit(bad == 0 ? 0 : 1)');
  fs.writeFileSync(path.join(dir, 'main.swift'), sw.join('\n') + '\n');
  console.log(`harness written to ${dir}`);
  process.exit(0);
}

// ---------------------------------------------------------------- write / check

const outputs = [
  ['platform/types/allternit-files-core/src/generated.rs', rust()],
  ['platform/types/allternit-files-core/tests/fixtures.rs', rustTests()],
  ['contracts/files-drive/gen/files-drive.gen.ts', ts()],
  ['contracts/files-drive/gen/FilesDriveContract.gen.swift', swift()],
  ['contracts/files-drive/gen/FilesDriveContract.gen.kt', kotlin()],
];

let stale = 0;
for (const [rel, content] of outputs) {
  const p = path.join(repo, rel);
  if (args.has('--check')) {
    const cur = fs.existsSync(p) ? fs.readFileSync(p, 'utf8') : null;
    if (cur !== content) { stale++; console.error(`stale: ${rel}`); }
  } else {
    fs.mkdirSync(path.dirname(p), { recursive: true });
    fs.writeFileSync(p, content);
    console.log(`wrote ${rel}`);
  }
}
if (args.has('--check')) {
  if (stale) { console.error(`files-drive: ${stale} generated file(s) stale; run node contracts/files-drive/generate.mjs`); process.exit(1); }
  console.log('files-drive generated files are up to date');
}
