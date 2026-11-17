import { afterEach, describe, expect, it, vi } from 'vitest';
import { NEEDS_YOU_SENTENCES } from './phone-link-health.js';
import {
  BT_MAC_NOT_IN_BUILD,
  BT_MAC_NOT_ON_THIS_OS,
  BT_MAC_OFF,
  BT_MAC_REFUSED,
  BtHidClient,
  BtHidError,
  btHidBinaryPath,
  btMacHealth,
  btMacLane,
  btMacSettings,
  btMacTransport,
  readBtHidState,
  resumeBtMacAtLaunch,
  type BtHidProcess,
  type BtHidState,
} from './phone-bt-mac.js';

type Answer = Record<string, unknown> | 'hang' | 'exit';

/**
 * A fake allternit-bthid process (the edge: the native binary). `answer` gets
 * each request and returns the reply, 'hang' (never answers) or 'exit' (the
 * process dies).
 */
function fakeBinary(answer: (req: Record<string, unknown>) => Answer) {
  const spawned: Array<{ requests: Array<Record<string, unknown>>; closed: boolean; killed: boolean; emit: (line: Record<string, unknown>) => void }> = [];
  const spawn = (): BtHidProcess => {
    let onLine: (l: string) => void = () => {};
    let onExit: (c: number | null) => void = () => {};
    const me = { requests: [] as Array<Record<string, unknown>>, closed: false, killed: false, emit: (l: Record<string, unknown>) => onLine(JSON.stringify(l)) };
    spawned.push(me);
    return {
      write: (line) => {
        const req = JSON.parse(line) as Record<string, unknown>;
        me.requests.push(req);
        const a = answer(req);
        if (a === 'hang') return;
        if (a === 'exit') return void queueMicrotask(() => onExit(1));
        queueMicrotask(() => onLine(JSON.stringify({ ...a, id: req.id })));
      },
      onLine: (h) => void (onLine = h),
      onExit: (h) => void (onExit = h),
      close: () => {
        me.closed = true;
        queueMicrotask(() => onExit(0));
      },
      kill: () => {
        me.killed = true;
        queueMicrotask(() => onExit(null));
      },
    };
  };
  return { spawn, spawned };
}

const STATE_ON = { ok: true, bluetooth: 'on', authorization: 'allowed', services_ready: true, advertising: true, connected: true, refused: false, name: null, last_error: null };
const state = (over: Partial<BtHidState> = {}): BtHidState => ({ ...readBtHidState(STATE_ON), ...over });
const health = (over: Partial<Parameters<typeof btMacHealth>[0]> = {}) =>
  btMacHealth({ platform: 'darwin', enabled: true, state: state(), broadcast: true, pointerSeen: true, locked: false, ...over });

afterEach(() => {
  vi.useRealTimers();
});

describe('state → route health', () => {
  it('is healthy only with the keyboard on, an iPhone connected, the broadcast live and the pointer seen', () => {
    expect(health()).toEqual({ state: 'healthy' });
  });

  it('is not available on Windows and Linux, or when the person has not turned it on', () => {
    expect(health({ platform: 'win32' })).toEqual({ state: 'unavailable', reason: BT_MAC_NOT_ON_THIS_OS });
    expect(health({ platform: 'linux' }).state).toBe('unavailable');
    expect(health({ enabled: false })).toEqual({ state: 'unavailable', reason: BT_MAC_OFF });
  });

  it('says macOS refused the keyboard service, ahead of anything else', () => {
    expect(health({ state: state({ refused: true, connected: false }) })).toEqual({ state: 'unavailable', reason: BT_MAC_REFUSED });
  });

  it('names the permission pane and the radio', () => {
    const denied = health({ state: state({ authorization: 'denied' }) });
    expect(denied.state === 'unavailable' && denied.reason).toContain('Privacy & Security > Bluetooth');
    expect(health({ state: state({ bluetooth: 'off', connected: false }) })).toEqual({ state: 'unavailable', reason: 'Turn on Bluetooth on this Mac.' });
    expect(health({ state: null, reason: 'helper crashed' })).toEqual({ state: 'unavailable', reason: 'helper crashed' });
  });

  it('maps the contract states in order: not paired, locked, broadcast, pointer', () => {
    expect(health({ state: state({ connected: false }) })).toEqual({ state: 'needs_you', needsYou: { code: 'BT_NOT_PAIRED', message: NEEDS_YOU_SENTENCES.BT_NOT_PAIRED } });
    expect(health({ locked: true }).state).toBe('unavailable');
    expect(health({ broadcast: false })).toMatchObject({ state: 'needs_you', needsYou: { code: 'BROADCAST_STOPPED' } });
    expect(health({ pointerSeen: false })).toMatchObject({ state: 'needs_you', needsYou: { code: 'ASSISTIVETOUCH_OFF' } });
    // Not looked for yet is not a failure.
    expect(health({ pointerSeen: null })).toEqual({ state: 'healthy' });
  });

  it('shows a failed advertise instead of waiting for a pairing that cannot come', () => {
    const h = health({ state: state({ advertising: false, connected: false, lastError: { code: 'advertise_failed', message: 'Bluetooth could not advertise.' } }) });
    expect(h).toEqual({ state: 'unavailable', reason: 'Bluetooth could not advertise.' });
  });

  it('reads unknown helper values as not ready, never as healthy', () => {
    const s = readBtHidState({ bluetooth: 'sideways', authorization: 7, connected: 'yes' });
    expect(s).toMatchObject({ bluetooth: 'unknown', authorization: 'not_determined', connected: false });
    expect(health({ state: s }).state).toBe('unavailable');
  });
});

describe('the settings row', () => {
  const row = (over: Partial<Parameters<typeof btMacSettings>[0]> = {}) => btMacSettings({ platform: 'darwin', hasBinary: true, enabled: true, state: state(), ...over });

  it('walks off → waiting → connected', () => {
    expect(row({ enabled: false })).toMatchObject({ available: true, status: 'off', enabled: false });
    expect(row({ state: state({ connected: false }) })).toMatchObject({ status: 'waiting', message: expect.stringContaining('"Allternit Keyboard"') });
    expect(row()).toMatchObject({ status: 'connected', connected: true, message: expect.stringContaining('AssistiveTouch') });
  });

  it('has no buttons where it cannot work, and says why', () => {
    expect(row({ platform: 'win32' })).toMatchObject({ available: false, status: 'unavailable', message: BT_MAC_NOT_ON_THIS_OS });
    expect(row({ hasBinary: false })).toMatchObject({ available: false, message: BT_MAC_NOT_IN_BUILD });
    expect(row({ state: state({ refused: true }) })).toMatchObject({ status: 'refused', message: BT_MAC_REFUSED });
  });
});

describe('the helper process', () => {
  it('pairs a fresh process before anything else while the keyboard is wanted', async () => {
    const bin = fakeBinary(() => STATE_ON);
    const client = new BtHidClient({ binary: () => '/bin/bthid', spawn: bin.spawn, wanted: true });
    await client.status();
    expect(bin.spawned[0].requests.map((r) => r.cmd)).toEqual(['pair', 'state']);
    await client.status();
    expect(bin.spawned[0].requests.map((r) => r.cmd)).toEqual(['pair', 'state', 'state']);
  });

  it('a failed reply rejects with the helper’s own code and sentence', async () => {
    const bin = fakeBinary((r) => (r.cmd === 'move' ? { ok: false, error: 'not_connected', message: 'No iPhone is connected.' } : STATE_ON));
    const client = new BtHidClient({ binary: () => '/bin/bthid', spawn: bin.spawn });
    await expect(client.call('move', { dx: 1, dy: 0 })).rejects.toMatchObject({ code: 'not_connected', message: 'No iPhone is connected.' });
  });

  it('kills a hung process, and the next call starts a new one and pairs again', async () => {
    let hang = true;
    const bin = fakeBinary((r) => (r.cmd === 'reset' && hang ? 'hang' : STATE_ON));
    const client = new BtHidClient({ binary: () => '/bin/bthid', spawn: bin.spawn, wanted: true, timeoutMs: () => 20 });
    await expect(client.call('reset')).rejects.toMatchObject({ code: 'timeout' });
    expect(bin.spawned[0].killed).toBe(true);
    hang = false;
    await client.call('reset');
    expect(bin.spawned).toHaveLength(2);
    expect(bin.spawned[1].requests.map((r) => r.cmd)).toEqual(['pair', 'reset']);
  });

  it('a process that dies fails what was waiting, and reports the iPhone gone', async () => {
    const bin = fakeBinary((r) => (r.cmd === 'type' ? 'exit' : STATE_ON));
    const client = new BtHidClient({ binary: () => '/bin/bthid', spawn: bin.spawn });
    await client.status();
    expect(client.lastState()?.connected).toBe(true);
    await expect(client.call('type', { text: 'hi' })).rejects.toMatchObject({ code: 'exited' });
    expect(client.running).toBe(false);
    expect(client.lastState()?.connected).toBe(false);
  });

  it('stays up while wanted; turned off it unpairs, closes, and an idle one closes by itself', async () => {
    vi.useFakeTimers();
    const bin = fakeBinary(() => STATE_ON);
    const client = new BtHidClient({ binary: () => '/bin/bthid', spawn: bin.spawn, idleMs: 1_000 });
    await client.pair();
    await vi.advanceTimersByTimeAsync(5_000);
    expect(bin.spawned[0].closed).toBe(false);
    await client.unpair();
    expect(bin.spawned[0].requests.map((r) => r.cmd)).toEqual(['pair', 'unpair']);
    expect(bin.spawned[0].closed).toBe(true);
    await client.status();
    await vi.advanceTimersByTimeAsync(1_500);
    expect(bin.spawned[1].closed).toBe(true);
  });

  it('keeps the newest state from unprompted events', async () => {
    const bin = fakeBinary(() => STATE_ON);
    const client = new BtHidClient({ binary: () => '/bin/bthid', spawn: bin.spawn });
    await client.status();
    const seen: boolean[] = [];
    client.onState((s) => seen.push(s.connected));
    bin.spawned[0].emit({ event: 'state', ...STATE_ON, connected: false });
    expect(client.lastState()?.connected).toBe(false);
    expect(seen).toEqual([false]);
  });

  it('says it is not in this build when there is no binary', async () => {
    const client = new BtHidClient({ binary: () => null });
    await expect(client.status()).rejects.toEqual(new BtHidError('no_binary', BT_MAC_NOT_IN_BUILD));
  });

  it('finds the binary only on a Mac', () => {
    const exists = (p: string) => p === '/R/bin/allternit-bthid';
    expect(btHidBinaryPath({ platform: 'darwin', packaged: true, resourcesPath: '/R', exists, env: {} })).toBe('/R/bin/allternit-bthid');
    expect(btHidBinaryPath({ platform: 'win32', packaged: true, resourcesPath: '/R', exists: () => true, env: {} })).toBeNull();
  });
});

describe('the transport', () => {
  it('splits long text and long scrolls into what the helper takes', async () => {
    const bin = fakeBinary(() => STATE_ON);
    const t = btMacTransport(new BtHidClient({ binary: () => '/bin/bthid', spawn: bin.spawn }));
    await t.typeText('a'.repeat(450));
    await t.scroll!(-250);
    const reqs = bin.spawned[0].requests;
    expect(reqs.filter((r) => r.cmd === 'type').map((r) => String(r.text).length)).toEqual([200, 200, 50]);
    expect(reqs.filter((r) => r.cmd === 'scroll').map((r) => r.lines)).toEqual([-100, -100, -50]);
  });
});

describe('the lane', () => {
  const iphone = { serial: 'app:ip1', platform: 'ios' as const, name: 'Joe’s iPhone', screen: true };
  const phones = (devices = [iphone]) => ({ refresh: async () => {}, devices: () => devices, call: async () => ({ ok: false as const, error: 'no_screen' as const, message: 'no' }) });
  const lane = (over: Partial<Parameters<typeof btMacLane>[0]> = {}) => {
    const bin = fakeBinary(() => STATE_ON);
    const client = new BtHidClient({ binary: () => '/bin/bthid', spawn: bin.spawn, wanted: true });
    return btMacLane({ platform: 'darwin', client: () => client, enabled: () => true, phones: () => phones(), decode: () => null, load: () => null, save: () => {}, ...over });
  };

  it('is healthy for the one iPhone with its broadcast on and the keyboard connected', async () => {
    expect(await lane().health({ serial: 'app:ip1', locked: false })).toEqual({ state: 'healthy' });
  });

  it('stays out when it cannot know which iPhone it drives, or on other OSes', async () => {
    const two = lane({ phones: () => phones([iphone, { ...iphone, serial: 'app:ip2' }]) });
    expect((await two.health({ serial: 'app:ip1', locked: false })).state).toBe('unavailable');
    expect(await lane({ platform: 'linux' }).health({ serial: 'app:ip1', locked: false })).toEqual({ state: 'unavailable', reason: BT_MAC_NOT_ON_THIS_OS });
    expect(await lane({ client: () => null }).health({ serial: 'app:ip1', locked: false })).toEqual({ state: 'unavailable', reason: BT_MAC_NOT_IN_BUILD });
  });

  it('asks for the pairing when the helper reports no iPhone, and a send then says the same', async () => {
    const bin = fakeBinary((r) => (r.cmd === 'key' ? { ok: false, error: 'not_connected', message: 'none' } : { ...STATE_ON, connected: false }));
    const client = new BtHidClient({ binary: () => '/bin/bthid', spawn: bin.spawn, wanted: true });
    const l = lane({ client: () => client });
    expect(await l.health({ serial: 'app:ip1', locked: false })).toMatchObject({ state: 'needs_you', needsYou: { code: 'BT_NOT_PAIRED' } });
    const res = await l.act('phone.key', 'app:ip1', { combo: 'cmd+h' });
    expect(res).toMatchObject({ ok: false, error: 'needs_you', needs_you: { code: 'BT_NOT_PAIRED' } });
  });

  it('refuses text the keyboard cannot type before sending anything', async () => {
    const res = await lane().act('phone.type', 'app:ip1', { text: 'café' });
    expect(res).toMatchObject({ ok: false, error: 'invalid_args' });
  });
});

describe('Control my iPhone by Bluetooth at launch', () => {
  it('publishes the keyboard at launch when the person left it on, so the iPhone can reconnect', async () => {
    const bin = fakeBinary(() => STATE_ON);
    const client = new BtHidClient({ binary: () => '/bin/allternit-bthid', spawn: bin.spawn, wanted: true });
    expect(await resumeBtMacAtLaunch({ platform: 'darwin', enabled: true, client: () => client })).toBe('started');
    expect(bin.spawned).toHaveLength(1);
    expect(bin.spawned[0]!.requests.map((r) => r.cmd)).toContain('pair');
    client.close();
  });

  it('starts nothing when it is off, off a Mac, or not in this build', async () => {
    const bin = fakeBinary(() => STATE_ON);
    const client = new BtHidClient({ binary: () => '/bin/allternit-bthid', spawn: bin.spawn });
    expect(await resumeBtMacAtLaunch({ platform: 'darwin', enabled: false, client: () => client })).toBe('off');
    expect(await resumeBtMacAtLaunch({ platform: 'win32', enabled: true, client: () => client })).toBe('unavailable');
    expect(await resumeBtMacAtLaunch({ platform: 'darwin', enabled: true, client: () => null })).toBe('unavailable');
    expect(bin.spawned).toHaveLength(0);
  });

  it('logs and carries on when the helper refuses at launch', async () => {
    const bin = fakeBinary(() => ({ ok: false, error: 'unauthorized', message: 'Bluetooth is not allowed.' }));
    const client = new BtHidClient({ binary: () => '/bin/allternit-bthid', spawn: bin.spawn, wanted: true });
    const logged: string[] = [];
    expect(await resumeBtMacAtLaunch({ platform: 'darwin', enabled: true, client: () => client, log: (m) => logged.push(m) })).toBe('failed');
    expect(logged[0]).toContain('Bluetooth is not allowed.');
    client.close();
  });
});
