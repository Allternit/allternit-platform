#!/usr/bin/env node
// Agency eval regression gate (C5).
//
// Runs the shipped task-type eval sets and the committed eval suites offline
// (scripted runner: no model, no effect, no secrets) through the cargo test
// `agency_api::evals::tests::eval_gate_offline_report`, which writes the
// report JSON, then compares it with the committed baseline:
//
//   - a suite's pass rate dropping more than `max_drop_pp` percentage points
//     (default 2) below its baseline fails the gate;
//   - any failed high-stakes case (all-of-k) fails the gate;
//   - a baseline suite missing from the report (or with nothing graded) fails.
//
// Usage:
//   node scripts/agency-eval-gate.mjs                       # build + run + compare
//   node scripts/agency-eval-gate.mjs --report r.json       # compare an existing report
//   node scripts/agency-eval-gate.mjs --write-baseline      # refresh the baseline from a run
// Options: --baseline <file> (default: the committed baseline), --max-drop-pp <n>.
// Exit codes: 0 pass, 1 regression, 2 usage or infrastructure error.

import { spawnSync } from 'node:child_process';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';

const ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
export const DEFAULT_BASELINE = path.join(ROOT, 'cmd/allternit-api/src/agency_api/evals/baseline.v1.json');
export const REPORT_SCHEMA = 'allternit.eval_report.v1';
export const BASELINE_SCHEMA = 'allternit.eval_baseline.v1';
export const TEST_NAME = 'agency_api::evals::tests::eval_gate_offline_report';

export function parseArgs(argv) {
  const opts = { report: null, baseline: DEFAULT_BASELINE, maxDropPp: null, writeBaseline: false };
  for (let i = 0; i < argv.length; i++) {
    const a = argv[i];
    const val = () => {
      const v = argv[++i];
      if (v === undefined) throw new Error(`${a} needs a value`);
      return v;
    };
    if (a === '--report') opts.report = val();
    else if (a === '--baseline') opts.baseline = val();
    else if (a === '--max-drop-pp') {
      opts.maxDropPp = Number(val());
      if (!Number.isFinite(opts.maxDropPp) || opts.maxDropPp < 0) throw new Error('--max-drop-pp must be a non-negative number');
    } else if (a === '--write-baseline') opts.writeBaseline = true;
    else throw new Error(`unknown argument ${a}`);
  }
  return opts;
}

/** Compare a report with a baseline. Pure: returns { ok, failures, notes }. */
export function compareReports(report, baseline, { maxDropPp } = {}) {
  if (report?.schema !== REPORT_SCHEMA) throw new Error(`report schema must be ${REPORT_SCHEMA}`);
  if (baseline?.schema !== BASELINE_SCHEMA) throw new Error(`baseline schema must be ${BASELINE_SCHEMA}`);
  const limit = maxDropPp ?? baseline.max_drop_pp ?? 2;
  const failures = [];
  const notes = [];
  const suites = report.suites ?? {};
  for (const [name, base] of Object.entries(baseline.suites ?? {})) {
    const cur = suites[name];
    if (!cur) {
      failures.push(`${name}: missing from the report`);
      continue;
    }
    if (typeof cur.pass_rate !== 'number') {
      failures.push(`${name}: nothing graded`);
      continue;
    }
    const drop = (base.pass_rate - cur.pass_rate) * 100;
    if (drop > limit + 1e-9) {
      failures.push(`${name}: pass rate ${(cur.pass_rate * 100).toFixed(1)}% is ${drop.toFixed(1)} pp below the baseline ${(base.pass_rate * 100).toFixed(1)}% (allowed ${limit} pp); failed: ${(cur.failed_cases ?? []).join(', ') || 'n/a'}`);
    } else if (cur.pass_rate > base.pass_rate + 1e-9) {
      notes.push(`${name}: pass rate improved to ${(cur.pass_rate * 100).toFixed(1)}% (refresh the baseline with --write-baseline)`);
    }
  }
  for (const [name, cur] of Object.entries(suites)) {
    const high = cur.high_stakes_failures ?? [];
    if (high.length > 0) failures.push(`${name}: high-stakes (all-of-k) case(s) failed: ${high.join(', ')}`);
    if (!(name in (baseline.suites ?? {}))) notes.push(`${name}: not in the baseline (not gated on pass rate yet)`);
  }
  return { ok: failures.length === 0, failures, notes };
}

export function baselineFrom(report, previous) {
  const suites = {};
  for (const [name, cur] of Object.entries(report.suites ?? {})) {
    if (typeof cur.pass_rate === 'number') suites[name] = { pass_rate: cur.pass_rate, graded: cur.graded };
  }
  return {
    schema: BASELINE_SCHEMA,
    description: previous?.description ?? 'Pass-rate baseline for scripts/agency-eval-gate.mjs.',
    max_drop_pp: previous?.max_drop_pp ?? 2.0,
    suites,
  };
}

function runCargoReport() {
  const out = path.join(fs.mkdtempSync(path.join(os.tmpdir(), 'agency-eval-')), 'report.json');
  const r = spawnSync('cargo', ['test', '-p', 'allternit-api', '--lib', TEST_NAME, '--', '--exact', '--nocapture'], {
    cwd: ROOT,
    stdio: ['ignore', 'inherit', 'inherit'],
    env: { ...process.env, AGENCY_EVAL_REPORT: out, ALLTERNIT_AGENCY_COGNITION: 'scripted' },
  });
  if (r.status !== 0 && !fs.existsSync(out)) throw new Error(`cargo test exited ${r.status ?? r.signal} without a report`);
  return out;
}

export function main(argv) {
  let opts;
  try {
    opts = parseArgs(argv);
  } catch (e) {
    console.error(`agency-eval-gate: ${e.message}`);
    return 2;
  }
  let report;
  let baseline;
  try {
    const file = opts.report ?? runCargoReport();
    report = JSON.parse(fs.readFileSync(file, 'utf8'));
    baseline = fs.existsSync(opts.baseline) ? JSON.parse(fs.readFileSync(opts.baseline, 'utf8')) : null;
    if (opts.writeBaseline) {
      fs.writeFileSync(opts.baseline, `${JSON.stringify(baselineFrom(report, baseline), null, 2)}\n`);
      console.log(`agency-eval-gate: baseline written to ${opts.baseline}`);
      return 0;
    }
    if (!baseline) throw new Error(`baseline ${opts.baseline} not found`);
    const res = compareReports(report, baseline, { maxDropPp: opts.maxDropPp ?? undefined });
    for (const [name, s] of Object.entries(report.suites ?? {})) {
      const ci = s.ci95 ? ` [95% CI ${s.ci95.low}-${s.ci95.high}]` : '';
      console.log(`  ${name}: ${s.passed}/${s.graded} passed (${s.pass_rate ?? 'n/a'})${ci}`);
    }
    for (const n of res.notes) console.log(`note: ${n}`);
    for (const f of res.failures) console.error(`FAIL: ${f}`);
    console.log(res.ok ? 'agency-eval-gate: PASS' : 'agency-eval-gate: FAIL');
    return res.ok ? 0 : 1;
  } catch (e) {
    console.error(`agency-eval-gate: ${e.message}`);
    return 2;
  }
}

if (process.argv[1] && import.meta.url === pathToFileURL(path.resolve(process.argv[1])).href) {
  process.exitCode = main(process.argv.slice(2));
}
