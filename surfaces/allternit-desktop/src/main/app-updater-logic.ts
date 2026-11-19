/**
 * Pure decisions for the app updater (app-updater.ts). No Electron imports,
 * so the unit tests run without an app.
 *
 * One updater on every OS: electron-updater, reading the GitHub releases of
 * Allternit/desktop (the same repo as build.publish).
 *
 * - macOS: MacUpdater reads latest-mac.yml and downloads the zip for this
 *   Mac's architecture. The install does NOT go through Squirrel.Mac: release
 *   builds are ad-hoc signed, and Squirrel.Mac only accepts a bundle that
 *   meets the running app's designated requirement, which for an ad-hoc
 *   signature is its exact cdhash, so every new build is refused.
 *   mac-app-swap.ts checks the new bundle and replaces the old one instead.
 * - Windows: NsisUpdater reads latest.yml, downloads Allternit-Setup-<v>.exe
 *   and runs it silently on Restart (or when the app quits).
 * - Linux .deb: DebUpdater reads latest-linux.yml, downloads the .deb and
 *   installs it with `pkexec dpkg -i` on Restart.
 *
 * Desktop 1.1.15 and older used update-electron-app (update.electronjs.org +
 * Electron's built-in autoUpdater), which never updated these builds: the
 * service wants -mac- / -darwin- zips that Squirrel.Mac then refuses (ad-hoc
 * signature), and Squirrel.Windows packages that an NSIS build does not have.
 * Those users install the next version by hand once; every version after
 * that updates itself.
 */

/** The GitHub repo that holds the Desktop releases (same as build.publish). */
export const DESKTOP_RELEASE_OWNER = 'Allternit';
export const DESKTOP_RELEASE_REPO = 'desktop';
export const DESKTOP_RELEASES_URL = `https://github.com/${DESKTOP_RELEASE_OWNER}/${DESKTOP_RELEASE_REPO}/releases`;

/** Env var that points a test build at a local feed (generic provider). */
export const UPDATE_FEED_OVERRIDE_ENV = 'ALLTERNIT_UPDATE_FEED';
/** The Linux-only name of the same override (Linux lab notes use it). */
export const LEGACY_LINUX_FEED_OVERRIDE_ENV = 'ALLTERNIT_LINUX_UPDATE_FEED';

/** Check at start, then every hour while the app runs. */
export const UPDATE_INTERVAL_MS = 60 * 60 * 1000;

/**
 * Graphical elevation helpers, in the order electron-updater's
 * LinuxUpdater.determineSudoCommand tries them. When none exists it falls back
 * to plain `sudo`, which cannot ask for a password without a terminal.
 */
export const GRAPHICAL_ELEVATION_COMMANDS = ['gksudo', 'kdesudo', 'pkexec', 'beesu'] as const;

export type AppUpdateMode =
  /** Unpackaged dev build, or an OS we do not ship: no updater. */
  | 'off'
  /** macOS: MacUpdater downloads the zip, mac-app-swap.ts replaces the bundle. */
  | 'mac'
  /** Windows: NsisUpdater downloads and runs the NSIS installer. */
  | 'windows'
  /** Installed from the .deb: DebUpdater installs with `pkexec dpkg -i`. */
  | 'deb'
  /** Linux, but not a .deb install (AppImage, unpacked dir): no self-update. */
  | 'unsupported';

export function chooseUpdateMode(input: {
  platform: string;
  isPackaged: boolean;
  /** Contents of resources/package-type (written by electron-builder), or null. */
  packageType: string | null;
}): AppUpdateMode {
  if (!input.isPackaged) return 'off';
  if (input.platform === 'darwin') return 'mac';
  if (input.platform === 'win32') return 'windows';
  if (input.platform !== 'linux') return 'off';
  return input.packageType?.trim() === 'deb' ? 'deb' : 'unsupported';
}

/** Whether installing an update for this mode needs root (and so a password prompt). */
export function needsElevation(mode: AppUpdateMode): boolean {
  return mode === 'deb';
}

/**
 * Whether a downloaded update installs when the app quits without Restart.
 * Windows: yes, the NSIS installer runs silently (electron-updater default).
 * macOS and Linux install only on Restart: the bundle swap and the password
 * prompt need the app alive to report a failure.
 */
export function installsOnQuit(mode: AppUpdateMode): boolean {
  return mode === 'windows';
}

export type LinuxElevation =
  | { kind: 'root' }
  | { kind: 'graphical'; command: string }
  | { kind: 'none' };

export function detectElevation(input: {
  isRoot: boolean;
  hasCommand: (cmd: string) => boolean;
}): LinuxElevation {
  if (input.isRoot) return { kind: 'root' };
  for (const cmd of GRAPHICAL_ELEVATION_COMMANDS) {
    if (input.hasCommand(cmd)) return { kind: 'graphical', command: cmd };
  }
  return { kind: 'none' };
}

/**
 * Download automatically only when the install can ask for a password (or
 * needs none). Without a way to elevate, the user gets a notification that
 * opens the release page instead of a download that can never install.
 */
export function shouldAutoDownload(elevation: LinuxElevation): boolean {
  return elevation.kind !== 'none';
}

export type UpdateFeed =
  | { provider: 'github'; owner: string; repo: string }
  | { provider: 'generic'; url: string };

/** The production feed, or a generic http(s) feed when the override env var is set. */
export function resolveUpdateFeed(override: string | undefined | null): { feed: UpdateFeed; ignoredOverride?: string } {
  const github: UpdateFeed = { provider: 'github', owner: DESKTOP_RELEASE_OWNER, repo: DESKTOP_RELEASE_REPO };
  const raw = override?.trim();
  if (!raw) return { feed: github };
  try {
    const url = new URL(raw);
    if (url.protocol === 'https:' || url.protocol === 'http:') {
      return { feed: { provider: 'generic', url: url.toString() } };
    }
  } catch {
    // fall through
  }
  return { feed: github, ignoredOverride: raw };
}

const VERSION_RE = /^\d+\.\d+\.\d+(?:[-.][0-9A-Za-z.]+)?$/;

/** The release page for a version (tags are v<version>), or the latest release. */
export function releasePageUrl(version?: string | null): string {
  const v = version?.trim().replace(/^v/, '');
  return v && VERSION_RE.test(v) ? `${DESKTOP_RELEASES_URL}/tag/v${v}` : `${DESKTOP_RELEASES_URL}/latest`;
}

export type UpdateFallbackReason = 'no-elevation' | 'install-failed';

/** How to install the new version by hand, per install type. */
export function manualInstallHint(mode: AppUpdateMode): string {
  switch (mode) {
    case 'mac':
      return 'Click to download the .dmg for your Mac, open it and drag Allternit Desktop to Applications.';
    case 'windows':
      return 'Click to download Allternit-Setup and run it.';
    default:
      return 'Click to download the .deb, then run: sudo apt install ./<file>.deb';
  }
}

/** The notification shown when the update cannot install itself. */
export function fallbackNotice(reason: UpdateFallbackReason, version?: string | null, mode: AppUpdateMode = 'deb'): {
  title: string;
  body: string;
  url: string;
} {
  const v = version?.trim().replace(/^v/, '');
  const name = v ? `Allternit Desktop ${v}` : 'A new Allternit Desktop';
  const url = releasePageUrl(v);
  const hint = manualInstallHint(mode);
  if (reason === 'no-elevation') {
    return {
      title: `${name} is available`,
      body: `This computer has no password prompt for installing updates (pkexec). ${hint}`,
      url,
    };
  }
  if (mode === 'mac') {
    return {
      title: `${name} could not be installed`,
      body: `Allternit could not replace its app (if macOS blocked it, allow Allternit under System Settings > Privacy & Security > App Management). ${hint}`,
      url,
    };
  }
  if (mode === 'windows') {
    return {
      title: `${name} could not be installed`,
      body: `The downloaded installer is missing. ${hint}`,
      url,
    };
  }
  return {
    title: `${name} could not be installed`,
    body: `The update needs your password to install. ${hint}`,
    url,
  };
}

// ── macOS bundle swap: pure parts (mac-app-swap.ts runs them) ──────────────

/**
 * The .app bundle that holds the running executable, from process.execPath
 * (".../Allternit Desktop.app/Contents/MacOS/Allternit Desktop"), or null.
 */
export function macBundleFromExecPath(execPath: string): string | null {
  const marker = '.app/Contents/MacOS/';
  const at = execPath.lastIndexOf(marker);
  if (at <= 0) return null;
  return execPath.slice(0, at + '.app'.length);
}

/**
 * Why the running bundle cannot be replaced in place, or null. A translocated
 * app (opened straight from Downloads or a DMG with quarantine) runs from a
 * random read-only mount: swapping there would change nothing the user keeps.
 */
export function macSwapBlocker(bundlePath: string): string | null {
  if (bundlePath.includes('/AppTranslocation/')) {
    return 'the app runs from a translocated copy (move Allternit Desktop to Applications first)';
  }
  return null;
}

/**
 * Where the old bundle goes during the swap: a hidden sibling whose name does
 * not end in .app, so Finder and Launch Services ignore it. The next launch
 * deletes it.
 */
export function macOldBundlePath(bundlePath: string, tag: string): string {
  const slash = bundlePath.lastIndexOf('/');
  const dir = bundlePath.slice(0, slash);
  const base = bundlePath.slice(slash + 1);
  return `${dir}/.${base}.old-${tag}`;
}

/** Whether a directory entry is an old bundle left by an earlier swap of `bundleBase`. */
export function isOldMacBundleName(entry: string, bundleBase: string): boolean {
  return entry.startsWith(`.${bundleBase}.old-`);
}

/** TeamIdentifier from `codesign -dv` output; null for ad-hoc ("not set") or none. */
export function parseCodesignTeamId(output: string): string | null {
  const m = /^TeamIdentifier=(.+)$/m.exec(output);
  const team = m?.[1]?.trim();
  return team && team !== 'not set' ? team : null;
}

export interface MacBundleFacts {
  bundleId: string | null;
  version: string | null;
  teamId: string | null;
}

/**
 * Whether the staged bundle may replace the running one: the same bundle id,
 * the version the feed promised, and, when the running app has a Developer ID
 * team, the same team. Returns the reason it may not, or null.
 */
export function macStagedBundleProblem(input: {
  current: MacBundleFacts;
  staged: MacBundleFacts;
  expectedVersion: string;
}): string | null {
  const { current, staged } = input;
  if (!current.bundleId) return 'cannot read the running app bundle id';
  if (staged.bundleId !== current.bundleId) {
    return `bundle id ${staged.bundleId ?? '(none)'} is not ${current.bundleId}`;
  }
  const want = input.expectedVersion.trim().replace(/^v/, '');
  if (staged.version !== want) return `version ${staged.version ?? '(none)'} is not ${want}`;
  if (current.teamId && staged.teamId !== current.teamId) {
    return `signed by team ${staged.teamId ?? '(ad-hoc)'}, not ${current.teamId}`;
  }
  return null;
}
