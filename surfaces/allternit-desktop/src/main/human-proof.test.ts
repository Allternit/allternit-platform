import { describe, expect, it } from 'vitest';
import { applyDesktopHumanProof, applyDesktopHumanProofTo, stripDesktopHumanProof } from './human-proof.js';

const PROOF = 'desktop:secret';

describe('Desktop human proof (D16)', () => {
  it('swaps the window marker for the proof on local API requests', () => {
    const headers: Record<string, string> = { 'X-Allternit-Human-Proof': 'desktop', Accept: '*/*' };
    applyDesktopHumanProof(headers, PROOF);
    expect(headers).toEqual({ 'X-Allternit-Human-Proof': PROOF, Accept: '*/*' });
  });

  it('drops the marker where no proof applies (cloud, other hosts)', () => {
    const headers: Record<string, string> = { 'x-allternit-human-proof': 'desktop' };
    applyDesktopHumanProof(headers, null);
    expect(headers).toEqual({});
  });

  it('never forwards a desktop-looking value it did not write', () => {
    const headers: Record<string, string> = { 'X-Allternit-Human-Proof': 'desktop:guess' };
    applyDesktopHumanProof(headers, PROOF);
    expect(headers).toEqual({});
  });

  it('leaves a Clerk session proof untouched', () => {
    const headers = new Headers({ 'X-Allternit-Human-Proof': 'eyJ.clerk.jwt' });
    applyDesktopHumanProofTo(headers, PROOF);
    expect(headers.get('X-Allternit-Human-Proof')).toBe('eyJ.clerk.jwt');
  });

  it('a relayed request from another device never gets this Desktop proof', () => {
    const headers = new Headers({ 'X-Allternit-Human-Proof': 'desktop' });
    stripDesktopHumanProof(headers);
    expect(headers.has('X-Allternit-Human-Proof')).toBe(false);
  });
});
