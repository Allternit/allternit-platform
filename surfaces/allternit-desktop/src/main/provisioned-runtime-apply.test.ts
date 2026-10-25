import { describe, expect, it } from 'vitest';
import { gizziSessionsIdle, startIdleApply } from './provisioned-runtime-apply.js';

describe('gizziSessionsIdle', () => {
  it('idle when no session is listed or every listed one is idle', () => {
    expect(gizziSessionsIdle({})).toBe(true);
    expect(gizziSessionsIdle({ ses_a: { type: 'idle' } })).toBe(true);
  });
  it('busy when a session runs or retries, or the answer is not a status map', () => {
    expect(gizziSessionsIdle({ ses_a: { type: 'busy' } })).toBe(false);
    expect(gizziSessionsIdle({ ses_a: { type: 'idle' }, ses_b: { type: 'retry' } })).toBe(false);
    expect(gizziSessionsIdle(null)).toBe(false);
    expect(gizziSessionsIdle([])).toBe(false);
  });
});

describe('startIdleApply', () => {
  function harness(answers: Array<unknown | Error>, pending = () => true) {
    const timers: Array<() => void> = [];
    let applied = 0;
    startIdleApply({
      hasPending: pending,
      fetchStatuses: async () => {
        const next = answers.shift();
        if (next instanceof Error) throw next;
        return next;
      },
      apply: () => {
        applied += 1;
      },
      setTimer: (fn: () => void) => {
        timers.push(fn);
        return {};
      },
    });
    const step = async () => {
      timers.shift()?.();
      await new Promise((r) => setTimeout(r, 0));
    };
    return { step, applied: () => applied, timers };
  }

  it('applies after two idle checks in a row, not after a busy one in between', async () => {
    const h = harness([{}, { s: { type: 'busy' } }, {}, {}]);
    await h.step();
    await h.step();
    await h.step();
    expect(h.applied()).toBe(0);
    await h.step();
    expect(h.applied()).toBe(1);
    expect(h.timers).toHaveLength(0);
  });

  it('treats an unreachable Gizzi as busy', async () => {
    const h = harness([{}, new Error('down'), {}, {}]);
    for (let i = 0; i < 3; i++) await h.step();
    expect(h.applied()).toBe(0);
    await h.step();
    expect(h.applied()).toBe(1);
  });

  it('does nothing while no runtime is staged', async () => {
    const h = harness([{}, {}, {}], () => false);
    for (let i = 0; i < 3; i++) await h.step();
    expect(h.applied()).toBe(0);
    expect(h.timers).toHaveLength(1);
  });
});
