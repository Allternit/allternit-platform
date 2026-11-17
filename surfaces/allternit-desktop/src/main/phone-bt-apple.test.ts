import { describe, expect, it } from 'vitest';
import {
  BT_APPLE_NO_SECOND_DEVICE,
  BT_APPLE_OPEN_APP,
  BT_APPLE_SEVERAL_KEYBOARDS,
  BT_APPLE_SEVERAL_TARGETS,
  BT_APPLE_UPDATE_APP,
  BtAppleLane,
  type BtApplePhones,
  type BtAppleRpcResult,
} from './phone-bt-apple.js';
import type { Calibration } from './phone-hid-core.js';
import type { Gray } from './phone-usb-agent-vision.js';

const MAIN = 'app:main-iphone';
const IPAD = 'app:second-ipad';
const CAL: Calibration = { w: 1000, h: 2000, seen: { x: [], y: [] } };
const CONNECTED = { paired: true, connected: true, available: true, availability: 'ready', sender: 'bt-apple' };
const WAITING = { paired: false, connected: false, available: true, availability: 'ready', needs_you: 'BT_NOT_PAIRED', sender: 'bt-apple' };
const OFF_SENTENCE = 'Turn on Be a keyboard for another iPhone in the Allternit app on your other iPhone or iPad (Settings > Device Access).';

type Answer = Record<string, unknown> | { error: { code: string; message: string; status?: number } };

function flat(v: number): Gray {
  return { width: 36, height: 64, data: new Uint8Array(36 * 64).fill(v) };
}

/**
 * Two (or more) App Store iPhones/iPads at the network edge: each answers
 * hid.pair.status with its own state, and records the hid.* requests it was
 * sent, as Core/Remote/KeyboardCommands.swift would.
 */
function setup(opts: { devices?: Array<{ serial: string; screen: boolean; status?: Answer }>; fail?: { code: string; message: string } } = {}) {
  const clock = { t: 1_000_000 };
  const list = opts.devices ?? [
    { serial: MAIN, screen: true },
    { serial: IPAD, screen: false, status: CONNECTED },
  ];
  const sent: Array<{ phoneId: string; method: string; params: Record<string, unknown> }> = [];
  const asked: string[] = [];
  const state = { fail: opts.fail ?? null };
  const phones: BtApplePhones = {
    refresh: async () => undefined,
    devices: () => list.map((d) => ({ serial: d.serial, platform: 'ios' as const, name: 'iPhone', screen: d.screen })),
    call: async () => ({ ok: true, dataBase64: Buffer.from('jpeg').toString('base64'), width: 1170, height: 2532, ageMs: 50 }),
  };
  const rpc = async (phoneId: string, method: string, params: Record<string, unknown>): Promise<BtAppleRpcResult> => {
    const d = list.find((x) => x.serial === `app:${phoneId}`);
    if (method === 'hid.pair.status') {
      asked.push(phoneId);
      const a = d?.status ?? { paired: false, connected: false, available: false, availability: 'off', message: OFF_SENTENCE };
      if ('error' in a) {
        const e = a.error as { code: string; message: string; status?: number };
        return { ok: false, status: e.status ?? 200, code: e.code, message: e.message };
      }
      return { ok: true, result: a };
    }
    sent.push({ phoneId, method, params });
    if (state.fail) return { ok: false, status: 200, ...state.fail };
    return { ok: true, result: method === 'hid.type' ? { sent: String(params.text).length } : {} };
  };
  const lane = new BtAppleLane({
    phones: () => phones,
    rpc,
    decode: () => flat(120),
    load: () => structuredClone(CAL),
    save: () => undefined,
    now: () => clock.t,
    sleep: async (ms) => void (clock.t += ms),
  });
  return { lane, sent, asked, state };
}

describe('bt-apple health (contract §5)', () => {
  it('is unavailable with a plain reason when the account has only the one iPhone', async () => {
    const { lane } = setup({ devices: [{ serial: MAIN, screen: true }] });
    expect(await lane.health({ serial: MAIN, locked: false })).toEqual({ state: 'unavailable', reason: BT_APPLE_NO_SECOND_DEVICE });
  });

  it('never asks the target iPhone to be its own keyboard', async () => {
    const { lane, asked } = setup({ devices: [{ serial: MAIN, screen: true, status: CONNECTED }] });
    expect((await lane.health({ serial: MAIN, locked: false })).state).toBe('unavailable');
    expect(asked).not.toContain('main-iphone');
  });

  it('asks the person to pair when keyboard mode is on but the main iPhone is not connected', async () => {
    const { lane } = setup({ devices: [{ serial: MAIN, screen: true }, { serial: IPAD, screen: false, status: WAITING }] });
    const h = await lane.health({ serial: MAIN, locked: false });
    expect(h.state === 'needs_you' && h.needsYou.code).toBe('BT_NOT_PAIRED');
  });

  it('passes on the second device’s own sentence when keyboard mode is off or refused', async () => {
    const { lane } = setup({ devices: [{ serial: MAIN, screen: true }, { serial: IPAD, screen: false }] });
    expect(await lane.health({ serial: MAIN, locked: false })).toEqual({ state: 'unavailable', reason: OFF_SENTENCE });
  });

  it('tells the person to update or open the app on the second device', async () => {
    const old = setup({ devices: [{ serial: MAIN, screen: true }, { serial: IPAD, screen: false, status: { error: { code: 'unknown_method', message: 'Unknown method hid.pair.status' } } }] });
    expect(await old.lane.health({ serial: MAIN, locked: false })).toEqual({ state: 'unavailable', reason: BT_APPLE_UPDATE_APP });
    const closed = setup({ devices: [{ serial: MAIN, screen: true }, { serial: IPAD, screen: false, status: { error: { code: 'app_not_open', message: 'Open the Allternit app on the iPhone', status: 409 } } }] });
    expect(await closed.lane.health({ serial: MAIN, locked: false })).toEqual({ state: 'unavailable', reason: BT_APPLE_OPEN_APP });
  });

  it('needs the main iPhone’s broadcast before it can act', async () => {
    const { lane } = setup({ devices: [{ serial: MAIN, screen: false }, { serial: IPAD, screen: false, status: CONNECTED }] });
    const h = await lane.health({ serial: MAIN, locked: false });
    expect(h.state === 'needs_you' && h.needsYou.code).toBe('BROADCAST_STOPPED');
  });

  it('is healthy when the second device is connected and the main iPhone is broadcasting', async () => {
    const { lane } = setup();
    expect(await lane.health({ serial: MAIN, locked: false })).toEqual({ state: 'healthy' });
  });

  it('refuses to guess when two keyboards are connected or two iPhones are broadcasting', async () => {
    const two = setup({
      devices: [
        { serial: MAIN, screen: true },
        { serial: IPAD, screen: false, status: CONNECTED },
        { serial: 'app:old-iphone', screen: false, status: CONNECTED },
      ],
    });
    expect(await two.lane.health({ serial: MAIN, locked: false })).toEqual({ state: 'unavailable', reason: BT_APPLE_SEVERAL_KEYBOARDS });
    const targets = setup({
      devices: [
        { serial: MAIN, screen: true },
        { serial: IPAD, screen: false, status: CONNECTED },
        { serial: 'app:work-iphone', screen: true },
      ],
    });
    expect(await targets.lane.health({ serial: MAIN, locked: false })).toEqual({ state: 'unavailable', reason: BT_APPLE_SEVERAL_TARGETS });
  });
});

describe('bt-apple acting', () => {
  it('sends a key combo to the second device as hid.key down and up', async () => {
    const { lane, sent } = setup();
    await lane.health({ serial: MAIN, locked: false });
    expect(await lane.act('phone.key', MAIN, { combo: 'cmd+space' })).toEqual({ ok: true });
    const keys = sent.filter((s) => s.method === 'hid.key');
    expect(new Set(keys.map((k) => k.phoneId))).toEqual(new Set(['second-ipad']));
    expect(keys.slice(-2).map((k) => k.params)).toEqual([
      { usage: 0x2c, modifiers: 8, down: true },
      { usage: 0x2c, modifiers: 8, down: false },
    ]);
  });

  it('types long text in contract-sized hid.type messages', async () => {
    const { lane, sent } = setup();
    await lane.health({ serial: MAIN, locked: false });
    expect(await lane.act('phone.type', MAIN, { text: 'a'.repeat(450) })).toEqual({ ok: true });
    const typed = sent.filter((s) => s.method === 'hid.type').map((s) => String(s.params.text).length);
    expect(typed).toEqual([200, 200, 50]);
  });

  it('turns the device’s bt_not_paired into the BT_NOT_PAIRED needs-you and hid_unavailable into route_unavailable', async () => {
    const { lane, state } = setup();
    await lane.health({ serial: MAIN, locked: false });
    state.fail = { code: 'bt_not_paired', message: 'Your iPhone is not connected to the Allternit Keyboard on your other device.' };
    const lost = await lane.act('phone.key', MAIN, { combo: 'enter' });
    expect(lost.ok === false && (lost as { needs_you?: { code: string } }).needs_you?.code).toBe('BT_NOT_PAIRED');
    state.fail = { code: 'hid_unavailable', message: 'Not available on this iPhone or iPad: iOS refused the keyboard service; an Apple update may have changed this.' };
    expect(await lane.act('phone.key', MAIN, { combo: 'enter' })).toEqual({
      ok: false,
      error: 'route_unavailable',
      message: 'Not available on this iPhone or iPad: iOS refused the keyboard service; an Apple update may have changed this.',
    });
  });

  it('acts without a prior health check by finding the second device first', async () => {
    const { lane, sent } = setup();
    expect(await lane.act('phone.home', MAIN, {})).toEqual({ ok: true });
    expect(sent.length).toBeGreaterThan(0);
    expect(sent.every((s) => s.phoneId === 'second-ipad')).toBe(true);
  });
});
