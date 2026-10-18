import { describe, expect, it } from 'vitest';
import {
  APP_CACHE_TTL_MS,
  buildMessages,
  buildThreads,
  DEFAULT_POLL_INTERVAL_MS,
  mapWithConcurrency,
  messageId,
  normalizeAddress,
  parseCallState,
  parseTelephonyState,
  parseContentQueryRows,
  parsePackageList,
  parseVersionName,
  PhoneUserComms,
  USER_SMS_LIMITS,
  type CommsEvent,
  type RawSmsRow,
} from './phone-user-comms.js';
import { buildIsmsSend, type AdbCall, type RateStore } from './phone-tools.js';

// ── Fixtures: real adb output formats (values are RAW, never escaped) ───────
//
// `adb shell content query --uri content://sms/inbox --projection address:body:date:read`
// on a Pixel/Android 14 prints one `Row: N …` per message. Bodies with commas,
// colons or newlines land in stdout as-is — a newline physically breaks the
// row across lines. These fixtures quote that exact shape.

const INBOX_STDOUT = [
  "Row: 0 address=+15551234567, body=Hey! Meet at 5:30, don't be late, date=1729123456000, read=1",
  // Body contains a comma AND a raw newline (the row spans two physical lines):
  'Row: 1 address=40404, body=Your Stripe code is 12,34',
  '56. It expires in 10 min, date=1729123459000, read=0',
  // Unicode body + an address with separators (normalizes into the same key):
  'Row: 2 address=+44 20 7123 4567, body=Dinner? 🍕 — see you at 7, date=1729123462000, read=0',
  // Body contains a DECOY that looks exactly like a field tail; the parser's
  // backward scan must skip it and use the genuine (last) tail:
  'Row: 3 address=40404, body=Moved the standup, date=1729000000000, read=1 — see you at 9, date=1729123465000, read=0',
].join('\n');

const SENT_STDOUT = [
  'Row: 0 address=+1 (555) 123-4567, body=On my way!, date=1729123457000, read=1',
  'Row: 1 address=+15551234567, body=Running late, sorry, date=1729123458000, read=1',
].join('\n');

const SIMINFO_STDOUT = [
  'Row: 0 _id=3, sim_id=0, display_name=Tello, carrier_name=Tello',
  'Row: 1 _id=4, sim_id=-1, display_name=, carrier_name=',
].join('\n');

const OK_PARCEL = "Result: Parcel(00000000    '....')";
const SERIAL = '192.168.1.20:41234';

// ── Test doubles ────────────────────────────────────────────────────────────

type Handler = (args: string[]) => { stdout?: string; stderr?: string; code?: number } | Promise<{ stdout?: string; stderr?: string; code?: number }>;

function fakeAdb(handler: Handler = () => ({})) {
  const calls: string[][] = [];
  const adb = (async (args: string[]) => {
    calls.push(args);
    const r = await handler(args);
    return { stdout: r.stdout ?? '', stderr: r.stderr ?? '', code: r.code ?? 0 };
  }) as AdbCall;
  adb.binary = async () => Buffer.alloc(0);
  return { adb, calls };
}

function memoryRates(): RateStore & { data: Record<string, number[]> } {
  const data: Record<string, number[]> = {};
  return { data, load: (k) => data[k] ?? [], save: (k, v) => void (data[k] = v) };
}

/** Routes the content-query / siminfo / isms shapes used across tests. */
function smsHandler(state: { inbox?: string; sent?: string; siminfo?: string; parcel?: string }) {
  return (args: string[]): { stdout?: string; code?: number } => {
    const joined = args.join(' ');
    if (joined.includes('content://sms/inbox')) return { stdout: state.inbox ?? '' };
    if (joined.includes('content://sms/sent')) return { stdout: state.sent ?? '' };
    if (joined.includes('content://telephony/siminfo')) return { stdout: state.siminfo ?? SIMINFO_STDOUT };
    if (joined.includes('service call isms')) return { stdout: state.parcel ?? OK_PARCEL };
    return {};
  };
}

function makeComms(opts: {
  handler?: Handler;
  online?: string[];
  now?: number;
  pollIntervalMs?: number;
  appCacheTtlMs?: number;
  maxAppConcurrency?: number;
} = {}) {
  const { adb, calls } = fakeAdb(opts.handler);
  const rates = memoryRates();
  let now = opts.now ?? 1_729_123_400_000;
  const comms = new PhoneUserComms({
    adb,
    rates,
    onlineSerials: () => opts.online ?? [SERIAL],
    now: () => now,
    pollIntervalMs: opts.pollIntervalMs,
    appCacheTtlMs: opts.appCacheTtlMs,
    maxAppConcurrency: opts.maxAppConcurrency,
  });
  return { comms, calls, rates, advance: (ms: number) => (now += ms), setNow: (v: number) => (now = v) };
}

const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));

// ── parseContentQueryRows ───────────────────────────────────────────────────

describe('parseContentQueryRows', () => {
  it('parses a plain single-line Row exactly as adb prints it', () => {
    const { rows, malformed } = parseContentQueryRows(
      'Row: 0 address=+15551234567, body=Hey, date=1729123456000, read=1',
      'in',
    );
    expect(malformed).toBe(0);
    expect(rows).toEqual([
      { address: '+15551234567', body: 'Hey', timestamp: 1729123456000, read: true, direction: 'in' },
    ]);
  });

  it('keeps colons, commas and quotes inside bodies', () => {
    const { rows } = parseContentQueryRows(INBOX_STDOUT, 'in');
    expect(rows[0].body).toBe("Hey! Meet at 5:30, don't be late");
  });

  it('rejoins a body that physically spans lines (raw newline in the SMS)', () => {
    const { rows } = parseContentQueryRows(INBOX_STDOUT, 'in');
    expect(rows[1].address).toBe('40404');
    expect(rows[1].body).toBe('Your Stripe code is 12,34\n56. It expires in 10 min');
    expect(rows[1].timestamp).toBe(1729123459000);
    expect(rows[1].read).toBe(false);
  });

  it('preserves unicode bodies', () => {
    const { rows } = parseContentQueryRows(INBOX_STDOUT, 'in');
    expect(rows[2].body).toBe('Dinner? 🍕 — see you at 7');
  });

  it('skips a decoy `, date=…, read=…` inside a body via the backward tail scan', () => {
    const { rows } = parseContentQueryRows(INBOX_STDOUT, 'in');
    expect(rows[3].body).toBe('Moved the standup, date=1729000000000, read=1 — see you at 9');
    expect(rows[3].timestamp).toBe(1729123465000);
    expect(rows[3].read).toBe(false);
  });

  it('handles CRLF line endings', () => {
    const crlf = 'Row: 0 address=40404, body=hi, date=1729123456000, read=1\r\nRow: 1 address=40404, body=yo, date=1729123457000, read=0';
    const { rows } = parseContentQueryRows(crlf, 'in');
    expect(rows.map((r) => r.body)).toEqual(['hi', 'yo']);
  });

  it('counts rows that do not match the pinned projection as malformed', () => {
    const { rows, malformed } = parseContentQueryRows(
      'Row: 0 address=+15551234567, body=oops, no date here\nRow: 1 address=, body=x, date=1729123456000, read=1',
      'in',
    );
    expect(rows).toHaveLength(0);
    expect(malformed).toBe(2);
  });

  it('ignores stray non-Row output lines', () => {
    const { rows, malformed } = parseContentQueryRows(
      'Some warning from the shell\nRow: 0 address=40404, body=ok, date=1729123456000, read=1',
      'in',
    );
    expect(rows).toHaveLength(1);
    expect(malformed).toBe(0);
  });
});

// ── normalizeAddress / buildThreads / buildMessages ─────────────────────────

describe('thread building', () => {
  const rows = (): RawSmsRow[] => [
    ...parseContentQueryRows(INBOX_STDOUT, 'in').rows,
    ...parseContentQueryRows(SENT_STDOUT, 'out').rows,
  ];

  it('normalizes separators but keeps + and shortcodes', () => {
    expect(normalizeAddress('+1 (555) 123-4567')).toBe('+15551234567');
    expect(normalizeAddress('+44 20 7123 4567')).toBe('+442071234567');
    expect(normalizeAddress('40404')).toBe('40404');
  });

  it('merges inbox+sent by normalized address, sorted by lastTimestamp desc', () => {
    const threads = buildThreads(rows());
    expect(threads.map((t) => t.address)).toEqual(['40404', '+442071234567', '+15551234567']);
    const merged = threads.find((t) => t.address === '+15551234567')!;
    expect(merged.lastTimestamp).toBe(1729123458000);
    expect(merged.lastSnippet).toBe('Running late, sorry');
    expect(merged.unreadCount).toBe(0);
    expect(threads.find((t) => t.address === '40404')!.unreadCount).toBe(2);
  });

  it('leaves displayName undefined (UI resolves contacts)', () => {
    for (const t of buildThreads(rows())) expect(t.displayName).toBeUndefined();
  });

  it('builds ascending messages for one address with in/out directions', () => {
    const messages = buildMessages(rows(), '+1 (555) 123-4567'); // separators in the query still match
    expect(messages.map((m) => [m.direction, m.timestamp])).toEqual([
      ['in', 1729123456000],
      ['out', 1729123457000],
      ['out', 1729123458000],
    ]);
  });

  it('afterTimestamp is an exclusive incremental-sync filter', () => {
    const messages = buildMessages(rows(), '+15551234567', { afterTimestamp: 1729123456000 });
    expect(messages.map((m) => m.timestamp)).toEqual([1729123457000, 1729123458000]);
  });

  it('message ids are deterministic and distinct per message', () => {
    const a = buildMessages(rows(), '+15551234567');
    const b = buildMessages(rows(), '+15551234567');
    expect(a.map((m) => m.id)).toEqual(b.map((m) => m.id));
    expect(new Set(a.map((m) => m.id)).size).toBe(a.length);
    expect(messageId('in', 1, 'x', 'y')).toMatch(/^in:1:[0-9a-f]{8}$/);
  });
});

// ── smsList / smsMessages (class) ───────────────────────────────────────────

describe('smsList', () => {
  it('queries inbox and sent with the pinned projection and merges threads', async () => {
    const state = { inbox: INBOX_STDOUT, sent: SENT_STDOUT };
    const { comms, calls } = makeComms({ handler: smsHandler(state) });
    const res = await comms.smsList(SERIAL);
    expect(res.ok).toBe(true);
    if (res.ok) expect(res.threads.map((t) => t.address)).toEqual(['40404', '+442071234567', '+15551234567']);
    expect(calls).toContainEqual(['shell', 'content', 'query', '--uri', 'content://sms/inbox', '--projection', 'address:body:date:read']);
    expect(calls).toContainEqual(['shell', 'content', 'query', '--uri', 'content://sms/sent', '--projection', 'address:body:date:read']);
  });

  it('an empty box is { ok: true, threads: [] }, not an error', async () => {
    const { comms } = makeComms({ handler: smsHandler({ inbox: '', sent: '' }) });
    const res = await comms.smsList(SERIAL);
    expect(res).toEqual({ ok: true, threads: [] });
  });

  it('OEM-restricted reads (SecurityException) are read_blocked, distinct from empty', async () => {
    const { comms } = makeComms({
      handler: () => ({ code: 1, stderr: 'java.lang.SecurityException: Reading SMS is not allowed' }),
    });
    const res = await comms.smsList(SERIAL);
    expect(res).toEqual({ ok: false, message: 'read_blocked' });
  });

  it('Permission Denial on stdout with a zero exit is still read_blocked', async () => {
    const { comms } = makeComms({
      handler: (args) =>
        args.join(' ').includes('content://sms') ? { stdout: 'Permission Denial: reading com.android.providers.telephony requires android.permission.READ_SMS' } : {},
    });
    expect(await comms.smsList(SERIAL)).toEqual({ ok: false, message: 'read_blocked' });
  });

  it('a plain transport failure is adb_failed, not read_blocked', async () => {
    const { comms } = makeComms({ handler: () => ({ code: 1, stderr: 'error: device offline' }) });
    expect(await comms.smsList(SERIAL)).toMatchObject({ ok: false, message: expect.stringContaining('adb_failed') });
  });

  it('non-empty output with no Row lines is parse_failed', async () => {
    const { comms } = makeComms({ handler: (args) => (args.join(' ').includes('content://sms') ? { stdout: 'garbage output' } : {}) });
    expect(await comms.smsList(SERIAL)).toEqual({ ok: false, message: 'parse_failed: content query returned unrecognized rows' });
  });

  it('never throws across the public API', async () => {
    const { comms } = makeComms({
      handler: () => {
        throw new Error('adb exploded');
      },
    });
    expect(await comms.smsList(SERIAL)).toMatchObject({ ok: false });
    expect(await comms.smsMessages(SERIAL, '40404')).toMatchObject({ ok: false });
    expect(await comms.callState(SERIAL)).toMatchObject({ ok: false });
    expect(await comms.listApps(SERIAL)).toMatchObject({ ok: false });
  });
});

describe('smsMessages', () => {
  it('returns ascending messages and supports incremental sync', async () => {
    const { comms } = makeComms({ handler: smsHandler({ inbox: INBOX_STDOUT, sent: SENT_STDOUT }) });
    const res = await comms.smsMessages(SERIAL, '40404');
    expect(res.ok).toBe(true);
    if (res.ok) {
      expect(res.messages.map((m) => m.direction)).toEqual(['in', 'in']);
      expect(res.messages[0].body).toBe('Your Stripe code is 12,34\n56. It expires in 10 min');
    }
    const inc = await comms.smsMessages(SERIAL, '40404', { afterTimestamp: 1729123459000 });
    expect(inc.ok && inc.messages.map((m) => m.timestamp)).toEqual([1729123465000]);
  });

  it('propagates read_blocked', async () => {
    const { comms } = makeComms({ handler: () => ({ code: 1, stderr: 'SecurityException: nope' }) });
    expect(await comms.smsMessages(SERIAL, '40404')).toEqual({ ok: false, message: 'read_blocked' });
  });

  it('rejects an empty address', async () => {
    const { comms } = makeComms({ handler: smsHandler({}) });
    expect(await comms.smsMessages(SERIAL, '   ')).toMatchObject({ ok: false });
  });
});

// ── smsSendUser ─────────────────────────────────────────────────────────────

describe('smsSendUser', () => {
  it('sends via buildIsmsSend with the slot-0 subId from siminfo and reports submitted', async () => {
    const { comms, calls } = makeComms({ handler: smsHandler({}) });
    const res = await comms.smsSendUser(SERIAL, '+15551234567', "it's fine");
    expect(res).toEqual({ ok: true, status: 'submitted' });
    expect(calls.at(-1)).toEqual(buildIsmsSend(3, '+15551234567', "it's fine"));
  });

  it('falls back to subId 0 when siminfo cannot be read (default-SIM path)', async () => {
    const { comms, calls } = makeComms({ handler: smsHandler({ siminfo: '' }) });
    expect(await comms.smsSendUser(SERIAL, '+15551234567', 'hi')).toEqual({ ok: true, status: 'submitted' });
    expect(calls.at(-1)).toEqual(buildIsmsSend(0, '+15551234567', 'hi'));
  });

  it('emergency destinations are sent but flagged so the UI can warn-confirm', async () => {
    const { comms, calls } = makeComms({ handler: smsHandler({}) });
    const res = await comms.smsSendUser(SERIAL, '911', 'help');
    expect(res).toEqual({ ok: true, status: 'submitted', emergency: true });
    expect(calls.at(-1)).toEqual(buildIsmsSend(3, '911', 'help'));
  });

  it('enforces the 5-second gap with an injectable clock', async () => {
    const { comms, advance } = makeComms({ handler: smsHandler({}) });
    expect(await comms.smsSendUser(SERIAL, '+15551234567', 'one')).toMatchObject({ ok: true });
    const denied = await comms.smsSendUser(SERIAL, '+15551234567', 'two');
    expect(denied).toMatchObject({ ok: false, message: expect.stringContaining('rate_limited') });
    if (!denied.ok) expect(denied.retryAfterSec).toBeGreaterThan(0);
    advance(USER_SMS_LIMITS.minGapMs);
    expect(await comms.smsSendUser(SERIAL, '+15551234567', 'three')).toMatchObject({ ok: true });
  });

  it('caps at 100 sends per device per day', async () => {
    const now = 1_729_123_400_000;
    const { comms, rates } = makeComms({ handler: smsHandler({}), now });
    rates.data[`${SERIAL}:sms:user`] = Array.from({ length: 100 }, (_, i) => now - (i + 1) * 60_000);
    const denied = await comms.smsSendUser(SERIAL, '+15551234567', 'one more');
    expect(denied).toMatchObject({ ok: false, message: expect.stringContaining('daily limit') });
  });

  it('uses a per-lane rate key (bot-lane history does not count against the user)', async () => {
    const now = 1_729_123_400_000;
    const { comms, rates } = makeComms({ handler: smsHandler({}), now });
    rates.data[`${SERIAL}:sms`] = [now - 1000]; // a bot send one second ago
    expect(await comms.smsSendUser(SERIAL, '+15551234567', 'mine')).toMatchObject({ ok: true });
  });

  it('counts a failed send against the quota (bot-lane parity)', async () => {
    const { comms, handlerCalls } = (() => {
      const made = makeComms({ handler: smsHandler({ parcel: 'Result: Parcel(ffffffff    \'....\')' }) });
      return { ...made, handlerCalls: made.calls };
    })();
    expect(await comms.smsSendUser(SERIAL, '+15551234567', 'boom')).toMatchObject({ ok: false });
    expect(handlerCalls.length).toBeGreaterThan(0);
    expect(await comms.smsSendUser(SERIAL, '+15551234567', 'again')).toMatchObject({ ok: false, message: expect.stringContaining('rate_limited') });
  });

  it('rejects a phone-shaped but empty destination and over-long text', async () => {
    const { comms } = makeComms({ handler: smsHandler({}) });
    expect(await comms.smsSendUser(SERIAL, 'abc', 'hi')).toMatchObject({ ok: false, message: expect.stringContaining('invalid') });
    expect(await comms.smsSendUser(SERIAL, '+15551234567', '')).toMatchObject({ ok: false });
    expect(await comms.smsSendUser(SERIAL, '+15551234567', 'x'.repeat(1001))).toMatchObject({ ok: false });
  });
});

// ── callDialUser / callEnd ──────────────────────────────────────────────────

describe('callDialUser', () => {
  it('launches the dialer with a quoted tel: URI', async () => {
    const { comms, calls } = makeComms({ handler: () => ({ stdout: 'Starting: Intent { act=android.intent.action.CALL dat=tel:xxx }' }) });
    const res = await comms.callDialUser(SERIAL, '+15551234567');
    expect(res).toEqual({ ok: true, status: 'dialing' });
    expect(calls.at(-1)).toEqual([
      'shell', 'am', 'start', '-a', 'android.intent.action.CALL', '-d', "'tel:%2B15551234567'",
    ]);
  });

  it('refuses emergency numbers without confirmEmergency and never touches adb', async () => {
    const { comms, calls } = makeComms({ handler: () => ({}) });
    const res = await comms.callDialUser(SERIAL, '112');
    expect(res).toEqual({ ok: false, message: 'emergency_confirm_required' });
    expect(calls).toHaveLength(0);
    expect(await comms.callDialUser(SERIAL, '112', { confirmEmergency: true })).toMatchObject({ ok: true, emergency: true });
  });

  it('reports dialer errors as adb_failed', async () => {
    const { comms } = makeComms({ handler: () => ({ stdout: 'Error: Activity not started, permission denied' }) });
    expect(await comms.callDialUser(SERIAL, '+15551234567')).toMatchObject({ ok: false, message: expect.stringContaining('adb_failed') });
  });
});

describe('callEnd', () => {
  it('sends KEYCODE_ENDCALL', async () => {
    const { comms, calls } = makeComms({ handler: () => ({}) });
    expect(await comms.callEnd(SERIAL)).toEqual({ ok: true });
    expect(calls.at(-1)).toEqual(['shell', 'input', 'keyevent', 'KEYCODE_ENDCALL']);
  });

  it('surfaces adb failure', async () => {
    const { comms } = makeComms({ handler: () => ({ code: 1, stderr: 'error: closed' }) });
    expect(await comms.callEnd(SERIAL)).toMatchObject({ ok: false });
  });
});

// ── callState ───────────────────────────────────────────────────────────────

const REGISTRY_IDLE = '  mCallState=0\n  mCallIncomingNumber=\n';
const REGISTRY_OFFHOOK = '  mCallState=2\n  mCallIncomingNumber=+15551234567\n';
const REGISTRY_RINGING = '  mCallState=1\n  mCallIncomingNumber=+15559876543\n';
const ACTIVITIES_INCALL_RESUMED = [
  '  mResumedActivity: ActivityRecord{u0 com.google.android.dialer/.incallui.InCallActivity t142}',
  '    * Hist  #0: ActivityRecord{u0 com.google.android.dialer/.incallui.InCallActivity t142}',
].join('\n');

describe('callState', () => {
  const commsWith = (activities: string, registry: string) =>
    makeComms({ handler: (args) => (args.join(' ').includes('telephony.registry') ? { stdout: registry } : { stdout: activities }) }).comms;

  it('idle when registry says 0 and no in-call UI', async () => {
    const res = await commsWith('', REGISTRY_IDLE).callState(SERIAL);
    expect(res).toEqual({ ok: true, state: 'idle' });
  });

  it('dialing during the pre-UI window (offhook, no incall match)', async () => {
    const res = await commsWith('', REGISTRY_OFFHOOK).callState(SERIAL);
    expect(res).toEqual({ ok: true, state: 'dialing', number: '+15551234567' });
  });

  it('active with the caller number once the in-call UI is foreground', async () => {
    const res = await commsWith(ACTIVITIES_INCALL_RESUMED, REGISTRY_OFFHOOK).callState(SERIAL);
    expect(res).toEqual({ ok: true, state: 'active', number: '+15551234567' });
  });

  it('maps ringing to active for 3-state consumers and flags the ring', async () => {
    const res = await commsWith('', REGISTRY_RINGING).callState(SERIAL);
    expect(res).toEqual({ ok: true, state: 'active', number: '+15559876543', ringing: true });
  });

  it('asks dumpsys for the caller-number line too (grep keeps mCallIncomingNumber)', async () => {
    const { comms, calls } = makeComms({ handler: () => ({ stdout: REGISTRY_IDLE }) });
    await comms.callState(SERIAL);
    expect(calls.some((c) => c.join(' ').includes('CallIncomingNumber'))).toBe(true);
  });

  it('falls back to UI presence when no mCallState lines parse (OEM rename)', async () => {
    const samsung = '  mResumedActivity: ActivityRecord{u0 com.samsung.android.incallui.InCallActivity t9}';
    expect(await commsWith(samsung, '  telephony_custom_state=2').callState(SERIAL)).toMatchObject({ ok: true, state: 'active' });
    expect(await commsWith('', '  telephony_custom_state=2').callState(SERIAL)).toMatchObject({ ok: true, state: 'idle' });
  });

  it('reads a number from a tel: URI in the activities dump as a fallback', () => {
    const parsed = parseCallState('  resumed: InCallActivity intent={act=CALL dat=tel:%2B15557654321}', '');
    expect(parsed.number).toBe('+15557654321');
  });

  it('tolerates one failed dumpsys when the other succeeds', async () => {
    const { comms } = makeComms({
      handler: (args) => (args.join(' ').includes('telephony.registry') ? { code: 1, stderr: 'fail' } : { stdout: '' }),
    });
    expect(await comms.callState(SERIAL)).toMatchObject({ ok: true, state: 'idle' });
  });

  it('is adb_failed only when both dumps fail', async () => {
    const { comms } = makeComms({ handler: () => ({ code: 1, stderr: 'error: device offline' }) });
    expect(await comms.callState(SERIAL)).toMatchObject({ ok: false });
  });
});

// ── telephonyState / callAnswer (call bridge) ──────────────────────────────

describe('parseTelephonyState', () => {
  it('reads idle, ringing and offhook', () => {
    expect(parseTelephonyState(REGISTRY_IDLE)).toEqual({ state: 'idle' });
    expect(parseTelephonyState(REGISTRY_RINGING)).toEqual({ state: 'ringing', number: '+15559876543' });
    expect(parseTelephonyState(REGISTRY_OFFHOOK)).toEqual({ state: 'offhook', number: '+15551234567' });
  });

  it('dual SIM: one subscription ringing beats an idle one', () => {
    const dual = '  Phone Id=0\n  mCallState=0\n  Phone Id=1\n  mCallState=1\n  mCallIncomingNumber=+15550001111\n';
    expect(parseTelephonyState(dual)).toEqual({ state: 'ringing', number: '+15550001111' });
  });

  it('a withheld caller rings with no number', () => {
    expect(parseTelephonyState('  mCallState=1\n  mCallIncomingNumber=\n')).toEqual({ state: 'ringing' });
  });

  it('unknown when no mCallState line parses', () => {
    expect(parseTelephonyState('')).toEqual({ state: 'unknown' });
  });
});

describe('telephonyState', () => {
  it('polls one cheap registry dump', async () => {
    const { comms, calls } = makeComms({ handler: () => ({ stdout: REGISTRY_RINGING }) });
    expect(await comms.telephonyState(SERIAL)).toEqual({ ok: true, state: 'ringing', number: '+15559876543' });
    expect(calls).toHaveLength(1);
    expect(calls[0].join(' ')).toContain('telephony.registry');
  });

  it('grep finding nothing (exit 1, no stderr) is unknown, not a failure', async () => {
    const { comms } = makeComms({ handler: () => ({ code: 1, stdout: '' }) });
    expect(await comms.telephonyState(SERIAL)).toEqual({ ok: true, state: 'unknown' });
  });

  it('a dead transport is adb_failed', async () => {
    const { comms } = makeComms({ handler: () => ({ code: 1, stderr: 'error: device offline' }) });
    expect(await comms.telephonyState(SERIAL)).toMatchObject({ ok: false, message: expect.stringContaining('adb_failed') });
  });
});

describe('callAnswer', () => {
  it('sends KEYCODE_CALL by default and HEADSETHOOK on request', async () => {
    const { comms, calls } = makeComms({ handler: () => ({}) });
    expect(await comms.callAnswer(SERIAL)).toEqual({ ok: true });
    expect(calls.at(-1)).toEqual(['shell', 'input', 'keyevent', 'KEYCODE_CALL']);
    await comms.callAnswer(SERIAL, 'KEYCODE_HEADSETHOOK');
    expect(calls.at(-1)).toEqual(['shell', 'input', 'keyevent', 'KEYCODE_HEADSETHOOK']);
  });
});

// ── SMS watcher ─────────────────────────────────────────────────────────────

describe('SMS watcher', () => {
  it('baselines silently, then emits when a thread advances, and pauses at zero subscribers', async () => {
    const state = { inbox: INBOX_STDOUT, sent: SENT_STDOUT };
    const { comms, calls } = makeComms({ handler: smsHandler(state), pollIntervalMs: 25 });
    const events: CommsEvent[] = [];
    const unsub = comms.onSmsEvent((ev) => events.push(ev));
    expect(comms.watcherActive).toBe(true);

    await sleep(90); // baseline poll(s)
    expect(events).toEqual([]); // no emit storm on first sight

    // A new message lands in the 40404 thread.
    state.inbox = [
      `Row: 0 address=40404, body=fresh message, date=1729123470000, read=0`,
      INBOX_STDOUT,
    ].join('\n');
    await sleep(90);
    expect(events).toEqual([{ type: 'sms', address: '40404' }]);

    const callsAfterEmit = calls.length;
    unsub();
    expect(comms.watcherActive).toBe(false);
    state.inbox = `Row: 0 address=40404, body=another, date=1729123480000, read=0\n${state.inbox}`;
    await sleep(90);
    expect(calls.length).toBe(callsAfterEmit); // polling paused
    expect(events).toEqual([{ type: 'sms', address: '40404' }]);
  });

  it('onCommsEvent is the same subscription under the pinned bridge name', async () => {
    const state = { inbox: INBOX_STDOUT, sent: '' };
    const { comms } = makeComms({ handler: smsHandler(state), pollIntervalMs: 25 });
    const events: CommsEvent[] = [];
    const unsub = comms.onCommsEvent((ev) => events.push(ev));
    await sleep(90);
    state.inbox = `Row: 0 address=40404, body=new!, date=1729130000000, read=0\n${state.inbox}`;
    await sleep(90);
    unsub();
    expect(events).toEqual([{ type: 'sms', address: '40404' }]);
  });

  it('supports several subscribers and survives a throwing listener', async () => {
    const state = { inbox: INBOX_STDOUT, sent: '' };
    const { comms } = makeComms({ handler: smsHandler(state), pollIntervalMs: 25 });
    const good: CommsEvent[] = [];
    const unsubA = comms.onSmsEvent(() => {
      throw new Error('bad listener');
    });
    const unsubB = comms.onSmsEvent((ev) => good.push(ev));
    await sleep(90);
    state.inbox = `Row: 0 address=40404, body=new!, date=1729130000000, read=0\n${state.inbox}`;
    await sleep(90);
    expect(good).toEqual([{ type: 'sms', address: '40404' }]);
    unsubA();
    unsubB();
    expect(comms.watcherActive).toBe(false);
  });

  it('polls every online serial and tolerates read failures', async () => {
    const { comms } = makeComms({
      handler: (args) => (args.join(' ').includes('content://sms') ? { code: 1, stderr: 'SecurityException' } : {}),
      online: ['A', 'B'],
      pollIntervalMs: 25,
    });
    const unsub = comms.onSmsEvent(() => {});
    await sleep(90); // must not reject or die
    unsub();
    expect(comms.watcherActive).toBe(false);
  });

  it('default poll interval is 15 s', () => {
    expect(DEFAULT_POLL_INTERVAL_MS).toBe(15_000);
  });
});

// ── listApps ────────────────────────────────────────────────────────────────

const PM_LIST = ['package:com.whatsapp', 'package:org.mozilla.firefox', 'package:com.bank.app'].join('\n');

function dumpsysHandler(behavior: { versionByPkg?: Record<string, string>; delayMs?: number; track?: { maxInFlight: number } }) {
  let inFlight = 0;
  return async (args: string[]): Promise<{ stdout?: string; stderr?: string; code?: number }> => {
    const joined = args.join(' ');
    if (joined.startsWith('shell pm list packages')) return { stdout: PM_LIST };
    if (joined.startsWith('shell dumpsys package ')) {
      const pkg = args[3];
      inFlight++;
      behavior.track && (behavior.track.maxInFlight = Math.max(behavior.track.maxInFlight, inFlight));
      if (behavior.delayMs) await sleep(behavior.delayMs);
      inFlight--;
      if (behavior.versionByPkg && pkg in behavior.versionByPkg) {
        return { stdout: `  versionCode=42 minSdk=28 targetSdk=34\n    versionName=${behavior.versionByPkg[pkg]}\n` };
      }
      return { code: 1, stderr: 'Error: unknown package' };
    }
    return {};
  };
}

describe('listApps', () => {
  it('lists third-party packages with versionName from dumpsys', async () => {
    const { comms, calls } = makeComms({
      handler: dumpsysHandler({ versionByPkg: { 'com.whatsapp': '2.24.15.78', 'org.mozilla.firefox': '131.0' } }),
    });
    const res = await comms.listApps(SERIAL);
    expect(res.ok).toBe(true);
    if (res.ok) {
      expect(res.apps).toEqual([
        { pkg: 'com.whatsapp', versionName: '2.24.15.78' },
        { pkg: 'org.mozilla.firefox', versionName: '131.0' },
        { pkg: 'com.bank.app', versionName: undefined },
      ]);
    }
    expect(calls.some((c) => c.join(' ') === 'shell pm list packages -3')).toBe(true);
  });

  it('bounds the dumpsys fan-out at 8 in flight', async () => {
    const track = { maxInFlight: 0 };
    let current = 0;
    const pkgs = Array.from({ length: 30 }, (_, i) => `package:app.pkg${i}`);
    const { comms } = makeComms({
      maxAppConcurrency: 8,
      handler: async (args) => {
        if (args.join(' ').startsWith('shell pm list packages')) return { stdout: pkgs.join('\n') };
        if (args.join(' ').startsWith('shell dumpsys package ')) {
          current++;
          track.maxInFlight = Math.max(track.maxInFlight, current);
          await sleep(5);
          current--;
          return { stdout: '    versionName=1.0\n' };
        }
        return {};
      },
    });
    const res = await comms.listApps(SERIAL);
    expect(res.ok).toBe(true);
    expect(track.maxInFlight).toBeGreaterThan(1); // actually parallel
    expect(track.maxInFlight).toBeLessThanOrEqual(8);
  });

  it('caches per serial for 10 minutes, then refreshes', async () => {
    let pmCalls = 0;
    const { comms, advance } = makeComms({
      handler: async (args) => {
        if (args.join(' ').startsWith('shell pm list packages')) {
          pmCalls++;
          return { stdout: PM_LIST };
        }
        return { stdout: '    versionName=1.0\n' };
      },
    });
    await comms.listApps(SERIAL);
    await comms.listApps(SERIAL);
    expect(pmCalls).toBe(1);
    advance(APP_CACHE_TTL_MS + 1);
    await comms.listApps(SERIAL);
    expect(pmCalls).toBe(2);
  });

  it('invalidateApps forces a refresh', async () => {
    let pmCalls = 0;
    const { comms } = makeComms({
      handler: async (args) => {
        if (args.join(' ').startsWith('shell pm list packages')) {
          pmCalls++;
          return { stdout: PM_LIST };
        }
        return { stdout: '    versionName=1.0\n' };
      },
    });
    await comms.listApps(SERIAL);
    comms.invalidateApps(SERIAL);
    await comms.listApps(SERIAL);
    expect(pmCalls).toBe(2);
  });

  it('pm failure is adb_failed; non-package output is parse_failed; no apps installed is empty', async () => {
    const failing = makeComms({ handler: () => ({ code: 1, stderr: 'error: device offline' }) });
    expect(await failing.comms.listApps(SERIAL)).toMatchObject({ ok: false, message: expect.stringContaining('adb_failed') });

    const junk = makeComms({ handler: (args) => (args.join(' ').startsWith('shell pm list packages') ? { stdout: 'Unexpected token package:foo' } : {}) });
    expect(await junk.comms.listApps(SERIAL)).toEqual({ ok: false, message: 'parse_failed: pm list packages returned unrecognized output' });

    const none = makeComms({ handler: (args) => (args.join(' ').startsWith('shell pm list packages') ? { stdout: '' } : {}) });
    expect(await none.comms.listApps(SERIAL)).toEqual({ ok: true, apps: [] });
  });
});

// ── small helpers ───────────────────────────────────────────────────────────

describe('parsing helpers', () => {
  it('parsePackageList requires the package: prefix on every line', () => {
    expect(parsePackageList('package:a.b\npackage:c.d\n')).toEqual({ pkgs: ['a.b', 'c.d'], malformed: false });
    expect(parsePackageList('')).toEqual({ pkgs: [], malformed: false });
    expect(parsePackageList('package:a.b\njunk\n')).toEqual({ pkgs: ['a.b'], malformed: true });
  });

  it('parseVersionName reads the indented versionName line', () => {
    expect(parseVersionName('  versionCode=7\n    versionName=3.2.1 extra\n')).toBe('3.2.1 extra');
    expect(parseVersionName('nothing here')).toBeUndefined();
  });

  it('mapWithConcurrency runs everything, bounded', async () => {
    let inFlight = 0;
    let max = 0;
    const out = await mapWithConcurrency(Array.from({ length: 20 }, (_, i) => i), 4, async (n) => {
      inFlight++;
      max = Math.max(max, inFlight);
      await sleep(1);
      inFlight--;
      return n * 2;
    });
    expect(out).toEqual(Array.from({ length: 20 }, (_, i) => i * 2));
    expect(max).toBeLessThanOrEqual(4);
    expect(max).toBeGreaterThan(1);
  });
});
