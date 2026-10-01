import { test } from 'node:test';
import assert from 'node:assert/strict';
import { mkdtempSync, writeFileSync, existsSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { consumeProvisionedBootstrap, pairWithBootstrap, readProvisionedBootstrap } from './provisioned';

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
