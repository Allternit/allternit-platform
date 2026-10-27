#!/usr/bin/env node
// ws.test.mjs — loopback tests for the hand-rolled RFC 6455 stack.
//   Part A (lib): masked text echo, binary echo, fragmentation reassembly,
//                 client ping→pong, close handshake — via an echo server
//                 built on lib/ws.mjs and a raw node:net client.
//   Part B (server): the real server/index.mjs — HTTP 403/200 token gate,
//                 WS 403 without token, hello + ping/pong with token.
// Run: node test/ws.test.mjs
import { createServer, get } from 'node:http';
import { connect } from 'node:net';
import { spawn } from 'node:child_process';
import { createHash, randomBytes } from 'node:crypto';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import assert from 'node:assert/strict';

import { upgrade, encodeFrame, OPCODES } from '../server/lib/ws.mjs';
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

// ── Minimal hand-rolled WS client (masked frames, per RFC) ──────────────────

function wsClient(port, path, headers = {}) {
  return new Promise((resolve, reject) => {
    const key = randomBytes(16).toString('base64');
    const sock = connect(port, '127.0.0.1', () => {
      const hs = [
        `GET ${path} HTTP/1.1`,
        `Host: 127.0.0.1:${port}`,
        'Upgrade: websocket',
        'Connection: Upgrade',
        `Sec-WebSocket-Key: ${key}`,
        'Sec-WebSocket-Version: 13',
        ...Object.entries(headers).map(([k, v]) => `${k}: ${v}`),
        '', '',
      ].join('\r\n');
      sock.write(hs);
    });
    let head = Buffer.alloc(0);
    let upgraded = false;
    const client = {
      sock,
      events: [],
      waiters: [],
      sendText(s) { sock.write(encodeFrame(OPCODES.TEXT, Buffer.from(s), { mask: true })); },
      sendBinary(b) { sock.write(encodeFrame(OPCODES.BINARY, b, { mask: true })); },
      sendFragmented(parts, opcode = OPCODES.TEXT) {
        parts.forEach((p, i) => {
          sock.write(encodeFrame(i === 0 ? opcode : OPCODES.CONT, p, { fin: i === parts.length - 1, mask: true }));
        });
      },
      sendPing(data = Buffer.alloc(0)) { sock.write(encodeFrame(OPCODES.PING, data, { mask: true })); },
      close(code = 1000) {
        const p = Buffer.alloc(2);
        p.writeUInt16BE(code);
        sock.write(encodeFrame(OPCODES.CLOSE, p, { mask: true }));
      },
      next(type, timeoutMs = 3000) {
        return new Promise((res, rej) => {
          const timer = setTimeout(() => rej(new Error(`timeout waiting for ${type}`)), timeoutMs);
          client.waiters.push({ type, res: (v) => { clearTimeout(timer); res(v); } });
          pump();
        });
      },
    };
    function emit(ev) {
      client.events.push(ev);
      pump();
    }
    function pump() {
      for (let i = client.waiters.length - 1; i >= 0; i--) {
        const w = client.waiters[i];
        const j = client.events.findIndex((e) => e.type === w.type);
        if (j >= 0) {
          const [ev] = client.events.splice(j, 1);
          client.waiters.splice(i, 1);
          w.res(ev);
        }
      }
    }
    const parserEvents = [];
    import('../server/lib/ws.mjs').then(({ FrameParser }) => {
      const parser = new FrameParser();
      sock.on('data', (chunk) => {
        if (!upgraded) {
          head = Buffer.concat([head, chunk]);
          const idx = head.indexOf('\r\n\r\n');
          if (idx < 0) return;
          const statusLine = head.subarray(0, head.indexOf('\r\n')).toString();
          const rest = head.subarray(idx + 4);
          upgraded = true;
          if (!statusLine.includes('101')) {
            reject(new Error(`upgrade refused: ${statusLine}`));
            return;
          }
          resolve(client);
          if (rest.length) sock.emit('data', rest);
          return;
        }
        for (const ev of parser.push(chunk)) {
          if (ev.type === 'message') emit({ type: ev.opcode === OPCODES.TEXT ? 'text' : 'binary', data: ev.data });
          else if (ev.type === 'pong') emit({ type: 'pong', data: ev.data });
          else if (ev.type === 'ping') sock.write(encodeFrame(OPCODES.PONG, ev.data, { mask: true }));
          else if (ev.type === 'close') emit({ type: 'close', code: ev.code });
          else if (ev.type === 'error') emit({ type: 'proto-error', error: ev.error });
        }
      });
    });
    sock.on('error', reject);
  });
}

// ── Part A: lib-level echo server ────────────────────────────────────────────

async function partA() {
  console.log('Part A — lib/ws.mjs framing');
  const server = createServer();
  server.on('upgrade', (req, socket, head) => {
    const conn = upgrade(req, socket, head);
    if (!conn) return;
    conn.on('text', (t) => conn.sendText(`echo:${t}`));
    conn.on('binary', (b) => conn.sendBinary(b));
  });
  await new Promise((r) => server.listen(0, '127.0.0.1', r));
  const port = server.address().port;

  const c = await wsClient(port, '/ws');
  ok('upgrade handshake (101)');

  c.sendText('hello');
  const t = await c.next('text');
  assert.equal(t.data.toString(), 'echo:hello');
  ok('masked client text frame unmasked + echoed');

  const bin = randomBytes(70 * 1024); // exercises 16-bit extended length
  c.sendBinary(bin);
  const b = await c.next('binary');
  assert.deepEqual(b.data, bin);
  ok('binary frame round-trip (70KB, 16-bit length)');

  const small = randomBytes(100);
  c.sendBinary(small);
  assert.deepEqual((await c.next('binary')).data, small);
  ok('binary frame round-trip (100B, 7-bit length)');

  c.sendFragmented([Buffer.from('{"part'), Buffer.from('":"one"}')]);
  const f = await c.next('text');
  assert.equal(f.data.toString(), 'echo:{"part":"one"}');
  ok('fragmented message reassembled');

  const pingPayload = randomBytes(8);
  c.sendPing(pingPayload);
  const p = await c.next('pong');
  assert.deepEqual(p.data, pingPayload);
  ok('client ping → server pong (payload preserved)');

  c.close(1000);
  const cl = await c.next('close');
  assert.equal(cl.code, 1000);
  ok('close handshake (echo close, code 1000)');

  await new Promise((r) => server.close(r));
}

// ── Part B: real server auth + hello ─────────────────────────────────────────

function httpGet(port, path, cookie) {
  return new Promise((resolve, reject) => {
    const req = get({ host: '127.0.0.1', port, path, headers: cookie ? { cookie } : {} }, (res) => {
      let body = '';
      res.on('data', (d) => (body += d));
      res.on('end', () => resolve({ status: res.statusCode, body, headers: res.headers }));
    });
    req.on('error', reject);
  });
}

async function partB() {
  console.log('Part B — server/index.mjs auth + protocol');
  const port = 8479;
  const child = spawn('node', [join(HERE, '..', 'server', 'index.mjs'),
    '--port', String(port), '--bind', '127.0.0.1', '--token', 'test-token-123',
    '--capture', 'none', '--no-input'], { stdio: ['ignore', 'pipe', 'pipe'] });
  let stderr = '';
  child.stderr.on('data', (d) => (stderr += d));
  await waitHealthy(port);

  try {
    const noToken = await httpGet(port, '/');
    assert.equal(noToken.status, 403);
    ok('GET / without token → 403');

    const wrong = await httpGet(port, '/?t=nope');
    assert.equal(wrong.status, 403);
    ok('GET / with wrong token → 403');

    const good = await httpGet(port, '/?t=test-token-123');
    assert.equal(good.status, 200);
    assert.match(good.body, /phone-remote/);
    assert.match(good.headers['set-cookie']?.[0] ?? '', /prt=/);
    ok('GET / with token → 200 + index.html + session cookie');

    const viaCookie = await httpGet(port, '/app.js', 'prt=test-token-123');
    assert.equal(viaCookie.status, 200);
    ok('static file via cookie auth → 200');

    const manifest = await httpGet(port, '/manifest.webmanifest?t=test-token-123');
    assert.equal(manifest.status, 200);
    assert.equal(manifest.headers['content-type'], 'application/manifest+json');
    assert.match(manifest.body, /"start_url":"\.\/\?t=test-token-123"/);
    ok('manifest served with token baked into start_url');

    const traversal = await httpGet(port, '/..%2F..%2FAGENTS.md?t=test-token-123');
    assert.notEqual(traversal.status, 200);
    ok('path traversal blocked');

    const helloHttp = await httpGet(port, '/hello');
    assert.equal(helloHttp.status, 200);
    const helloBody = JSON.parse(helloHttp.body);
    assert.equal(helloBody.capture.mode, 'none');
    assert.equal(helloBody.hasFrame, false);
    ok('loopback GET /hello without token → capture status');

    const noFrame = await httpGet(port, '/frame');
    assert.equal(noFrame.status, 503);
    ok('loopback GET /frame with capture none → 503');

    // WS without token → 403 (raw socket, since wsClient expects 101)
    await new Promise((resolve, reject) => {
      const sock = connect(port, '127.0.0.1', () => {
        const rfcKey = Buffer.from('the sample nonce').toString('base64');
        sock.write(`GET /ws HTTP/1.1\r\nHost: x\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Key: ${rfcKey}\r\nSec-WebSocket-Version: 13\r\n\r\n`);
      });
      let data = '';
      sock.on('data', (d) => { data += d; sock.end(); });
      sock.on('close', () => {
        try { assert.match(data, /403/); ok('WS upgrade without token → 403'); resolve(); }
        catch (e) { reject(e); }
      });
      sock.on('error', reject);
    });

    const c = await wsClient(port, '/ws?t=test-token-123');
    ok('WS upgrade with token → 101');

    const hello = await c.next('text');
    const helloMsg = JSON.parse(hello.data.toString());
    assert.equal(helloMsg.type, 'hello');
    assert.equal(helloMsg.input.enabled, false);
    ok('hello message received');

    c.sendText(JSON.stringify({ type: 'ping' }));
    const pong = JSON.parse((await c.next('text')).data.toString());
    assert.equal(pong.type, 'pong');
    ok('protocol ping → pong');

    c.sendText('not json');
    const err = JSON.parse((await c.next('text')).data.toString());
    assert.equal(err.type, 'error');
    ok('garbage text → JSON error, connection stays up');

    // single-viewer: second connection is refused
    const c2 = await wsClient(port, '/ws?t=test-token-123');
    const refuse = JSON.parse((await c2.next('text')).data.toString());
    assert.equal(refuse.type, 'error');
    const closed = await c2.next('close');
    assert.ok(closed.code === 1013 || closed.code === 1005 || closed.code === 1006);
    ok('second viewer refused (single-viewer build)');

    c.close();
    await c.next('close');
    ok('viewer close handshake');
  } finally {
    child.kill('SIGTERM');
  }
}

await partA();
await partB();
console.log(`\n${passed} checks passed`);
process.exit(0);
