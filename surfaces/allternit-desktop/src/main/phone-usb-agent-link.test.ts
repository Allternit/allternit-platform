import * as net from 'node:net';
import { afterEach, describe, expect, it } from 'vitest';
import { LineReader, UsbAgentLink } from './phone-usb-agent-link.js';
import { USBA_MAX_LINE, newKeyHex } from './phone-usb-agent-protocol.js';
import { SimBoard, until } from './phone-usb-agent.testkit.js';

const ID = 'usba-0123456789ab';
const links: UsbAgentLink[] = [];
const boards: SimBoard[] = [];

async function setup(keys: Record<string, string>, opts = {}) {
  const link = new UsbAgentLink((id) => keys[id] ?? null, { heartbeatMs: 60_000, ...opts });
  links.push(link);
  const port = await link.listen(0, '127.0.0.1');
  return { link, port };
}

function board(id: string, key: string) {
  const b = new SimBoard(id, key);
  boards.push(b);
  return b;
}

afterEach(() => {
  for (const b of boards.splice(0)) b.close();
  for (const l of links.splice(0)) l.close();
});

describe('UsbAgentLink', () => {
  it('authenticates a known board and runs commands only while armed', async () => {
    const key = newKeyHex();
    const { link, port } = await setup({ [ID]: key });
    const b = board(ID, key);
    await b.connect(port);
    await until(() => link.session(ID) !== null);
    expect(link.session(ID)?.state.armed).toBe(false);

    expect(await link.send(ID, { op: 'click' })).toMatchObject({ ok: false, error: 'disarmed' });
    b.arm();
    await until(() => link.session(ID)?.state.armed === true);
    expect(await link.send(ID, { op: 'move', dx: 10, dy: -5 })).toEqual({ ok: true });
    expect(await link.send(ID, { op: 'click' })).toEqual({ ok: true });
    expect(b.ref.log).toEqual(['move 10 -5', 'click']);
  });

  it('refuses unknown boards and boards with the wrong key', async () => {
    const { link, port } = await setup({ [ID]: newKeyHex() });
    const rejected: string[] = [];
    link.on('rejected', (r: { reason: string }) => rejected.push(r.reason));
    await expect(board('usba-ffffffffffff', newKeyHex()).connect(port)).rejects.toThrow();
    // A board with the wrong key refuses Desktop's proof and hangs up itself.
    await expect(board(ID, newKeyHex()).connect(port)).rejects.toThrow();
    // A client that answers the challenge with a forged proof is dropped by Desktop.
    const forger = net.connect(port, '127.0.0.1', () => forger.write(`hello usba/1 ${ID} 1.0.0 power-on ${'a'.repeat(32)}\n`));
    forger.on('data', (d) => {
      if (d.toString().startsWith('challenge')) forger.write(`auth ${'b'.repeat(64)}\n`);
    });
    await new Promise((r) => forger.on('close', r));
    await until(() => rejected.length === 2);
    expect(rejected).toEqual(['unknown_board', 'bad_proof']);
    expect(link.sessionsInfo()).toEqual([]);
  });

  it('drops a peer that never finishes the handshake', async () => {
    const { port } = await setup({}, { handshakeTimeoutMs: 100 });
    const socket = net.connect(port, '127.0.0.1');
    const closed = new Promise((r) => socket.on('close', r));
    await closed;
  });

  it('drops a peer that sends an oversized line', async () => {
    const { port } = await setup({});
    const socket = net.connect(port, '127.0.0.1', () => socket.write('x'.repeat(USBA_MAX_LINE + 10)));
    await new Promise((r) => socket.on('close', r));
  });

  it('reports a command with no answer as uncertain, never ok', async () => {
    const key = newKeyHex();
    const { link, port } = await setup({ [ID]: key }, { commandTimeoutMs: 100 });
    const b = board(ID, key);
    await b.connect(port);
    b.arm();
    await until(() => link.session(ID)?.state.armed === true);
    b.mute = true;
    expect(await link.send(ID, { op: 'click' })).toMatchObject({ ok: false, error: 'uncertain' });
  });

  it('a disconnect mid-command is uncertain, and later commands say offline', async () => {
    const key = newKeyHex();
    const { link, port } = await setup({ [ID]: key });
    const b = board(ID, key);
    await b.connect(port);
    b.arm();
    await until(() => link.session(ID)?.state.armed === true);
    b.mute = true;
    const pending = link.send(ID, { op: 'click' });
    setTimeout(() => b.close(), 20);
    expect(await pending).toMatchObject({ ok: false, error: 'uncertain' });
    await until(() => link.session(ID) === null);
    expect(await link.send(ID, { op: 'click' })).toMatchObject({ ok: false, error: 'offline' });
  });

  it('a new authenticated session replaces the old one', async () => {
    const key = newKeyHex();
    const { link, port } = await setup({ [ID]: key });
    const first = board(ID, key);
    await first.connect(port);
    const firstSocket = first.socket!;
    const closed = new Promise((r) => firstSocket.on('close', r));
    const second = board(ID, key);
    await second.connect(port);
    await closed;
    second.arm();
    await until(() => link.session(ID)?.state.armed === true);
    expect(await link.send(ID, { op: 'home' })).toEqual({ ok: true });
    expect(second.ref.log).toEqual(['home']);
  });

  it('rejects out-of-bounds commands without sending them', async () => {
    const key = newKeyHex();
    const { link, port } = await setup({ [ID]: key });
    const b = board(ID, key);
    await b.connect(port);
    b.arm();
    await until(() => link.session(ID)?.state.armed === true);
    expect(await link.send(ID, { op: 'move', dx: 99_999, dy: 0 })).toMatchObject({ ok: false, error: 'invalid' });
    expect(b.ref.log).toEqual([]);
  });

  it('heartbeats keep the lease alive on an idle link', async () => {
    const key = newKeyHex();
    const { link, port } = await setup({ [ID]: key }, { heartbeatMs: 20 });
    const b = board(ID, key);
    const seen: string[] = [];
    const orig = b.ref.receive.bind(b.ref);
    b.ref.receive = (line: string) => {
      seen.push(line.split(' ')[2] ?? line);
      return orig(line);
    };
    await b.connect(port);
    await until(() => seen.filter((s) => s === 'ping').length >= 2);
    void link;
  });

  it('LineReader splits lines and flags overflow', () => {
    const lines: string[] = [];
    let overflow = 0;
    const r = new LineReader((l) => lines.push(l), () => (overflow += 1));
    r.push('a\r\nb');
    r.push('\n\nc\n');
    expect(lines).toEqual(['a', 'b', 'c']);
    r.push('y'.repeat(USBA_MAX_LINE + 1));
    expect(overflow).toBe(1);
  });
});
