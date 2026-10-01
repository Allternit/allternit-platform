import { test } from 'node:test';
import assert from 'node:assert/strict';
import { mkdtempSync, writeFileSync, existsSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import {
  consumeProvisionedBootstrap,
  isProvisioned,
  pairWithBootstrap,
  readProvisionedBootstrap,
  relayedRequestHeaders,
  waitForProvisionedPairing,
  type ProvisionedBootstrap,
} from './provisioned';

function file(content: unknown): string {
  const path = join(mkdtempSync(join(tmpdir(), 'agentd-bootstrap-')), 'bootstrap.json');
  writeFileSync(path, typeof content === 'string' ? content : JSON.stringify(content));
  return path;
}
const valid = { api: 'https://api.allternit.com/', token: 'tok', instance_id: 'pi_1', user_id: 'u_1' };
const json = (status: number, body: unknown) => new Response(JSON.stringify(body), { status, headers: { 'Content-Type': 'application/json' } });

test('reads a valid bootstrap; ignores missing, malformed and http ones', async () => {
  const path = file(valid);
  assert.deepEqual(await readProvisionedBootstrap(path), { api: 'https://api.allternit.com', token: 'tok', instanceId: 'pi_1', userId: 'u_1', path });
  assert.equal(await readProvisionedBootstrap('/nonexistent/bootstrap.json'), null);
  assert.equal(await readProvisionedBootstrap(file('{bad')), null);
  assert.equal(await readProvisionedBootstrap(file({ ...valid, api: 'http://x' })), null);
});

test('pairs with the token, retries the exchange past 428, then returns the credential', async () => {
  const bootstrap = (await readProvisionedBootstrap(file(valid)))!;
  const calls: Array<{ url: string; body: any; headers: any }> = [];
  let exchanges = 0;
  const fakeFetch = (async (url: string, init: any) => {
    calls.push({ url, body: JSON.parse(init.body), headers: init.headers });
    if (url.endsWith('/runtime-pairings')) return json(201, { pairingId: 'p1', deviceCode: 'd1', challenge: 'c1', pollIntervalSeconds: 1 });
    exchanges += 1;
    return exchanges < 3 ? json(428, { error: 'pending' }) : json(200, { runtimeId: 'rt_1', userId: 'u_1', deviceToken: 'dev' });
  }) as typeof fetch;
  const payload = await pairWithBootstrap(bootstrap, { publicKey: 'pk' }, { fetch: fakeFetch, sign: (m) => `sig(${m})`, sleep: async () => {} });
  assert.equal(payload.runtimeId, 'rt_1');
  assert.equal(exchanges, 3);
  assert.equal(calls[0].headers['X-Allternit-Bootstrap-Token'], 'tok');
  assert.deepEqual({ ...calls[0].body }, { publicKey: 'pk', name: 'Allternit cloud computer', runtimeType: 'provisioned', bootstrapToken: 'tok', instanceId: 'pi_1' });
  assert.equal(calls[1].body.signature, 'sig(allternit-runtime-pairing:p1:c1)');
});

test('a refused bootstrap throws, so the daemon falls back to its other pairing', async () => {
  const bootstrap = (await readProvisionedBootstrap(file(valid)))!;
  const fakeFetch = (async () => json(401, { error: 'invalid bootstrap token' })) as unknown as typeof fetch;
  await assert.rejects(pairWithBootstrap(bootstrap, {}, { fetch: fakeFetch, sign: () => '', sleep: async () => {} }), /refused \(401\)/);
});

test('the bootstrap file is removed after use', async () => {
  const bootstrap = (await readProvisionedBootstrap(file(valid)))!;
  await consumeProvisionedBootstrap(bootstrap);
  assert.equal(existsSync(bootstrap.path), false);
});

test('provisioned mode is only ALLTERNIT_PROVISIONED=1', () => {
  assert.equal(isProvisioned({ ALLTERNIT_PROVISIONED: '1' }), true);
  assert.equal(isProvisioned({}), false);
  assert.equal(isProvisioned({ ALLTERNIT_PROVISIONED: 'true' }), false);
});

test('a provisioned computer waits for its bootstrap and retries a refusal with backoff, never exiting', async () => {
  const bootstrap: ProvisionedBootstrap = { api: 'https://api.allternit.com', token: 'tok', instanceId: 'pi_1', userId: 'u_1', path: '/x' };
  const reads = [null, null, bootstrap, bootstrap, bootstrap];
  const sleeps: number[] = [];
  const logs: string[] = [];
  let pairs = 0;
  const result = await waitForProvisionedPairing({
    read: async () => (reads.length ? reads.shift()! : bootstrap),
    pair: async () => {
      pairs += 1;
      if (pairs < 3) throw new Error('Bootstrap pairing was refused (401)');
      return { runtimeId: 'rt_1' };
    },
    sleep: async (ms) => { sleeps.push(ms); },
    log: (m) => logs.push(m),
    baseDelayMs: 10,
    maxDelayMs: 15,
  });
  assert.equal(result.payload.runtimeId, 'rt_1');
  assert.equal(result.bootstrap, bootstrap);
  assert.deepEqual(sleeps, [10, 10, 10, 15]);
  assert.equal(logs.filter((m) => m.includes('waiting')).length, 1);
  assert.equal(logs.filter((m) => m.includes('refused (401)')).length, 2);
});

test('relayed requests carry the relay marker and never a caller human proof', () => {
  const identity = { userId: 'u_1', userEmail: 'a@b.c', organizationId: 'org_1', deviceToken: 'allternit_runtime_x' };
  const own = relayedRequestHeaders({ 'X-Allternit-Human-Proof': 'forged', 'X-Allternit-Tenant-Id': 'org_evil', Accept: 'text/plain' }, identity);
  assert.equal(own.get('authorization'), 'Bearer allternit_runtime_x');
  assert.equal(own.get('x-allternit-relayed'), '1');
  assert.equal(own.get('x-allternit-human-proof'), null);
  assert.equal(own.get('x-allternit-desktop-access-token'), 'allternit_runtime_x');
  assert.equal(own.get('x-allternit-user-id'), 'u_1');
  assert.equal(own.get('x-allternit-tenant-id'), 'org_1');
  assert.equal(own.get('accept'), 'text/plain');
  const clerk = relayedRequestHeaders({ Authorization: 'Bearer clerk.jwt' }, { ...identity, organizationId: undefined });
  assert.equal(clerk.get('authorization'), 'Bearer clerk.jwt');
  assert.equal(clerk.get('x-allternit-tenant-id'), null);
});
