import { test } from 'node:test';
import assert from 'node:assert/strict';
import { createHmac } from 'node:crypto';
import worker, { verifySignature, shouldRebuild } from './index.js';

const secret = 's3cret';
const sign = (body) => 'sha256=' + createHmac('sha256', secret).update(body).digest('hex');
const push = (ref, repo = 'Allternit/allternit-platform') => JSON.stringify({ ref, after: 'abcdef1234', repository: { full_name: repo } });

test('signature must match the body', async () => {
  const body = push('refs/heads/main');
  assert.equal(await verifySignature(secret, body, sign(body)), true);
  assert.equal(await verifySignature(secret, body + ' ', sign(body)), false);
  assert.equal(await verifySignature(secret, body, null), false);
  assert.equal(await verifySignature('', body, sign(body)), false);
});

test('only pushes to allternit-platform main rebuild', () => {
  assert.equal(shouldRebuild('push', JSON.parse(push('refs/heads/main'))).rebuild, true);
  assert.equal(shouldRebuild('push', JSON.parse(push('refs/heads/feat/x'))).rebuild, false);
  assert.equal(shouldRebuild('push', JSON.parse(push('refs/heads/main', 'Someone/else'))).rebuild, false);
  assert.equal(shouldRebuild('pull_request', {}).rebuild, false);
  assert.equal(shouldRebuild('ping', {}).status, 200);
});

test('fetch calls the deploy hook once for main and never for branches', async () => {
  const calls = [];
  const realFetch = globalThis.fetch;
  globalThis.fetch = async (url) => { calls.push(url); return new Response('ok'); };
  try {
    const env = { GITHUB_WEBHOOK_SECRET: secret, PAGES_DEPLOY_HOOK_URL: 'https://hook.example/x' };
    const send = (body, event = 'push') => worker.fetch(new Request('https://w.example/', {
      method: 'POST', body, headers: { 'x-hub-signature-256': sign(body), 'x-github-event': event } }), env);
    assert.equal((await send(push('refs/heads/main'))).status, 200);
    assert.equal((await send(push('refs/heads/other'))).status, 202);
    const forged = await worker.fetch(new Request('https://w.example/', { method: 'POST', body: push('refs/heads/main'),
      headers: { 'x-hub-signature-256': 'sha256=00', 'x-github-event': 'push' } }), env);
    assert.equal(forged.status, 401);
    assert.deepEqual(calls, ['https://hook.example/x']);
  } finally { globalThis.fetch = realFetch; }
});
