import { EventEmitter } from 'node:events';
import { describe, expect, it, vi } from 'vitest';
import {
  nextPhoneState,
  PhoneDeviceManager,
  POST_PAIR_CONNECT_ATTEMPTS,
  type PhoneConnState,
  type PhoneStore,
  type StoredPhone,
} from './phone-device-manager.js';
import type { AdbCall } from './phone-tools.js';

describe('state machine', () => {
  const seen = (adbState: 'device' | 'offline' | 'unauthorized' | 'connecting' | 'unknown') => ({ type: 'adb_seen' as const, adbState });
  const cases: Array<[PhoneConnState, Parameters<typeof nextPhoneState>[1], PhoneConnState]> = [
    ['connecting', seen('device'), 'online'],
    ['lost', seen('device'), 'online'],
    ['reconnecting', seen('device'), 'online'],
    ['online', seen('offline'), 'lost'],
    ['online', { type: 'adb_missing' }, 'lost'],
    ['online', seen('unauthorized'), 'unauthorized'],
    ['online', seen('connecting'), 'online'],
    ['connecting', seen('connecting'), 'connecting'],
    ['lost', { type: 'reconnect_started' }, 'reconnecting'],
    ['reconnecting', { type: 'reconnect_failed' }, 'lost'],
    ['reconnecting', { type: 'adb_missing' }, 'reconnecting'],
    ['online', { type: 'reconnect_started' }, 'online'],
  ];
  it.each(cases)('%s + %j → %s', (from, event, to) => {
    expect(nextPhoneState(from, event)).toBe(to);
  });
});

/** A scriptable fake adb: the "phone" is a few mutable fields. */
function fakePhone() {
  const phone = {
    host: '192.168.1.20',
    connectPort: 41234,
    pairPort: 37123,
    pairingName: null as string | null,
    pairingCode: '123456',
    visible: false, // appears in `adb devices` once connected
    authorized: true,
    reachable: true,
    battery: 77,
    // mDNS lag: after a successful pair, the next N `mdns services` calls
    // don't list the connect service yet (what a real phone does for ~1–2 s).
    paired: false,
    hideConnectAfterPair: 0,
  };
  const calls: string[][] = [];
  const adb = (async (args: string[]) => {
    calls.push(args);
    const out = (stdout: string, code = 0) => ({ stdout, stderr: '', code });
    const [cmd] = args;
    if (cmd === 'mdns') {
      const lines = ['List of discovered mdns services'];
      const lagging = phone.paired && phone.hideConnectAfterPair > 0;
      if (lagging) phone.hideConnectAfterPair -= 1;
      if (phone.reachable && !lagging) lines.push(`adb-R5CT	_adb-tls-connect._tcp	${phone.host}:${phone.connectPort}`);
      if (phone.pairingName) lines.push(`${phone.pairingName}	_adb-tls-pairing._tcp	${phone.host}:${phone.pairPort}`);
      return out(lines.join('\n'));
    }
    if (cmd === 'pair') {
      phone.paired = args[2] === phone.pairingCode;
      return phone.paired ? out('Successfully paired to x [guid=adb-R5CT]') : out('Failed: Wrong password');
    }
    if (cmd === 'connect') {
      if (!phone.reachable) return out('failed to connect to ' + args[1]);
      phone.visible = true;
      return out('connected to ' + args[1]);
    }
    if (cmd === 'devices') {
      if (!phone.visible) return out('List of devices attached\n');
      const state = phone.reachable ? (phone.authorized ? 'device' : 'unauthorized') : 'offline';
      return out(`List of devices attached\n${phone.host}:${phone.connectPort}\t${state} model:Pixel_8\n`);
    }
    if (cmd === 'disconnect') {
      phone.visible = false;
      return out('');
    }
    if (cmd === 'shell') {
      const rest = args.slice(1).join(' ');
      if (rest.includes('ro.serialno')) return out('R5CT1234\n');
      if (rest.includes('ro.product.model')) return out('Pixel 8\n');
      if (rest.includes('ro.build.version.release')) return out('15\n');
      if (rest.includes('dumpsys battery')) return out(`  level: ${phone.battery}\n`);
      if (rest.includes('siminfo')) return out('Row: 0 _id=3, sim_id=0, display_name=Tello, carrier_name=Tello\nRow: 1 _id=4, sim_id=-1, display_name=, carrier_name=\n');
      if (rest.includes('multi_sim_sms_subscription')) return out('null\n');
    }
    return out('');
  }) as AdbCall;
  adb.binary = async () => Buffer.alloc(0);
  return { phone, adb, calls };
}

function memoryStore(initial: StoredPhone[] = []): PhoneStore & { saved: StoredPhone[][] } {
  const saved: StoredPhone[][] = [];
  let current = initial;
  return { saved, load: () => current, save: (p) => (saved.push(p), (current = p)) };
}

function makeManager(initial: StoredPhone[] = [], spawnScrcpy: ConstructorParameters<typeof PhoneDeviceManager>[0]['spawnScrcpy'] = null) {
  const { phone, adb, calls } = fakePhone();
  const store = memoryStore(initial);
  let now = 1_000_000;
  const manager = new PhoneDeviceManager({ adb, store, spawnScrcpy, now: () => now, sleep: async () => { now += 1000; } });
  return { manager, phone, calls, store, advance: (ms: number) => (now += ms) };
}

describe('pairing', () => {
  it('QR pairing: waits for the scan, pairs, connects, stores the phone', async () => {
    const { manager, phone, store } = makeManager();
    const qr = manager.startQrPairing();
    expect(qr.payload).toMatch(/^WIFI:T:ADB;S:allternit-/);
    expect(manager.pairingStatus()).toMatchObject({ phase: 'waiting_for_scan', qrPayload: qr.payload });

    phone.pairingName = qr.name; // the phone scanned the QR and now advertises the pairing service
    phone.pairingCode = qr.code;
    await vi.waitFor(() => expect(manager.pairingStatus().phase).toBe('done'));

    const [info] = manager.list();
    expect(info).toMatchObject({ id: 'R5CT1234', name: 'Pixel 8', model: 'Pixel 8', androidVersion: '15', battery: 77, state: 'online', serial: '192.168.1.20:41234' });
    expect(info.sims).toEqual([{ subscriptionId: 3, slot: 0, carrier: 'Tello' }]);
    expect(store.saved.at(-1)?.[0]).toMatchObject({ id: 'R5CT1234', host: '192.168.1.20', port: 41234 });
    expect(manager.onlineSerials()).toEqual(['192.168.1.20:41234']);
  });

  it('QR pairing gives up after the timeout', async () => {
    const { manager } = makeManager();
    manager.startQrPairing();
    await vi.waitFor(() => expect(manager.pairingStatus().phase).toBe('failed'));
    expect(manager.pairingStatus().error).toMatch(/not scanned/);
  });

  it('a stale QR run is cancelled when a new one starts', async () => {
    const { manager, phone } = makeManager();
    const first = manager.startQrPairing();
    const second = manager.startQrPairing();
    phone.pairingName = first.name;
    phone.pairingCode = first.code;
    phone.pairingName = second.name;
    phone.pairingCode = second.code;
    await vi.waitFor(() => expect(manager.pairingStatus().phase).toBe('done'));
    expect(manager.list()).toHaveLength(1);
  });

  it('six-digit fallback pairs by address + code', async () => {
    const { manager } = makeManager();
    expect((await manager.pairWithCode('192.168.1.20:37123', '123456')).phase).toBe('done');
    expect(manager.list()[0].state).toBe('online');
  });

  it('six-digit fallback rejects bad input and wrong codes without connecting', async () => {
    const { manager, calls } = makeManager();
    expect((await manager.pairWithCode('nonsense', '123456')).phase).toBe('failed');
    expect((await manager.pairWithCode('192.168.1.20:37123', '12')).phase).toBe('failed');
    const wrong = await manager.pairWithCode('192.168.1.20:37123', '000000');
    expect(wrong.phase).toBe('failed');
    expect(wrong.error).toMatch(/rejected/);
    expect(calls.some((c) => c[0] === 'connect')).toBe(false);
    expect(manager.list()).toHaveLength(0);
  });

  it('retries the post-pair connect while the connect advertisement lags mDNS (#1632)', async () => {
    const { manager, phone, calls } = makeManager();
    phone.hideConnectAfterPair = 2;
    const status = await manager.pairWithCode('192.168.1.20:37123', '123456');
    expect(status.phase).toBe('done');
    expect(manager.list()[0].state).toBe('online');
    const afterPair = calls.slice(calls.findIndex((c) => c[0] === 'pair') + 1);
    expect(afterPair.filter((c) => c[0] === 'mdns')).toHaveLength(3);
    expect(afterPair.filter((c) => c[0] === 'connect')).toEqual([['connect', '192.168.1.20:41234']]);
  });

  it('QR pairing also survives the mDNS lag', async () => {
    const { manager, phone } = makeManager();
    phone.hideConnectAfterPair = POST_PAIR_CONNECT_ATTEMPTS - 1;
    const qr = manager.startQrPairing();
    phone.pairingName = qr.name;
    phone.pairingCode = qr.code;
    await vi.waitFor(() => expect(manager.pairingStatus().phase).toBe('done'));
    expect(manager.list()).toHaveLength(1);
  });

  it('gives up after a bounded number of connect attempts, without a trailing wait', async () => {
    const { phone, adb, calls } = fakePhone();
    const sleeps: number[] = [];
    const manager = new PhoneDeviceManager({ adb, store: memoryStore(), spawnScrcpy: null, now: () => 0, sleep: async (ms) => { sleeps.push(ms); } });
    phone.hideConnectAfterPair = 99;
    const status = await manager.pairWithCode('192.168.1.20:37123', '123456');
    expect(status.phase).toBe('failed');
    expect(status.error).toMatch(/could not connect/);
    const afterPair = calls.slice(calls.findIndex((c) => c[0] === 'pair') + 1);
    expect(afterPair.filter((c) => c[0] === 'mdns')).toHaveLength(POST_PAIR_CONNECT_ATTEMPTS);
    expect(afterPair.some((c) => c[0] === 'connect')).toBe(false);
    expect(sleeps).toEqual(Array(POST_PAIR_CONNECT_ATTEMPTS - 1).fill(1_500));
    expect(manager.list()).toHaveLength(0);
  });

  it('cancelling during the connect retries stops them and leaves the status idle', async () => {
    const { phone, adb, calls } = fakePhone();
    phone.hideConnectAfterPair = 99;
    let manager!: PhoneDeviceManager;
    manager = new PhoneDeviceManager({ adb, store: memoryStore(), spawnScrcpy: null, now: () => 0, sleep: async () => { manager.cancelPairing(); } });
    const status = await manager.pairWithCode('192.168.1.20:37123', '123456');
    expect(status.phase).toBe('idle');
    const afterPair = calls.slice(calls.findIndex((c) => c[0] === 'pair') + 1);
    expect(afterPair.filter((c) => c[0] === 'mdns')).toHaveLength(1);
    expect(manager.list()).toHaveLength(0);
  });

  it('reports when the phone connects but never authorizes this computer', async () => {
    const { manager, phone } = makeManager();
    phone.authorized = false;
    const status = await manager.pairWithCode('192.168.1.20:37123', '123456');
    expect(status.phase).toBe('failed');
    expect(status.error).toMatch(/authorize/);
  });
});

describe('keep-alive and reconnect', () => {
  const stored: StoredPhone = { id: 'R5CT1234', name: 'Pixel 8', host: '192.168.1.20', port: 41234, pairedAt: 1 };

  it('auto-reconnects stored phones on start-up', async () => {
    const { manager, phone } = makeManager([stored]);
    expect(manager.list()[0].state).toBe('lost');
    phone.visible = false;
    await manager.poll();
    expect(manager.list()[0].state).toBe('online');
    expect(manager.list()[0].battery).toBe(77);
  });

  it('connect → lost → reconnecting → online, with backoff while the phone is away', async () => {
    const { manager, phone, advance, calls } = makeManager([stored]);
    await manager.poll();
    expect(manager.list()[0].state).toBe('online');

    phone.reachable = false; // Wi-Fi off / out of range
    await manager.poll();
    expect(manager.list()[0].state).toBe('lost');
    const connectsAfterFirstFailure = calls.filter((c) => c[0] === 'connect').length;

    await manager.poll(); // inside the backoff window: no hammering
    expect(calls.filter((c) => c[0] === 'connect').length).toBe(connectsAfterFirstFailure);
    expect(manager.onlineSerials()).toEqual([]);

    phone.reachable = true;
    phone.connectPort = 45555; // Wireless debugging was toggled: new port
    advance(120_000);
    await manager.poll();
    expect(manager.list()[0]).toMatchObject({ state: 'online', serial: '192.168.1.20:45555', port: 45555 });
  });

  it('an unauthorized phone is reported, not treated as online', async () => {
    const { manager, phone } = makeManager([stored]);
    phone.authorized = false;
    phone.visible = true;
    await manager.poll();
    expect(manager.list()[0].state).toBe('unauthorized');
    expect(manager.onlineSerials()).toEqual([]);
  });

  it('forget removes it from the store and disconnects', async () => {
    const { manager, store, calls } = makeManager([stored]);
    await manager.poll();
    manager.forget('R5CT1234');
    expect(manager.list()).toHaveLength(0);
    expect(store.saved.at(-1)).toEqual([]);
    expect(calls.some((c) => c[0] === 'disconnect')).toBe(true);
  });

  it('emits devices when state changes', async () => {
    const { manager } = makeManager([stored]);
    const listener = vi.fn();
    manager.on('devices', listener);
    await manager.poll();
    expect(listener).toHaveBeenCalled();
  });
});

describe('default SMS subscription', () => {
  const stored: StoredPhone = { id: 'R5CT1234', name: 'Pixel 8', host: '192.168.1.20', port: 41234, pairedAt: 1 };
  it('uses the only SIM when there is no explicit setting, and refuses to guess with several', async () => {
    const { manager } = makeManager([stored]);
    await manager.poll();
    expect(await manager.defaultSmsSubscription('192.168.1.20:41234')).toBe(3);
  });
});

describe('scrcpy', () => {
  const stored: StoredPhone = { id: 'R5CT1234', name: 'Pixel 8', host: '192.168.1.20', port: 41234, pairedAt: 1 };

  it('says how to install scrcpy when it is missing', async () => {
    const { manager } = makeManager([stored]);
    await manager.poll();
    expect(manager.startScrcpy('192.168.1.20:41234')).toMatchObject({ ok: false, error: expect.stringContaining('scrcpy is not installed') });
  });

  it('starts one stream per phone and kills it on stopAll', async () => {
    const child = Object.assign(new EventEmitter(), { exitCode: null as number | null, kill: vi.fn() });
    const spawn = vi.fn(() => child as never);
    const { manager } = makeManager([stored], spawn);
    await manager.poll();
    expect(manager.startScrcpy('192.168.1.20:41234')).toEqual({ ok: true });
    expect(manager.startScrcpy('192.168.1.20:41234')).toEqual({ ok: true });
    expect(spawn).toHaveBeenCalledTimes(1);
    expect(manager.list()[0].streaming).toBe(true);
    manager.stopAll();
    expect(child.kill).toHaveBeenCalledWith('SIGTERM');
  });

  it('refuses to stream a phone that is not online', () => {
    const { manager } = makeManager([stored], vi.fn() as never);
    expect(manager.startScrcpy('192.168.1.20:41234')).toMatchObject({ ok: false });
  });
});

describe('agent busy flag', () => {
  const stored: StoredPhone = { id: 'R5CT1234', name: 'Pixel 8', host: '192.168.1.20', port: 41234, pairedAt: 1 };

  it('reflects in-flight bot/ARTEMIS actions and emits devices when it flips', async () => {
    const { manager } = makeManager([stored]);
    await manager.poll();
    const serial = '192.168.1.20:41234';
    expect(manager.list()[0].agentBusy).toBe(false);

    const listener = vi.fn();
    manager.on('devices', listener);
    manager.noteAgentActivity(serial, 1);
    manager.noteAgentActivity(serial, 1); // overlapping actions: still one flag
    expect(manager.list()[0].agentBusy).toBe(true);
    expect(listener).toHaveBeenCalledTimes(1); // only the first flip emits

    manager.noteAgentActivity(serial, -1);
    expect(manager.list()[0].agentBusy).toBe(true);
    manager.noteAgentActivity(serial, -1);
    expect(manager.list()[0].agentBusy).toBe(false);
    expect(listener).toHaveBeenCalledTimes(2);
  });

  it('never goes negative and ignores unknown serials', async () => {
    const { manager } = makeManager([stored]);
    await manager.poll();
    manager.noteAgentActivity('192.168.1.20:41234', -1);
    expect(manager.list()[0].agentBusy).toBe(false);
    manager.noteAgentActivity('nope:1234', 1);
    expect(manager.list()[0].agentBusy).toBe(false);
  });
});
