import { describe, expect, it, vi } from 'vitest';
import { classifyHealth, PhoneLinkHealthMonitor, type HealthEvidence, type LinkedPhone } from './phone-link-health.js';

const NOW = Date.parse('2026-10-17T12:00:00.000Z');
const ISO = new Date(NOW).toISOString();

function ev(over: Partial<HealthEvidence>): HealthEvidence {
  return { deviceId: 'dev-1', platform: 'ios', whoami: { deviceId: 'dev-1' }, call: 'ok', ...over };
}

describe('classifyHealth', () => {
  it('HEALTHY when whoami matches and a sealed call works', () => {
    expect(classifyHealth(ev({}), NOW)).toEqual({ state: 'HEALTHY', checkedAt: ISO, recovery: 'automatic' });
  });

  it('DEVICE_LOCKED when is_locked says locked', () => {
    expect(classifyHealth(ev({ call: 'locked' }), NOW)).toEqual({
      state: 'DEVICE_LOCKED',
      checkedAt: ISO,
      recovery: 'manual',
      action: 'Unlock your phone.',
    });
  });

  it('PAIRING_INVALID when the phone says re-pair', () => {
    expect(classifyHealth(ev({ call: 'needs_repair' }), NOW)).toMatchObject({
      state: 'PAIRING_INVALID',
      recovery: 'manual',
      action: 'Pair this phone again in Settings › Computers › Phones.',
    });
  });

  it('TRANSPORT_OFFLINE when nothing answers, with the iPhone restart hint', () => {
    expect(classifyHealth(ev({ whoami: null }), NOW)).toEqual({
      state: 'TRANSPORT_OFFLINE',
      checkedAt: ISO,
      recovery: 'automatic',
      detail: 'The iPhone may have restarted; its developer image then needs remounting.',
    });
    expect(classifyHealth(ev({ whoami: null, platform: 'android' }), NOW)).toMatchObject({
      state: 'TRANSPORT_OFFLINE',
      detail: undefined,
    });
  });

  it('TRANSPORT_OFFLINE when a different device answers at the saved address', () => {
    expect(classifyHealth(ev({ whoami: { deviceId: 'someone-else' } }), NOW)).toMatchObject({
      state: 'TRANSPORT_OFFLINE',
      detail: 'A different device answered at the saved address.',
    });
  });

  it('HELPER_NOT_RESPONDING when whoami matches but the sealed call fails', () => {
    expect(classifyHealth(ev({ call: 'failed' }), NOW)).toMatchObject({
      state: 'HELPER_NOT_RESPONDING',
      recovery: 'automatic',
    });
  });

  it('SIGNATURE_RENEWAL_REQUIRED when the signature lapsed and the phone is silent', () => {
    const health = classifyHealth(ev({ whoami: null, expiresAt: '2026-10-15T00:00:00.000Z' }), NOW);
    expect(health).toMatchObject({ state: 'SIGNATURE_RENEWAL_REQUIRED', recovery: 'manual' });
    expect(health.action).toBe("Your iPhone helper's signature runs out on October 15, 2026. Plug in your iPhone to renew it.");
  });

  it('SIGNATURE_RENEWAL_REQUIRED proactively within 24 h even when reachable', () => {
    const health = classifyHealth(ev({ expiresAt: new Date(NOW + 10 * 3600_000).toISOString() }), NOW);
    expect(health).toMatchObject({ state: 'SIGNATURE_RENEWAL_REQUIRED', recovery: 'manual' });
    expect(health.action).toContain('Plug in your iPhone to renew it.');
    // Android has no signature concept; an expiry there never triggers it.
    expect(classifyHealth(ev({ platform: 'android', expiresAt: new Date(NOW + 10 * 3600_000).toISOString() }), NOW)).toMatchObject({ state: 'HEALTHY' });
    // More than 24 h out stays HEALTHY.
    expect(classifyHealth(ev({ expiresAt: new Date(NOW + 25 * 3600_000).toISOString() }), NOW)).toMatchObject({ state: 'HEALTHY' });
  });

  it('DEVELOPER_IMAGE_REMOUNT_REQUIRED only on evidence, always manual, with the phone-side alternative', () => {
    expect(classifyHealth(ev({ remountRequired: true }), NOW)).toEqual({
      state: 'DEVELOPER_IMAGE_REMOUNT_REQUIRED',
      checkedAt: ISO,
      recovery: 'manual',
      action: 'Plug your iPhone into this computer and open Allternit',
      detail: 'The phone is connected but has no developer image mounted. Or on the iPhone: open iCTRL › Mount without a Mac, then Start Server.',
    });
    // Without the probe it stays TRANSPORT_OFFLINE.
    expect(classifyHealth(ev({ whoami: null }), NOW)).toMatchObject({ state: 'TRANSPORT_OFFLINE' });
  });

  it('the two remount/signature states are never automatic', () => {
    for (const health of [
      classifyHealth(ev({ remountRequired: true }), NOW),
      classifyHealth(ev({ whoami: null, expiresAt: '2026-10-16T00:00:00.000Z' }), NOW),
      classifyHealth(ev({ expiresAt: new Date(NOW + 60_000).toISOString() }), NOW),
    ]) {
      expect(health.recovery).toBe('manual');
      expect(health.action).toBeTruthy();
    }
  });

  it('UNKNOWN when the evidence fits nothing', () => {
    expect(classifyHealth({ ...ev({}), call: 'weird' } as unknown as HealthEvidence, NOW)).toMatchObject({ state: 'UNKNOWN', recovery: 'automatic' });
  });
});

function phone(over: Partial<LinkedPhone> = {}): LinkedPhone {
  return { deviceId: 'dev-1', platform: 'ios', storeKey: 'k', ...over };
}

describe('PhoneLinkHealthMonitor', () => {
  it('caches per phone for 30 s, then goes stale', async () => {
    let now = NOW;
    const probe = vi.fn(async () => ev({}));
    const monitor = new PhoneLinkHealthMonitor({ phones: () => [phone()], probe, now: () => now });

    expect((await monitor.check('dev-1')).state).toBe('HEALTHY');
    expect(probe).toHaveBeenCalledTimes(1);
    expect(monitor.health('dev-1')?.state).toBe('HEALTHY');
    expect(monitor.fresh('dev-1')?.state).toBe('HEALTHY');

    now += 31_000;
    expect(monitor.health('dev-1')?.state).toBe('HEALTHY'); // cached value kept
    expect(monitor.fresh('dev-1')).toBeNull(); // but stale
    await monitor.refreshStale();
    expect(probe).toHaveBeenCalledTimes(2);
  });

  it('refreshStale refreshes only stale entries, in parallel, each within the budget', async () => {
    let now = NOW;
    const slow = vi.fn(() => new Promise<HealthEvidence>((resolve) => setTimeout(() => resolve(ev({ whoami: null })), 5_000)));
    const fast = vi.fn(async () => ev({}));
    const phones = [phone({ deviceId: 'slow' }), phone({ deviceId: 'fast' })];
    const monitor = new PhoneLinkHealthMonitor({
      phones: () => phones,
      probe: (p) => (p.deviceId === 'slow' ? slow() : fast()),
      now: () => now,
      ttlMs: 30_000,
      budgetMs: 100,
    });

    const start = Date.now();
    await monitor.refreshStale();
    const elapsed = Date.now() - start;

    expect(elapsed).toBeLessThan(2_000); // parallel, not 5 s serial
    expect(fast).toHaveBeenCalledTimes(1);
    expect(monitor.health('slow')?.state).toBe('TRANSPORT_OFFLINE'); // budget exceeded = silent
    expect(monitor.health('fast')?.state).toBe('HEALTHY');

    // Both fresh now: a second refresh probes nothing.
    await monitor.refreshStale();
    expect(fast).toHaveBeenCalledTimes(1);
    now += 31_000;
    await monitor.refreshStale(['slow']); // targeted refresh
    expect(slow).toHaveBeenCalledTimes(2);
    expect(fast).toHaveBeenCalledTimes(1);
  });

  it('a probe that throws counts as "nothing answered"', async () => {
    const monitor = new PhoneLinkHealthMonitor({ phones: () => [phone({ platform: 'android' })], probe: async () => { throw new Error('boom'); } });
    expect((await monitor.check('dev-1')).state).toBe('TRANSPORT_OFFLINE');
  });
});
