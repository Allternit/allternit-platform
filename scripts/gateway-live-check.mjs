#!/usr/bin/env node
// Live Agent Gateway check: real turns through a vendor-bound bot, end to end
// (Bots thread -> allternit-api -> Sessions gateway -> vendor). Encodes the
// failures found by hand on 2026-09-30/10-01 so they can't come back silently.
//
//   node scripts/gateway-live-check.mjs --bot <botId> [--turns 2] [--api http://127.0.0.1:8013]
//
// Auth: against the local Desktop API it reads the running allternit-api's
// desktop access token (same as the Settings UI). Each turn spends one real
// message on the bound vendor account, so keep --turns small.
import { execSync } from 'node:child_process';

const arg = (k, d) => { const i = process.argv.indexOf(`--${k}`); return i > 0 ? process.argv[i + 1] : d; };
const API = arg('api', 'http://127.0.0.1:8013');
const BOT = arg('bot');
const TURNS = Number(arg('turns', '2'));
const USER = arg('user', 'user_3J98Yz8K5m5WVkDix19nQb7AgiL');
if (!BOT) { console.error('usage: --bot <botId> [--turns N]'); process.exit(2); }

function desktopToken() {
  // The bundled binary, or a runtime update's copy under userData/runtime/versions/<v>/bin.
  const pid = execSync("pgrep -f 'Allternit Desktop.app/Contents/Resources/bin/allternit-api|@allternit/desktop/runtime/versions/[^/]+/bin/allternit-api' | head -1").toString().trim();
  const env = execSync(`ps eww -o command= -p ${pid}`).toString().split(' ');
  return (env.find((e) => e.startsWith('ALLTERNIT_DESKTOP_ACCESS_TOKEN=')) ?? '').split('=')[1];
}
const headers = { 'x-allternit-user-id': USER, 'x-allternit-desktop-access-token': desktopToken(), 'content-type': 'application/json' };
const call = async (method, path, body) => {
  const r = await fetch(`${API}/api/v1${path}`, { method, headers, body: body ? JSON.stringify(body) : undefined });
  const text = await r.text();
  let json = null; try { json = JSON.parse(text); } catch { /* keep text */ }
  return { status: r.status, json, text };
};
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const results = [];
const check = (name, ok, detail = '') => { results.push({ name, ok, detail }); console.log(`${ok ? 'PASS' : 'FAIL'}  ${name}${detail ? ` — ${detail}` : ''}`); };

const b = await call('GET', `/gateway/bots/${BOT}/execution-binding`);
const binding = b.json?.binding;
check('bot is vendor-bound and READY', binding?.type === 'vendor' && binding?.state === 'READY', `${binding?.vendor}/${binding?.adapterId} ${binding?.state}`);
if (!binding || binding.state !== 'READY') process.exit(1);

const t = await call('POST', '/threads', { botId: BOT, title: `Live check ${new Date().toISOString()}`, kind: 'task' });
const thread = t.json?.thread;
check('thread created', Boolean(thread?.currentSessionId), thread?.id ?? t.text.slice(0, 120));
if (!thread) process.exit(1);

const prompts = ['Reply with one short sentence: what is 2 + 3?', 'Reply with one short sentence: what is 10 minus 4?'];
const seenIds = new Set();
let cursor = 0;
for (let n = 0; n < TURNS; n++) {
  const corr = `live-check-${Date.now()}-${n}`;
  const started = Date.now();
  const s = await call('POST', `/agent-sessions/${thread.currentSessionId}/messages`, { text: prompts[n % prompts.length], metadata: { correlationId: corr } });
  check(`turn ${n + 1}: accepted`, s.status === 200, s.status === 200 ? '' : s.text.slice(0, 200));
  if (s.status !== 200) break;
  let done = null, failed = null;
  while (Date.now() - started < 240_000 && !done && !failed) {
    await call('POST', `/threads/${thread.id}/gateway/sync`, {});
    const ev = await call('GET', `/threads/${thread.id}/events?after=${cursor}&limit=200`);
    for (const e of ev.json?.events ?? []) {
      cursor = Math.max(cursor, e.sequence ?? 0);
      const d = e.payload?.data ?? e.payload ?? {};
      if (e.type === 'agent.message.completed' && e.payload?.envelope?.correlationId === corr) done = d;
      if (e.type === 'gateway.turn.failed') failed = d;
    }
    if (!done && !failed) await sleep(4000);
  }
  const secs = Math.round((Date.now() - started) / 1000);
  check(`turn ${n + 1}: reply synced into the thread`, Boolean(done), done ? `${secs}s` : failed ? `${failed.code}: ${failed.message}` : 'timed out (no agent.message.completed for this turn)');
  if (!done) break;
  const reply = String(done.reply ?? done.text ?? '');
  check(`turn ${n + 1}: reply has text`, reply.trim().length > 0, JSON.stringify(reply.slice(0, 80)));
  check(`turn ${n + 1}: no leftover draft or thinking text in the reply`, !/allternit live check ok|untangling|thought process/i.test(reply));
  check(`turn ${n + 1}: message id is new`, Boolean(done.messageId) && !seenIds.has(done.messageId), done.messageId);
  seenIds.add(done.messageId);
}
const failedCount = results.filter((r) => !r.ok).length;
console.log(`\n${failedCount ? 'FAILED' : 'ALL PASSED'}: ${results.length - failedCount}/${results.length}  (thread ${thread.id})`);
process.exit(failedCount ? 1 : 0);
