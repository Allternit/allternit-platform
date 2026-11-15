import { describe, expect, it } from 'vitest';

import { OfferGate, matchOffer, offerCopy, offerMuteLabel, type OfferCandidate } from './pet-notice.js';
import { PET_OFFER_FIXTURES, PET_OFFER_LIMITS, petOfferSettingsDefaults } from './pet-offers.gen.js';

const MAX = petOfferSettingsDefaults().maxTextChars;

describe('matchOffer: every contract fixture', () => {
  it('has fixtures to run', () => expect(PET_OFFER_FIXTURES.length).toBeGreaterThanOrEqual(20));
  for (const fixture of PET_OFFER_FIXTURES) {
    it(fixture.name, () => {
      const hit = matchOffer(fixture.text, { dialog: fixture.dialog, maxTextChars: MAX });
      expect(hit?.kind ?? null).toBe(fixture.expected);
      if (fixture.rule) expect(hit?.ruleId).toBe(fixture.rule);
      if (hit) {
        // The line is a real, trimmed line of the text.
        expect(hit.line.length).toBeGreaterThan(0);
        expect(hit.line).toBe(hit.line.trim());
        expect(fixture.text.replace(/\r\n?/g, '\n')).toContain(hit.line);
      }
    });
  }
});

describe('matchOffer: contract steps', () => {
  it('reads only the tail: an error pushed out of the last maxTextChars is gone', () => {
    const text = `npm ERR! code ELIFECYCLE\n${'ok\n'.repeat(4000)}`;
    expect(matchOffer(text, { maxTextChars: MAX })).toBeNull();
    expect(matchOffer(`${'ok\n'.repeat(4000)}npm ERR! code ELIFECYCLE`, { maxTextChars: MAX })?.kind).toBe('build_failed');
  });

  it('treats \\r\\n like \\n, so ^-anchored rules still see line starts', () => {
    expect(matchOffer('> build\r\nnpm ERR! code 1\r\n', { maxTextChars: MAX })).toEqual({ kind: 'build_failed', ruleId: 'npm.err', line: 'npm ERR! code 1' });
  });

  it('cuts the matched line to lineMaxChars', () => {
    const hit = matchOffer(`src/a.ts(1,1): error TS2322: ${'x'.repeat(600)}`, { maxTextChars: MAX });
    expect(hit?.line.length).toBe(PET_OFFER_LIMITS.lineMaxChars);
  });
});

describe('copy', () => {
  it('fills in the app, and falls back when there is none', () => {
    expect(offerCopy('error_dialog', 'Xcode')).toBe('Xcode is showing an error. Fix it?');
    expect(offerCopy('error_dialog', '  ')).toBe('This app is showing an error. Fix it?');
    expect(offerMuteLabel('Terminal')).toBe('Mute Terminal');
  });
});

describe('OfferGate', () => {
  const settings = (patch: Partial<ReturnType<typeof petOfferSettingsDefaults>> = {}) => ({ ...petOfferSettingsDefaults(), readOtherApps: true, ...patch });
  const cand = (patch: Partial<OfferCandidate> = {}): OfferCandidate => ({
    source: 'look', app: 'Terminal', appId: 'com.apple.Terminal', windowKey: 'com.apple.Terminal|zsh', line: 'npm ERR! code 1', ...patch,
  });

  it('one offer per window per cooldown; another window is not held back', () => {
    const gate = new OfferGate();
    const s = settings();
    gate.record(cand(), 0);
    expect(gate.check(cand({ line: 'error TS2322: other' }), s, 60_000)).toBe('cooldown');
    expect(gate.check(cand({ windowKey: 'com.apple.Terminal|other', line: 'error TS2322: other' }), s, 60_000)).toBe('ok');
    expect(gate.check(cand({ line: 'error TS2322: other' }), s, s.cooldownMs)).toBe('ok');
  });

  it('never offers the same line twice for one window, even after the cooldown', () => {
    const gate = new OfferGate();
    gate.record(cand(), 0);
    expect(gate.check(cand(), settings(), 10 * 60 * 60_000)).toBe('same_line');
  });

  it('an event offer with no line has only the cooldown', () => {
    const gate = new OfferGate();
    const hook = cand({ source: 'hook', line: '', windowKey: 'hook:gizzi' });
    gate.record(hook, 0);
    expect(gate.check(hook, settings(), 1000)).toBe('cooldown');
    expect(gate.check(hook, settings(), settings().cooldownMs + 1)).toBe('ok');
  });

  it('a muted app gets nothing, matched by name or by id', () => {
    const gate = new OfferGate();
    expect(gate.check(cand(), settings({ muted: ['terminal'] }), 0)).toBe('muted');
    expect(gate.check(cand({ app: 'Renamed' }), settings({ muted: ['com.apple.Terminal'] }), 0)).toBe('muted');
    expect(gate.check(cand(), settings({ muted: ['Term'] }), 0)).toBe('ok');
  });

  it('switches: offers off stops everything; reading off stops only passive reads', () => {
    const gate = new OfferGate();
    expect(gate.check(cand({ source: 'app' }), settings({ enabled: false }), 0)).toBe('off');
    expect(gate.check(cand(), settings({ readOtherApps: false }), 0)).toBe('reading_off');
    expect(gate.check(cand({ source: 'hook' }), settings({ readOtherApps: false }), 0)).toBe('ok');
    expect(gate.check(cand({ source: 'app' }), settings({ readOtherApps: false }), 0)).toBe('ok');
  });

  it('stays bounded: the oldest window is forgotten first', () => {
    const gate = new OfferGate(2);
    gate.record(cand({ windowKey: 'a' }), 0);
    gate.record(cand({ windowKey: 'b' }), 0);
    gate.record(cand({ windowKey: 'c' }), 0);
    expect(gate.check(cand({ windowKey: 'a' }), settings(), 1)).toBe('ok');
    expect(gate.check(cand({ windowKey: 'c' }), settings(), 1)).toBe('same_line');
  });
});
