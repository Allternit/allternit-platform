import { describe, expect, it } from 'vitest';
import {
  buildAppImageAppArmorProfile,
  decideAppImageSandbox,
  noSandboxNotice,
  relaunchArgs,
  relaunchEnv,
  type AppImageSandboxFacts,
} from './appimage-sandbox-logic.js';

const APPIMAGE = '/home/ana/Applications/Allternit-Desktop-1.2.0-x86_64.AppImage';

// Ubuntu 24.04 desktop, AppImage started by AppRun with --no-sandbox because
// AppArmor blocks unprivileged user namespaces.
const ubuntu2404: AppImageSandboxFacts = {
  platform: 'linux',
  appImagePath: APPIMAGE,
  noSandboxSwitch: true,
  usernsProbe: 'failed',
  apparmorRestrictUserns: '1\n',
  unprivilegedUsernsClone: null,
  maxUserNamespaces: '63206\n',
  apparmorEnabled: true,
  hasPkexec: true,
  hasApparmorParser: true,
  promptDisabled: false,
  relaunchedAfterProfile: false,
  state: {},
};

describe('decideAppImageSandbox', () => {
  it('does nothing outside an AppImage (deb, rpm, unpacked) or off Linux', () => {
    expect(decideAppImageSandbox({ ...ubuntu2404, appImagePath: null })).toEqual({ action: 'none', reason: 'not-appimage' });
    expect(decideAppImageSandbox({ ...ubuntu2404, platform: 'darwin' })).toEqual({ action: 'none', reason: 'not-appimage' });
  });

  it('does nothing when the sandbox is on', () => {
    expect(decideAppImageSandbox({ ...ubuntu2404, noSandboxSwitch: false })).toEqual({ action: 'none', reason: 'sandbox-on' });
  });

  it('treats --no-sandbox with working user namespaces as the user\'s own flag', () => {
    expect(decideAppImageSandbox({ ...ubuntu2404, usernsProbe: 'ok' })).toEqual({ action: 'none', reason: 'explicit-flag' });
  });

  it('offers the AppArmor profile on Ubuntu 24.04 when pkexec and apparmor_parser exist', () => {
    expect(decideAppImageSandbox(ubuntu2404)).toEqual({ action: 'offer-profile', cause: 'apparmor' });
  });

  it('falls back to the notice when the profile cannot be installed', () => {
    for (const patch of [{ hasPkexec: false }, { hasApparmorParser: false }, { apparmorEnabled: false }, { promptDisabled: true }]) {
      expect(decideAppImageSandbox({ ...ubuntu2404, ...patch })).toEqual({ action: 'notice', cause: 'apparmor', showNotice: true });
    }
  });

  it('does not ask again for a path the user declined, and shows the notice only once per path', () => {
    expect(decideAppImageSandbox({ ...ubuntu2404, state: { declinedFor: APPIMAGE } }))
      .toEqual({ action: 'notice', cause: 'apparmor', showNotice: true });
    expect(decideAppImageSandbox({ ...ubuntu2404, state: { declinedFor: APPIMAGE, noticeShownFor: APPIMAGE } }))
      .toEqual({ action: 'notice', cause: 'apparmor', showNotice: false });
    // Moved the AppImage: the old profile no longer matches, so ask again.
    expect(decideAppImageSandbox({ ...ubuntu2404, state: { declinedFor: '/tmp/old.AppImage' } }))
      .toEqual({ action: 'offer-profile', cause: 'apparmor' });
  });

  it('never loops when the relaunch after the profile install is still unsandboxed', () => {
    expect(decideAppImageSandbox({ ...ubuntu2404, relaunchedAfterProfile: true }))
      .toEqual({ action: 'notice', cause: 'apparmor-profile-ineffective', showNotice: true });
  });

  it('names a Debian/hardened kernel as the cause and does not offer AppArmor there', () => {
    const debian12 = { ...ubuntu2404, apparmorRestrictUserns: null, unprivilegedUsernsClone: '0\n' };
    expect(decideAppImageSandbox(debian12)).toEqual({ action: 'notice', cause: 'kernel', showNotice: true });
    const maxZero = { ...ubuntu2404, apparmorRestrictUserns: '0', maxUserNamespaces: '0' };
    expect(decideAppImageSandbox(maxZero)).toEqual({ action: 'notice', cause: 'kernel', showNotice: true });
  });

  it('names a missing unshare, else unknown', () => {
    const base = { ...ubuntu2404, apparmorRestrictUserns: null, unprivilegedUsernsClone: null };
    expect(decideAppImageSandbox({ ...base, usernsProbe: 'missing' })).toEqual({ action: 'notice', cause: 'no-unshare', showNotice: true });
    expect(decideAppImageSandbox(base)).toEqual({ action: 'notice', cause: 'unknown', showNotice: true });
  });

  it('does not offer a profile for a path it cannot write safely', () => {
    expect(decideAppImageSandbox({ ...ubuntu2404, appImagePath: '/home/a/"x".AppImage' }).action).toBe('notice');
  });
});

describe('buildAppImageAppArmorProfile', () => {
  it('attaches a userns profile to the exact AppImage path, glob characters escaped', () => {
    const p = buildAppImageAppArmorProfile('/home/ana/My Apps/Allternit[1].AppImage')!;
    expect(p).toContain('abi <abi/4.0>,');
    expect(p).toContain('profile allternit-appimage "/home/ana/My Apps/Allternit\\[1\\].AppImage" flags=(unconfined) {');
    expect(p).toContain('  userns,');
  });

  it('refuses relative paths and paths with quotes or newlines', () => {
    expect(buildAppImageAppArmorProfile('Allternit.AppImage')).toBeNull();
    expect(buildAppImageAppArmorProfile('/a/"b".AppImage')).toBeNull();
    expect(buildAppImageAppArmorProfile('/a/b\n} profile x /** {')).toBeNull();
  });
});

describe('relaunch', () => {
  it('keeps the user\'s args and drops --no-sandbox so AppRun decides again', () => {
    expect(relaunchArgs(['/tmp/.mount_AlltXYZ/allternit', '--no-sandbox', 'allternit://pair?code=1']))
      .toEqual(['allternit://pair?code=1']);
  });

  it('drops the old mount from the env and marks the relaunch', () => {
    const env = relaunchEnv({
      APPDIR: '/tmp/.mount_AlltXYZ',
      APPIMAGE: APPIMAGE,
      ARGV0: 'Allternit',
      PATH: '/tmp/.mount_AlltXYZ:/tmp/.mount_AlltXYZ/usr/sbin:/usr/local/bin:/usr/bin',
      LD_LIBRARY_PATH: '/tmp/.mount_AlltXYZ/usr/lib',
      HOME: '/home/ana',
    }, 4242);
    expect(env).toEqual({
      PATH: '/usr/local/bin:/usr/bin',
      HOME: '/home/ana',
      ALLTERNIT_APPIMAGE_SANDBOX_RELAUNCHED: '1',
      ALLTERNIT_OLD_PID: '4242',
    });
  });
});

describe('noSandboxNotice', () => {
  it('recommends the .deb or .rpm whatever the cause', () => {
    for (const cause of ['apparmor', 'apparmor-profile-ineffective', 'kernel', 'no-unshare', 'unknown'] as const) {
      expect(noSandboxNotice(cause).body).toContain('.deb (Ubuntu, Debian, Mint) or .rpm (Fedora, openSUSE)');
    }
  });
});
