import { describe, expect, it } from 'vitest';
import { isDesktopAuthNavigation, pairingLookupFailureMessage, shouldPairWithLateClerkToken } from './desktop-auth-url.js';

const BASE = 'https://accounts.allternit.com/__desktop_auth__/';

describe('isDesktopAuthNavigation', () => {
  it('allows the canonical auth renderer URL with and without a trailing slash', () => {
    expect(isDesktopAuthNavigation(BASE, BASE)).toBe(true);
    expect(isDesktopAuthNavigation('https://accounts.allternit.com/__desktop_auth__', BASE)).toBe(true);
  });

  it('allows hash and query variants Clerk uses during sign-in', () => {
    expect(isDesktopAuthNavigation('https://accounts.allternit.com/__desktop_auth__#/factor-one', BASE)).toBe(true);
    expect(isDesktopAuthNavigation('https://accounts.allternit.com/__desktop_auth__/#/factor-two', BASE)).toBe(true);
    expect(isDesktopAuthNavigation('https://accounts.allternit.com/__desktop_auth__?redirect_url=/', BASE)).toBe(true);
  });

  it('rejects Account Portal home and other origins', () => {
    expect(isDesktopAuthNavigation('https://accounts.allternit.com/', BASE)).toBe(false);
    expect(isDesktopAuthNavigation('https://ai.allternit.com/', BASE)).toBe(false);
    expect(isDesktopAuthNavigation('https://github.com/login', BASE)).toBe(false);
  });
});

describe('shouldPairWithLateClerkToken', () => {
  const late = { waiting: false, token: 'jwt', paired: false, pairingInFlight: false };

  it('pairs when a sign-in finishes after its pairing expired', () => {
    expect(shouldPairWithLateClerkToken(late)).toBe(true);
  });

  it('leaves the token to a waiting sign-in, a running pairing, or an already-paired desktop', () => {
    expect(shouldPairWithLateClerkToken({ ...late, waiting: true })).toBe(false);
    expect(shouldPairWithLateClerkToken({ ...late, pairingInFlight: true })).toBe(false);
    expect(shouldPairWithLateClerkToken({ ...late, paired: true })).toBe(false);
    expect(shouldPairWithLateClerkToken({ ...late, token: '' })).toBe(false);
  });
});

describe('pairingLookupFailureMessage', () => {
  it('says invalid or expired only for a missing or expired code', () => {
    expect(pairingLookupFailureMessage(404)).toMatch(/invalid or expired/);
    expect(pairingLookupFailureMessage(401, 'TOKEN_EXPIRED')).toMatch(/invalid or expired/);
  });

  it('asks for a new sign-in when the sign-in itself was rejected', () => {
    expect(pairingLookupFailureMessage(401, 'UNAUTHORIZED')).toMatch(/sign in again/);
  });

  it('reports a server error as a server error', () => {
    const message = pairingLookupFailureMessage(500, 'DATABASE_ERROR');
    expect(message).not.toMatch(/invalid or expired/);
    expect(message).toMatch(/error 500/);
  });
});
