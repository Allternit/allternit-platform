import assert from 'node:assert/strict';
import http from 'node:http';
import { mkdtempSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import test from 'node:test';
import {
  SubsClient,
  followTaskEvents,
  gatewaySocketPath,
  resolveGatewayToken,
} from './client.js';

test('gatewaySocketPath honors SUBS_GATEWAY_STATE_DIR and defaults under ~/.allternit', () => {
  assert.equal(
    gatewaySocketPath({ SUBS_GATEWAY_STATE_DIR: '/tmp/subs-state' }),
    '/tmp/subs-state/gateway.sock',
  );
  assert.ok(gatewaySocketPath({}).endsWith('/.allternit/subscriptions/gateway.sock'));
});

test('resolveGatewayToken prefers SUBS_GATEWAY_TOKEN', () => {
  assert.equal(resolveGatewayToken({ SUBS_GATEWAY_TOKEN: 'sgw_test' }), 'sgw_test');
  // The keychain fallback path depends on machine state (a booted gateway
  // stores cli-token), so it is covered by the gateway's boot tests instead.
});

test('SubsClient.request round-trips over a unix socket with the bearer token', async () => {
  const dir = mkdtempSync(join(tmpdir(), 'subs-client-test-'));
  const sock = join(dir, 'gw.sock');
  const server = http.createServer((req, res) => {
    assert.equal(req.headers.authorization, 'Bearer sgw_test');
    res.writeHead(200, { 'content-type': 'application/json' });
    res.end(JSON.stringify({ ok: true, path: req.url }));
  });
  await new Promise<void>((r) => server.listen(sock, r));
  try {
    const client = new SubsClient({ socketPath: sock, token: 'sgw_test' });
    const res = await client.requestOk<{ ok: boolean; path: string }>('GET', '/v1/accounts');
    assert.equal(res.ok, true);
    assert.equal(res.path, '/v1/accounts');
  } finally {
    await new Promise((r) => server.close(r));
    rmSync(dir, { recursive: true, force: true });
  }
});

test('followTaskEvents reads SSE, stops at a terminal event, and acks ids', async () => {
  const dir = mkdtempSync(join(tmpdir(), 'subs-client-test-'));
  const sock = join(dir, 'gw.sock');
  let acked: string[] = [];
  const server = http.createServer((req, res) => {
    if (req.url === '/v1/events/ack') {
      let data = '';
      req.on('data', (c) => (data += c));
      req.on('end', () => {
        acked = JSON.parse(data).event_ids;
        res.writeHead(200, { 'content-type': 'application/json' });
        res.end('{"acked":2}');
      });
      return;
    }
    res.writeHead(200, { 'content-type': 'text/event-stream' });
    res.write('id: e1\nevent: progress\ndata: {"t":"progress","label":"working"}\n\n');
    res.write('id: e2\nevent: done\ndata: {"t":"done","outcome":"success"}\n\n');
  });
  await new Promise<void>((r) => server.listen(sock, r));
  try {
    const client = new SubsClient({ socketPath: sock, token: 'sgw_test' });
    const seen: string[] = [];
    const result = await followTaskEvents(
      client,
      'task-1',
      (ev) => seen.push(ev.event),
      (ev) => ev.event === 'done',
    );
    assert.deepEqual(seen, ['progress', 'done']);
    assert.equal(result.acked, 2);
    assert.deepEqual(acked, ['e1', 'e2']);
  } finally {
    await new Promise((r) => server.close(r));
    rmSync(dir, { recursive: true, force: true });
  }
});
