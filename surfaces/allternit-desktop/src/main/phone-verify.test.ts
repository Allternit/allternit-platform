import { describe, expect, it } from 'vitest';
import { actThenVerify, failEnvelope, settle, toEnvelope, type VerifyFrame, type VerifyTarget } from './phone-verify.js';
import type { ToolResult } from './phone-tools.js';

/**
 * A fake phone screen: the frame only changes when a send "lands". `lands`
 * says which send (1-based) changes the screen; 0 = none do.
 */
function fakeScreen(opts: { lands?: number; sendResult?: ToolResult; ocr?: { text: string; fieldVisible: boolean } | null; noFrames?: boolean } = {}) {
  let screen = 'home';
  let sends = 0;
  let rehomes = 0;
  const log: string[] = [];
  const target: VerifyTarget = {
    frame: async (): Promise<VerifyFrame | null> => (opts.noFrames ? null : { gray: null, hash: screen }),
    send: async () => {
      sends += 1;
      log.push(`send${sends}`);
      if (opts.sendResult) return opts.sendResult;
      if (opts.lands !== undefined && sends === opts.lands) screen = `after-${sends}`;
      return { ok: true };
    },
    rehome: async () => {
      rehomes += 1;
      log.push('rehome');
    },
    ...(opts.ocr !== undefined ? { readText: async () => opts.ocr! } : {}),
  };
  return { target, sends: () => sends, rehomes: () => rehomes, log };
}

const fast = { sleep: async () => {}, quietMs: 1, settleMaxMs: 5 };

describe('act-then-verify', () => {
  it('verifies on the first send when the screen changes', async () => {
    const s = fakeScreen({ lands: 1 });
    const res = await actThenVerify(s.target, { route: 'mirror', tool: 'phone.tap', ...fast });
    expect(res).toMatchObject({ ok: true, route: 'mirror', verified: true, detail: { attempts: 1, changed: true } });
    expect(s.sends()).toBe(1);
  });

  it('retries an unchanged tap twice, re-homing the pointer before each retry', async () => {
    const s = fakeScreen({ lands: 3 });
    const res = await actThenVerify(s.target, { route: 'bt-mac', tool: 'phone.tap', ...fast });
    expect(res).toMatchObject({ ok: true, verified: true, detail: { attempts: 3, changed: true } });
    expect(s.log).toEqual(['send1', 'rehome', 'send2', 'rehome', 'send3']);
  });

  it('does not re-home for non-tap tools', async () => {
    const s = fakeScreen({ lands: 2 });
    await actThenVerify(s.target, { route: 'bt-mac', tool: 'phone.swipe', ...fast });
    expect(s.rehomes()).toBe(0);
    expect(s.sends()).toBe(2);
  });

  it('gives up after three sends with no_change and both frame hashes', async () => {
    const s = fakeScreen({ lands: 0 });
    const res = await actThenVerify(s.target, { route: 'mirror', tool: 'phone.tap', ...fast });
    expect(res).toMatchObject({
      ok: false,
      error: 'no_change',
      route: 'mirror',
      verified: false,
      detail: { error: 'no_change', attempts: 3, changed: false, frames: { before: 'home', after: 'home' } },
    });
    expect(res.needs_you).toBeUndefined();
    expect(s.sends()).toBe(3);
  });

  it('returns a failed send at once and never repeats it', async () => {
    const s = fakeScreen({ sendResult: { ok: false, error: 'phone_failed', message: 'link dropped' } });
    const res = await actThenVerify(s.target, { route: 'bt-android', tool: 'phone.key', ...fast });
    expect(res).toMatchObject({ ok: false, error: 'phone_failed', verified: false, detail: { attempts: 1, error: 'phone_failed', message: 'link dropped' } });
    expect(s.sends()).toBe(1);
  });

  it('with no frame to compare, sends once and reports unverified', async () => {
    const s = fakeScreen({ noFrames: true });
    const res = await actThenVerify(s.target, { route: 'mirror', tool: 'phone.tap', ...fast });
    expect(res).toMatchObject({ ok: true, verified: false, detail: { attempts: 1, changed: null, verify: 'no_frame' } });
    expect(s.sends()).toBe(1);
  });

  it('phone.type is verified only when the typed text shows in a visible field', async () => {
    const seen = fakeScreen({ lands: 1, ocr: { text: 'Notes\nBuy milk', fieldVisible: true } });
    expect(await actThenVerify(seen.target, { route: 'mirror', tool: 'phone.type', typed: 'buy milk', ...fast })).toMatchObject({ ok: true, verified: true });

    // The screen changed but the text is not in the field: sent, not verified, and never typed a second time.
    const missing = fakeScreen({ lands: 1, ocr: { text: 'Notes', fieldVisible: true } });
    const res = await actThenVerify(missing.target, { route: 'mirror', tool: 'phone.type', typed: 'buy milk', ...fast });
    expect(res).toMatchObject({ ok: true, verified: false, detail: { attempts: 1, changed: true, typed_seen: false } });
    expect(missing.sends()).toBe(1);

    // No field on screen: only the changed-screen check applies.
    const noField = fakeScreen({ lands: 1, ocr: { text: '', fieldVisible: false } });
    expect(await actThenVerify(noField.target, { route: 'mirror', tool: 'phone.type', typed: 'x', ...fast })).toMatchObject({ verified: true });
  });
});

describe('settle', () => {
  it('waits until two frames in a row match', async () => {
    const frames = ['a', 'b', 'c', 'c'];
    let i = 0;
    const last = await settle({ frame: async () => ({ gray: null, hash: frames[Math.min(i++, frames.length - 1)] }) }, { sleep: async () => {}, quietMs: 1, settleMaxMs: 1000 });
    expect(last?.hash).toBe('c');
    expect(i).toBe(4);
  });

  it('stops at the time limit on a screen that never settles', async () => {
    let t = 0;
    let i = 0;
    const last = await settle(
      { frame: async () => ({ gray: null, hash: `f${i++}` }) },
      { sleep: async (ms) => void (t += ms), now: () => t, quietMs: 100, settleMaxMs: 300 },
    );
    expect(last?.hash).toMatch(/^f\d+$/);
    expect(t).toBeLessThanOrEqual(400);
  });
});

describe('result envelope', () => {
  it('keeps existing fields at the top level and adds route, verified and detail', () => {
    const shot = toEnvelope('usb-agent', 'phone.screenshot', { ok: true, mimeType: 'image/jpeg', dataBase64: '/9j/', width: 10, height: 20 });
    expect(shot).toEqual({ ok: true, mimeType: 'image/jpeg', dataBase64: '/9j/', width: 10, height: 20, route: 'usb-agent', verified: true, detail: {} });
    // An acting tool is verified only when the lane saw the screen change.
    expect(toEnvelope('usb-agent', 'phone.tap', { ok: true, screenChanged: false })).toMatchObject({ verified: false });
    expect(toEnvelope('usb-agent', 'phone.tap', { ok: true, screenChanged: true })).toMatchObject({ verified: true });
  });

  it('a needs-you failure carries the code and the fixed sentence', () => {
    const res = failEnvelope('mirror', 'needs_you', 'Unlock your Mac so I can use your iPhone through it.', {
      needsYou: { code: 'MAC_LOCKED', message: 'Unlock your Mac so I can use your iPhone through it.' },
    });
    expect(res).toMatchObject({ ok: false, route: 'mirror', verified: false, needs_you: { code: 'MAC_LOCKED' }, detail: { error: 'needs_you' } });
  });
});
