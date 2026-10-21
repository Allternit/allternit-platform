import { describe, expect, it, vi } from 'vitest';
import {
  buildIsmsSend,
  checkRate,
  DIAL_LIMITS,
  encodeInputText,
  isEmergencyNumber,
  normalizeNumber,
  PhoneToolRunner,
  resolveApp,
  SMS_LIMITS,
  shQuote,
  type AdbCall,
  type ApprovalDecision,
  type ApprovalRequest,
  type RateStore,
} from './phone-tools.js';

const PNG = Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 1, 2, 3]);
const OK_PARCEL = 'Result: Parcel(00000000    \'....\')';

function makeAdb(handler: (args: string[]) => { stdout?: string; stderr?: string; code?: number } = () => ({})) {
  const calls: string[][] = [];
  const adb = (async (args: string[]) => {
    calls.push(args);
    const r = handler(args);
    return { stdout: r.stdout ?? '', stderr: r.stderr ?? '', code: r.code ?? 0 };
  }) as AdbCall;
  adb.binary = async (args: string[]) => {
    calls.push(args);
    return PNG;
  };
  return { adb, calls };
}

function memoryRates(): RateStore & { data: Record<string, number[]> } {
  const data: Record<string, number[]> = {};
  return { data, load: (k) => data[k] ?? [], save: (k, v) => void (data[k] = v) };
}

function runner(opts: { decision?: ApprovalDecision; handler?: Parameters<typeof makeAdb>[0]; now?: () => number; online?: string[]; artemis?: boolean } = {}) {
  const { adb, calls } = makeAdb(opts.handler);
  const asked: ApprovalRequest[] = [];
  const rates = memoryRates();
  const r = new PhoneToolRunner({
    adb,
    gate: { request: async (req) => (asked.push(req), opts.decision ?? 'approved') },
    rates,
    artemis: () => (opts.artemis ? { runTask: vi.fn(async () => ({ status: 'completed' })) } : null),
    now: opts.now,
    onlineSerials: () => opts.online ?? ['192.168.1.20:41234'],
    defaultSmsSubscription: async () => 3,
  });
  return { r, calls, asked, rates };
}

describe('argument validation', () => {
  it('rejects bad coordinates and never calls adb', async () => {
    const { r, calls } = runner();
    for (const args of [{ x: -1, y: 5 }, { x: 'a', y: 5 }, { x: 1.5, y: 5 }, { y: 5 }, { x: 99999999, y: 1 }]) {
      expect(await r.call('phone.tap', args)).toMatchObject({ ok: false, error: 'invalid_args' });
    }
    expect(calls).toHaveLength(0);
  });

  it('taps with integer args', async () => {
    const { r, calls } = runner();
    expect(await r.call('phone.tap', { x: 120, y: 480 })).toEqual({ ok: true });
    expect(calls[0]).toEqual(['shell', 'input', 'tap', '120', '480']);
  });

  it('quotes typed text for the on-device shell and refuses non-ASCII / "%s"', async () => {
    const { r, calls } = runner();
    expect(await r.call('phone.type', { text: "it's $(rm -rf /)" })).toEqual({ ok: true });
    expect(calls[0]).toEqual(['shell', 'input', 'text', `'it'\\''s%s$(rm%s-rf%s/)'`]);
    expect(await r.call('phone.type', { text: 'héllo' })).toMatchObject({ error: 'invalid_args' });
    expect(await r.call('phone.type', { text: '100%s' })).toMatchObject({ error: 'invalid_args' });
    expect(await r.call('phone.type', { text: 'x'.repeat(501) })).toMatchObject({ error: 'invalid_args' });
    expect(encodeInputText('a b')).toBe('a%sb');
  });

  it('swipes by direction using the screen size, or by coordinates', async () => {
    const { r, calls } = runner({ handler: (a) => (a.includes('size') ? { stdout: 'Physical size: 1080x2400' } : {}) });
    expect(await r.call('phone.swipe', { direction: 'up' })).toEqual({ ok: true });
    expect(calls.at(-1)).toEqual(['shell', 'input', 'swipe', '540', '2000', '540', '400', '300']);
    expect(await r.call('phone.swipe', { x1: 1, y1: 2, x2: 3, y2: 4, durationMs: 50 })).toEqual({ ok: true });
    expect(await r.call('phone.swipe', { x1: 1, y1: 2, x2: 3 })).toMatchObject({ error: 'invalid_args' });
    expect(await r.call('phone.swipe', { direction: 'sideways' })).toMatchObject({ error: 'invalid_args' });
    expect(await r.call('phone.swipe', { direction: 'up', durationMs: 5 })).toMatchObject({ error: 'invalid_args' });
  });

  it('opens apps by package or alias only', async () => {
    const { r, calls } = runner();
    expect(await r.call('phone.open_app', { app: 'Settings' })).toEqual({ ok: true, package: 'com.android.settings' });
    expect(calls[0]).toEqual(['shell', 'monkey', '-p', 'com.android.settings', '-c', 'android.intent.category.LAUNCHER', '1']);
    expect(await r.call('phone.open_app', { app: 'x; reboot' })).toMatchObject({ error: 'unknown_app' });
    expect(resolveApp('com.foo.bar')).toBe('com.foo.bar');
    expect(resolveApp('nonsense')).toBeNull();
  });

  it('returns a PNG screenshot as base64 and refuses non-PNG bytes', async () => {
    const { r } = runner();
    const res = await r.call('phone.screenshot', {});
    expect(res).toMatchObject({ ok: true, mimeType: 'image/png', dataBase64: PNG.toString('base64') });
  });

  it('phone.task needs ARTEMIS and passes the model through', async () => {
    expect(await runner().r.call('phone.task', { task: 'open settings' })).toMatchObject({ error: 'artemis_unavailable' });
    const { r } = runner({ artemis: true });
    expect(await r.call('phone.task', { task: 'open settings', model: 'Pro' })).toMatchObject({ ok: true });
    expect(await r.call('phone.task', { task: '' })).toMatchObject({ error: 'invalid_args' });
    expect(await r.call('phone.task', { task: 'x', model: 'Turbo' })).toMatchObject({ error: 'invalid_args' });
  });

  it('rejects unknown tools and resolves the device', async () => {
    expect(await runner().r.call('phone.format', {})).toMatchObject({ error: 'unknown_tool' });
    expect(await runner({ online: [] }).r.call('phone.tap', { x: 1, y: 1 })).toMatchObject({ error: 'device_offline' });
    expect(await runner({ online: ['a:1', 'b:2'] }).r.call('phone.tap', { x: 1, y: 1 })).toMatchObject({ error: 'device_required' });
    expect(await runner({ online: ['a:1', 'b:2'] }).r.call('phone.tap', { serial: 'zzz', x: 1, y: 1 })).toMatchObject({ error: 'device_offline' });
  });
});

describe('approval gate: sms and dial', () => {
  const sms = { to: '+1 (555) 010-2030', text: 'On my way' };

  it('asks the user before sending, showing the recipient and exact text', async () => {
    const { r, asked, calls } = runner({ handler: () => ({ stdout: OK_PARCEL }) });
    const res = await r.call('phone.sms.send', sms);
    expect(res).toMatchObject({ ok: true, status: 'submitted' });
    expect(asked).toHaveLength(1);
    expect(asked[0].kind).toBe('sms');
    expect(asked[0].summary).toContain('+15550102030');
    expect(asked[0].summary).toContain('On my way');
    expect(asked[0].actionHash).toMatch(/^[0-9a-f]{64}$/);
    expect(calls.at(-1)?.slice(0, 5)).toEqual(['shell', 'service', 'call', 'isms', '5']);
  });

  it('never touches the phone when the user denies or does not answer', async () => {
    for (const decision of ['denied', 'timeout'] as const) {
      const { r, calls } = runner({ decision });
      expect(await r.call('phone.sms.send', sms)).toMatchObject({ ok: false, error: decision === 'denied' ? 'approval_denied' : 'approval_timeout' });
      expect(await r.call('phone.call.dial', { number: '+15550102030' })).toMatchObject({ ok: false });
      expect(calls).toHaveLength(0);
    }
  });

  it('binds the approval hash to the exact action', async () => {
    const { r, asked } = runner({ handler: () => ({ stdout: OK_PARCEL }), now: (() => { let t = 0; return () => (t += 3_600_000 * 2); })() });
    await r.call('phone.sms.send', sms);
    await r.call('phone.sms.send', { ...sms, text: 'On my way!' });
    await r.call('phone.sms.send', sms);
    expect(asked[0].actionHash).not.toBe(asked[1].actionHash);
    expect(asked[0].actionHash).toBe(asked[2].actionHash);
  });

  it('validates numbers and blocks emergency numbers before asking', async () => {
    const { r, asked } = runner();
    expect(await r.call('phone.sms.send', { to: 'abc', text: 'x' })).toMatchObject({ error: 'invalid_args' });
    expect(await r.call('phone.sms.send', { to: '+15550102030', text: '' })).toMatchObject({ error: 'invalid_args' });
    expect(await r.call('phone.sms.send', { to: '911', text: 'x' })).toMatchObject({ error: 'blocked_number' });
    expect(await r.call('phone.call.dial', { number: '+1911' })).toMatchObject({ error: 'blocked_number' });
    expect(await r.call('phone.call.dial', { number: '112' })).toMatchObject({ error: 'blocked_number' });
    expect(asked).toHaveLength(0);
    expect(isEmergencyNumber('+1911')).toBe(true);
    expect(isEmergencyNumber('+15550102030')).toBe(false);
  });

  it('dials only after approval, from the chosen SIM, and says it cannot speak', async () => {
    const { r, calls, asked } = runner();
    const res = await r.call('phone.call.dial', { number: '555-010-2030', subscription_id: 4 });
    expect(res).toMatchObject({ ok: true, status: 'dialing' });
    expect(asked[0].kind).toBe('dial');
    expect(calls[0]).toContain('android.intent.action.CALL');
    expect(calls[0]).toContain('4');
    expect(String((res as unknown as { note: string }).note)).toMatch(/cannot speak/);
  });

  it('refuses a send when the phone does not accept it', async () => {
    const { r } = runner({ handler: () => ({ stdout: 'Result: Parcel(ffffffff Exception)' }) });
    expect(await r.call('phone.sms.send', sms)).toMatchObject({ ok: false, error: 'adb_failed' });
  });
});

describe('person-to-person rate limits', () => {
  it('blocks a second text inside the minimum gap, with retryAfterSec', async () => {
    let now = 1_000_000;
    const { r, asked } = runner({ handler: () => ({ stdout: OK_PARCEL }), now: () => now });
    expect((await r.call('phone.sms.send', { to: '+15550102030', text: 'a' })).ok).toBe(true);
    now += 5_000;
    const second = await r.call('phone.sms.send', { to: '+15550102031', text: 'b' });
    expect(second).toMatchObject({ ok: false, error: 'rate_limited', retryAfterSec: 10 });
    expect(asked).toHaveLength(1); // the user is not even bothered
  });

  it('caps texts per hour and per day (no bulk sends)', () => {
    const now = 100 * 3_600_000;
    const tenInLastHour = Array.from({ length: SMS_LIMITS.perHour }, (_, i) => now - 60_000 * (i + 1));
    expect(checkRate(tenInLastHour, now, SMS_LIMITS)).toMatchObject({ ok: false, reason: expect.stringContaining('hourly') });
    const thirtyToday = Array.from({ length: SMS_LIMITS.perDay }, (_, i) => now - 4_000_000 - i * 1000);
    expect(checkRate(thirtyToday, now, SMS_LIMITS)).toMatchObject({ ok: false, reason: expect.stringContaining('daily') });
    expect(checkRate([now - 86_400_001], now, SMS_LIMITS)).toEqual({ ok: true });
    expect(checkRate([now - 11_000], now, DIAL_LIMITS)).toEqual({ ok: true });
  });

  it('counts a denied-after-approval send attempt but not a denied request', async () => {
    const { r, rates } = runner({ decision: 'denied' });
    await r.call('phone.sms.send', { to: '+15550102030', text: 'a' });
    expect(Object.keys(rates.data)).toHaveLength(0);
  });
});

describe('helpers', () => {
  it('normalizes numbers', () => {
    expect(normalizeNumber('+1 (555) 010-2030')).toBe('+15550102030');
    expect(normalizeNumber('12')).toBeNull();
    expect(normalizeNumber('+1234567890123456')).toBeNull();
    expect(normalizeNumber(5)).toBeNull();
  });

  it('shQuote survives single quotes', () => {
    expect(shQuote("a'b")).toBe(`'a'\\''b'`);
  });

  it('builds the isms call with quoted recipient and body', () => {
    const args = buildIsmsSend(3, '+15550102030', "hi 'there'");
    expect(args.slice(0, 7)).toEqual(['shell', 'service', 'call', 'isms', '5', 'i32', '3']);
    expect(args).toContain(`'hi '\\''there'\\'''`);
  });
});
