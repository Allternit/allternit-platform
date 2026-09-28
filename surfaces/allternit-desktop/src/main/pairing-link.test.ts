import { describe, expect, it } from 'vitest';
import { isPairingLink, pairingCodeFromUrl } from './pairing-link.js';

describe('gizzi login pairing link', () => {
  it('recognizes pair links in both schemes', () => {
    expect(isPairingLink('allternit://pair?code=ABCD-1234')).toBe(true);
    expect(isPairingLink('allternit-dev://pair?code=ABCD-1234')).toBe(true);
    expect(isPairingLink('allternit://design?prompt=x')).toBe(false);
  });
  it('takes only a plausible code', () => {
    expect(pairingCodeFromUrl('allternit://pair?code=ABCD-1234')).toBe('ABCD-1234');
    expect(pairingCodeFromUrl('allternit://pair?code=%3Cscript%3E')).toBeNull();
    expect(pairingCodeFromUrl('allternit://pair?code=../../x')).toBeNull();
    expect(pairingCodeFromUrl('allternit://pair')).toBeNull();
    expect(pairingCodeFromUrl('not a url')).toBeNull();
  });
});
