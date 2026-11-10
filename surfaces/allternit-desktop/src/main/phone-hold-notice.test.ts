import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { holdNoticeSender, HOLD_NOTICE_POLL_MS } from './phone-hold-notice.js';
import type { HoldView, PhoneHoldClient } from './phone-hold-client.js';
import type { PhoneHoldNotice } from './phone-tools.js';

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
      if (opts.helperFails) throw new Error('Method not found');
    },
  });
  const tick = (n = 1) => vi.advanceTimersByTimeAsync(HOLD_NOTICE_POLL_MS * n);
  return { send, states, calls, tick };
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
});
