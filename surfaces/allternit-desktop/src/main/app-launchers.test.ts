import { mkdtempSync, readFileSync, existsSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { afterEach, describe, expect, it } from 'vitest';
import {
  appBundleId,
  appUserModelId,
  createAppLaunchers,
  desktopExecArg,
  isAllowedIconUrl,
  isValidAppSlug,
  linuxDesktopEntry,
  macInfoPlist,
  pngToIco,
  safeAppName,
} from './app-launchers.js';

describe('app ids', () => {
  it('uses the cloud slug rule and one id for Dock and taskbar', () => {
    expect(isValidAppSlug('notes2')).toBe(true);
    expect(isValidAppSlug('2notes')).toBe(false);
    expect(isValidAppSlug('a'.repeat(17))).toBe(false);
    expect(isValidAppSlug('../x')).toBe(false);
    expect(appBundleId('notes2')).toBe('com.allternit.app.notes2');
    expect(appUserModelId('notes2')).toBe('com.allternit.app.notes2');
    expect(() => appBundleId('Bad Slug')).toThrow();
  });
});

describe('safeAppName', () => {
  it('drops path separators and characters Windows refuses', () => {
    expect(safeAppName('  My/App: v2?  ')).toBe('My App v2');
    expect(safeAppName('..hidden')).toBe('hidden');
    expect(safeAppName('CON')).toBe('CON App');
    expect(safeAppName('   ')).toBeNull();
    expect(safeAppName(42)).toBeNull();
  });
});

describe('macInfoPlist', () => {
  it('carries the bundle id, escapes the name and URL, and keeps menu bar off by default', () => {
    const plist = macInfoPlist({ slug: 'notes', name: 'Notes & <Co>', appUrl: 'https://ai.allternit.com/app/notes?a=1&b=2', menuBar: false, hasIcon: true });
    expect(plist).toContain('<key>CFBundleIdentifier</key>\n  <string>com.allternit.app.notes</string>');
    expect(plist).toContain('<string>Notes &amp; &lt;Co&gt;</string>');
    expect(plist).toContain('<string>https://ai.allternit.com/app/notes?a=1&amp;b=2</string>');
    expect(plist).toContain('<key>AllternitMenuBar</key>\n  <false/>');
    expect(plist).toContain('<key>CFBundleExecutable</key>\n  <string>AllternitAppHelper</string>');
    expect(plist).toContain('<key>CFBundleIconFile</key>');
    expect(macInfoPlist({ slug: 'notes', name: 'N', appUrl: 'https://x', menuBar: true, hasIcon: false })).not.toContain('CFBundleIconFile');
  });
});

describe('linuxDesktopEntry', () => {
  it('quotes an AppImage path with spaces and runs the app deep link', () => {
    const entry = linuxDesktopEntry({ slug: 'notes', name: 'Notes\nInjected=1', execPath: '/home/a b/Allternit.AppImage', iconPath: '/home/a b/.local/share/icons/n.png' });
    expect(entry).toContain('Exec="/home/a b/Allternit.AppImage" allternit://app/notes\n');
    expect(entry).toContain('Name=Notes Injected=1\n');
    expect(entry).not.toMatch(/^Injected=1$/m);
    expect(entry).toContain('Icon=/home/a b/.local/share/icons/n.png\n');
  });

  it('escapes the Exec field specials', () => {
    expect(desktopExecArg('/opt/allternit')).toBe('/opt/allternit');
    expect(desktopExecArg('/opt/100%/x')).toBe('/opt/100%%/x');
    expect(desktopExecArg('/opt/$HOME "x"')).toBe('"/opt/\\$HOME \\"x\\""');
  });
});

describe('pngToIco', () => {
  it('writes one PNG entry with 256 stored as 0', () => {
    const png = Buffer.from([0x89, 0x50, 0x4e, 0x47, 1, 2, 3]);
    const ico = pngToIco(png, 256);
    expect(ico.readUInt16LE(2)).toBe(1);
    expect(ico.readUInt16LE(4)).toBe(1);
    expect(ico.readUInt8(6)).toBe(0);
    expect(ico.readUInt32LE(6 + 8)).toBe(png.length);
    expect(ico.readUInt32LE(6 + 12)).toBe(22);
    expect(ico.subarray(22).equals(png)).toBe(true);
  });
});

describe('isAllowedIconUrl', () => {
  it('takes https and inline PNG/JPEG only', () => {
    expect(isAllowedIconUrl('https://cdn.allternit.com/i.png')).toBe(true);
    expect(isAllowedIconUrl('data:image/png;base64,AAAA')).toBe(true);
    expect(isAllowedIconUrl('http://cdn.allternit.com/i.png')).toBe(false);
    expect(isAllowedIconUrl('file:///etc/passwd')).toBe(false);
    expect(isAllowedIconUrl('data:image/svg+xml;base64,AAAA')).toBe(false);
  });
});

describe('createAppLaunchers on Linux (real temp home)', () => {
  let home = '';
  afterEach(() => { if (home) rmSync(home, { recursive: true, force: true }); });

  it('installs a .desktop launcher with its own icon, then removes it', async () => {
    home = mkdtempSync(join(tmpdir(), 'app-launchers-'));
    const launchers = createAppLaunchers({
      platform: 'linux',
      home,
      userData: join(home, 'userData'),
      appData: join(home, 'appData'),
      helperBinary: null,
      execPath: '/opt/Allternit/allternit',
      execArgs: [],
      resizePng: (bytes: Buffer) => bytes,
      writeShortcut: () => false,
      registerLinuxScheme: async () => {},
    });
    const icon = `data:image/png;base64,${Buffer.from('png-bytes').toString('base64')}`;
    const result = await launchers.install({ slug: 'notes', name: 'Notes', iconUrl: icon, tier: 'app' }, 'https://ai.allternit.com/app/notes');
    expect(result.ok).toBe(true);
    const file = join(home, '.local', 'share', 'applications', 'allternit-app-notes.desktop');
    const text = readFileSync(file, 'utf8');
    expect(text).toContain('Exec=/opt/Allternit/allternit allternit://app/notes');
    const iconPath = join(home, '.local', 'share', 'icons', 'allternit-apps', 'allternit-app-notes.png');
    expect(text).toContain(`Icon=${iconPath}`);
    expect(readFileSync(iconPath, 'utf8')).toBe('png-bytes');
    expect((await launchers.get('notes'))?.name).toBe('Notes');

    const removed = await launchers.uninstall('notes');
    expect(removed).toEqual({ ok: true, removed: true });
    expect(existsSync(file)).toBe(false);
    expect(existsSync(iconPath)).toBe(false);
    expect(await launchers.get('notes')).toBeNull();
  });

  it('refuses a bad slug before touching the disk', async () => {
    home = mkdtempSync(join(tmpdir(), 'app-launchers-'));
    const launchers = createAppLaunchers({
      platform: 'linux', home, userData: join(home, 'u'), appData: join(home, 'a'), helperBinary: null,
      execPath: '/x', execArgs: [], resizePng: () => null, writeShortcut: () => false, registerLinuxScheme: async () => {},
    });
    const result = await launchers.install({ slug: '../../etc', name: 'x', tier: 'browser' }, 'https://x');
    expect(result).toMatchObject({ ok: false, error: 'bad_slug' });
    expect(existsSync(join(home, '.local'))).toBe(false);
  });
});
