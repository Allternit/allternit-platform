/**
 * Allternit Link, lane D: cloud relay registration, Forget and the start-up
 * sync, against a real UsbAgentManager, a SimBoard over TCP, a fake USB
 * serial port that follows the firmware's `prov` rules, and a local HTTP
 * server that serves the cloud-api `link_boards` routes in the contract's
 * shapes (hardware/link/README.md section 4.2). Only the network and the
 * serial port are fake.
 */
import * as http from 'node:http';
import { createHash, randomBytes } from 'node:crypto';
import { afterEach, describe, expect, it } from 'vitest';
import { UsbAgentManager, type StoredUsbAgent, type UsbAgentConfig, type UsbAgentStore } from './phone-usb-agent.js';
import { httpLinkBoardCloud, LinkBoardCloudError, type LinkBoardCloud } from './phone-usb-agent-cloud.js';
import type { Calibration } from './phone-usb-agent-pointer.js';
import type { SerialPort } from './phone-usb-agent-provision.js';
import { PHONE_GATEWAY_TOKEN_HEADER, startPhoneGateway } from './phone-gateway.js';
import { PhoneToolRunner, type AdbCall } from './phone-tools.js';
import { SimBoard, fakeUxplaySpawner, until } from './phone-usb-agent.testkit.js';

const BOARD = 'usba-00aa11bb22cc';
const RUNTIME = 'rt-desk-1';
const DEVICE_TOKEN = 'adt_test_device_token';

// ── the cloud-api edge ──────────────────────────────────────────────────────

interface Row {
  boardId: string;
  name: string | null;
  runtimeDeviceId: string;
  tokenHash: string;
  createdAt: string;
  revokedAt: string | null;
}

/** cloud-api's link_boards routes for one account, as section 4.2 states them. */
async function fakeCloud() {
  const rows = new Map<string, Row>();
  const requests: Array<{ method: string; path: string; body: unknown; authorization: string | undefined }> = [];
  const issued: string[] = [];
  let down = false;
  const server = http.createServer((req, res) => {
    let raw = '';
    req.on('data', (c: Buffer) => (raw += c.toString()));
    req.on('end', () => {
      const body = raw ? (JSON.parse(raw) as Record<string, unknown>) : undefined;
      const path = req.url ?? '';
      requests.push({ method: req.method ?? '', path, body, authorization: req.headers.authorization });
      const reply = (status: number, b: unknown) => {
        res.writeHead(status, { 'content-type': 'application/json' });
        res.end(JSON.stringify(b));
      };
      if (down) return reply(503, { error: 'unavailable', message: 'The service is restarting.' });
      if (req.headers.authorization !== `Bearer ${DEVICE_TOKEN}`) return reply(401, { error: 'unauthorized', message: 'Sign in.' });
      if (req.method === 'POST' && path === '/api/v1/link-boards') {
        const boardId = String(body?.boardId ?? '');
        if (!/^usba-[0-9a-f]{12}$/.test(boardId)) return reply(400, { error: 'bad_request', message: 'Bad boardId.' });
        if (body?.runtimeDeviceId !== RUNTIME) return reply(404, { error: 'not_found', message: 'No such runtime.' });
        const token = randomBytes(32).toString('hex');
        issued.push(token);
        rows.set(boardId, {
          boardId,
          name: typeof body?.name === 'string' ? body.name : null,
          runtimeDeviceId: RUNTIME,
          tokenHash: createHash('sha256').update(Buffer.from(token, 'hex')).digest('hex'),
          createdAt: new Date().toISOString(),
          revokedAt: null,
        });
        return reply(200, { boardId, relayToken: token, runtimeDeviceId: RUNTIME });
      }
      if (req.method === 'GET' && path === '/api/v1/link-boards') {
        return reply(200, {
          boards: [...rows.values()].map((r) => ({ boardId: r.boardId, serial: `usb:${r.boardId}`, name: r.name, runtimeDeviceId: r.runtimeDeviceId, createdAt: r.createdAt, lastSeenAt: null, revokedAt: r.revokedAt })),
        });
      }
      const del = /^\/api\/v1\/link-boards\/([^/]+)$/.exec(path);
      if (req.method === 'DELETE' && del) {
        const row = rows.get(decodeURIComponent(del[1]));
        if (!row) return reply(404, { error: 'not_found', message: 'No such board.' });
        row.revokedAt = new Date().toISOString();
        return reply(200, { revoked: true });
      }
      reply(404, { error: 'not_found' });
    });
  });
  await new Promise<void>((r) => server.listen(0, '127.0.0.1', r));
  const port = (server.address() as { port: number }).port;
  closers.push(() => server.close());
  return {
    url: `http://127.0.0.1:${port}`,
    rows,
    requests,
    issued,
    setDown: (v: boolean) => (down = v),
  };
}

function cloudClient(url: string, signedIn = true): LinkBoardCloud {
  return httpLinkBoardCloud({
    cloudUrl: () => url,
    headers: (): Record<string, string> => (signedIn ? { Authorization: `Bearer ${DEVICE_TOKEN}` } : {}),
    runtimeDeviceId: () => (signedIn ? RUNTIME : null),
  });
}

// ── the board's USB serial edge (the firmware's prov rules) ─────────────────

function fakeSerial(firmware = '1.1.0', boardId = BOARD) {
  const relayFirmware = firmware !== '1.0.0';
  const pending: Record<string, string> = {};
  let saved: Record<string, string> | null = null;
  const written: string[] = [];
  const out: string[] = ['boot: ESP-ROM chatter'];
  const port: SerialPort = {
    async writeLine(line) {
      written.push(line);
      const [, cmd, key, value] = line.split(' ');
      if (cmd === 'info') return void out.push(`prov info ${boardId} ${firmware} provisioned=${saved ? 1 : 0}`);
      if (cmd === 'commit') {
        saved = { ...pending };
        // The relay is stored only when both its host and token were set.
        if (!(saved.relay && saved.rtok)) {
          delete saved.relay;
          delete saved.rtok;
        }
        return void out.push('prov ok');
      }
      if (cmd !== 'set') return void out.push('prov err unknown');
      const extra = /^(ssid|pass)[1-3]$/.test(key);
      if (!relayFirmware && (key === 'relay' || key === 'rtok' || extra)) return void out.push('prov err unknown setting');
      if (key === 'relay' && !/^[a-z0-9.-]{1,63}$/.test(value)) return void out.push('prov err bad value');
      if (key === 'rtok' && !/^[0-9a-f]{64}$/.test(value)) return void out.push('prov err bad value');
      pending[key] = value;
      out.push('prov ok');
    },
    async readLine() {
      return out.shift() ?? null;
    },
    close() {},
  };
  return { port, written, saved: () => saved };
}

// ── Desktop's on-disk store, in memory ──────────────────────────────────────

function memoryStore(): UsbAgentStore & { boards: StoredUsbAgent[]; config: () => UsbAgentConfig | null } {
  let config: UsbAgentConfig | null = null;
  const cal = new Map<string, Calibration>();
  const store = {
    boards: [] as StoredUsbAgent[],
    config: () => config,
    load: () => store.boards,
    save: (b: StoredUsbAgent[]) => {
      store.boards = structuredClone(b);
    },
    loadConfig: () => (config ? structuredClone(config) : null),
    saveConfig: (c: UsbAgentConfig) => {
      config = structuredClone(c);
    },
    loadCalibration: (id: string) => cal.get(id) ?? null,
    saveCalibration: (id: string, c: Calibration | null) => {
      if (c) cal.set(id, c);
      else cal.delete(id);
    },
  };
  return store;
}

const closers: Array<() => void> = [];
afterEach(() => closers.splice(0).forEach((c) => c()));

function manager(store: ReturnType<typeof memoryStore>, serial: ReturnType<typeof fakeSerial>, cloud?: LinkBoardCloud) {
  const m = new UsbAgentManager({
    store,
    sealer: { seal: (h) => `test:${h}`, open: (s) => s.slice(5) },
    spawnUxplay: fakeUxplaySpawner().spawn,
    advertise: null,
    decode: () => null,
    registerFile: (slot) => `/tmp/reg-${slot}`,
    openSerial: async () => serial.port,
    listSerial: () => ['/dev/cu.usbmodem101'],
    lanAddresses: () => ['192.168.1.20'],
    linkPort: 0,
    ...(cloud ? { cloud } : {}),
  });
  closers.push(() => m.stopAll());
  return m;
}

const keyOf = (serial: ReturnType<typeof fakeSerial>) => serial.saved()?.key ?? '';

async function connect(boardId: string, keyHex: string, port: number) {
  const board = new SimBoard(boardId, keyHex);
  closers.push(() => board.close());
  await board.connect(port);
  return board;
}

// ── httpLinkBoardCloud ──────────────────────────────────────────────────────

describe('httpLinkBoardCloud against the link_boards routes', () => {
  it('sends the right method, path, credential and body, and reads the answers', async () => {
    const cloud = await fakeCloud();
    const client = cloudClient(cloud.url);

    const reg = await client.register(BOARD, '  Eoj iPhone  ');
    expect(reg).toEqual({ relayToken: cloud.issued[0], relayHost: '127.0.0.1' });
    expect(cloud.requests[0]).toEqual({
      method: 'POST',
      path: '/api/v1/link-boards',
      body: { boardId: BOARD, name: 'Eoj iPhone', runtimeDeviceId: RUNTIME },
      authorization: `Bearer ${DEVICE_TOKEN}`,
    });

    expect(await client.list()).toEqual([{ boardId: BOARD, revokedAt: null }]);
    expect(cloud.requests[1]).toMatchObject({ method: 'GET', path: '/api/v1/link-boards' });

    await client.revoke(BOARD);
    expect(cloud.requests[2]).toMatchObject({ method: 'DELETE', path: `/api/v1/link-boards/${BOARD}`, body: undefined });
    expect(cloud.rows.get(BOARD)?.revokedAt).not.toBeNull();
    // A board this account has no row for: nothing of it is live, so revoke resolves.
    await expect(client.revoke('usba-ffffffffffff')).resolves.toBeUndefined();
  });

  it('refuses without a sign-in and reports cloud errors without any token in them', async () => {
    const cloud = await fakeCloud();
    await expect(cloudClient(cloud.url, false).register(BOARD, 'x')).rejects.toMatchObject({ code: 'signed_out' });
    expect(cloud.requests).toEqual([]);
    cloud.setDown(true);
    const err = await cloudClient(cloud.url).register(BOARD, 'x').catch((e: unknown) => e);
    expect(err).toBeInstanceOf(LinkBoardCloudError);
    expect(err).toMatchObject({ code: 'http', status: 503 });
    expect((err as Error).message).toContain('503');
    await expect(cloudClient(cloud.url).revoke(BOARD)).rejects.toMatchObject({ status: 503 });
  });
});

// ── setup, forget, start ────────────────────────────────────────────────────

describe('Link board setup with the cloud relay', () => {
  it('signed in: registers the board, writes relay and rtok over serial, keeps no token on the computer', async () => {
    const cloud = await fakeCloud();
    const store = memoryStore();
    const serial = fakeSerial('1.1.0');
    const m = manager(store, serial, cloudClient(cloud.url));
    const port = await m.start();

    const info = await m.setUp('/dev/cu.usbmodem101', { ssid: 'Home Wi-Fi', password: 'correct horse' }, 'Eoj iPhone', {
      networks: [{ ssid: 'Office', password: 'office pass 1' }],
    });

    const token = cloud.issued[0];
    expect(serial.saved()).toMatchObject({ relay: '127.0.0.1', rtok: token, ssid1: Buffer.from('Office').toString('hex'), pass1: Buffer.from('office pass 1').toString('hex') });
    expect(info.relay).toBe('registered');
    expect(info.relayNote).toBeUndefined();
    expect(store.boards[0]).toMatchObject({ boardId: BOARD, relay: 'registered' });
    // The cloud stores only the hash; Desktop stores nothing of the token.
    expect(cloud.rows.get(BOARD)?.tokenHash).toBe(createHash('sha256').update(Buffer.from(token, 'hex')).digest('hex'));
    expect(JSON.stringify([store.boards, store.config()])).not.toContain(token);

    // The board still comes in over the LAN with the key setup wrote.
    await connect(BOARD, keyOf(serial), port);
    await until(() => m.list()[0]?.online === true);
  });

  it('cloud fails: setup still finishes with a working LAN board and relay none', async () => {
    const cloud = await fakeCloud();
    cloud.setDown(true);
    const store = memoryStore();
    const serial = fakeSerial('1.1.0');
    const m = manager(store, serial, cloudClient(cloud.url));
    const port = await m.start();

    const info = await m.setUp('/dev/cu.usbmodem101', { ssid: 'Home Wi-Fi', password: 'correct horse' }, 'Eoj iPhone');

    expect(info.relay).toBe('none');
    expect(info.relayNote).toContain('503');
    expect(serial.written.some((l) => l.startsWith('prov set relay') || l.startsWith('prov set rtok'))).toBe(false);
    expect(serial.saved()).toMatchObject({ host: '192.168.1.20' });
    expect(store.boards[0].relay).toBeUndefined();
    const board = await connect(BOARD, keyOf(serial), port);
    board.arm();
    await until(() => m.list()[0]?.armed === true);
  });

  it('signed out, or firmware without the relay: no cloud row is made and setup finishes on the LAN', async () => {
    const cloud = await fakeCloud();
    const outStore = memoryStore();
    const out = manager(outStore, fakeSerial('1.1.0'), cloudClient(cloud.url, false));
    await out.start();
    expect(await out.setUp('/dev/cu.usbmodem101', { ssid: 'Home', password: 'correct horse' }, 'A')).toMatchObject({ relay: 'none', relayNote: expect.stringContaining('Sign in') });

    const oldSerial = fakeSerial('1.0.0');
    const old = manager(memoryStore(), oldSerial, cloudClient(cloud.url));
    await old.start();
    expect(await old.setUp('/dev/cu.usbmodem101', { ssid: 'Home', password: 'correct horse' }, 'B')).toMatchObject({ relay: 'none', relayNote: expect.stringContaining('1.0.0') });
    expect(cloud.requests.filter((r) => r.method === 'POST')).toEqual([]);
    // Firmware 1.0.0 saves one network: asking for more fails loudly instead of dropping it.
    await expect(old.setUp('/dev/cu.usbmodem101', { ssid: 'Home', password: 'correct horse' }, 'B', { networks: [{ ssid: 'Office', password: 'office pass 1' }] })).rejects.toThrow(/one Wi-Fi network/);
  });

  it('forget revokes the cloud row and the local key stops working', async () => {
    const cloud = await fakeCloud();
    const store = memoryStore();
    const serial = fakeSerial('1.1.0');
    const m = manager(store, serial, cloudClient(cloud.url));
    const port = await m.start();
    await m.setUp('/dev/cu.usbmodem101', { ssid: 'Home', password: 'correct horse' }, 'Eoj iPhone');
    await connect(BOARD, keyOf(serial), port);
    await until(() => m.list()[0]?.online === true);

    expect(await m.forget(BOARD)).toEqual({ ok: true, cloud: 'revoked' });
    expect(cloud.requests.at(-1)).toMatchObject({ method: 'DELETE', path: `/api/v1/link-boards/${BOARD}` });
    expect(cloud.rows.get(BOARD)?.revokedAt).not.toBeNull();
    expect(m.list()).toEqual([]);
    expect(store.boards).toEqual([]);
    await expect(connect(BOARD, keyOf(serial), port)).rejects.toThrow();
  });

  it('forget with the cloud down: reported, key gone at once, revoke retried at the next start', async () => {
    const cloud = await fakeCloud();
    const store = memoryStore();
    const serial = fakeSerial('1.1.0');
    const m = manager(store, serial, cloudClient(cloud.url));
    const port = await m.start();
    await m.setUp('/dev/cu.usbmodem101', { ssid: 'Home', password: 'correct horse' }, 'Eoj iPhone');

    cloud.setDown(true);
    const res = await m.forget(BOARD);
    expect(res).toMatchObject({ ok: true, cloud: 'failed', message: expect.stringContaining('could not revoke') });
    expect(store.boards).toEqual([]);
    await expect(connect(BOARD, keyOf(serial), port)).rejects.toThrow();
    expect(cloud.rows.get(BOARD)?.revokedAt).toBeNull();
    expect(store.config()?.pendingRevokes).toEqual([BOARD]);
    m.stopAll();

    cloud.setDown(false);
    const next = manager(store, serial, cloudClient(cloud.url));
    await next.start();
    await until(() => cloud.rows.get(BOARD)?.revokedAt !== null);
    expect(await next.syncCloud()).toEqual({ removed: [] });
    expect(store.config()?.pendingRevokes).toBeUndefined();
  });

  it('a board forgotten on another surface (cloud row revoked) is gone after start, and its key is refused', async () => {
    const cloud = await fakeCloud();
    const store = memoryStore();
    const serial = fakeSerial('1.1.0');
    const first = manager(store, serial, cloudClient(cloud.url));
    await first.start();
    await first.setUp('/dev/cu.usbmodem101', { ssid: 'Home', password: 'correct horse' }, 'Eoj iPhone');
    first.stopAll();

    // Forget on the web while this Desktop is off.
    await cloudClient(cloud.url).revoke(BOARD);

    const next = manager(store, serial, cloudClient(cloud.url));
    expect(next.list().map((b) => b.boardId)).toEqual([BOARD]);
    const port = await next.start();
    await until(() => next.list().length === 0);
    expect(store.boards).toEqual([]);
    await expect(connect(BOARD, keyOf(serial), port)).rejects.toThrow();
  });
});

describe('status carried to other surfaces', () => {
  it('has the LinkBoardStatus fields only: no board key, no relay token, no LAN address', async () => {
    const cloud = await fakeCloud();
    const store = memoryStore();
    const serial = fakeSerial('1.1.0');
    const m = manager(store, serial, cloudClient(cloud.url));
    const port = await m.start();
    await m.setUp('/dev/cu.usbmodem101', { ssid: 'Home', password: 'correct horse' }, 'Eoj iPhone');
    await connect(BOARD, keyOf(serial), port);
    await until(() => m.list()[0]?.online === true);

    const adb = (async () => ({ stdout: '', stderr: '', code: 0 })) as unknown as AdbCall;
    adb.binary = async () => Buffer.alloc(0);
    const runner = new PhoneToolRunner({ adb, gate: { request: async () => 'denied' }, rates: { load: () => [], save: () => {} }, artemis: () => null, onlineSerials: () => [] });
    const gw = await startPhoneGateway({ token: 'gw-token', runner, boards: { list: () => m.list(), stop: (id) => m.stop(id), forget: (id) => m.forget(id) } });
    closers.push(gw.close);
    const raw = await (await fetch(`${gw.url}/phone/boards`, { headers: { [PHONE_GATEWAY_TOKEN_HEADER]: 'gw-token' } })).text();
    const body = JSON.parse(raw) as { boards: Array<Record<string, unknown>> };

    expect(Object.keys(body.boards[0]).sort()).toEqual(
      ['armed', 'boardId', 'calibrated', 'dock', 'eyes', 'face', 'firmware', 'firmwareUpdate', 'hid', 'link', 'name', 'online', 'phone', 'prog', 'protocol', 'relay', 'screen', 'serial', 'status', 'usb'].sort(),
    );
    expect(body.boards[0]).toMatchObject({ boardId: BOARD, online: true, relay: 'registered', link: 'lan', protocol: 2 });
    for (const secret of [keyOf(serial), cloud.issued[0], 'sealedKey', '192.168.1.20', '127.0.0.1']) expect(raw).not.toContain(secret);
    // The Desktop UI's own snapshot (phone:usb-status) carries no secret either.
    const ui = JSON.stringify(m.list());
    expect(ui).not.toContain(keyOf(serial));
    expect(ui).not.toContain(cloud.issued[0]);
  });
});
