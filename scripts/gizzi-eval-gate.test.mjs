// Tests for scripts/gizzi-eval-gate.mjs: the committed baseline, the comparison, the exit codes and
// the CLI output parser, on crafted reports (no bun, no model). Run: node --test scripts/gizzi-eval-gate.test.mjs
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { evaluate, parseReportOutput, baselineFrom, parseArgs, DEFAULT_BASELINE } from './gizzi-eval-gate.mjs';

const SCRIPT = path.join(path.dirname(fileURLToPath(import.meta.url)), 'gizzi-eval-gate.mjs');
const baseline = JSON.parse(fs.readFileSync(DEFAULT_BASELINE, 'utf8'));
const IDS = ['case_shared_session_good', 'case_shared_session_bad', 'case_shared_session_guardrail'];

// What replay prints for the shared suite when every synthetic case passes.
const report = (failing = []) => {
  const per_case = IDS.map((id) => ({ id, pass: !failing.includes(id), passes: failing.includes(id) ? 0 : 1, attempts: 1 }));
  const passed = per_case.filter((c) => c.pass).length;
  return {
    schema: 'allternit.eval_report.v1',
    seed: 1,
    suites: {
      'shared.v1': {
        cases: 13, graded: 3, skipped: 10, passed,
        pass_rate: passed / 3, high_stakes_failures: [], failed_cases: failing,
      },
    },
    extra: { mode: 'replay', per_case },
  };
};

test('the committed baseline covers the 3 synthetic session cases', () => {
  assert.equal(baseline.schema, 'allternit.eval_baseline.v1');
  assert.equal(baseline.suites['shared.v1'].pass_rate, 1);
  assert.deepEqual(baseline.extra.per_case.map((c) => c.id), IDS);
});

test('a passing report passes the committed baseline', () => {
  const r = evaluate(report(), baseline);
  assert.equal(r.ok, true);
  assert.deepEqual(r.flips, []);
});

test('a planted failing case fails the gate and is named as flipped', () => {
  const r = evaluate(report(['case_shared_session_bad']), baseline);
  assert.equal(r.ok, false);
  assert.match(r.failures[0], /^shared\.v1: pass rate 66\.7% is 33\.3 pp below the baseline 100\.0%.*case_shared_session_bad/);
  assert.deepEqual(r.flips, [{ id: 'case_shared_session_bad', was: true, now: false }]);
});

test('a failed high-stakes case fails even at the baseline rate', () => {
  const rep = report();
  rep.suites['shared.v1'].high_stakes_failures = ['case_shared_session_good'];
  assert.equal(evaluate(rep, baseline).ok, false);
});

test('output parser finds the report after log lines and rejects output without one', () => {
  const text = `starting\n${JSON.stringify(report(), null, 2)}\n`;
  assert.equal(parseReportOutput(text).suites['shared.v1'].graded, 3);
  assert.throws(() => parseReportOutput('nothing here'), /no eval report/);
});

test('baselineFrom keeps per-case results and the previous max drop', () => {
  const b = baselineFrom(report(['case_shared_session_bad']), baseline);
  assert.equal(b.max_drop_pp, 2);
  assert.equal(b.suites['shared.v1'].graded, 3);
  assert.deepEqual(b.extra.per_case.find((c) => c.id === 'case_shared_session_bad'), { id: 'case_shared_session_bad', pass: false });
});

test('exit codes: 0 pass, 1 regression, 2 usage error', () => {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'gizzi-gate-'));
  const write = (name, obj) => {
    const f = path.join(dir, name);
    fs.writeFileSync(f, JSON.stringify(obj));
    return f;
  };
  const run = (...args) => spawnSync('node', [SCRIPT, ...args], { encoding: 'utf8' });
  const ok = run('--report', write('ok.json', report()));
  assert.equal(ok.status, 0, ok.stderr);
  assert.match(ok.stdout, /gizzi-eval-gate: PASS/);
  const bad = run('--report', write('bad.json', report(['case_shared_session_bad'])));
  assert.equal(bad.status, 1);
  assert.match(bad.stdout, /flipped: case_shared_session_bad passed before, fails now/);
  assert.match(bad.stderr, /FAIL: shared\.v1/);
  assert.equal(run('--nope').status, 2);
  assert.throws(() => parseArgs(['--max-drop-pp', '-1']), /non-negative/);
});

test('--write-baseline rewrites the baseline file from a report', () => {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'gizzi-gate-'));
  const rep = path.join(dir, 'r.json');
  const out = path.join(dir, 'b.json');
  fs.writeFileSync(rep, JSON.stringify(report(['case_shared_session_bad'])));
  const r = spawnSync('node', [SCRIPT, '--report', rep, '--baseline', out, '--write-baseline'], { encoding: 'utf8' });
  assert.equal(r.status, 0, r.stderr);
  assert.equal(JSON.parse(fs.readFileSync(out, 'utf8')).suites['shared.v1'].pass_rate, 2 / 3);
});
