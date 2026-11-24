/**
 * Pure decisions for the AppImage sandbox check (appimage-sandbox.ts). No
 * Electron imports, so the unit tests run without an app.
 *
 * Why this exists: an AppImage cannot ship a setuid chrome-sandbox (it runs
 * from a FUSE mount or a /tmp extract owned by the user), so Chromium's only
 * sandbox there is the user-namespace sandbox. Where normal users cannot make
 * a user namespace (Ubuntu 23.10+ AppArmor restriction, Debian/hardened
 * kernels with unprivileged_userns_clone=0, max_user_namespaces=0) the zygote
 * dies at start with "FATAL credentials.cc:131".
 *
 * The zygote is forked before main.js runs, so the main process cannot turn
 * the sandbox off for itself (app.commandLine.appendSwitch('no-sandbox') is too
 * late). electron-builder's AppRun handles the start: it runs
 * `unshare -Ur true` and adds --no-sandbox when that fails. This module decides
 * what to do once the app is up and finds itself running that way:
 *   - AppArmor is the cause and we can fix it: offer to install an AppArmor
 *     profile (`userns,`) for this AppImage's path through pkexec, then
 *     relaunch. AppArmor profiles in unconfined mode carry over exec, so the
 *     AppRun probe and the Electron binary both get user namespaces and the
 *     sandbox comes back on.
 *   - Anything else, or the user said no: keep running without the sandbox,
 *     log it, and show one notice that recommends the .deb or .rpm.
 */

/** Profile name and file under /etc/apparmor.d. The .deb's profile is "allternit". */
export const APPIMAGE_APPARMOR_PROFILE = 'allternit-appimage';
export const APPIMAGE_APPARMOR_PROFILE_PATH = `/etc/apparmor.d/${APPIMAGE_APPARMOR_PROFILE}`;

/** Set on the relaunch after the profile install, so a profile that did not take never loops. */
export const SANDBOX_RELAUNCH_ENV = 'ALLTERNIT_APPIMAGE_SANDBOX_RELAUNCHED';
/** `0` turns the install offer off (labs and CI, where a modal dialog would block the window check). */
export const SANDBOX_PROMPT_ENV = 'ALLTERNIT_APPIMAGE_SANDBOX_PROMPT';

export const LINUX_DOWNLOAD_PAGE = 'https://github.com/Allternit/desktop/releases/latest';

export type UsernsProbe = 'ok' | 'failed' | 'missing';

export type SandboxCause =
  /** kernel.apparmor_restrict_unprivileged_userns=1 (Ubuntu 23.10+). */
  | 'apparmor'
  /** The profile was installed, the app relaunched, and the sandbox is still off. */
  | 'apparmor-profile-ineffective'
  /** kernel.unprivileged_userns_clone=0 (Debian) or user.max_user_namespaces=0. */
  | 'kernel'
  /** No `unshare` binary, so AppRun could not test and turned the sandbox off. */
  | 'no-unshare'
  | 'unknown';

export interface AppImageSandboxState {
  /** AppImage path the user said "No" to the profile install for. */
  declinedFor?: string | null;
  /** AppImage path the no-sandbox notice was already shown for. */
  noticeShownFor?: string | null;
}

export interface AppImageSandboxFacts {
  platform: string;
  /** process.env.APPIMAGE (set by the AppImage runtime), or null. */
  appImagePath: string | null;
  /** The process runs with --no-sandbox (AppRun added it, or the user passed it). */
  noSandboxSwitch: boolean;
  /** `unshare -Ur true` from this process: the same probe AppRun uses. */
  usernsProbe: UsernsProbe;
  /** Raw contents of the /proc/sys files, or null when absent. */
  apparmorRestrictUserns: string | null;
  unprivilegedUsernsClone: string | null;
  maxUserNamespaces: string | null;
  /** /sys/module/apparmor/parameters/enabled is "Y". */
  apparmorEnabled: boolean;
  hasPkexec: boolean;
  hasApparmorParser: boolean;
  /** SANDBOX_PROMPT_ENV is "0". */
  promptDisabled: boolean;
  /** SANDBOX_RELAUNCH_ENV is "1". */
  relaunchedAfterProfile: boolean;
  state: AppImageSandboxState;
}

export type AppImageSandboxDecision =
  | { action: 'none'; reason: 'not-appimage' | 'sandbox-on' | 'explicit-flag' }
  | { action: 'offer-profile'; cause: 'apparmor' }
  | { action: 'notice'; cause: SandboxCause; showNotice: boolean };

const trimmed = (v: string | null): string | null => (v == null ? null : v.trim());

export function classifyCause(f: Pick<AppImageSandboxFacts,
  'apparmorRestrictUserns' | 'unprivilegedUsernsClone' | 'maxUserNamespaces' | 'usernsProbe' | 'relaunchedAfterProfile'>): SandboxCause {
  if (trimmed(f.apparmorRestrictUserns) === '1') {
    return f.relaunchedAfterProfile ? 'apparmor-profile-ineffective' : 'apparmor';
  }
  if (trimmed(f.unprivilegedUsernsClone) === '0' || trimmed(f.maxUserNamespaces) === '0') return 'kernel';
  if (f.usernsProbe === 'missing') return 'no-unshare';
  return 'unknown';
}

export function decideAppImageSandbox(f: AppImageSandboxFacts): AppImageSandboxDecision {
  if (f.platform !== 'linux' || !f.appImagePath) return { action: 'none', reason: 'not-appimage' };
  if (!f.noSandboxSwitch) return { action: 'none', reason: 'sandbox-on' };
  // User namespaces work, so AppRun would not have added --no-sandbox: the
  // user (or a lab script) passed it. Respect it and only log.
  if (f.usernsProbe === 'ok') return { action: 'none', reason: 'explicit-flag' };

  const cause = classifyCause(f);
  const canOffer = cause === 'apparmor'
    && f.apparmorEnabled
    && f.hasPkexec
    && f.hasApparmorParser
    && !f.promptDisabled
    && buildAppImageAppArmorProfile(f.appImagePath) !== null
    && f.state.declinedFor !== f.appImagePath;
  if (canOffer) return { action: 'offer-profile', cause: 'apparmor' };
  return { action: 'notice', cause, showNotice: f.state.noticeShownFor !== f.appImagePath };
}

/**
 * The AppArmor profile for one AppImage file, or null when the path cannot be
 * written safely into a profile (quotes, newlines, NUL, or not absolute).
 * Same shape as electron-builder's apparmor-profile.tpl that the .deb installs.
 */
export function buildAppImageAppArmorProfile(appImagePath: string): string | null {
  if (!appImagePath.startsWith('/') || /["\n\r\0]/.test(appImagePath)) return null;
  // AppArmor treats these as glob characters even inside quotes.
  const escaped = appImagePath.replace(/[\\*?[\]{}^]/g, (c) => `\\${c}`);
  return [
    'abi <abi/4.0>,',
    'include <tunables/global>',
    '',
    `# Allternit Desktop AppImage: lets ${APPIMAGE_APPARMOR_PROFILE} create user namespaces`,
    '# so the Chromium sandbox stays on. Written by the app on the user\'s request.',
    `profile ${APPIMAGE_APPARMOR_PROFILE} "${escaped}" flags=(unconfined) {`,
    '  userns,',
    '',
    `  include if exists <local/${APPIMAGE_APPARMOR_PROFILE}>`,
    '}',
    '',
  ].join('\n');
}

/** The args for the relaunch: everything after argv[0], minus --no-sandbox (AppRun decides again). */
export function relaunchArgs(argv: readonly string[]): string[] {
  return argv.slice(1).filter((a) => a !== '--no-sandbox');
}

const APPRUN_PATH_VARS = ['PATH', 'LD_LIBRARY_PATH', 'XDG_DATA_DIRS', 'GSETTINGS_SCHEMA_DIR'] as const;

/**
 * Env for the relaunched AppImage. The current mount ($APPDIR) goes away when
 * this process exits, so drop the AppImage runtime's variables (it sets them
 * again) and the entries AppRun prepended that point into the old mount.
 */
export function relaunchEnv(env: Readonly<Record<string, string | undefined>>, oldPid: number): Record<string, string> {
  const out: Record<string, string> = {};
  const appDir = env.APPDIR;
  for (const [k, v] of Object.entries(env)) {
    if (v == null) continue;
    if (k === 'APPDIR' || k === 'APPIMAGE' || k === 'ARGV0' || k === 'OWD') continue;
    if (appDir && (APPRUN_PATH_VARS as readonly string[]).includes(k)) {
      const kept = v.split(':').filter((p) => p && !(p === appDir || p.startsWith(`${appDir}/`)));
      if (kept.length) out[k] = kept.join(':');
      continue;
    }
    out[k] = v;
  }
  out[SANDBOX_RELAUNCH_ENV] = '1';
  out.ALLTERNIT_OLD_PID = String(oldPid);
  return out;
}

/** The text of the one-time notice when the app keeps running without the sandbox. */
export function noSandboxNotice(cause: SandboxCause): { title: string; body: string } {
  const title = 'Allternit is running without the Chromium sandbox';
  const tail = 'For full security, install the .deb (Ubuntu, Debian, Mint) or .rpm (Fedora, openSUSE) instead.';
  switch (cause) {
    case 'apparmor':
      return { title, body: `AppArmor on this computer blocks the user namespaces the sandbox needs. ${tail}` };
    case 'apparmor-profile-ineffective':
      return { title, body: `The AppArmor profile was installed but did not apply to this AppImage. ${tail}` };
    case 'kernel':
      return { title, body: `This kernel does not let normal users create user namespaces (unprivileged_userns_clone or max_user_namespaces is 0). ${tail}` };
    case 'no-unshare':
      return { title, body: `The unshare command (util-linux) is missing, so the sandbox could not be tested. ${tail}` };
    default:
      return { title, body: `User namespaces are not available here. ${tail}` };
  }
}
