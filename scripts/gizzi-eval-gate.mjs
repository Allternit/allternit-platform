#!/usr/bin/env node
// Gizzi eval replay gate (eval loop 2.11).
//
// Runs the shared suite in replay mode (no model, no network, no secrets) by running the gizzi CLI
// from source, then compares the report with the committed baseline using compareReports from
// scripts/agency-eval-gate.mjs:
//
//   - a suite's pass rate dropping more than `max_drop_pp` (default 2) below the baseline fails;
//   - any failed high-stakes case fails;
//   - a baseline suite missing from the report, or with nothing graded, fails.
//
// Replay grades stored traces. It tests the grading machinery and the suite, not prompt changes.
//
// Usage:
//   node scripts/gizzi-eval-gate.mjs                    # run replay + compare
//   node scripts/gizzi-eval-gate.mjs --report r.json    # compare an existing report
//   node scripts/gizzi-eval-gate.mjs --write-baseline   # refresh the baseline from a run
// Options: --baseline <file>, --suite-dir <dir>, --max-drop-pp <n>.
// Exit codes: 0 pass, 1 regression, 2 usage or infrastructure error.

import { spawnSync } from 'node:child_process';
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';
import { compareReports, baselineFrom as agencyBaselineFrom, REPORT_SCHEMA } from './agency-eval-gate.mjs';

const ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
export const DEFAULT_SUITE_DIR = path.join(ROOT, 'domains/agent-evals/gizzi/suites/shared.v1');
export const DEFAULT_BASELINE = path.join(ROOT, 'domains/agent-evals/gizzi/baseline.replay.v1.json');
const DEV = path.join(ROOT, 'cmd/gizzi-code/script/dev.mjs');

export function parseArgs(argv) {
  const opts = { report: null, baseline: DEFAULT_BASELINE, suiteDir: DEFAULT_SUITE_DIR, maxDropPp: null, writeBaseline: false };
  for (let i = 0; i < argv.length; i++) {
    const a = argv[i];
    const val = () => {
      const v = argv[++i];
      if (v === undefined) throw new Error(`${a} needs a value`);
      return v;
    };
    if (a === '--report') opts.report = val();
    else if (a === '--baseline') opts.baseline = val();
    else if (a === '--suite-dir') opts.suiteDir = val();
    else if (a === '--max-drop-pp') {
      opts.maxDropPp = Number(val());
      if (!Number.isFinite(opts.maxDropPp) || opts.maxDropPp < 0) throw new Error('--max-drop-pp must be a non-negative number');
    } else if (a === '--write-baseline') opts.writeBaseline = true;
    else throw new Error(`unknown argument ${a}`);
  }
  return opts;
}

/** The report JSON inside CLI output. The CLI prints one pretty JSON document; log lines may come before it. */
export function parseReportOutput(text) {
  const at = text.indexOf(`{\n  "schema": "${REPORT_SCHEMA}"`);
  if (at < 0) throw new Error('no eval report in the CLI output');
  return JSON.parse(text.slice(at));
}

/** Cases that passed in the baseline and fail now (or the other way). Needs per_case on both sides. */
export function flippedCases(report, baseline) {
  const was = new Map((baseline?.extra?.per_case ?? []).map((r) => [r.id, !!r.pass]));
  const out = [];
  for (const r of report?.extra?.per_case ?? []) {
    if (was.has(r.id) && was.get(r.id) !== !!r.pass) out.push({ id: r.id, was: was.get(r.id), now: !!r.pass });
  }
  return out;
}

export function baselineFrom(report, previous) {
  const base = agencyBaselineFrom(report, previous);
  base.description = previous?.description ?? 'Replay pass-rate baseline for scripts/gizzi-eval-gate.mjs.';
  base.extra = { per_case: (report.extra?.per_case ?? []).map((r) => ({ id: r.id, pass: !!r.pass })) };
  return base;
}

export function evaluate(report, baseline, { maxDropPp } = {}) {
  const res = compareReports(report, baseline, { maxDropPp: maxDropPp ?? undefined });
  return { ...res, flips: flippedCases(report, baseline) };
}

function runReplay(suiteDir) {
  const r = spawnSync('bun', [DEV, 'eval', 'run', '--mode', 'replay', '--suite-dir', suiteDir, '--json'], {
    cwd: ROOT,
    encoding: 'utf8',
    maxBuffer: 64 * 1024 * 1024,
    stdio: ['ignore', 'pipe', 'inherit'],
  });
  if (r.error) throw r.error;
  return parseReportOutput(r.stdout ?? '');
}

export function main(argv) {
  try {
    const opts = parseArgs(argv);
    const report = opts.report ? JSON.parse(fs.readFileSync(opts.report, 'utf8')) : runReplay(opts.suiteDir);
    const previous = fs.existsSync(opts.baseline) ? JSON.parse(fs.readFileSync(opts.baseline, 'utf8')) : undefined;
    if (opts.writeBaseline) {
      fs.writeFileSync(opts.baseline, JSON.stringify(baselineFrom(report, previous), null, 2) + '\n');
      console.log(`gizzi-eval-gate: wrote ${opts.baseline}`);
      return 0;
    }
    if (!previous) throw new Error(`baseline ${opts.baseline} not found`);
    const res = evaluate(report, previous, { maxDropPp: opts.maxDropPp });
    for (const [name, s] of Object.entries(report.suites ?? {})) {
      console.log(`  ${name}: ${s.passed}/${s.graded} passed (${s.pass_rate ?? 'n/a'}), ${s.skipped ?? 0} skipped`);
    }
    for (const n of res.notes) console.log(`note: ${n}`);
    for (const f of res.flips) console.log(`flipped: ${f.id} ${f.was ? 'passed' : 'failed'} before, ${f.now ? 'passes' : 'fails'} now`);
    for (const f of res.failures) console.error(`FAIL: ${f}`);
    console.log(res.ok ? 'gizzi-eval-gate: PASS' : 'gizzi-eval-gate: FAIL');
    return res.ok ? 0 : 1;
  } catch (e) {
    console.error(`gizzi-eval-gate: ${e.message}`);
    return 2;
  }
}

if (process.argv[1] && import.meta.url === pathToFileURL(path.resolve(process.argv[1])).href) {
  process.exitCode = main(process.argv.slice(2));
}
