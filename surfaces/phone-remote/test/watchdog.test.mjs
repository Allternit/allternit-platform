#!/usr/bin/env node
// watchdog.test.mjs — frame-freshness watchdog + honest /hello.
//   Part A: isStale() pure judgment (no screen needed).
//   Part B: FrameWatchdog timer behavior with a tiny real window.
//   Part C: helloBody() with a mocked capture — stale/error fields.
//   Part D: live server on a throwaway port (--capture none) → /hello shape.
// Run: node test/watchdog.test.mjs
import { get } from 'node:http';
import { spawn } from 'node:child_process';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import assert from 'node:assert/strict';

import { isStale, FrameWatchdog } from '../server/lib/watchdog.mjs';
import { helloBody } from '../server/lib/status.mjs';
// Wait until a spawned server answers /healthz (a fixed sleep flaked on a loaded machine: startup can take >2 s).
async function waitHealthy(port, ms = 15000) {
  const t0 = Date.now();
  while (Date.now() - t0 < ms) {
    const ok = await new Promise((r) => { const q = get({ host: '127.0.0.1', port, path: '/healthz' }, (res) => { res.resume(); r(res.statusCode === 200); }); q.on('error', () => r(false)); });
    if (ok) return;
    await new Promise((r) => setTimeout(r, 150));
  }
  throw new Error(`server on :${port} never became healthy`);
}


const HERE = dirname(fileURLToPath(import.meta.url));
let passed = 0;
function ok(name) { passed++; console.log(`  ok ${passed} — ${name}`); }
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

// ── Part A: isStale ──────────────────────────────────────────────────────────

function partA() {
  console.log('Part A — isStale() freshness judgment');
  assert.equal(isStale(null, 10000, 5000), false, 'never-framed is "not started", not stale');
  ok('null lastFrameAt → not stale');
  assert.equal(isStale(4000, 8000, 5000), false, 'inside the window');
  ok('4s silence vs 5s window → fresh');
  assert.equal(isStale(2000, 8000, 5000), true, 'past the window');
  ok('6s silence vs 5s window → stale');
  assert.equal(isStale(2999, 8000, 5000), true, 'boundary is exclusive');
  ok('window boundary is exclusive (now-last > staleMs)');
}

// ── Part B: FrameWatchdog ────────────────────────────────────────────────────
// Driven with an injected fake scheduler so deadlines are deterministic —
// this machine's real timers are too coarse for margin-based timing tests.

function partB() {
  console.log('Part B — FrameWatchdog (injected scheduler)');
  const events = [];
  let scheduled = null; // latest pending deadline callback
  const wd = new FrameWatchdog({
    staleMs: 5000,
    log: () => {},
    onStale: () => events.push('stale'),
    schedule: (fn) => { scheduled = fn; return fn; },
    cancel: () => { scheduled = null; },
  });

  wd.arm();
  assert.equal(wd.armed, true);
  assert.equal(typeof scheduled, 'function', 'arm schedules a deadline');
  ok('arm() arms + schedules a deadline');

  const firstDeadline = scheduled;
  wd.reset();
  const secondDeadline = scheduled;
  assert.notEqual(firstDeadline, secondDeadline, 'reset reschedules a fresh deadline');
  ok('reset() reschedules the deadline');

  firstDeadline(); // superseded deadline fires late (coarse timers) — must be ignored
  assert.equal(events.length, 0, 'a superseded deadline must not trip the watchdog');
  ok('superseded deadline cannot fire a phantom trip');

  secondDeadline(); // fire the real (rescheduled) deadline
  assert.deepEqual(events, ['stale'], 'fires exactly once after the silence window');
  ok('deadline past with no frames → onStale fires once');
  assert.equal(wd.armed, false, 'watchdog is one-shot per arming');
  ok('trip disarms (one-shot until re-armed)');

  wd.arm();
  wd.disarm();
  assert.equal(scheduled, null, 'disarm cancels the pending deadline');
  ok('disarm() cancels the pending deadline');
}

// ── Part C: helloBody with a mocked capture ──────────────────────────────────

function partC() {
  console.log('Part C — /hello body (mocked capture)');

  const alive = helloBody({
    capture: { actualMode: 'sckit', lastInfo: { width: 1440, height: 810 }, lastFrame: Buffer.from([1, 2]), dead: null },
    captureMode: 'sckit', fps: 10,
    input: null, inputDryRun: false, display: null,
  });
  assert.equal(alive.hasFrame, true);
  assert.equal(alive.stale, false);
  assert.equal('error' in alive, false, 'living capture carries no error field');
  assert.equal(alive.capture.mode, 'sckit');
  assert.equal(alive.capture.width, 1440);
  ok('living capture → hasFrame true, stale false, no error field, fields unchanged');

  const frozen = helloBody({
    capture: { actualMode: 'sckit', lastInfo: { width: 1440, height: 810 }, lastFrame: Buffer.from([1, 2]), dead: 'frozen: no frame for >5000ms (sckit)' },
    captureMode: 'sckit', fps: 10,
    input: null, inputDryRun: false, display: null,
  });
  assert.equal(frozen.hasFrame, true, 'hasFrame still reports the cached frame');
  assert.equal(frozen.stale, true);
  assert.match(frozen.error, /frozen/);
  ok('dead capture → stale true + error string; hasFrame stays honest (lastFrame still cached)');

  const noCapture = helloBody({
    capture: null, captureMode: 'none', fps: 10,
    input: null, inputDryRun: false, display: null,
  });
  assert.equal(noCapture.stale, false);
  assert.equal(noCapture.hasFrame, false);
  assert.equal(noCapture.capture.mode, 'none');
  ok('--capture none → stale false, capture mode falls back to cfg');
}

// ── Part D: live server on a throwaway port ──────────────────────────────────

function httpGet(port, path) {
  return new Promise((resolve, reject) => {
    const req = get({ host: '127.0.0.1', port, path }, (res) => {
      let body = '';
      res.on('data', (d) => (body += d));
      res.on('end', () => resolve({ status: res.statusCode, body }));
    });
    req.on('error', reject);
  });
}

async function partD() {
  console.log('Part D — live server /hello (capture none, throwaway port)');
  const port = 18479;
  const child = spawn('node', [join(HERE, '..', 'server', 'index.mjs'),
    '--port', String(port), '--bind', '127.0.0.1', '--token', 'wd-test-token',
    '--capture', 'none', '--no-input'], { stdio: ['ignore', 'pipe', 'pipe'] });
  let stderr = '';
  child.stderr.on('data', (d) => (stderr += d));
  await waitHealthy(port);

  try {
    const hello = await httpGet(port, '/hello');
    assert.equal(hello.status, 200);
    const body = JSON.parse(hello.body);
    assert.equal(body.stale, false);
    assert.equal('error' in body, false);
    ok('loopback GET /hello on live server → stale false, no error field');

    const healthz = await httpGet(port, '/healthz');
    assert.equal(healthz.status, 200);
    ok('/healthz answers on the throwaway port');
  } finally {
    child.kill('SIGTERM');
    // up to 5 s on a loaded machine; a SIGTERM that lands before the handler is installed ends it by signal (exitCode stays null)
    for (let i = 0; i < 50 && child.exitCode === null && child.signalCode === null; i++) await sleep(100);
    assert.ok(child.exitCode !== null || child.signalCode !== null, 'server exited on SIGTERM');
    ok('server stops cleanly on SIGTERM');
  }
}

partA();
partB();
partC();
await partD();
console.log(`\n${passed} checks passed`);
process.exit(0);
