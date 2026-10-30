import { describe, expect, it } from 'vitest';
import {
  clampHudBounds,
  defaultHudBounds,
  hudBoundsForDisplay,
  keepHudReachable,
  parseHudBoundsByDisplay,
  resizeHudFromBottom,
} from './hud-geometry.js';

// Primary 1920×1080 with a 25 px macOS menu bar; a second monitor stacked
// ABOVE it (negative y) and a third to its LEFT (negative x).
const primary = { x: 0, y: 25, width: 1920, height: 1055 };
const above = { x: 0, y: -1440, width: 2560, height: 1415 };
const left = { x: -1280, y: 0, width: 1280, height: 1024 };

describe('hud geometry', () => {
  it('opens 72 px above the bottom of the work area, honoring its origin', () => {
    expect(defaultHudBounds(primary)).toEqual({ x: 650, y: 25 + 1055 - 220 - 72, width: 620, height: 220 });
    expect(defaultHudBounds(left)).toEqual({ x: -1280 + 330, y: 1024 - 220 - 72, width: 620, height: 220 });
  });

  it('grows upward on a monitor above the primary without jumping to y=0 (regression: resize-hud clamped to 0)', () => {
    const onAbove = defaultHudBounds(above);
    const grown = resizeHudFromBottom(onAbove, 600, above);
    expect(grown.y + grown.height).toBe(onAbove.y + onAbove.height);
    expect(grown.height).toBe(600);
    expect(grown.y).toBeLessThan(0);
  });

  it('stops growing at the top of its own display', () => {
    const nearTop = { x: 100, y: -1400, width: 620, height: 220 };
    const grown = resizeHudFromBottom(nearTop, 900, above);
    expect(grown.y).toBe(above.y);
    expect(grown.y + grown.height).toBe(-1400 + 220);
  });

  it('pulls an off-screen request back inside a negative-coordinate display', () => {
    expect(clampHudBounds({ x: -2000, y: 5000, width: 620, height: 220 }, left)).toEqual({
      x: -1280,
      y: 1024 - 220,
      width: 620,
      height: 220,
    });
  });

  it('lets a drag hang off an edge but keeps a strip reachable', () => {
    const dragged = keepHudReachable({ x: -1900, y: -50, width: 620, height: 220 }, left);
    expect(dragged.x).toBe(-1280 - 620 + 80);
    expect(dragged.y).toBe(0);
  });

  it('restores a remembered spot, re-clamped if that display shrank', () => {
    const saved = parseHudBoundsByDisplay({ '7': { x: 1700, y: 900, width: 620, height: 220 }, width: 720, height: 72 });
    expect(Object.keys(saved)).toEqual(['7']);
    expect(hudBoundsForDisplay(primary, saved['7'])).toEqual({ x: 1300, y: 860, width: 620, height: 220 });
    expect(hudBoundsForDisplay(primary, saved['9'])).toEqual(defaultHudBounds(primary));
  });
});
