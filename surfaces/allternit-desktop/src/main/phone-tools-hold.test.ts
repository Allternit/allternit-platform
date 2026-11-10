import { describe, expect, it } from 'vitest';
import { parseForegroundPackage, PhoneToolRunner, type AdbCall, type PhoneHoldNotice, type RateStore } from './phone-tools.js';
import type { GuardAnswer, GuardCheckBody, PhoneHoldClient } from './phone-hold-client.js';

// Edges only: the runtime's guard (HTTP client) and the phone (adb). The runner is real.

const SERIAL = '192.168.1.20:41234';
const PNG = Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 1, 2, 3]);
const OK_PARCEL = 'Result: Parcel(00000000    \'....\')';
const XML =
  '<?xml version="1.0" encoding="UTF-8"?><hierarchy rotation="0">' +
  '<node index="0" text="" resource-id="" class="android.widget.FrameLayout" package="com.example.files" content-desc="" focused="false" password="false" bounds="[0,0][1080,2400]">' +
  '<node index="1" text="Delete" resource-id="com.example.files:id/delete" class="android.widget.Button" package="com.example.files" content-desc="" focused="false" password="false" bounds="[100,2000][500,2200]" />' +
  '<node index="2" text="Open" resource-id="com.example.files:id/open" class="android.widget.Button" package="com.example.files" content-desc="" focused="false" password="false" bounds="[600,2000][1000,2200]" />' +
  '</node></hierarchy>';

const allow: GuardAnswer = { decision: 'allow', holdId: null };
const hold = (id: string): GuardAnswer => ({ decision: 'hold', approvalId: id, inboxId: `approval:${id}`, effect: 'delete', expiresAt: '2026-11-10T10:05:00Z' });

function fakePhone() {
  const shell: string[][] = [];
  let dumps = 0;
  const adb = (async (args: string[]) => {
    shell.push(args);
    const line = args.join(' ');
    if (line.includes('dumpsys activity')) return { stdout: '  topResumedActivity=ActivityRecord{9f1 u0 com.example.files/.MainActivity t12}\n', stderr: '', code: 0 };
    if (line.includes('isms')) return { stdout: OK_PARCEL, stderr: '', code: 0 };
    return { stdout: '', stderr: '', code: 0 };
  }) as AdbCall;
  adb.binary = async (args: string[]) => {
    if (args.includes('uiautomator')) {
      dumps++;
      return Buffer.from(XML);
    }
    return PNG;
  };
  return {
    adb,
    taps: () => shell.filter((a) => a.join(' ').startsWith('shell input tap')),
    sends: () => shell.filter((a) => a.join(' ').includes('isms')),
    dumps: () => dumps,
  };
}

/** A runtime that holds what `decide` says; every body it got is kept as sent. */
function fakeRuntime(decide: (b: GuardCheckBody) => GuardAnswer) {
  const bodies: GuardCheckBody[] = [];
  const client: PhoneHoldClient = {
    check: async (b) => {
      bodies.push(JSON.parse(JSON.stringify(b)) as GuardCheckBody);
      return decide(b);
    },
    getHold: async () => null,
    resolve: async () => true,
  };
  return { client, bodies };
}

function memoryRates(): RateStore {
  const data: Record<string, number[]> = {};
  return { load: (k) => data[k] ?? [], save: (k, v) => void (data[k] = v) };
}

function setup(guard: (() => PhoneHoldClient | null) | PhoneHoldClient, now = () => 1_000_000) {
  const phone = fakePhone();
  const notices: PhoneHoldNotice[] = [];
  const r = new PhoneToolRunner({
    adb: phone.adb,
    gate: {
      request: async () => {
        throw new Error('the in-process dialog gate must not run when the guard is wired');
      },
    },
    rates: memoryRates(),
    artemis: () => null,
    now,
    onlineSerials: () => [SERIAL],
    defaultSmsSubscription: async () => 3,
    guard: typeof guard === 'function' ? guard : () => guard,
    onHold: (n) => notices.push(n),
  });
  return { r, phone, notices };
}

describe('approval hold at dispatch', () => {
  it('holds a direct tap on "Delete" without touching the phone, and the approved retry taps once', async () => {
    const rt = fakeRuntime((b) => (b.holdId === 'h1' ? allow : /delete/i.test(b.el?.label ?? '') ? hold('h1') : allow));
    const { r, phone, notices } = setup(rt.client);

    const first = await r.call('phone.tap', { x: 300, y: 2100 });
    expect(first).toMatchObject({ ok: false, error: 'approval_required', approvalId: 'h1', inboxId: 'approval:h1', expiresAt: '2026-11-10T10:05:00Z' });
    expect(phone.taps()).toHaveLength(0);
    expect(rt.bodies[0]).toMatchObject({ serial: SERIAL, tool: 'phone.tap', el: { class: 'android.widget.Button', label: 'Delete', id: 'com.example.files:id/delete' }, app: 'com.example.files' });
    expect(notices[0]).toMatchObject({ approvalId: 'h1', summary: 'Tap “Delete”' });

    expect(await r.call('phone.tap', { x: 300, y: 2100, _hold: 'h1' })).toEqual({ ok: true });
    expect(phone.taps()).toEqual([['shell', 'input', 'tap', '300', '2100']]);
  });

  it('lets a tap on a harmless element through after one screen read', async () => {
    const rt = fakeRuntime((b) => (/delete/i.test(b.el?.label ?? '') ? hold('h1') : allow));
    const { r, phone } = setup(rt.client);
    expect(await r.call('phone.tap', { x: 700, y: 2100 })).toEqual({ ok: true });
    expect(rt.bodies[0].el?.label).toBe('Open');
    expect(phone.taps()).toHaveLength(1);
  });

  it('runs a call carrying a valid pass without reading the screen, sending the args as received', async () => {
    const rt = fakeRuntime((b) => (b.pass === 'pass_ok' ? allow : hold('h9')));
    const { r, phone } = setup(rt.client);
    const args = { x: 300, y: 2100, _pass: 'pass_ok', _run_id: 'run-7' };
    expect(await r.call('phone.tap', args)).toEqual({ ok: true });
    expect(phone.dumps()).toBe(0);
    expect(phone.taps()).toHaveLength(1);
    expect(rt.bodies[0]).toMatchObject({ tool: 'phone.tap', args, pass: 'pass_ok', runId: 'run-7' });
  });

  it('never lets a wrong pass through as a bare allow: it is held, not run', async () => {
    // A redeemed pass answers allow; anything else is classified, and Desktop marks it sensitive since it read no screen.
    const rt = fakeRuntime((b) => (b.pass === 'pass_ok' ? allow : b.sensitive ? hold('h2') : allow));
    const { r, phone } = setup(rt.client);
    expect(await r.call('phone.tap', { x: 700, y: 2100, _pass: 'pass_forged' })).toMatchObject({ ok: false, error: 'approval_required', approvalId: 'h2' });
    expect(phone.taps()).toHaveLength(0);
  });

  it('refuses acting calls when the runtime cannot be reached or nobody is signed in, but still reads', async () => {
    const down = fakeRuntime(() => ({ decision: 'unavailable', message: 'connect ECONNREFUSED' }));
    const a = setup(down.client);
    expect(await a.r.call('phone.tap', { x: 300, y: 2100 })).toMatchObject({ ok: false, error: 'guard_unavailable' });
    expect(await a.r.call('phone.open_app', { app: 'com.android.settings' })).toMatchObject({ ok: false, error: 'guard_unavailable' });
    expect(a.phone.taps()).toHaveLength(0);
    expect(await a.r.call('phone.screenshot', {})).toMatchObject({ ok: true, mimeType: 'image/png' });

    const signedOut = setup(() => null);
    expect(await signedOut.r.call('phone.type', { text: 'hello' })).toMatchObject({ ok: false, error: 'guard_unavailable' });
    expect(await signedOut.r.call('phone.read_ui', {})).toMatchObject({ ok: true });
  });

  it('returns approval_denied when the owner denied the hold', async () => {
    const rt = fakeRuntime(() => ({ decision: 'denied', approvalId: 'h3' }));
    const { r, phone } = setup(rt.client);
    expect(await r.call('phone.tap', { x: 300, y: 2100, _hold: 'h3' })).toMatchObject({ ok: false, error: 'approval_denied', approvalId: 'h3' });
    expect(phone.taps()).toHaveLength(0);
  });
});

describe('texts, calls and inbox reads through the hold', () => {
  const readHold = (b: GuardCheckBody): GuardAnswer => (b.holdId === 'r1' ? allow : hold('r1'));

  it('asks once per bot and phone to read texts', async () => {
    const rt = fakeRuntime(readHold);
    const { r } = setup(rt.client);
    expect(await r.call('phone.sms.read', { bot_id: 'bot-1' })).toMatchObject({ ok: false, error: 'approval_required', approvalId: 'r1' });
    expect(await r.call('phone.sms.read', { bot_id: 'bot-1', _hold: 'r1' })).toMatchObject({ ok: true, messages: [] });
    // Remembered for this bot on this phone: no new hold.
    expect(await r.call('phone.sms.read', { bot_id: 'bot-1' })).toMatchObject({ ok: true, messages: [] });
    // Another bot asks for itself.
    expect(await r.call('phone.sms.read', { bot_id: 'bot-2' })).toMatchObject({ ok: false, error: 'approval_required' });
  });

  it('always asks an API-key caller, even after an approved read', async () => {
    const rt = fakeRuntime(readHold);
    const { r } = setup(rt.client);
    expect(await r.call('phone.sms.read', { bot_id: 'bot-1', _source: 'api', _hold: 'r1' })).toMatchObject({ ok: true });
    expect(await r.call('phone.sms.read', { bot_id: 'bot-1', _source: 'api' })).toMatchObject({ ok: false, error: 'approval_required' });
  });

  it('binds a text hold to its recipient and keeps the rate limit', async () => {
    const rt = fakeRuntime((b) => (b.holdId === 's1' ? allow : hold('s1')));
    const { r, phone } = setup(rt.client);
    expect(await r.call('phone.sms.send', { to: '+15551234567', text: 'On my way' })).toMatchObject({ ok: false, error: 'approval_required', approvalId: 's1' });
    expect(phone.sends()).toHaveLength(0);
    expect(rt.bodies[0]).toMatchObject({ tool: 'phone.sms.send', el: { class: 'phone.sms.send', label: 'Text +15551234567' } });
    expect(await r.call('phone.sms.send', { to: '+15551234567', text: 'On my way', _hold: 's1' })).toMatchObject({ ok: true, status: 'submitted' });
    // A second text right away is stopped by the rate limit before any hold is made.
    expect(await r.call('phone.sms.send', { to: '+15551234567', text: 'Here', _hold: 's1' })).toMatchObject({ ok: false, error: 'rate_limited' });
    expect(phone.sends()).toHaveLength(1);
  });
});

describe('parseForegroundPackage', () => {
  it('reads the resumed activity package', () => {
    expect(parseForegroundPackage('  topResumedActivity=ActivityRecord{1a2b u0 com.chase.sig.android/.MainActivity t3}')).toBe('com.chase.sig.android');
    expect(parseForegroundPackage('    mResumedActivity: ActivityRecord{77 u0 com.whatsapp/com.whatsapp.HomeActivity t9}')).toBe('com.whatsapp');
    expect(parseForegroundPackage('nothing here')).toBeNull();
  });
});
