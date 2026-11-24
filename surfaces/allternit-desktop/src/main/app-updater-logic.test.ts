import { describe, expect, it } from 'vitest';
import {
  chooseUpdateMode,
  detectElevation,
  fallbackNotice,
  installsOnQuit,
  isOldMacBundleName,
  macBundleFromExecPath,
  macOldBundlePath,
  macStagedBundleProblem,
  macSwapBlocker,
  needsElevation,
  parseCodesignTeamId,
  releasePageUrl,
  resolveUpdateFeed,
  shouldAutoDownload,
} from './app-updater-logic.js';

describe('chooseUpdateMode', () => {
  it('uses MacUpdater on macOS and NsisUpdater on Windows, whatever the package type says', () => {
    expect(chooseUpdateMode({ platform: 'darwin', isPackaged: true, packageType: 'deb' })).toBe('mac');
    expect(chooseUpdateMode({ platform: 'win32', isPackaged: true, packageType: null })).toBe('windows');
  });

  it('stays off in an unpackaged dev build on every OS', () => {
    expect(chooseUpdateMode({ platform: 'linux', isPackaged: false, packageType: 'deb' })).toBe('off');
    expect(chooseUpdateMode({ platform: 'darwin', isPackaged: false, packageType: null })).toBe('off');
    expect(chooseUpdateMode({ platform: 'win32', isPackaged: false, packageType: null })).toBe('off');
  });

  it('asks for a password only for the .deb, and installs on quit only on Windows', () => {
    expect(needsElevation('deb')).toBe(true);
    expect(needsElevation('rpm')).toBe(true);
    expect(needsElevation('appimage')).toBe(false);
    expect(needsElevation('mac')).toBe(false);
    expect(needsElevation('windows')).toBe(false);
    expect(installsOnQuit('windows')).toBe(true);
    expect(installsOnQuit('mac')).toBe(false);
    expect(installsOnQuit('deb')).toBe(false);
  });

  it('uses the deb updater for a .deb install (package-type file ends with a newline)', () => {
    expect(chooseUpdateMode({ platform: 'linux', isPackaged: true, packageType: 'deb\n' })).toBe('deb');
  });

  it('uses the rpm updater for a .rpm install', () => {
    expect(chooseUpdateMode({ platform: 'linux', isPackaged: true, packageType: 'rpm\n' })).toBe('rpm');
  });

  it('uses the AppImage updater whenever APPIMAGE is set, whatever package-type says', () => {
    // electron-builder packs the deb, rpm and AppImage from one resources dir,
    // so the AppImage can carry package-type=deb or rpm.
    const appImagePath = '/home/ana/Applications/Allternit-Desktop-1.2.0-x86_64.AppImage';
    expect(chooseUpdateMode({ platform: 'linux', isPackaged: true, packageType: 'deb', appImagePath })).toBe('appimage');
    expect(chooseUpdateMode({ platform: 'linux', isPackaged: true, packageType: null, appImagePath })).toBe('appimage');
    expect(chooseUpdateMode({ platform: 'linux', isPackaged: true, packageType: 'deb', appImagePath: '  ' })).toBe('deb');
  });

  it('does not self-update a build without package-type or APPIMAGE', () => {
    expect(chooseUpdateMode({ platform: 'linux', isPackaged: true, packageType: 'pacman' })).toBe('unsupported');
    expect(chooseUpdateMode({ platform: 'linux', isPackaged: true, packageType: null })).toBe('unsupported');
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

describe('resolveUpdateFeed', () => {
  it('reads the Allternit/desktop GitHub releases by default', () => {
    expect(resolveUpdateFeed(undefined)).toEqual({ feed: { provider: 'github', owner: 'Allternit', repo: 'desktop' } });
    expect(resolveUpdateFeed('  ')).toEqual({ feed: { provider: 'github', owner: 'Allternit', repo: 'desktop' } });
  });

  it('uses a generic feed for an http(s) override (lab)', () => {
    expect(resolveUpdateFeed('http://10.0.0.5:8080/feed/')).toEqual({
      feed: { provider: 'generic', url: 'http://10.0.0.5:8080/feed/' },
    });
  });

  it('ignores an override that is not an http(s) URL and keeps the real feed', () => {
    expect(resolveUpdateFeed('file:///tmp/feed')).toEqual({
      feed: { provider: 'github', owner: 'Allternit', repo: 'desktop' },
      ignoredOverride: 'file:///tmp/feed',
    });
    expect(resolveUpdateFeed('not a url').ignoredOverride).toBe('not a url');
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

  it('gives Mac and Windows their own manual install steps, never apt', () => {
    const mac = fallbackNotice('install-failed', '1.1.16', 'mac');
    expect(mac.body).toContain('App Management');
    expect(mac.body).toContain('.dmg');
    expect(mac.body).not.toContain('apt');
    const win = fallbackNotice('install-failed', '1.1.16', 'windows');
    expect(win.body).toContain('Allternit-Setup');
    expect(win.body).not.toContain('apt');
  });
});

describe('macOS bundle swap decisions', () => {
  it('finds the .app bundle that holds the running executable', () => {
    expect(macBundleFromExecPath('/Applications/Allternit Desktop.app/Contents/MacOS/Allternit Desktop'))
      .toBe('/Applications/Allternit Desktop.app');
    // A helper's own .app inside Frameworks resolves to that helper, not the outer app.
    expect(macBundleFromExecPath('/Applications/Allternit Desktop.app/Contents/Frameworks/X Helper.app/Contents/MacOS/X Helper'))
      .toBe('/Applications/Allternit Desktop.app/Contents/Frameworks/X Helper.app');
    expect(macBundleFromExecPath('/usr/local/bin/electron')).toBeNull();
  });

  it('refuses to swap a translocated copy (the real app would not change)', () => {
    expect(macSwapBlocker('/private/var/folders/x/T/AppTranslocation/ABC/d/Allternit Desktop.app')).toContain('translocated');
    expect(macSwapBlocker('/Applications/Allternit Desktop.app')).toBeNull();
  });

  it('parks the old bundle as a hidden sibling Launch Services ignores, and recognises it later', () => {
    const old = macOldBundlePath('/Applications/Allternit Desktop.app', '42-1');
    expect(old).toBe('/Applications/.Allternit Desktop.app.old-42-1');
    expect(old.endsWith('.app')).toBe(false);
    expect(isOldMacBundleName('.Allternit Desktop.app.old-42-1', 'Allternit Desktop.app')).toBe(true);
    expect(isOldMacBundleName('Allternit Desktop.app', 'Allternit Desktop.app')).toBe(false);
    expect(isOldMacBundleName('.Other.app.old-1', 'Allternit Desktop.app')).toBe(false);
  });

  it('reads the team from codesign -dv, treating ad-hoc as none', () => {
    expect(parseCodesignTeamId('Identifier=com.allternit.desktop\nTeamIdentifier=MQ85968PQP\n')).toBe('MQ85968PQP');
    expect(parseCodesignTeamId('Signature=adhoc\nTeamIdentifier=not set\n')).toBeNull();
    expect(parseCodesignTeamId('')).toBeNull();
  });

  it('accepts an ad-hoc build of the same app at the promised version', () => {
    expect(macStagedBundleProblem({
      current: { bundleId: 'com.allternit.desktop', version: '1.1.16', teamId: null },
      staged: { bundleId: 'com.allternit.desktop', version: '1.1.17', teamId: null },
      expectedVersion: 'v1.1.17',
    })).toBeNull();
  });

  it('refuses another app, another version, or a lost Developer ID team', () => {
    const current = { bundleId: 'com.allternit.desktop', version: '1.1.16', teamId: 'MQ85968PQP' };
    expect(macStagedBundleProblem({ current, staged: { ...current, bundleId: 'com.evil.app', version: '1.1.17' }, expectedVersion: '1.1.17' }))
      .toContain('bundle id');
    expect(macStagedBundleProblem({ current, staged: { ...current, version: '1.1.15' }, expectedVersion: '1.1.17' }))
      .toContain('version');
    expect(macStagedBundleProblem({ current, staged: { ...current, version: '1.1.17', teamId: null }, expectedVersion: '1.1.17' }))
      .toContain('team');
  });
});
