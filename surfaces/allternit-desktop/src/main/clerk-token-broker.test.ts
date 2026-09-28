import { describe, expect, it, vi, afterEach } from 'vitest';
import { ClerkTokenBroker } from './clerk-token-broker.js';

const expiryOf = (token: string) => Number(token.split(':')[1]);

function never(): Promise<string> {
  return new Promise(() => undefined);
}

afterEach(() => {
  vi.useRealTimers();
});

describe('ClerkTokenBroker', () => {
  it('returns a fresh cached token without refreshing', async () => {
    const refresh = vi.fn(never);
    const broker = new ClerkTokenBroker({ refresh, expiresAt: expiryOf, now: () => 0 });
    broker.remember('a:100000');
    await expect(broker.get(4000)).resolves.toBe('a:100000');
    expect(refresh).not.toHaveBeenCalled();
  });

  it('returns a near-expiry token at once and refreshes in the background', async () => {
    let t = 0;
    let finish!: (token: string) => void;
    const refresh = vi.fn(() => new Promise<string>((resolve) => { finish = resolve; }));
    const broker = new ClerkTokenBroker({ refresh, expiresAt: expiryOf, now: () => t });
    broker.remember('old:15000');
    t = 10_000; // inside the 10s skew, not yet expired
    await expect(broker.get(4000)).resolves.toBe('old:15000');
    expect(refresh).toHaveBeenCalledTimes(1);
    finish('new:900000');
    await new Promise((resolve) => setTimeout(resolve, 0));
    await expect(broker.get(4000)).resolves.toBe('new:900000');
  });

  it('never makes a caller wait out a slow refresh (the 20s app-wide stall)', async () => {
    vi.useFakeTimers();
    const broker = new ClerkTokenBroker({ refresh: never, expiresAt: expiryOf, now: () => 0 });
    const got = broker.get(1500);
    await vi.advanceTimersByTimeAsync(1500);
    await expect(got).resolves.toBeNull();
  });

  it('shares one refresh between callers', async () => {
    let finish!: (token: string) => void;
    const refresh = vi.fn(() => new Promise<string>((resolve) => { finish = resolve; }));
    const broker = new ClerkTokenBroker({ refresh, expiresAt: expiryOf, now: () => 0 });
    const a = broker.get(10_000);
    const b = broker.get(10_000);
    finish('t:900000');
    await expect(Promise.all([a, b])).resolves.toEqual(['t:900000', 't:900000']);
    expect(refresh).toHaveBeenCalledTimes(1);
  });

  it('backs off exponentially after failures and resets on success', async () => {
    let t = 0;
    const onFailure = vi.fn();
    const refresh = vi.fn((): Promise<string> => Promise.reject(new Error('Clerk session timed out')));
    const broker = new ClerkTokenBroker({ refresh, expiresAt: expiryOf, now: () => t, onFailure });

    await expect(broker.get(100)).resolves.toBeNull();
    expect(onFailure).toHaveBeenLastCalledWith(expect.any(Error), 60_000);
    // Inside the backoff: no new attempt, answers immediately.
    t = 30_000;
    await expect(broker.get(100)).resolves.toBeNull();
    expect(refresh).toHaveBeenCalledTimes(1);

    t = 61_000;
    await broker.get(100);
    expect(onFailure).toHaveBeenLastCalledWith(expect.any(Error), 120_000);

    t = 61_000 + 120_001;
    refresh.mockImplementationOnce(() => Promise.resolve('ok:99999999'));
    await expect(broker.get(100)).resolves.toBe('ok:99999999');
  });

  it('caps the backoff', async () => {
    let t = 0;
    const onFailure = vi.fn();
    const broker = new ClerkTokenBroker({
      refresh: () => Promise.reject(new Error('down')),
      expiresAt: expiryOf,
      now: () => t,
      onFailure,
      maxBackoffMs: 15 * 60_000,
    });
    for (let i = 0; i < 8; i++) {
      await broker.get(0).catch(() => null);
      await Promise.resolve();
      t += 60 * 60_000;
    }
    const waits = onFailure.mock.calls.map((c) => c[1]);
    expect(Math.max(...waits)).toBe(15 * 60_000);
  });
});
