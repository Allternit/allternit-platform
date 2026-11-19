import { describe, expect, it } from 'vitest';
import {
  chooseLinuxUpdateMode,
  detectElevation,
  fallbackNotice,
  needsElevation,
  releasePageUrl,
  resolveLinuxFeed,
  shouldAutoDownload,
} from './linux-updater-logic.js';

describe('chooseLinuxUpdateMode', () => {
  it('stays off on macOS and Windows, whatever the package type says', () => {
    expect(chooseLinuxUpdateMode({ platform: 'darwin', isPackaged: true, packageType: 'deb' })).toBe('off');
    expect(chooseLinuxUpdateMode({ platform: 'win32', isPackaged: true, packageType: 'deb' })).toBe('off');
  });

  it('stays off in an unpackaged Linux dev build', () => {
    expect(chooseLinuxUpdateMode({ platform: 'linux', isPackaged: false, packageType: 'deb' })).toBe('off');
  });

  it('uses the deb updater for a .deb install (package-type file ends with a newline)', () => {
    expect(chooseLinuxUpdateMode({ platform: 'linux', isPackaged: true, packageType: 'deb\n' })).toBe('deb');
  });

  it('uses the rpm updater for a .rpm install', () => {
    expect(chooseLinuxUpdateMode({ platform: 'linux', isPackaged: true, packageType: 'rpm\n' })).toBe('rpm');
  });

  it('uses the AppImage updater whenever APPIMAGE is set, whatever package-type says', () => {
    // electron-builder packs the deb, rpm and AppImage from one resources dir,
    // so the AppImage can carry package-type=deb or rpm.
    const appImagePath = '/home/ana/Applications/Allternit-Desktop-1.2.0-x86_64.AppImage';
    expect(chooseLinuxUpdateMode({ platform: 'linux', isPackaged: true, packageType: 'deb', appImagePath })).toBe('appimage');
    expect(chooseLinuxUpdateMode({ platform: 'linux', isPackaged: true, packageType: null, appImagePath })).toBe('appimage');
    expect(chooseLinuxUpdateMode({ platform: 'linux', isPackaged: true, packageType: 'deb', appImagePath: '  ' })).toBe('deb');
  });

  it('does not self-update a build without package-type or APPIMAGE', () => {
    expect(chooseLinuxUpdateMode({ platform: 'linux', isPackaged: true, packageType: 'pacman' })).toBe('unsupported');
    expect(chooseLinuxUpdateMode({ platform: 'linux', isPackaged: true, packageType: null })).toBe('unsupported');
  });

  it('asks for a password only for the system packages', () => {
    expect(needsElevation('deb')).toBe(true);
    expect(needsElevation('rpm')).toBe(true);
    expect(needsElevation('appimage')).toBe(false);
  });
});

describe('detectElevation and shouldAutoDownload', () => {
  it('needs no prompt when running as root', () => {
    const e = detectElevation({ isRoot: true, hasCommand: () => false });
    expect(e).toEqual({ kind: 'root' });
    expect(shouldAutoDownload(e)).toBe(true);
  });

  it('picks pkexec on a GNOME/KDE desktop and downloads automatically', () => {
    const e = detectElevation({ isRoot: false, hasCommand: (c) => c === 'pkexec' || c === 'sudo' });
    expect(e).toEqual({ kind: 'graphical', command: 'pkexec' });
    expect(shouldAutoDownload(e)).toBe(true);
  });

  it('follows electron-updater order: gksudo before pkexec', () => {
    const e = detectElevation({ isRoot: false, hasCommand: (c) => c === 'pkexec' || c === 'gksudo' });
    expect(e).toEqual({ kind: 'graphical', command: 'gksudo' });
  });

  it('falls back (no download) when only terminal sudo exists', () => {
    const e = detectElevation({ isRoot: false, hasCommand: (c) => c === 'sudo' });
    expect(e).toEqual({ kind: 'none' });
    expect(shouldAutoDownload(e)).toBe(false);
  });
});

describe('resolveLinuxFeed', () => {
  it('reads the Allternit/desktop GitHub releases by default', () => {
    expect(resolveLinuxFeed(undefined)).toEqual({ feed: { provider: 'github', owner: 'Allternit', repo: 'desktop' } });
    expect(resolveLinuxFeed('  ')).toEqual({ feed: { provider: 'github', owner: 'Allternit', repo: 'desktop' } });
  });

  it('uses a generic feed for an http(s) override (lab)', () => {
    expect(resolveLinuxFeed('http://10.0.0.5:8080/feed/')).toEqual({
      feed: { provider: 'generic', url: 'http://10.0.0.5:8080/feed/' },
    });
  });

  it('ignores an override that is not an http(s) URL and keeps the real feed', () => {
    expect(resolveLinuxFeed('file:///tmp/feed')).toEqual({
      feed: { provider: 'github', owner: 'Allternit', repo: 'desktop' },
      ignoredOverride: 'file:///tmp/feed',
    });
    expect(resolveLinuxFeed('not a url').ignoredOverride).toBe('not a url');
  });
});

describe('releasePageUrl and fallbackNotice', () => {
  it('links the exact release tag, or latest when the version is unknown', () => {
    expect(releasePageUrl('1.1.16')).toBe('https://github.com/Allternit/desktop/releases/tag/v1.1.16');
    expect(releasePageUrl('v1.1.16')).toBe('https://github.com/Allternit/desktop/releases/tag/v1.1.16');
    expect(releasePageUrl(null)).toBe('https://github.com/Allternit/desktop/releases/latest');
    expect(releasePageUrl('../../evil')).toBe('https://github.com/Allternit/desktop/releases/latest');
  });

  it('tells the user how to install by hand when elevation is missing or failed', () => {
    const none = fallbackNotice('no-elevation', '1.1.16');
    expect(none.title).toBe('Allternit Desktop 1.1.16 is available');
    expect(none.body).toContain('sudo apt install');
    expect(none.url).toBe('https://github.com/Allternit/desktop/releases/tag/v1.1.16');

    const failed = fallbackNotice('install-failed', null);
    expect(failed.title).toBe('A new Allternit Desktop could not be installed');
    expect(failed.url).toBe('https://github.com/Allternit/desktop/releases/latest');
  });

  it('gives the install command for the package type the user has', () => {
    expect(fallbackNotice('install-failed', '1.2.0', 'rpm').body).toContain('sudo dnf install ./<file>.rpm');
    expect(fallbackNotice('install-failed', '1.2.0', 'rpm').body).toContain('sudo zypper install');
    const appimage = fallbackNotice('install-failed', '1.2.0', 'appimage');
    expect(appimage.body).toContain('chmod +x');
    expect(appimage.body).not.toContain('password');
  });
});
