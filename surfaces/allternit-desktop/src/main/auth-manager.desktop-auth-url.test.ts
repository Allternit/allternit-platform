import { describe, expect, it } from 'vitest';
import { isDesktopAuthNavigation } from './desktop-auth-url.js';

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
