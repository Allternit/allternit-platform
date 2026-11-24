#!/usr/bin/env node
/**
 * phone-map-bench: does the Phone App Map (go_to) + vision "auto" save work?
 *
 * Runs the same read-only Android tasks on one phone three ways and compares:
 *   1. always : vision "always", map forgotten first (baseline, today's behaviour)
 *   2. auto   : vision "auto",   map forgotten first
 *   3. map    : vision "auto",   map on. Each task runs twice; the 2nd run is
 *               measured, so the map built by the 1st run exists.
 * It reads steps, model_calls, tokens_in/out, map_steps, screenshots_sent,
 * fallbacks, status and duration from each task report and prints a table per
 * task plus totals per mode, with % change against mode 1.
 *
 * Contract: docs/contracts/CONTRACT-phone-app-map-2026-11-18.md sections 5, 6, 13.
 * Note: the task API has no flag that disables go_to. Modes 1 and 2 forget the
 * app's map first (DELETE /phones/maps/:app), so go_to is rejected with
 * "no map" for the run's first screens; the run still records a fresh map as it
 * goes. That is the "fresh map" caveat, and it is stated in the output.
 *
 * COST WARNING: a real run drives a real phone and SPENDS THE USER'S MODEL
 * TOKENS (10 tasks x 4 runs by default). It is not run by CI. The build train
 * or Eoj runs it (Gate 5). `--dry` touches nothing.
 *
 * Usage:
 *   node scripts/phone-map-bench.mjs --dry
 *   node scripts/phone-map-bench.mjs --serial <serial> [--limit N] [--yes]
 *        [--api http://localhost:8080] [--tasks file.json] [--json out.json]
 * Auth: ALLTERNIT_API_TOKEN (bearer) when the API needs one.
 * API URL: --api, else ALLTERNIT_API_URL, else http://localhost:8080 (the local
 * allternit-api default); routes live under /api/v1.
 * Completion: POST /phones/:serial/task answers with the finished report. If it
 * answers 202 {run_id} (background), the script polls
 * GET /phones/:serial/history?run_id=... until the run has a final status.
 * Node 20+, no dependencies.
 */
import fs from 'node:fs';
import path from 'node:path';
import readline from 'node:readline/promises';
import { fileURLToPath } from 'node:url';

const HERE = path.dirname(fileURLToPath(import.meta.url));
const MONEY = /pay|wallet|bank|venmo|cash|paypal|revolut|coinbase|robinhood|gpay|card|stripe|crypto/i;
const MODES = [
  { key: 'always', label: '1 always, map off', vision: 'always', forget: true, runs: 1 },
  { key: 'auto', label: '2 auto, map off', vision: 'auto', forget: true, runs: 1 },
  { key: 'map', label: '3 auto, map on', vision: 'auto', forget: false, runs: 2 },
];
const FIELDS = ['steps', 'model_calls', 'tokens_in', 'tokens_out', 'map_steps', 'screenshots_sent', 'fallbacks'];

function parseArgs(argv) {
  const o = { api: process.env.ALLTERNIT_API_URL || 'http://localhost:8080', tasks: path.join(HERE, 'phone-map-bench.tasks.json') };
  for (let i = 0; i < argv.length; i++) {
    const a = argv[i];
    if (a === '--dry') o.dry = true;
    else if (a === '--yes') o.yes = true;
    else if (['--serial', '--limit', '--api', '--tasks', '--json'].includes(a)) o[a.slice(2)] = argv[++i];
    else { console.error(`unknown argument: ${a}`); process.exit(2); }
  }
  if (o.limit !== undefined) o.limit = Number(o.limit);
  return o;
}

// ---- API ------------------------------------------------------------------
function makeApi(base) {
  const root = base.replace(/\/+$/, '').replace(/\/api\/v1$/, '') + '/api/v1';
  const headers = { 'content-type': 'application/json' };
  if (process.env.ALLTERNIT_API_TOKEN) headers.authorization = `Bearer ${process.env.ALLTERNIT_API_TOKEN}`;
  const call = async (method, p, body, timeoutMs = 600000) => {
    const res = await fetch(root + p, { method, headers, body: body ? JSON.stringify(body) : undefined, signal: AbortSignal.timeout(timeoutMs) });
    let json = null;
    try { json = await res.json(); } catch { /* empty body */ }
    return { status: res.status, json };
  };
  return { call };
}

async function forgetMaps(api, apps) {
  for (const app of apps) {
    if (MONEY.test(app)) throw new Error(`refusing money app: ${app}`);
    const r = await api.call('DELETE', `/phones/maps/${encodeURIComponent(app)}`, null, 30000);
    if (![200, 204, 404].includes(r.status)) console.warn(`  warn: forget map ${app} -> HTTP ${r.status}`);
  }
}

async function runTask(api, serial, task, vision) {
  const t0 = Date.now();
  const r = await api.call('POST', `/phones/${encodeURIComponent(serial)}/task`, { goal: task.goal, max_steps: task.max_steps ?? 15, vision });
  if (r.status === 400 && r.json?.error === 'invalid_args') throw new Error('API rejected vision (build without the App Map step 5?)');
  let rep = r.json || {};
  if (r.status === 202 && rep.run_id) {
    for (let i = 0; i < 300; i++) {
      await new Promise((ok) => setTimeout(ok, 2000));
      const h = await api.call('GET', `/phones/${encodeURIComponent(serial)}/history?run_id=${encodeURIComponent(rep.run_id)}`, null, 30000);
      const run = (h.json?.runs || []).find((x) => x.run_id === rep.run_id);
      if (run?.status && !['running', 'waiting_approval'].includes(run.status)) { rep = { ...run, ...rep, ...run }; break; }
    }
  }
  return normalize(rep, Date.now() - t0);
}

function normalize(rep, wallMs) {
  const steps = Array.isArray(rep.steps) ? rep.steps.length : Number(rep.steps ?? 0);
  const n = (k) => Number(rep[k] ?? 0);
  return {
    status: rep.status || (rep.ok ? 'done' : 'failed'),
    ok: rep.status ? rep.status === 'done' : !!rep.ok,
    steps, model_calls: n('model_calls'), tokens_in: n('tokens_in'), tokens_out: n('tokens_out'),
    map_steps: n('map_steps'), screenshots_sent: n('screenshots_sent'), fallbacks: n('fallbacks'),
    duration_ms: Number(rep.duration_ms ?? wallMs),
  };
}

// ---- Fake data for --dry --------------------------------------------------
function fakeReports(tasks) {
  const out = {};
  tasks.forEach((t, i) => {
    const w = 4 + (i % 4);
    const mk = (steps, calls, ti, to, ms, shots, fb, ok = true) => ({
      status: ok ? 'done' : 'failed', ok, steps, model_calls: calls, tokens_in: ti, tokens_out: to,
      map_steps: ms, screenshots_sent: shots, fallbacks: fb, duration_ms: steps * 3400 + calls * 1500,
    });
    out[t.id] = {
      always: mk(w, w, 9000 * w, 160 * w, 0, w, 0),
      auto: mk(w, w, 3400 * w, 150 * w, 0, 1, 0, i !== 6),
      map: mk(w, Math.max(1, Math.floor(w / 3)), 3000 * Math.max(1, Math.floor(w / 3)), 140, w - 1, 0, i === 3 ? 1 : 0),
    };
  });
  return out;
}

// ---- Report ---------------------------------------------------------------
const pad = (s, n) => String(s).padEnd(n);
const rpad = (s, n) => String(s).padStart(n);
const avg = (a) => (a.length ? a.reduce((x, y) => x + y, 0) / a.length : 0);
const pct = (v, base) => (base ? `${v - base >= 0 ? '+' : ''}${Math.round(((v - base) / base) * 100)}%` : 'n/a');
const secs = (ms) => (ms / 1000).toFixed(1) + 's';

function totals(results, tasks, key) {
  const rows = tasks.map((t) => results[t.id]?.[key]).filter(Boolean);
  const t = { n: rows.length, success: rows.length ? rows.filter((r) => r.ok).length / rows.length : 0 };
  for (const f of FIELDS) t[f] = avg(rows.map((r) => r[f]));
  t.tokens = t.tokens_in + t.tokens_out;
  t.duration_ms = avg(rows.map((r) => r.duration_ms));
  return t;
}

function printReport(results, tasks, meta) {
  console.log(`\nphone-map-bench ${meta.dry ? '(DRY: built-in fake reports, nothing touched)' : `serial=${meta.serial}`}`);
  console.log('Modes: ' + MODES.map((m) => m.label).join(' | '));
  console.log('Note: maps-off modes forget the app map first; the API has no go_to disable flag, so a fresh map is built during those runs.\n');
  console.log(pad('task', 28) + MODES.map((m) => pad(m.key + ' (ok steps calls tok shots map fb time)', 46)).join(''));
  for (const t of tasks) {
    const cells = MODES.map((m) => {
      const r = results[t.id]?.[m.key];
      if (!r) return pad('-', 46);
      return pad(`${r.ok ? 'ok ' : 'FAIL'} ${rpad(r.steps, 3)} ${rpad(r.model_calls, 3)} ${rpad(r.tokens_in + r.tokens_out, 7)} ${rpad(r.screenshots_sent, 3)} ${rpad(r.map_steps, 3)} ${rpad(r.fallbacks, 2)} ${rpad(secs(r.duration_ms), 7)}`, 46);
    });
    console.log(pad(t.id, 28) + cells.join(''));
  }
  const tot = Object.fromEntries(MODES.map((m) => [m.key, totals(results, tasks, m.key)]));
  const base = tot.always;
  console.log('\nTotals per mode (averages per task; % change vs mode 1)');
  console.log(pad('mode', 20) + ['success', 'steps', 'time', 'tokens', 'model calls', 'screenshots', 'map steps', 'fallbacks'].map((h) => rpad(h, 14)).join(''));
  for (const m of MODES) {
    const t = tot[m.key];
    const d = m.key === 'always';
    const cell = (v, b, fmt) => rpad(d ? fmt(v) : `${fmt(v)} ${pct(v, b)}`, 14);
    console.log(pad(m.label, 20)
      + rpad(`${Math.round(t.success * 100)}%`, 14)
      + cell(t.steps, base.steps, (v) => v.toFixed(1))
      + cell(t.duration_ms, base.duration_ms, secs)
      + cell(t.tokens, base.tokens, (v) => Math.round(v))
      + cell(t.model_calls, base.model_calls, (v) => v.toFixed(1))
      + cell(t.screenshots_sent, base.screenshots_sent, (v) => v.toFixed(1))
      + rpad(t.map_steps.toFixed(1), 14) + rpad(t.fallbacks.toFixed(1), 14));
  }
  return tot;
}

// ---- Main -----------------------------------------------------------------
async function main() {
  const opts = parseArgs(process.argv.slice(2));
  if (!opts.dry && !opts.serial) { console.error('refusing to run without --serial (use --dry to print a fake table)'); process.exit(2); }
  let tasks = JSON.parse(fs.readFileSync(opts.tasks, 'utf8')).tasks;
  if (opts.limit > 0) tasks = tasks.slice(0, opts.limit);
  const bad = tasks.find((t) => (t.apps || []).some((a) => MONEY.test(a)) || MONEY.test(t.goal));
  if (bad) { console.error(`refusing: task ${bad.id} touches a money app`); process.exit(2); }

  let results = {};
  if (opts.dry) {
    results = fakeReports(tasks);
  } else {
    console.log(`Tasks (${tasks.length}) on ${opts.serial} via ${opts.api}:`);
    tasks.forEach((t, i) => console.log(`  ${i + 1}. [${t.id}] ${t.goal}`));
    console.log(`This drives the phone and spends your model tokens (${tasks.length} x 4 runs).`);
    if (!opts.yes) {
      const rl = readline.createInterface({ input: process.stdin, output: process.stdout });
      const ans = (await rl.question('Run? type "y" to continue: ')).trim().toLowerCase();
      rl.close();
      if (ans !== 'y') { console.log('aborted'); process.exit(1); }
    }
    const api = makeApi(opts.api);
    for (const mode of MODES) {
      console.log(`\n== mode ${mode.label}`);
      for (const t of tasks) {
        if (mode.forget) await forgetMaps(api, t.apps || []);
        let rep;
        for (let k = 0; k < mode.runs; k++) rep = await runTask(api, opts.serial, t, mode.vision);
        (results[t.id] ||= {})[mode.key] = rep;
        console.log(`  ${t.id}: ${rep.status} steps=${rep.steps} calls=${rep.model_calls} map=${rep.map_steps}`);
      }
    }
  }
  const tot = printReport(results, tasks, { dry: opts.dry, serial: opts.serial });
  if (opts.json) {
    fs.writeFileSync(opts.json, JSON.stringify({ dry: !!opts.dry, serial: opts.serial || null, modes: MODES, tasks: tasks.map((t) => t.id), results, totals: tot }, null, 2));
    console.log(`\nwrote ${opts.json}`);
  }
}
main().catch((e) => { console.error(e.message); process.exit(1); });
