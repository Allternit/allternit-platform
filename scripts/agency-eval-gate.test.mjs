// Tests for scripts/agency-eval-gate.mjs: the comparison rules and the exit
// codes, on crafted reports (no cargo build). Run: node --test scripts/agency-eval-gate.test.mjs
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { compareReports, baselineFrom, parseArgs, DEFAULT_BASELINE } from './agency-eval-gate.mjs';

const SCRIPT = path.join(path.dirname(fileURLToPath(import.meta.url)), 'agency-eval-gate.mjs');

const baseline = {
  schema: 'allternit.eval_baseline.v1',
  max_drop_pp: 2,
  suites: { a: { pass_rate: 1.0 }, b: { pass_rate: 0.9 } },
};
const report = (a, b, extra = {}) => ({
  schema: 'allternit.eval_report.v1',
  suites: {
    a: { pass_rate: a, graded: 50, passed: Math.round(a * 50), high_stakes_failures: [], ...(extra.a ?? {}) },
    b: { pass_rate: b, graded: 50, passed: Math.round(b * 50), high_stakes_failures: [], ...(extra.b ?? {}) },
  },
});

test('equal pass rates pass', () => {
  assert.equal(compareReports(report(1.0, 0.9), baseline).ok, true);
});

test('a drop within 2 pp passes, above fails', () => {
  assert.equal(compareReports(report(0.98, 0.9), baseline).ok, true);
  const r = compareReports(report(0.96, 0.9), baseline);
  assert.equal(r.ok, false);
  assert.match(r.failures[0], /^a: .*4\.0 pp below/);
  assert.equal(compareReports(report(0.96, 0.9), baseline, { maxDropPp: 5 }).ok, true);
});

test('any high-stakes failure fails even at baseline rate', () => {
  const r = compareReports(report(1.0, 0.9, { b: { high_stakes_failures: ['no_write'] } }), baseline);
  assert.equal(r.ok, false);
  assert.match(r.failures[0], /high-stakes .* no_write/);
});

test('missing or ungraded suites fail; new suites are notes', () => {
  const rep = report(1.0, 0.9);
  delete rep.suites.b;
  rep.suites.c = { pass_rate: 0.5, graded: 2 };
  const r = compareReports(rep, baseline);
  assert.equal(r.ok, false);
  assert.match(r.failures.join('\n'), /b: missing/);
  assert.match(r.notes.join('\n'), /c: not in the baseline/);
  const un = report(1.0, null);
  assert.match(compareReports(un, baseline).failures.join('\n'), /b: nothing graded/);
});

test('schemas are checked; baseline can be rebuilt from a report', () => {
  assert.throws(() => compareReports({ schema: 'x' }, baseline));
  const b = baselineFrom(report(1.0, 0.8), baseline);
  assert.deepEqual(b.suites.b, { pass_rate: 0.8, graded: 50 });
  assert.equal(b.max_drop_pp, 2);
  assert.equal(parseArgs([]).baseline, DEFAULT_BASELINE);
  assert.throws(() => parseArgs(['--max-drop-pp', '-1']));
});

function runGate(rep, base, args = []) {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'gate-test-'));
  const rf = path.join(dir, 'report.json');
  const bf = path.join(dir, 'baseline.json');
  fs.writeFileSync(rf, JSON.stringify(rep));
  fs.writeFileSync(bf, JSON.stringify(base));
  return spawnSync(process.execPath, [SCRIPT, '--report', rf, '--baseline', bf, ...args], { encoding: 'utf8' });
}

test('exit codes: 0 pass, 1 regression, 2 usage error', () => {
  assert.equal(runGate(report(1.0, 0.9), baseline).status, 0);
  const fail = runGate(report(0.9, 0.9), baseline);
  assert.equal(fail.status, 1);
  assert.match(fail.stderr, /FAIL: a:/);
  assert.equal(runGate(report(1.0, 0.9, { a: { high_stakes_failures: ['x'] } }), baseline).status, 1);
  assert.equal(runGate(report(1.0, 0.9), baseline, ['--nope']).status, 2);
  assert.equal(runGate({ schema: 'wrong' }, baseline).status, 2);
});

test('the committed baseline is well formed', () => {
  const b = JSON.parse(fs.readFileSync(DEFAULT_BASELINE, 'utf8'));
  assert.equal(b.schema, 'allternit.eval_baseline.v1');
  assert.ok(Object.keys(b.suites).length >= 7);
  for (const s of Object.values(b.suites)) assert.ok(s.pass_rate >= 0 && s.pass_rate <= 1);
});
