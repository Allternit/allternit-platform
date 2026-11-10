import { describe, expect, it, vi } from 'vitest';
import { bundlePathFromExe, messagesAccessHint, openPrivacySettings } from './messages-permissions.js';

describe('openPrivacySettings', () => {
  it('opens the matching pane on macOS', async () => {
    const openExternal = vi.fn().mockResolvedValue(undefined);
    await openPrivacySettings('fullDisk', { platform: 'darwin', openExternal });
    expect(openExternal).toHaveBeenCalledWith(expect.stringContaining('Privacy_AllFiles'));
    await openPrivacySettings('automation', { platform: 'darwin', openExternal });
    expect(openExternal).toHaveBeenLastCalledWith(expect.stringContaining('Privacy_Automation'));
  });
  it('rejects other values', async () => {
    const openExternal = vi.fn();
    await expect(openPrivacySettings('https://x', { platform: 'darwin', openExternal })).rejects.toThrow();
    expect(openExternal).not.toHaveBeenCalled();
  });
  it('is unsupported off macOS', async () => {
    const openExternal = vi.fn();
    expect(await openPrivacySettings('fullDisk', { platform: 'win32', openExternal })).toEqual({ supported: false });
    expect(openExternal).not.toHaveBeenCalled();
  });
});

describe('messagesAccessHint', () => {
  it('returns the bundle path', () => {
    expect(bundlePathFromExe('/Applications/Allternit Desktop.app/Contents/MacOS/Allternit Desktop')).toBe(
      '/Applications/Allternit Desktop.app',
    );
    expect(messagesAccessHint({ platform: 'linux', exePath: '', appName: 'x' })).toEqual({ supported: false });
  });
});
