import * as net from 'node:net';
import { afterEach, describe, expect, it } from 'vitest';
import { LineReader, UsbAgentLink } from './phone-usb-agent-link.js';
import { USBA_MAX_LINE, newKeyHex, type UsbaVersion } from './phone-usb-agent-protocol.js';
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

function board(id: string, key: string, version: UsbaVersion = 2) {
  const b = new SimBoard(id, key, null, version);
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

  it('accepts a version 1 and a version 2 board on the same link', async () => {
    const ID2 = 'usba-ba9876543210';
    const k1 = newKeyHex();
    const k2 = newKeyHex();
    const { link, port } = await setup({ [ID]: k1, [ID2]: k2 });
    const old = board(ID, k1, 1);
    const fresh = board(ID2, k2, 2);
    await old.connect(port);
    await fresh.connect(port);
    await until(() => link.session(ID) !== null && link.session(ID2)?.state.face === 'default');
    expect(link.session(ID)).toMatchObject({ version: 1, firmware: '1.0.0', state: { armed: false, usb: 'mounted' } });
    expect(link.session(ID2)).toMatchObject({ version: 2, firmware: '1.1.0', state: { armed: false, usb: 'mounted', dock: false, hid: 'usb', link: 'lan', face: 'default', prog: 0 } });
    old.arm();
    fresh.arm();
    await until(() => link.session(ID)?.state.armed === true && link.session(ID2)?.state.armed === true);
    expect(await link.send(ID, { op: 'click' })).toEqual({ ok: true });
    expect(await link.send(ID2, { op: 'click' })).toEqual({ ok: true });
    expect(await link.send(ID2, { op: 'face', name: 'working' })).toEqual({ ok: true });
    await until(() => link.session(ID2)?.state.face === 'working');
    expect(old.ref.log).toEqual(['click']);
    expect(fresh.ref.log).toEqual(['click']);
  });

  it('a forged state line ends a version 2 session and a pending command resolves uncertain', async () => {
    const key = newKeyHex();
    const { link, port } = await setup({ [ID]: key });
    const ended: string[] = [];
    link.on('ended', (e: { reason: string }) => ended.push(e.reason));
    const b = board(ID, key);
    await b.connect(port);
    await until(() => link.session(ID)?.state.face === 'default');
    b.mute = true;
    const pending = link.send(ID, { op: 'ping' });
    // The relay claims the board was armed. It has no session key to sign with.
    b.socket!.write(`s armed=1 usb=mounted dock=0 hid=usb link=lan face=default prog=0 n=99 ${'0'.repeat(32)}\n`);
    expect(await pending).toMatchObject({ ok: false, error: 'uncertain' });
    await until(() => link.session(ID) === null);
    expect(ended).toEqual(['bad_tag']);
  });

  it('an unsigned reply on a version 2 session is bad_tag, never ok', async () => {
    const key = newKeyHex();
    const { link, port } = await setup({ [ID]: key });
    const ended: string[] = [];
    link.on('ended', (e: { reason: string }) => ended.push(e.reason));
    const b = board(ID, key);
    await b.connect(port);
    b.arm();
    await until(() => link.session(ID)?.state.armed === true);
    b.mute = true;
    const pending = link.send(ID, { op: 'click' });
    await new Promise((r) => setTimeout(r, 20));
    b.socket!.write('r 1 ok\n');
    expect(await pending).toMatchObject({ ok: false, error: 'uncertain' });
    expect(ended).toEqual(['bad_tag']);
  });

  it('ignores a replayed state line whose n did not rise', async () => {
    const key = newKeyHex();
    const { link, port } = await setup({ [ID]: key });
    const b = board(ID, key);
    await b.connect(port);
    await until(() => link.session(ID) !== null);
    const armedLine = b.ref.pressButton()!;
    b.socket!.write(`${armedLine}\n`);
    await until(() => link.session(ID)?.state.armed === true);
    b.arm(false);
    await until(() => link.session(ID)?.state.armed === false);
    // Replaying the signed "armed=1" line: valid tag, old n. Ignored, session kept.
    b.socket!.write(`${armedLine}\n`);
    expect(await link.send(ID, { op: 'ping' })).toEqual({ ok: true });
    expect(link.session(ID)?.state).toMatchObject({ armed: false, face: 'stopped' });
  });

  it('face on a version 1 session returns invalid and the board receives no line', async () => {
    const key = newKeyHex();
    const { link, port } = await setup({ [ID]: key });
    const b = board(ID, key, 1);
    const seen: string[] = [];
    const orig = b.ref.receive.bind(b.ref);
    b.ref.receive = (line: string) => {
      seen.push(line);
      return orig(line);
    };
    await b.connect(port);
    await until(() => link.session(ID) !== null);
    const before = seen.length;
    expect(await link.send(ID, { op: 'face', name: 'working' })).toMatchObject({ ok: false, error: 'invalid' });
    expect(await link.send(ID, { op: 'prog', n: 2 })).toMatchObject({ ok: false, error: 'invalid' });
    expect(await link.send(ID, { op: 'label', text: 'hi' })).toMatchObject({ ok: false, error: 'invalid' });
    expect(seen.length).toBe(before);
    // The session's sequence counter was not used either: the next command is c 1.
    expect(await link.send(ID, { op: 'ping' })).toEqual({ ok: true });
    expect(seen[seen.length - 1]).toMatch(/^c 1 ping /);
  });

  it('sends typed text as typex on version 2 and as type on version 1', async () => {
    const k1 = newKeyHex();
    const k2 = newKeyHex();
    const ID2 = 'usba-ba9876543210';
    const { link, port } = await setup({ [ID]: k1, [ID2]: k2 });
    const lines: Record<string, string[]> = { [ID]: [], [ID2]: [] };
    const boardsByVersion = [board(ID, k1, 1), board(ID2, k2, 2)];
    for (const b of boardsByVersion) {
      const orig = b.ref.receive.bind(b.ref);
      b.ref.receive = (line: string) => {
        lines[b.ref.boardId].push(line);
        return orig(line);
      };
      await b.connect(port);
      b.arm();
    }
    await until(() => link.session(ID)?.state.armed === true && link.session(ID2)?.state.armed === true);
    expect(await link.send(ID, { op: 'type', text: 'secret' })).toEqual({ ok: true });
    expect(await link.send(ID2, { op: 'type', text: 'secret' })).toEqual({ ok: true });
    const hex = Buffer.from('secret').toString('hex');
    expect(lines[ID].some((l) => l.includes(`type ${hex}`))).toBe(true);
    expect(lines[ID2].some((l) => / typex [0-9a-f]+ /.test(l))).toBe(true);
    expect(lines[ID2].some((l) => l.includes(hex))).toBe(false);
    // Both boards typed the same text.
    expect(boardsByVersion[0].ref.log).toEqual([`type ${hex}`]);
    expect(boardsByVersion[1].ref.log).toEqual([`type ${hex}`]);
  });

  it('a button press during the reconnect window leaves the next session disarmed and refusing input', async () => {
    const key = newKeyHex();
    const { link, port } = await setup({ [ID]: key });
    const b = board(ID, key);
    await b.connect(port);
    b.arm();
    await until(() => link.session(ID)?.state.armed === true);
    expect(await link.send(ID, { op: 'click' })).toEqual({ ok: true });
    // The link drops; the board stays armed (no self-disarm for a minute).
    b.close();
    await until(() => link.session(ID) === null);
    expect(b.ref.armed).toBe(true);
    // The press arrives while the board is blocked connecting, before any session exists.
    b.pressWhileBlocked();
    await b.connect(port);
    await until(() => link.session(ID)?.state.face === 'stopped');
    expect(link.session(ID)?.state.armed).toBe(false);
    expect(await link.send(ID, { op: 'click' })).toMatchObject({ ok: false, error: 'disarmed' });
    expect(b.ref.log).toEqual(['click']);
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
