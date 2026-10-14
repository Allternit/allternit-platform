import { afterEach, describe, expect, it } from 'vitest';
import { UsbAgentManager, serialOf, statusOf, type StoredUsbAgent, type UsbAgentConfig, type UsbAgentStore } from './phone-usb-agent.js';
import type { Calibration } from './phone-usb-agent-pointer.js';
import type { SerialPort } from './phone-usb-agent-provision.js';
import { PhoneToolRunner } from './phone-tools.js';
import { SimBoard, SimPhone, fakeDecode, fakeJpeg, fakeUxplaySpawner, until } from './phone-usb-agent.testkit.js';

const BOARD = 'usba-00aa11bb22cc';

function memoryStore(): UsbAgentStore & { boards: StoredUsbAgent[]; cal: Map<string, Calibration> } {
  let config: UsbAgentConfig | null = null;
  const store = {
    boards: [] as StoredUsbAgent[],
    cal: new Map<string, Calibration>(),
    load: () => store.boards,
    save: (b: StoredUsbAgent[]) => {
      store.boards = b;
    },
    loadConfig: () => config,
    saveConfig: (c: UsbAgentConfig) => {
      config = c;
    },
    loadCalibration: (id: string) => store.cal.get(id) ?? null,
    saveCalibration: (id: string, c: Calibration | null) => {
      if (c) store.cal.set(id, structuredClone(c));
      else store.cal.delete(id);
    },
  };
  return store;
}

/** A board on the serial port that speaks the setup protocol. */
function fakeSerial(boardId = BOARD) {
  const settings: Record<string, string> = {};
  let committed = false;
  const out: string[] = ['boot: ESP-ROM chatter'];
  const port: SerialPort = {
    async writeLine(line) {
      const [, cmd, key, value] = line.split(' ');
      if (cmd === 'info') out.push(`prov info ${boardId} 1.0.0 provisioned=${committed ? 1 : 0}`);
      else if (cmd === 'set') {
        settings[key] = value;
        out.push('prov ok');
      } else if (cmd === 'commit') {
        committed = true;
        out.push('prov ok');
      } else out.push('prov err unknown');
    },
    async readLine() {
      return out.shift() ?? null;
    },
    close() {},
  };
  return { port, settings, committed: () => committed };
}

const managers: UsbAgentManager[] = [];
const boards: SimBoard[] = [];
afterEach(() => {
  for (const b of boards.splice(0)) b.close();
  for (const m of managers.splice(0)) m.stopAll();
});

async function rig(phone = new SimPhone(600, 1300, 1.04)) {
  const store = memoryStore();
  const ux = fakeUxplaySpawner();
  const serial = fakeSerial();
  const manager = new UsbAgentManager({
    store,
    sealer: { seal: (h) => `test:${h}`, open: (s) => s.slice(5) },
    spawnUxplay: ux.spawn,
    advertise: null,
    decode: (jpeg) => fakeDecode(jpeg),
    registerFile: (slot) => `/tmp/reg-${slot}`,
    openSerial: async () => serial.port,
    listSerial: () => ['/dev/cu.usbmodem101'],
    lanAddresses: () => ['192.168.1.20'],
    linkPort: 0,
  });
  managers.push(manager);
  const port = await manager.start();
  const info = await manager.setUp('/dev/cu.usbmodem101', { ssid: 'Home Wi-Fi', password: 'correct horse' }, 'Eoj iPhone');
  const board = new SimBoard(info.boardId, serial.settings.key, phone);
  boards.push(board);
  const uxplay = () => ux.spawned[ux.spawned.length - 1];
  const pushFrame = () => uxplay().write(fakeJpeg(phone.render()));
  board.onCommand = () => setTimeout(pushFrame, 5);
  return { manager, store, serial, board, phone, port, ux, uxplay, pushFrame, serialId: serialOf(info.boardId) };
}

describe('UsbAgentManager end to end (simulated board + iPhone)', () => {
  it('sets a board up over USB serial: Wi-Fi in hex, this computer, a fresh 32-byte key sealed at rest', async () => {
    const { serial, store, manager, ux } = await rig();
    expect(serial.committed()).toBe(true);
    expect(Buffer.from(serial.settings.ssid, 'hex').toString()).toBe('Home Wi-Fi');
    expect(Buffer.from(serial.settings.pass, 'hex').toString()).toBe('correct horse');
    expect(serial.settings.host).toBe('192.168.1.20');
    expect(serial.settings.key).toMatch(/^[0-9a-f]{64}$/);
    expect(store.boards[0]).toMatchObject({ boardId: BOARD, name: 'Eoj iPhone', sealedKey: `test:${serial.settings.key}`, slot: 0 });
    expect(manager.list()[0]).toMatchObject({ status: 'board_offline', receiverName: 'Allternit' });
    // The AirPlay receiver is PIN-protected with its own trust register.
    const args = ux.spawned[0].args;
    expect(args[args.indexOf('-pin') + 1]).toBe(manager.airplayPin());
    expect(args[args.indexOf('-reg') + 1]).toBe('/tmp/reg-0');
  });

  it('walks the statuses a user sees, then taps a button through calibration and closed-loop aiming', async () => {
    const r = await rig();
    await r.board.connect(r.port);
    await until(() => r.manager.list()[0].status === 'no_screen');
    r.pushFrame();
    await until(() => r.manager.list()[0].status === 'not_calibrated');

    // Screenshots work as soon as the phone mirrors; input waits for the button.
    const shot = await r.manager.call('phone.screenshot', r.serialId, {});
    expect(shot).toMatchObject({ ok: true, mimeType: 'image/jpeg', width: 120, height: 260 });
    expect(await r.manager.call('phone.tap', r.serialId, { x: 90, y: 130 })).toMatchObject({ ok: false, error: 'disarmed' });

    r.board.arm();
    await until(() => r.manager.list()[0].armed);
    expect(await r.manager.call('phone.tap', r.serialId, { x: 90, y: 130 })).toMatchObject({ ok: false, error: 'not_calibrated' });

    expect(await r.manager.calibrate(BOARD)).toEqual({ ok: true });
    expect(r.manager.list()[0].status).toBe('ready');
    expect(r.store.cal.get(BOARD)).toBeTruthy();

    // The simulated iPhone's pointer gain is 4% off; aiming still lands inside the 40x20 px button.
    const tap = await r.manager.call('phone.tap', r.serialId, { x: 90, y: 130 });
    expect(tap).toMatchObject({ ok: true, screenChanged: true });
    expect(r.phone.checked).toBe(true);
    const [tx, ty] = r.phone.taps[r.phone.taps.length - 1];
    expect(Math.abs(tx - 90)).toBeLessThan(4);
    expect(Math.abs(ty - 130)).toBeLessThan(4);

    // A tap on empty screen reports no change, so the bot knows it missed.
    const miss = await r.manager.call('phone.tap', r.serialId, { x: 30, y: 60 });
    expect(miss).toMatchObject({ ok: true, screenChanged: false });

    expect(await r.manager.call('phone.type', r.serialId, { text: 'hello world' })).toMatchObject({ ok: true });
    expect(r.phone.typed).toBe('hello world');
  }, 30_000);

  it('Stop disarms at once and blocks the next action; texts, calls and ARTEMIS tasks are refused', async () => {
    const r = await rig();
    await r.board.connect(r.port);
    r.pushFrame();
    r.board.arm();
    await until(() => r.manager.list()[0].armed);
    expect(await r.manager.stop(BOARD)).toEqual({ ok: true });
    await until(() => !r.manager.list()[0].armed);
    expect(await r.manager.call('phone.type', r.serialId, { text: 'x' })).toMatchObject({ ok: false, error: 'disarmed' });
    expect(await r.manager.call('phone.sms.send', r.serialId, { to: '5551234567', text: 'hi' })).toMatchObject({ ok: false, error: 'unsupported' });
    expect(await r.manager.call('phone.call.dial', r.serialId, { number: '5551234567' })).toMatchObject({ ok: false, error: 'unsupported' });
    expect(await r.manager.call('phone.task', r.serialId, { task: 'open settings' })).toMatchObject({ ok: false, error: 'unsupported' });
  });

  it('a locked phone and an unplugged board report clearly', async () => {
    const r = await rig();
    await r.board.connect(r.port);
    r.pushFrame();
    r.board.arm();
    r.board.setUsb('asleep');
    await until(() => r.manager.list()[0].status === 'phone_locked');
    expect(await r.manager.call('phone.type', r.serialId, { text: 'x' })).toMatchObject({ ok: false, error: 'device_locked' });
    r.board.setUsb('none');
    await until(() => r.manager.list()[0].status === 'phone_not_connected');
  });

  it('rejects bad tool arguments before touching the board', async () => {
    const r = await rig();
    await r.board.connect(r.port);
    r.pushFrame();
    r.board.arm();
    await until(() => r.manager.list()[0].armed);
    await r.manager.calibrate(BOARD);
    const before = r.board.ref.log.length;
    expect(await r.manager.call('phone.tap', r.serialId, { x: 500, y: 10 })).toMatchObject({ ok: false, error: 'invalid_args' });
    expect(await r.manager.call('phone.type', r.serialId, { text: 'héllo' })).toMatchObject({ ok: false, error: 'invalid_args' });
    expect(await r.manager.call('phone.key', r.serialId, { combo: 'cmd+rm -rf' })).toMatchObject({ ok: false });
    expect(r.board.ref.log.length).toBe(before);
  }, 30_000);

  it('forget drops the board, its key and its calibration; the board is refused afterwards', async () => {
    const r = await rig();
    await r.board.connect(r.port);
    await until(() => r.manager.list()[0].status !== 'board_offline');
    r.manager.forget(BOARD);
    expect(r.manager.list()).toEqual([]);
    expect(r.store.boards).toEqual([]);
    const again = new SimBoard(BOARD, r.serial.settings.key);
    boards.push(again);
    await expect(again.connect(r.port)).rejects.toThrow();
  });

  it('bots reach the iPhone through the same phone tools as Android', async () => {
    const r = await rig();
    await r.board.connect(r.port);
    r.pushFrame();
    await until(() => r.manager.serials().length === 1);
    const adb = Object.assign(async () => ({ stdout: '', stderr: '', code: 0 }), { binary: async () => Buffer.alloc(0) });
    const runner = new PhoneToolRunner({
      adb,
      gate: { request: async () => 'denied' },
      rates: { load: () => [], save: () => undefined },
      artemis: () => null,
      onlineSerials: () => ['192.168.1.30:5555'],
      usb: () => r.manager,
    });
    expect(await runner.call('phone.devices', {})).toMatchObject({
      ok: true,
      devices: [{ serial: '192.168.1.30:5555', platform: 'android' }, { serial: r.serialId, platform: 'ios', via: 'usb-agent' }],
    });
    expect(await runner.call('phone.screenshot', { serial: r.serialId })).toMatchObject({ ok: true, mimeType: 'image/jpeg' });
    // Two phones online: the serial is required.
    expect(await runner.call('phone.screenshot', {})).toMatchObject({ ok: false, error: 'device_required' });
  });
});

describe('statusOf', () => {
  const session = (armed: boolean, usb: 'mounted' | 'asleep' | 'none') => ({ boardId: BOARD, firmware: '1', resetReason: 'x', address: 'a', connectedAt: 0, state: { armed, usb } });
  it('reports the most blocking step first', () => {
    expect(statusOf(null, 'mirroring', true)).toBe('board_offline');
    expect(statusOf(session(true, 'none'), 'mirroring', true)).toBe('phone_not_connected');
    expect(statusOf(session(true, 'asleep'), 'mirroring', true)).toBe('phone_locked');
    expect(statusOf(session(true, 'mounted'), 'waiting', true)).toBe('no_screen');
    expect(statusOf(session(true, 'mounted'), 'mirroring', false)).toBe('not_calibrated');
    expect(statusOf(session(false, 'mounted'), 'mirroring', true)).toBe('disarmed');
    expect(statusOf(session(true, 'mounted'), 'mirroring', true)).toBe('ready');
  });
});
