import { describe, expect, it } from 'vitest';
import { PetActivityHub, isPetState, pickRoamTarget, roamStep } from './pet-activity-hub.js';

describe('PetActivityHub', () => {
  it('shows the most important source and sleeps only when all are idle or asleep', () => {
    const hub = new PetActivityHub();
    expect(hub.current()).toBe('idle');
    hub.set('app', 'sleep');
    expect(hub.current()).toBe('sleep');
    hub.set('hook:codex', 'review');
    hub.set('app', 'waiting');
    expect(hub.current()).toBe('waiting');
    hub.set('app', 'idle');
    expect(hub.current()).toBe('review');
  });
  it('expires timed sources', () => {
    const hub = new PetActivityHub();
    hub.set('hook:claude', 'running', 1000, 0);
    expect(hub.current(500)).toBe('running');
    expect(hub.nextExpiry()).toBe(1000);
    expect(hub.current(1000)).toBe('idle');
  });
  it('accepts only pet states', () => {
    expect(isPetState('waving')).toBe(true);
    expect(isPetState('working')).toBe(false);
    expect(isPetState(3)).toBe(false);
  });
});

describe('roaming', () => {
  it('picks a target on screen at least 120 px away', () => {
    const area = { x: 0, width: 1280 };
    for (const r of [0, 0.3, 0.99]) {
      const t = pickRoamTarget(area, 96, 600, () => r);
      expect(t).toBeGreaterThanOrEqual(8);
      expect(t).toBeLessThanOrEqual(1280 - 96 - 8);
      expect(Math.abs(t - 600)).toBeGreaterThanOrEqual(120);
    }
    expect(pickRoamTarget({ x: 0, width: 300 }, 96, 50)).toBe(50);
  });
  it('steps at a steady speed and lands on the target', () => {
    expect(roamStep(0, 100, 1000)).toBe(70);
    expect(roamStep(90, 100, 1000)).toBe(100);
    expect(roamStep(100, 0, 500)).toBe(65);
  });
});
