import * as http from 'node:http';
import { afterEach, describe, expect, it } from 'vitest';
import { PHONE_GATEWAY_TOKEN_HEADER, startPhoneGateway } from './phone-gateway.js';
import { PhoneToolRunner, type AdbCall } from './phone-tools.js';
import { UsbAgentManager, type StoredUsbAgent, type UsbAgentConfig } from './phone-usb-agent.js';
import type { SerialPort } from './phone-usb-agent-provision.js';
import { SimBoard, fakeUxplaySpawner, until } from './phone-usb-agent.testkit.js';

const closers: Array<() => void> = [];
afterEach(() => closers.splice(0).forEach((c) => c()));

function runner() {
  const adb = (async () => ({ stdout: '', stderr: '', code: 0 })) as unknown as AdbCall;
  adb.binary = async () => Buffer.alloc(0);
  return new PhoneToolRunner({ adb, gate: { request: async () => 'denied' }, rates: { load: () => [], save: () => {} }, artemis: () => null, onlineSerials: () => [] });
}

/** A real UsbAgentManager with one board set up over a fake serial port, and that board connected as a SimBoard. */
async function boardRig() {
  const BOARD = 'usba-00aa11bb22cc';
  let saved: StoredUsbAgent[] = [];
  let config: UsbAgentConfig | null = null;
  const settings: Record<string, string> = {};
  const out: string[] = [];
  const port: SerialPort = {
    async writeLine(line) {
      const [, cmd, key, value] = line.split(' ');
      if (cmd === 'info') out.push(`prov info ${BOARD} 1.1.0 provisioned=0`);
      else {
        if (cmd === 'set') settings[key] = value;
        out.push('prov ok');
      }
    },
    readLine: async () => out.shift() ?? null,
    close() {},
  };
  const m = new UsbAgentManager({
    store: { load: () => saved, save: (b) => (saved = b), loadConfig: () => config, saveConfig: (c) => (config = c), loadCalibration: () => null, saveCalibration: () => {} },
    sealer: { seal: (h) => `test:${h}`, open: (s) => s.slice(5) },
    spawnUxplay: fakeUxplaySpawner().spawn,
    advertise: null,
    decode: () => null,
    registerFile: () => '/tmp/reg',
    openSerial: async () => port,
    listSerial: () => [],
    lanAddresses: () => ['192.168.1.20'],
    linkPort: 0,
  });
  closers.push(() => m.stopAll());
  const linkPort = await m.start();
  await m.setUp('/dev/x', { ssid: 'Home', password: 'correct horse' }, 'Eoj iPhone');
  const board = new SimBoard(BOARD, settings.key);
  closers.push(() => board.close());
  await board.connect(linkPort);
  const gw = await startPhoneGateway({ token: 'secret-token', runner: runner(), boards: { list: () => m.list(), stop: (id) => m.stop(id), forget: (id) => m.forget(id) } });
  closers.push(gw.close);
  const post = (path: string, body: unknown, token = 'secret-token') =>
    fetch(`${gw.url}${path}`, { method: 'POST', headers: { [PHONE_GATEWAY_TOKEN_HEADER]: token }, body: JSON.stringify(body) });
  return { BOARD, m, board, url: gw.url, post, saved: () => saved };
}

async function boot() {
  const adb = (async () => ({ stdout: '', stderr: '', code: 0 })) as unknown as AdbCall;
  adb.binary = async () => Buffer.alloc(0);
  const runner = new PhoneToolRunner({
    adb,
    gate: { request: async () => 'denied' },
    rates: { load: () => [], save: () => {} },
    artemis: () => null,
    onlineSerials: () => ['a:1'],
  });
  const gw = await startPhoneGateway({ token: 'secret-token', runner });
  closers.push(gw.close);
  return gw.url;
}

describe('phone gateway', () => {
  it('rejects requests without the token', async () => {
    const url = await boot();
    expect((await fetch(`${url}/phone/tools`)).status).toBe(403);
    expect((await fetch(`${url}/phone/tools`, { headers: { [PHONE_GATEWAY_TOKEN_HEADER]: 'nope' } })).status).toBe(403);
  });

  it('lists the tools the spec requires', async () => {
    const url = await boot();
    const body = (await (await fetch(`${url}/phone/tools`, { headers: { [PHONE_GATEWAY_TOKEN_HEADER]: 'secret-token' } })).json()) as { tools: Array<{ name: string }> };
    expect(body.tools.map((t) => t.name)).toEqual(
      expect.arrayContaining(['phone.screenshot', 'phone.tap', 'phone.type', 'phone.swipe', 'phone.open_app', 'phone.task', 'phone.sms.send', 'phone.call.dial']),
    );
  });

  it('runs a tool call, and the approval gate still applies through the gateway', async () => {
    const url = await boot();
    const post = (body: unknown) =>
      fetch(`${url}/phone/call`, { method: 'POST', headers: { [PHONE_GATEWAY_TOKEN_HEADER]: 'secret-token' }, body: JSON.stringify(body) });
    expect(await (await post({ name: 'phone.tap', arguments: { x: 1, y: 2 } })).json()).toEqual({ ok: true });
    expect(await (await post({ name: 'phone.sms.send', arguments: { to: '+15550102030', text: 'hi', subscription_id: 1 } })).json()).toMatchObject({ ok: false, error: 'approval_denied' });
    expect((await fetch(`${url}/phone/call`, { method: 'POST', headers: { [PHONE_GATEWAY_TOKEN_HEADER]: 'secret-token' }, body: '{' })).status).toBe(400);
  });

  it('board routes: refused without the token, the list, Stop disarms, Forget removes; nothing arms', async () => {
    const r = await boardRig();
    expect((await fetch(`${r.url}/phone/boards`)).status).toBe(403);
    expect((await r.post('/phone/board/stop', { boardId: r.BOARD }, 'nope')).status).toBe(403);
    expect((await r.post('/phone/board/forget', { boardId: r.BOARD }, '')).status).toBe(403);
    // Refused calls changed nothing.
    expect(r.m.list().map((b) => b.boardId)).toEqual([r.BOARD]);

    r.board.arm();
    await until(() => r.m.list()[0]?.armed === true);
    const list = (await (await fetch(`${r.url}/phone/boards`, { headers: { [PHONE_GATEWAY_TOKEN_HEADER]: 'secret-token' } })).json()) as { boards: Array<Record<string, unknown>> };
    expect(list.boards).toEqual([expect.objectContaining({ boardId: r.BOARD, serial: `usb:${r.BOARD}`, name: 'Eoj iPhone', online: true, armed: true, relay: 'none' })]);
    expect(list.boards[0]).not.toHaveProperty('address');

    expect((await r.post('/phone/board/stop', { boardId: 'usba-1' })).status).toBe(400);
    expect(await (await r.post('/phone/board/stop', { boardId: r.BOARD })).json()).toEqual({ ok: true });
    await until(() => r.m.list()[0]?.armed === false);
    expect(r.board.ref.armed).toBe(false);
    // An unknown board is offline, not an error page.
    expect(await (await r.post('/phone/board/stop', { boardId: 'usba-ffffffffffff' })).json()).toMatchObject({ ok: false, error: 'device_offline' });
    // There is no arm route.
    expect((await r.post('/phone/board/arm', { boardId: r.BOARD })).status).toBe(404);
    expect(r.board.ref.armed).toBe(false);

    expect(await (await r.post('/phone/board/forget', { boardId: r.BOARD })).json()).toEqual({ ok: true, cloud: 'none' });
    expect(r.m.list()).toEqual([]);
    expect(r.saved()).toEqual([]);
  });

  it('board list is empty, not an error, when no board lane is wired', async () => {
    const gw = await startPhoneGateway({ token: 't', runner: runner() });
    closers.push(gw.close);
    expect(await (await fetch(`${gw.url}/phone/boards`, { headers: { [PHONE_GATEWAY_TOKEN_HEADER]: 't' } })).json()).toEqual({ boards: [] });
  });

  it('is loopback only', async () => {
    const url = await boot();
    expect(new URL(url).hostname).toBe('127.0.0.1');
    void http;
  });
});
