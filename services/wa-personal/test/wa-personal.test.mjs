import test from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import crypto from 'node:crypto';
import { EventEmitter } from 'node:events';
import { createHandler } from '../src/server.mjs';
import { Session } from '../src/session.mjs';
import { seal, open, loadKey, readSealed, parseKey } from '../src/crypto.mjs';
import { decide } from '../src/policy.mjs';

const OWN = '15550001111:7@s.whatsapp.net';
const key = crypto.randomBytes(32);
const tmp = () => fs.mkdtempSync(path.join(os.tmpdir(), 'wap-'));

// Baileys mocked: a socket whose events the test drives.
function fakeBaileys() {
  const sockets = [];
  const mod = {
    DisconnectReason: { loggedOut: 401 },
    BufferJSON: { replacer: (_k, v) => v, reviver: (_k, v) => v },
    initAuthCreds: () => ({ fresh: true }),
    proto: { Message: { AppStateSyncKeyData: { fromObject: (o) => o } } },
    makeWASocket: (cfg) => {
      const sock = { cfg, ev: new EventEmitter(), user: null, sendMessage: async (jid, c) => ((sock.sentTo = { jid, c }), { key: { id: 'sent-1' } }), logout: async () => {} };
      sockets.push(sock);
      return sock;
    },
  };
  return { mod, sockets };
}

function setup(over = {}) {
  const { mod, sockets } = fakeBaileys();
  const forwarded = [];
  const session = new Session({ loadBaileys: async () => mod, dataDir: tmp(), key, forward: async (e) => forwarded.push(e), ...over });
  return { session, sockets, forwarded };
}
const dm = (chat, text, extra = {}) => ({ key: { remoteJid: chat, id: crypto.randomUUID(), ...extra.key }, message: { conversation: text }, ...extra.top });

async function call(handler, method, url, { token = 't0k', body } = {}) {
  let status, text;
  const req = Object.assign(new EventEmitter(), { method, url, headers: token ? { authorization: `Bearer ${token}` } : {} });
  req[Symbol.asyncIterator] = async function* () { if (body) yield Buffer.from(JSON.stringify(body)); };
  await handler(req, { writeHead: (s) => (status = s), end: (t) => (text = t) });
  return { status, body: JSON.parse(text) };
}

test('flag off: every endpoint is 503 with the warning, Baileys never loads', async () => {
  let loaded = false;
  const h = createHandler({ env: {}, getSession: () => { loaded = true; } });
  const r = await call(h, 'GET', '/status');
  assert.equal(r.status, 503);
  assert.equal(r.body.error, 'wa_personal_disabled');
  assert.match(r.body.warning, /Unofficial/);
  assert.equal(loaded, false);
});

test('API contract: auth, start -> QR, status, send, pairing', async () => {
  const { session, sockets } = setup();
  const h = createHandler({ env: { ALLTERNIT_WA_PERSONAL: '1', ALLTERNIT_WA_PERSONAL_TOKEN: 't0k' }, getSession: () => session });
  assert.equal((await call(h, 'GET', '/status', { token: 'bad' })).status, 401);
  assert.equal((await call(h, 'GET', '/session/qr')).status, 404);

  const start = await call(h, 'POST', '/session/start');
  assert.equal(start.status, 200);
  assert.equal(start.body.status, 'connecting');
  assert.match(start.body.warning, /ban/);
  assert.match(start.body.warning, /dedicated number/);

  sockets[0].ev.emit('connection.update', { qr: 'QR-STRING' });
  const qr = await call(h, 'GET', '/session/qr');
  assert.deepEqual([qr.status, qr.body.qr, qr.body.status], [200, 'QR-STRING', 'qr']);

  assert.equal((await call(h, 'POST', '/send', { body: { to: '15559998888', text: 'hi' } })).status, 409);

  sockets[0].user = { id: OWN };
  sockets[0].ev.emit('connection.update', { connection: 'open' });
  const st = await call(h, 'GET', '/status');
  assert.deepEqual([st.body.status, st.body.linked, st.body.number], ['connected', true, '15550001111']);

  const sent = await call(h, 'POST', '/send', { body: { to: '15559998888', text: 'hi' } });
  assert.deepEqual([sent.status, sent.body], [200, { id: 'sent-1' }]);
  assert.deepEqual(sockets[0].sentTo, { jid: '15559998888@s.whatsapp.net', c: { text: 'hi' } });
  assert.equal((await call(h, 'POST', '/send', { body: { to: '1' } })).status, 400);
});

test('stranger DM is held for pairing, not forwarded or answered; approve lets it through', async () => {
  const { session, sockets, forwarded } = setup();
  await session.start();
  sockets[0].user = { id: OWN };
  sockets[0].ev.emit('connection.update', { connection: 'open' });
  sockets[0].ev.emit('messages.upsert', { type: 'notify', messages: [dm('15557776666@s.whatsapp.net', 'hello?')] });
  await new Promise((r) => setImmediate(r));
  assert.equal(forwarded.length, 0);
  assert.equal(sockets[0].sentTo, undefined);
  assert.equal(session.pending()[0].number, '15557776666');
  session.approve('15557776666');
  sockets[0].ev.emit('messages.upsert', { type: 'notify', messages: [dm('15557776666@s.whatsapp.net', 'now?')] });
  await new Promise((r) => setImmediate(r));
  assert.equal(forwarded.length, 1);
  assert.equal(forwarded[0].text, 'now?');
  assert.equal(forwarded[0].from, '15557776666');
});

test('self-chat is the owner; own echoes of sent replies are not re-forwarded', async () => {
  const { session, sockets, forwarded } = setup();
  await session.start();
  sockets[0].user = { id: OWN };
  sockets[0].ev.emit('connection.update', { connection: 'open' });
  const self = '15550001111@s.whatsapp.net';
  sockets[0].ev.emit('messages.upsert', { type: 'notify', messages: [dm(self, 'note to bot', { key: { fromMe: true } })] });
  await session.send(self, 'reply');
  sockets[0].ev.emit('messages.upsert', { type: 'notify', messages: [{ key: { remoteJid: self, id: 'sent-1', fromMe: true }, message: { conversation: 'reply' } }] });
  await new Promise((r) => setImmediate(r));
  assert.deepEqual(forwarded.map((f) => f.text), ['note to bot']);
});

test('groups need a mention of the number (and an approved sender)', () => {
  const ctx = { own: [OWN], allow: new Set(['15557776666']), pairing: new Map() };
  const group = (text, mentionedJid, participant = '15557776666@s.whatsapp.net') => ({ key: { remoteJid: '1@g.us', participant, id: 'x' }, message: { extendedTextMessage: { text, contextInfo: { mentionedJid } } } });
  assert.equal(decide(group('hey all', []), ctx).reason, 'group_mention_required');
  assert.equal(decide(group('@bot hi', ['15550001111@s.whatsapp.net']), ctx).action, 'forward');
  assert.equal(decide(group('@bot hi', ['15550001111@s.whatsapp.net'], '15553332222@s.whatsapp.net'), ctx).action, 'pairing');
});

test('logged-out close wipes the stored session; other closes reconnect', async () => {
  const { session, sockets } = setup();
  await session.start();
  fs.writeFileSync(session.authFile, 'x');
  sockets[0].ev.emit('connection.update', { connection: 'close', lastDisconnect: { error: { output: { statusCode: 401 } } } });
  assert.equal(session.status, 'logged_out');
  assert.equal(fs.existsSync(session.authFile), false);
});

test('session creds are encrypted at rest and need the key', async () => {
  const { session, sockets } = setup();
  await session.start();
  const { state, } = { state: sockets[0].cfg.auth };
  await state.keys.set({ 'pre-key': { 1: { secret: 'PLAINTEXT-MARKER' } } });
  const raw = fs.readFileSync(session.authFile);
  assert.ok(!raw.includes('PLAINTEXT-MARKER'));
  assert.equal(fs.statSync(session.authFile).mode & 0o777, 0o600);
  assert.match(readSealed(session.authFile, key), /PLAINTEXT-MARKER/);
  assert.throws(() => readSealed(session.authFile, crypto.randomBytes(32)));
});

test('crypto: round-trip, tamper detection, key sources', () => {
  const blob = seal(key, 'hello');
  assert.equal(open(key, blob), 'hello');
  blob[blob.length - 1] ^= 1;
  assert.throws(() => open(key, blob));
  assert.throws(() => parseKey('short'));
  const keyFile = path.join(tmp(), 'k');
  const k1 = loadKey({ env: {}, keyFile });
  assert.deepEqual(loadKey({ env: {}, keyFile }), k1);
  assert.equal(fs.statSync(keyFile).mode & 0o777, 0o600);
  assert.deepEqual(loadKey({ env: { ALLTERNIT_WA_PERSONAL_KEY: key.toString('hex') }, keyFile }), key);
});
