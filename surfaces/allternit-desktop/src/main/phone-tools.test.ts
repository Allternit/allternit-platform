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

function runner(opts: { decision?: ApprovalDecision; handler?: Parameters<typeof makeAdb>[0]; now?: () => number; online?: string[]; artemis?: boolean; activity?: (serial: string, delta: number) => void } = {}) {
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
    activity: opts.activity,
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

describe('screen reading over adb (uiautomator dump)', () => {
  // A dump shaped like `uiautomator dump /dev/tty` prints it: nested nodes,
  // entities, text OR content-desc, bounds as [x1,y1][x2,y2].
  const DUMP = `<?xml version='1.0' encoding='UTF-8' standalone='yes' ?>
<hierarchy rotation="0">
  <node index="0" text="" resource-id="" class="android.widget.FrameLayout" package="com.openai.chat" content-desc="" clickable="false" bounds="[0,0][1080,2400]">
    <node index="0" text="Say &quot;hi&quot;" resource-id="com.openai.chat:id/title" class="android.widget.TextView" content-desc="" clickable="false" bounds="[24,96][1056,180]" />
    <node index="1" text="" resource-id="com.openai.chat:id/login" class="android.widget.Button" package="com.openai.chat" content-desc="Sign in" clickable="true" bounds="[24,836][336,884]" />
  </node>
</hierarchy>`;

  function dumpAdb(xml: string | Error) {
    const calls: string[][] = [];
    const adb = (async (args: string[]) => {
      calls.push(args);
      return { stdout: '', stderr: '', code: 0 };
    }) as AdbCall;
    adb.binary = async (args: string[]) => {
      calls.push(args);
      if (xml instanceof Error) throw xml;
      return Buffer.from(xml, 'utf8');
    };
    return { adb, calls };
  }

  function dumpRunner(xml: string | Error) {
    const { adb, calls } = dumpAdb(xml);
    const r = new PhoneToolRunner({
      adb,
      gate: { request: async () => 'denied' },
      rates: { load: () => [], save: () => {} },
      artemis: () => null,
      onlineSerials: () => ['10.0.0.7:5555'],
    });
    return { r, calls };
  }

  it('phone.read_ui turns the dump into the shared text form', async () => {
    const { r, calls } = dumpRunner(DUMP);
    const res = await r.call('phone.read_ui', {});
    expect(res).toEqual({
      ok: true,
      text: [
        'android.widget.FrameLayout {{0, 0}, {1080, 2400}}',
        'android.widget.TextView "Say "hi"" [com.openai.chat:id/title] {{24, 96}, {1032, 84}}',
        'android.widget.Button "Sign in" [com.openai.chat:id/login] {{24, 836}, {312, 48}}',
      ].join('\n'),
    });
    expect(calls[0]).toEqual(['exec-out', 'uiautomator', 'dump', '/dev/tty']);
  });

  it('phone.read_ui filters by label and type and caps max_elements', async () => {
    const { r } = dumpRunner(DUMP);
    expect(await r.call('phone.read_ui', { filter: { label: 'sign' } })).toEqual({
      ok: true,
      text: 'android.widget.Button "Sign in" [com.openai.chat:id/login] {{24, 836}, {312, 48}}',
    });
    expect(await r.call('phone.read_ui', { filter: { type: 'textview' } })).toMatchObject({ ok: true });
    expect(((await r.call('phone.read_ui', { filter: { type: 'textview' } })) as unknown as { text: string }).text).not.toContain('Button');
    expect(await r.call('phone.read_ui', { max_elements: 1 })).toEqual({
      ok: true,
      text: 'android.widget.FrameLayout {{0, 0}, {1080, 2400}}',
    });
    expect(await r.call('phone.read_ui', { filter: 'oops' })).toMatchObject({ error: 'invalid_args' });
    expect(await r.call('phone.read_ui', { max_elements: 0 })).toMatchObject({ error: 'invalid_args' });
    expect(await r.call('phone.read_ui', { visible_only: 'yes' })).toMatchObject({ ok: true }); // no-op on adb, accepted for parity
  });

  it('phone.screen_text joins the readable labels', async () => {
    const { r } = dumpRunner(DUMP);
    expect(await r.call('phone.screen_text', {})).toEqual({ ok: true, text: 'Say "hi"\nSign in' });
  });

  it('phone.find_and_tap taps the element centre, or reports not_found', async () => {
    const { r, calls } = dumpRunner(DUMP);
    expect(await r.call('phone.find_and_tap', { element_id: 'login' })).toMatchObject({ ok: true });
    expect(calls.at(-1)).toEqual(['shell', 'input', 'tap', '180', '860']);
    // match narrows by label; content-desc counts as the label.
    expect(await r.call('phone.find_and_tap', { element_id: 'login', match: 'log in' })).toMatchObject({ ok: false, error: 'not_found' });
    expect(await r.call('phone.find_and_tap', { element_id: 'login', match: 'sign' })).toMatchObject({ ok: true });
    expect(await r.call('phone.find_and_tap', { element_id: 'nope' })).toMatchObject({ ok: false, error: 'not_found' });
    expect(await r.call('phone.find_and_tap', {})).toMatchObject({ error: 'invalid_args' });
    // Full resource-id also matches.
    expect(await r.call('phone.find_and_tap', { element_id: 'com.openai.chat:id/title' })).toMatchObject({ ok: true });
  });

  it('a failed dump is adb_failed, not a crash', async () => {
    const { r } = dumpRunner(new Error('device offline'));
    expect(await r.call('phone.read_ui', {})).toMatchObject({ ok: false, error: 'adb_failed' });
    const empty = dumpRunner('');
    expect(await empty.r.call('phone.read_ui', {})).toMatchObject({ ok: false, error: 'adb_failed' });
  });
});

describe('Phone-Linked phones in the runner', () => {
  function linksBackend(serials: string[]) {
    const calls: { name: string; serial: string; args: Record<string, unknown> }[] = [];
    return {
      calls,
      backend: {
        has: (s: string) => serials.includes(s),
        serials: () => serials,
        call: async (name: string, serial: string, args: Record<string, unknown>): Promise<import('./phone-tools.js').ToolResult> => {
          calls.push({ name, serial, args });
          return { ok: true, via: 'phone-link' };
        },
        linkedDevices: async () =>
          serials.map((serial) => ({
            serial,
            platform: 'ios' as const,
            via: 'phone-link' as const,
            route: 'iphone-app-ictrl' as const,
            name: 'iPhone 15',
            health: { state: 'HEALTHY' as const, checkedAt: '2026-10-17T00:00:00.000Z', recovery: 'automatic' as const },
          })),
      },
    };
  }

  function linkedRunner(serials: string[], online: string[] = []) {
    const { adb, calls } = makeAdb();
    const links = linksBackend(serials);
    const r = new PhoneToolRunner({
      adb,
      gate: { request: async () => 'denied' },
      rates: { load: () => [], save: () => {} },
      artemis: () => null,
      onlineSerials: () => online,
      links: () => links.backend,
    });
    return { r, links, adbCalls: calls };
  }

  it('routes calls to the links lane by link: serial, defaulting to the only online phone', async () => {
    const { r, links, adbCalls } = linkedRunner(['link:dev-1']);
    const res = await r.call('phone.tap', { x: 10, y: 20 });
    expect(res).toMatchObject({ ok: true, via: 'phone-link' });
    expect(links.calls).toEqual([{ name: 'phone.tap', serial: 'link:dev-1', args: { x: 10, y: 20 } }]);
    expect(adbCalls).toHaveLength(0);

    expect(await r.call('phone.tap', { serial: 'link:dev-1', x: 1, y: 2 })).toMatchObject({ ok: true });
    expect(await r.call('phone.tap', { serial: 'zzz', x: 1, y: 2 })).toMatchObject({ ok: false, error: 'device_offline' });
  });

  it('treats a linked phone whose health is not online as offline', async () => {
    const { r } = linkedRunner([]); // monitor says TRANSPORT_OFFLINE → serials() is empty
    expect(await r.call('phone.tap', { serial: 'link:dev-1', x: 1, y: 2 })).toMatchObject({ ok: false, error: 'device_offline' });
    expect(await r.call('phone.tap', { x: 1, y: 2 })).toMatchObject({ ok: false, error: 'device_offline' });
  });

  it('phone.devices merges Android, USB Agent and linked phones with health', async () => {
    const { r } = linkedRunner(['link:dev-1'], ['10.0.0.7:5555']);
    const res = await r.call('phone.devices', {});
    expect(res).toEqual({
      ok: true,
      serials: ['10.0.0.7:5555', 'link:dev-1'],
      devices: [
        { serial: '10.0.0.7:5555', platform: 'android' },
        {
          serial: 'link:dev-1',
          platform: 'ios',
          via: 'phone-link',
          route: 'iphone-app-ictrl',
          name: 'iPhone 15',
          health: { state: 'HEALTHY', checkedAt: '2026-10-17T00:00:00.000Z', recovery: 'automatic' },
        },
      ],
    });
  });

  it('mixes lanes: linked + adb phones need an explicit serial', async () => {
    const { r } = linkedRunner(['link:dev-1'], ['10.0.0.7:5555']);
    expect(await r.call('phone.tap', { x: 1, y: 2 })).toMatchObject({ ok: false, error: 'device_required' });
    expect(await r.call('phone.tap', { serial: '10.0.0.7:5555', x: 1, y: 2 })).toEqual({ ok: true });
  });
});

describe('agent activity (takeover banner)', () => {
  it('marks acting tools busy around the call, and reads stay quiet', async () => {
    const activity = vi.fn();
    const { r } = runner({ activity });
    await r.call('phone.screenshot', {});
    expect(activity).not.toHaveBeenCalled();
    await r.call('phone.devices', {});
    expect(activity).not.toHaveBeenCalled();

    await r.call('phone.tap', { x: 5, y: 5 });
    expect(activity).toHaveBeenNthCalledWith(1, '192.168.1.20:41234', 1);
    expect(activity).toHaveBeenNthCalledWith(2, '192.168.1.20:41234', -1);
    expect(activity).toHaveBeenCalledTimes(2);
  });

  it('balances the counter even when the tool fails', async () => {
    const activity = vi.fn();
    const { r } = runner({ activity, handler: () => ({ stdout: 'Error', code: 1 }) });
    const result = await r.call('phone.home', {});
    expect(result.ok).toBe(false);
    expect(activity).toHaveBeenNthCalledWith(1, '192.168.1.20:41234', 1);
    expect(activity).toHaveBeenNthCalledWith(2, '192.168.1.20:41234', -1);
  });
});
