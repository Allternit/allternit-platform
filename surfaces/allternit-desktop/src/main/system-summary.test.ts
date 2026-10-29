import { describe, expect, it } from 'vitest';
import { bubbleText, dotTab, dotTone, sanitizeSummary } from './system-summary.js';

describe('system summary from the app window', () => {
  it('reads anything malformed or switched off as off: no dot', () => {
    for (const raw of [null, 'x', {}, { enabled: 'yes', needsYou: 3 }, { enabled: false, needsYou: 3 }]) {
      expect(dotTone(sanitizeSummary(raw))).toBeNull();
    }
  });

  it('keeps only known tabs, trims long text and caps the list', () => {
    const s = sanitizeSummary({
      enabled: true,
      needsYou: 2,
      problems: -4,
      items: [
        { tab: 'needs-you', text: `sub asks: ${'x'.repeat(200)}` },
        { tab: 'evil', text: 'drop me' },
        ...Array.from({ length: 10 }, (_, i) => ({ tab: 'problems', text: `p${i}`, tone: 'error' })),
      ],
    });
    expect(s.problems).toBe(0);
    expect(s.items).toHaveLength(5);
    expect(s.items.some((i) => i.text === 'drop me')).toBe(false);
    expect(s.items[0].text.length).toBeLessThanOrEqual(80);
  });

  it('points the dot at what needs you first, then what broke', () => {
    const both = sanitizeSummary({ enabled: true, needsYou: 1, problems: 2, items: [{ tab: 'needs-you', text: 'sub asks to run pnpm add' }] });
    expect([dotTone(both), dotTab(both)]).toEqual(['warning', 'needs-you']);
    expect(bubbleText(both)).toBe('1 needs you · 2 problems: sub asks to run pnpm add. Click the dot to open.');
    const broken = sanitizeSummary({ enabled: true, problems: 1 });
    expect([dotTone(broken), dotTab(broken)]).toEqual(['error', 'problems']);
  });
});
