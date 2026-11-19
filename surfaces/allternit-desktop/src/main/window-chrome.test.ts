import { describe, expect, it } from 'vitest';
import { hidesMenuBarBeforePairing, isPrePairingPage, mayOpenHud, uvInstallHint } from './window-chrome.js';

describe('window chrome', () => {
  it('hides the menu bar on Linux and Windows only', () => {
    expect(hidesMenuBarBeforePairing('linux')).toBe(true);
    expect(hidesMenuBarBeforePairing('win32')).toBe(true);
    expect(hidesMenuBarBeforePairing('darwin')).toBe(false);
  });
  it('recognises the Connect page', () => {
    expect(isPrePairingPage('file:///opt/Allternit/resources/app.asar/static/connect.html')).toBe(true);
    expect(isPrePairingPage('file:///x/static/connect.html?error=1')).toBe(true);
    expect(isPrePairingPage('https://ai.allternit.com/connect.html')).toBe(false);
  });
  it('gates the HUD on a paired identity', () => {
    expect(mayOpenHud(false)).toBe(false);
    expect(mayOpenHud(true)).toBe(true);
  });
  it('gives each OS its own uv command', () => {
    expect(uvInstallHint('darwin')).toContain("'brew install uv'");
    expect(uvInstallHint('linux')).toContain('curl -LsSf https://astral.sh/uv/install.sh | sh');
    expect(uvInstallHint('linux')).not.toContain('brew');
    expect(uvInstallHint('win32')).toContain('install.ps1');
    expect(uvInstallHint('win32')).not.toContain('brew');
  });
});
