import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { holdNoticeSender, phoneHoldHandler, HOLD_NOTICE_HEARTBEAT_MS, HOLD_NOTICE_POLL_MS } from './phone-hold-notice.js';
import type { HoldView, PhoneHoldClient } from './phone-hold-client.js';
import { PhoneToolRunner, type AdbCall, type PhoneHoldNotice } from './phone-tools.js';

// Edges only: the runtime's hold read (HTTP client) and the link transport to the helper.

const NOW = Date.parse('2026-11-10T10:00:00Z');
const notice = (over: Partial<PhoneHoldNotice> = {}): PhoneHoldNotice => ({
  approvalId: 'a1',
  inboxId: 'approval:a1',
  expiresAt: '2026-11-10T10:10:00Z',
  effect: 'send',
  serial: 'link:pixel',
  tool: 'phone.tap',
  app: 'com.whatsapp',
  summary: 'Tap “Send”',
  ...over,
});

function rig(opts: { android?: (serial: string) => boolean; helperFails?: boolean } = {}) {
  const link = { down: opts.helperFails === true };
  const states = new Map<string, string | null>();
  const calls: Array<{ serial: string; method: string; params: Record<string, unknown> }> = [];
  const client: PhoneHoldClient = {
    check: async () => ({ decision: 'unavailable', message: 'unused' }),
    getHold: async (id) => {
      const state = states.has(id) ? (states.get(id) ?? null) : 'requested';
      return state === null ? null : ({ approvalId: id, inboxId: `approval:${id}`, state: state as string, consumed: false, effect: 'send', phone: {}, expiresAt: '' } satisfies HoldView);
    },
    resolve: async () => true,
  };
  const send = holdNoticeSender({
    client,
    isAndroidLink: opts.android ?? ((s) => s.startsWith('link:')),
    helperCall: async (serial, method, params) => {
      calls.push({ serial, method, params });
      if (link.down) throw new Error('Method not found');
    },
  });
  const tick = (n = 1) => vi.advanceTimersByTimeAsync(HOLD_NOTICE_POLL_MS * n);
  return { send, states, calls, tick, link };
}

describe('hold notice on the driven Android phone', () => {
  beforeEach(() => {
    vi.useFakeTimers();
    vi.setSystemTime(NOW);
  });
  afterEach(() => {
    vi.useRealTimers();
  });

  it('sends waiting once when a hold is created, with effect, app and expiry only', async () => {
    const r = rig();
    r.send(notice());
    await r.tick(1);
    expect(r.calls).toEqual([
      { serial: 'link:pixel', method: 'hold_notice', params: { state: 'waiting', effect: 'send', app: 'com.whatsapp', expiresAt: '2026-11-10T10:10:00Z' } },
    ]);
    // The tool is retried while the hold is still open: no second notice.
    r.send(notice());
    await r.tick(2);
    expect(r.calls).toHaveLength(1);
  });

  it.each(['approved', 'denied', 'expired', 'changed'])('sends cleared when the hold is %s', async (state) => {
    const r = rig();
    r.send(notice());
    r.states.set('a1', state);
    await r.tick(1);
    expect(r.calls.map((c) => c.params.state)).toEqual(['waiting', 'cleared']);
    await r.tick(3);
    expect(r.calls).toHaveLength(2);
  });

  it('sends cleared at the hold expiry time when nothing else closed it', async () => {
    const r = rig();
    r.send(notice({ expiresAt: '2026-11-10T10:00:10Z' }));
    r.states.set('a1', null); // unreadable on the first polls only
    await vi.advanceTimersByTimeAsync(10_000);
    expect(r.calls.map((c) => c.params.state)).toEqual(['waiting', 'cleared']);
  });

  it('sends cleared when the runtime stays unreachable (guard unavailable)', async () => {
    const r = rig();
    r.send(notice());
    r.states.set('a1', null);
    await r.tick(4);
    expect(r.calls.map((c) => c.params.state)).toEqual(['waiting', 'cleared']);
  });

  it('keeps the notice until the last of two holds on one phone closes', async () => {
    const r = rig();
    r.send(notice({ approvalId: 'a1' }));
    r.send(notice({ approvalId: 'a2', effect: 'pay' }));
    r.states.set('a1', 'approved');
    await r.tick(1);
    expect(r.calls.map((c) => c.params.state)).toEqual(['waiting', 'waiting']);
    r.states.set('a2', 'denied');
    await r.tick(1);
    expect(r.calls.map((c) => c.params.state)).toEqual(['waiting', 'waiting', 'cleared']);
  });

  it('sends nothing for an iPhone or an adb-only phone', async () => {
    const r = rig({ android: () => false });
    r.send(notice({ serial: 'app:iphone' }));
    r.send(notice({ approvalId: 'a2', serial: '192.168.1.5:5555' }));
    await r.tick(2);
    expect(r.calls).toEqual([]);
  });

  it('ignores a helper that does not know the message', async () => {
    const r = rig({ helperFails: true });
    r.send(notice());
    r.states.set('a1', 'approved');
    await r.tick(1);
    expect(r.calls.map((c) => c.params.state)).toEqual(['waiting', 'cleared']);
  });

  it('re-sends waiting once after the link comes back while the hold is still open', async () => {
    const r = rig();
    r.send(notice());
    await r.tick(1);
    expect(r.calls).toHaveLength(1);
    // The link drops: the helper clears itself on its own lease; Desktop's repeats fail meanwhile.
    r.link.down = true;
    await vi.advanceTimersByTimeAsync(2 * HOLD_NOTICE_HEARTBEAT_MS);
    const before = r.calls.length; // the failed attempts were still sent to the transport
    r.link.down = false;
    r.calls.length = 0;
    await vi.advanceTimersByTimeAsync(HOLD_NOTICE_HEARTBEAT_MS);
    expect(before).toBeGreaterThan(1);
    expect(r.calls).toEqual([
      { serial: 'link:pixel', method: 'hold_notice', params: { state: 'waiting', effect: 'send', app: 'com.whatsapp', expiresAt: '2026-11-10T10:10:00Z' } },
    ]);
  });

  it('keeps repeating waiting as a heartbeat while the hold is open, and stops once it closes', async () => {
    const r = rig();
    r.send(notice());
    await vi.advanceTimersByTimeAsync(3 * HOLD_NOTICE_HEARTBEAT_MS);
    expect(r.calls.map((c) => c.params.state)).toEqual(['waiting', 'waiting', 'waiting', 'waiting']);
    r.states.set('a1', 'approved');
    await r.tick(1);
    const n = r.calls.length;
    await vi.advanceTimersByTimeAsync(3 * HOLD_NOTICE_HEARTBEAT_MS);
    expect(r.calls).toHaveLength(n);
    expect(r.calls[n - 1]?.params.state).toBe('cleared');
  });
});

describe('hold notice through the real phone tool runner', () => {
  beforeEach(() => {
    vi.useFakeTimers();
    vi.setSystemTime(NOW);
  });
  afterEach(() => {
    vi.useRealTimers();
  });

  it('a sensitive tap on a linked phone shows waiting, then cleared when the owner approves', async () => {
    const hold: HoldView = { approvalId: 'h1', inboxId: 'approval:h1', state: 'requested', consumed: false, effect: 'delete', phone: {}, expiresAt: '' };
    const client: PhoneHoldClient = {
      check: async () => ({ decision: 'hold', approvalId: 'h1', inboxId: 'approval:h1', effect: 'delete', expiresAt: '2026-11-10T10:05:00Z' }),
      getHold: async () => hold,
      resolve: async () => true,
    };
    const helper: Array<{ serial: string; method: string; params: Record<string, unknown> }> = [];
    const dialogs: string[] = [];
    // The link lane (transport to the phone) is the edge: it answers the screen read the guard makes.
    const links = {
      has: (s: string) => s === 'link:pixel',
      serials: () => ['link:pixel'],
      linkedPhone: () => null,
      helperCall: async () => ({ foreground_app: 'com.example.files', nodes: [] }),
      call: async () => ({ ok: true }),
    } as unknown as NonNullable<ReturnType<NonNullable<ConstructorParameters<typeof PhoneToolRunner>[0]['links']>>>;
    const runner = new PhoneToolRunner({
      adb: Object.assign(async () => ({ stdout: '', stderr: '', code: 0 }), { binary: async () => Buffer.alloc(0) }) as unknown as AdbCall,
      gate: { request: async () => 'denied' },
      rates: { load: () => [], save: () => {} },
      artemis: () => null,
      onlineSerials: () => [],
      links: () => links,
      guard: () => client,
      onHold: phoneHoldHandler({
        dialog: (n) => dialogs.push(n.approvalId),
        client,
        isAndroidLink: (s) => s === 'link:pixel',
        helperCall: async (serial, method, params) => {
          helper.push({ serial, method, params });
        },
      }),
    });
    const res = await runner.call('phone.open_app', { serial: 'link:pixel', package: 'com.bank.app' });
    expect(res).toMatchObject({ ok: false, error: 'approval_required', approvalId: 'h1' });
    expect(dialogs).toEqual(['h1']);
    expect(helper.map((h) => [h.serial, h.method, h.params.state, h.params.effect])).toEqual([['link:pixel', 'hold_notice', 'waiting', 'delete']]);
    hold.state = 'approved';
    await vi.advanceTimersByTimeAsync(HOLD_NOTICE_POLL_MS);
    expect(helper.map((h) => h.params.state)).toEqual(['waiting', 'cleared']);
  });
});
