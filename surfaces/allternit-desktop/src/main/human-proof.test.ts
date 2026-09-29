import { describe, expect, it } from 'vitest';
import {
  applyDesktopHumanProof,
  applyDesktopHumanProofTo,
  stripDesktopHumanProof,
  stripDesktopProofParam,
  takeDesktopProofParam,
} from './human-proof.js';

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

  it('reads and removes the URL marker (it survives the /api redirect)', () => {
    const url = new URL('allternit-api://localhost/api/v1/cowork/approvals?x=1&allternit_person=desktop');
    expect(takeDesktopProofParam(url)).toBe(true);
    expect(url.search).toBe('?x=1');
    const other = new URL('allternit-api://localhost/api/v1/x?allternit_person=guess');
    expect(takeDesktopProofParam(other)).toBe(false);
    expect(other.search).toBe('');
  });

  it('strips the URL marker from a relayed path', () => {
    expect(stripDesktopProofParam('/api/v1/cowork/approvals?allternit_person=desktop')).toBe('/api/v1/cowork/approvals');
    expect(stripDesktopProofParam('/api/v1/x?a=1')).toBe('/api/v1/x?a=1');
  });

  it('keeps its own proof when a second hook sees the same request', () => {
    const headers = new Headers({ 'X-Allternit-Human-Proof': PROOF });
    applyDesktopHumanProofTo(headers, PROOF);
    expect(headers.get('X-Allternit-Human-Proof')).toBe(PROOF);
    const bag: Record<string, string> = { 'X-Allternit-Human-Proof': PROOF };
    applyDesktopHumanProof(bag, PROOF);
    expect(bag['X-Allternit-Human-Proof']).toBe(PROOF);
    // …but not toward the cloud, where no proof applies.
    applyDesktopHumanProofTo(headers, null);
    expect(headers.has('X-Allternit-Human-Proof')).toBe(false);
  });
});
