import { describe, expect, it } from 'vitest';
import { BroadcastScreen, BtAndroidLane, POINTER_RECHECK_MS, type BtAndroidPhones, type BtRpcResult } from './phone-bt-android.js';
import type { Calibration } from './phone-hid-core.js';
import type { Gray } from './phone-usb-agent-vision.js';

const IPHONE = 'app:iphone-1';
const ANDROID = 'app:android-1';
const CAL: Calibration = { w: 1000, h: 2000, seen: { x: [], y: [] } };

/** The Android app at the network edge: answers hid.* like HidCommands does, and records what it was sent. */
function androidApp(status: Record<string, unknown> = { paired: true, connected: true, available: true, name: "Eve's iPhone" }) {
  const sent: Array<{ phoneId: string; method: string; params: Record<string, unknown> }> = [];
  const app = {
    status,
    sent,
    fail: null as null | { code: string; message: string },
    rpc: async (phoneId: string, method: string, params: Record<string, unknown>): Promise<BtRpcResult> => {
      if (method === 'hid.pair.status') return { ok: true, result: app.status };
      sent.push({ phoneId, method, params });
      if (app.fail) return { ok: false, status: 200, ...app.fail };
      return { ok: true, result: method === 'hid.type' ? { sent: String(params.text).length } : {} };
    },
  };
  return app;
}

function flat(v: number): Gray {
  return { width: 36, height: 64, data: new Uint8Array(36 * 64).fill(v) };
}

function setup(opts: { screen?: boolean; androids?: Array<{ serial: string; status: Record<string, unknown> }>; cal?: Calibration | null } = {}) {
  const clock = { t: 1_000_000 };
  const app = androidApp();
  const statuses = new Map<string, Record<string, unknown>>((opts.androids ?? [{ serial: ANDROID, status: app.status }]).map((a) => [a.serial.slice(4), a.status]));
  const devices = [
    { serial: IPHONE, platform: 'ios' as const, name: "Eve's iPhone", screen: opts.screen ?? true },
    ...[...statuses.keys()].map((id) => ({ serial: `app:${id}`, platform: 'android' as const, name: 'Pixel', screen: false })),
  ];
  const phones: BtAndroidPhones = {
    refresh: async () => undefined,
    devices: () => devices,
    call: async () => ({ ok: true, dataBase64: Buffer.from('jpeg').toString('base64'), width: 1170, height: 2532, ageMs: 50 }),
  };
  const saved: Calibration[] = [];
  const lane = new BtAndroidLane({
    phones: () => phones,
    rpc: async (phoneId, method, params) => {
      if (method === 'hid.pair.status' && statuses.has(phoneId)) return { ok: true, result: statuses.get(phoneId)! };
      return app.rpc(phoneId, method, params);
    },
    decode: () => flat(120),
    load: () => (opts.cal === undefined ? structuredClone(CAL) : opts.cal),
    save: (_s, c) => void saved.push(c),
    now: () => clock.t,
    sleep: async (ms) => void (clock.t += ms),
  });
  return { lane, app, devices, saved, clock };
}

describe('bt-android health (contract §5)', () => {
  it('is unavailable with a plain reason when no Android phone is connected to the account', async () => {
    const { lane } = setup({ androids: [] });
    const h = await lane.health({ serial: IPHONE, locked: false });
    expect(h).toEqual({ state: 'unavailable', reason: 'No Android phone with the Allternit app is connected to this account.' });
  });

  it('asks the person to pair when the Android phone is ready but no iPhone is connected', async () => {
    const { lane } = setup({ androids: [{ serial: ANDROID, status: { paired: false, connected: false, available: true, needs_you: 'BT_NOT_PAIRED' } }] });
    const h = await lane.health({ serial: IPHONE, locked: false });
    expect(h.state === 'needs_you' && h.needsYou.code).toBe('BT_NOT_PAIRED');
  });

  it('passes on the phone’s own sentence when it cannot be a keyboard at all', async () => {
    const message = 'Bluetooth keyboard and mouse is not available on this phone: Android does not offer it on this model.';
    const { lane } = setup({ androids: [{ serial: ANDROID, status: { paired: false, connected: false, available: false, availability: 'unsupported', message } }] });
    expect(await lane.health({ serial: IPHONE, locked: false })).toEqual({ state: 'unavailable', reason: message });
  });

  it('needs the broadcast running before it can act', async () => {
    const { lane } = setup({ screen: false });
    const h = await lane.health({ serial: IPHONE, locked: false });
    expect(h.state === 'needs_you' && h.needsYou.code).toBe('BROADCAST_STOPPED');
  });

  it('is healthy when connected and broadcasting, and never serves an Android serial', async () => {
    const { lane } = setup();
    expect(await lane.health({ serial: IPHONE, locked: false })).toEqual({ state: 'healthy' });
    expect((await lane.health({ serial: ANDROID, locked: false })).state).toBe('unavailable');
  });

  it('drives through the Android phone connected to this iPhone by name when several are connected', async () => {
    const { lane, app } = setup({
      androids: [
        { serial: 'app:other', status: { paired: true, connected: true, available: true, name: "Bob's iPhone" } },
        { serial: ANDROID, status: { paired: true, connected: true, available: true, name: "Eve's iPhone" } },
      ],
    });
    await lane.health({ serial: IPHONE, locked: false });
    await lane.act('phone.key', IPHONE, { combo: 'cmd+space' });
    expect(new Set(app.sent.map((s) => s.phoneId))).toEqual(new Set(['android-1']));
  });
});

describe('bt-android acting', () => {
  it('turns a key combo into hid.key down and up with the contract modifier bits', async () => {
    const { lane, app } = setup();
    await lane.health({ serial: IPHONE, locked: false });
    expect(await lane.act('phone.key', IPHONE, { combo: 'cmd+space' })).toEqual({ ok: true });
    const keys = app.sent.filter((s) => s.method === 'hid.key').map((s) => s.params);
    expect(keys.slice(-2)).toEqual([
      { usage: 0x2c, modifiers: 8, down: true },
      { usage: 0x2c, modifiers: 8, down: false },
    ]);
  });

  it('types long text in contract-sized hid.type messages and refuses what the layout cannot send', async () => {
    const { lane, app } = setup();
    await lane.health({ serial: IPHONE, locked: false });
    const text = 'a'.repeat(450);
    expect(await lane.act('phone.type', IPHONE, { text })).toEqual({ ok: true });
    const chunks = app.sent.filter((s) => s.method === 'hid.type').map((s) => String(s.params.text));
    expect(chunks.join('')).toBe(text);
    expect(chunks.every((c) => c.length <= 500)).toBe(true);
    const bad = await lane.act('phone.type', IPHONE, { text: 'café' });
    expect(bad).toMatchObject({ ok: false, error: 'invalid_args' });
  });

  it('taps at the screenshot pixel: corner reset, then moves of at most 127 that add up to the target', async () => {
    const { lane, app } = setup();
    await lane.health({ serial: IPHONE, locked: false });
    // The centre pixel of the 1170×2532 still is (0.5, 0.5) of the screen: 500 × 1000 mouse units with CAL.
    expect(await lane.act('phone.tap', IPHONE, { x: 585, y: 1266 })).toEqual({ ok: true });
    const after = app.sent.slice(app.sent.findIndex((s) => s.method === 'hid.pointer.reset') + 1);
    const moves = after.filter((s) => s.method === 'hid.pointer.move').map((s) => s.params as { dx: number; dy: number });
    // The aim check nudges right and back; those cancel out.
    expect(moves.reduce((s, m) => s + m.dx, 0)).toBe(500);
    expect(moves.reduce((s, m) => s + m.dy, 0)).toBe(1000);
    expect(moves.every((m) => Math.abs(m.dx) <= 127 && Math.abs(m.dy) <= 127)).toBe(true);
    expect(after.some((s) => s.method === 'hid.pointer.click' && s.params.down === true && s.params.up === true)).toBe(true);
  });

  it('refuses a tap outside the screenshot', async () => {
    const { lane } = setup();
    await lane.health({ serial: IPHONE, locked: false });
    expect(await lane.act('phone.tap', IPHONE, { x: 1170, y: 10 })).toMatchObject({ ok: false, error: 'invalid_args' });
  });

  it('says AssistiveTouch is off when calibration cannot find the pointer, and health then reports it', async () => {
    const { lane, clock } = setup({ cal: null });
    await lane.health({ serial: IPHONE, locked: false });
    const res = await lane.act('phone.tap', IPHONE, { x: 10, y: 10 });
    expect(res).toMatchObject({ ok: false, error: 'needs_you', needs_you: { code: 'ASSISTIVETOUCH_OFF' } });
    const h = await lane.health({ serial: IPHONE, locked: false });
    expect(h.state === 'needs_you' && h.needsYou.code).toBe('ASSISTIVETOUCH_OFF');
    // After the recheck window the route is offered again, so the next tap can look for the pointer.
    clock.t += POINTER_RECHECK_MS;
    expect(await lane.health({ serial: IPHONE, locked: false })).toEqual({ state: 'healthy' });
  });

  it('maps the phone’s bt_not_paired answer to the BT_NOT_PAIRED needs-you', async () => {
    const { lane, app } = setup();
    await lane.health({ serial: IPHONE, locked: false });
    app.fail = { code: 'bt_not_paired', message: 'not connected' };
    expect(await lane.act('phone.home', IPHONE, {})).toMatchObject({ ok: false, error: 'needs_you', needs_you: { code: 'BT_NOT_PAIRED' } });
  });
});

describe('the broadcast as the pointer screen', () => {
  it('waits for a still newer than the action and settles when two stills match', async () => {
    let t = 0;
    let shot = 0;
    const screen = new BroadcastScreen(
      async () => ({ jpeg: Buffer.from([shot++]), ageMs: 0, width: 10, height: 10 }),
      () => true,
      () => flat(shot < 3 ? 0 : 200),
      { now: () => t, sleep: async (ms) => void (t += ms) },
    );
    const since = t;
    t += 10;
    expect(await screen.nextGray(since, 1_000)).not.toBeNull();
    expect(screen.latestAt()).toBeGreaterThan(since);
    const start = t;
    await screen.settled(100, 50, 5_000);
    expect(t - start).toBeLessThan(5_000);
  });

  it('gives up on a frozen broadcast after the timeout', async () => {
    let t = 0;
    // Same picture, older each time (age grows as fast as the clock): never newer.
    const frozen = new BroadcastScreen(async () => ({ jpeg: Buffer.from([1]), ageMs: t, width: 1, height: 1 }), () => true, () => flat(1), {
      now: () => t,
      sleep: async (ms) => void (t += ms),
    });
    await frozen.pull();
    expect(await frozen.nextGray(t, 500)).toBeNull();
  });
});
