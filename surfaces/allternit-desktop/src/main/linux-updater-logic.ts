/**
 * Pure decisions for the Linux app updater (linux-updater.ts). No Electron
 * imports, so the unit tests run without an app.
 *
 * Electron's built-in autoUpdater has no Linux support, so Linux uses
 * electron-updater per package type: DebUpdater (.deb), RpmUpdater (.rpm) and
 * AppImageUpdater (AppImage, no root needed). All three read the same
 * latest-linux.yml, which lists every Linux file; each picks its own by
 * extension. Desktop 1.1.15 and older shipped without it: those users install
 * the next package by hand once, and every version after that updates itself.
 */

/** The GitHub repo that holds the Desktop releases (same as build.publish). */
export const DESKTOP_RELEASE_OWNER = 'Allternit';
export const DESKTOP_RELEASE_REPO = 'desktop';
export const DESKTOP_RELEASES_URL = `https://github.com/${DESKTOP_RELEASE_OWNER}/${DESKTOP_RELEASE_REPO}/releases`;

/** Env var that points a test build at a local feed (generic provider). */
export const LINUX_FEED_OVERRIDE_ENV = 'ALLTERNIT_LINUX_UPDATE_FEED';

/** Checks while the app runs, matching update-electron-app's '1 hour' on Mac/Windows. */
export const LINUX_UPDATE_INTERVAL_MS = 60 * 60 * 1000;

/**
 * Graphical elevation helpers, in the order electron-updater's
 * LinuxUpdater.determineSudoCommand tries them. When none exists it falls back
 * to plain `sudo`, which cannot ask for a password without a terminal.
 */
export const GRAPHICAL_ELEVATION_COMMANDS = ['gksudo', 'kdesudo', 'pkexec', 'beesu'] as const;

export type LinuxUpdateMode =
  /** Not Linux, or an unpackaged dev build: the Linux updater does not start. */
  | 'off'
  /** Installed from the .deb: DebUpdater installs with `pkexec dpkg -i`. */
  | 'deb'
  /** Installed from the .rpm: RpmUpdater installs with pkexec zypper/dnf/yum/rpm. */
  | 'rpm'
  /** Running as an AppImage: AppImageUpdater swaps the file in place, no root. */
  | 'appimage'
  /** Linux, but none of those (unpacked dir, unknown package type): no self-update. */
  | 'unsupported';

/** The package types that install through the system package manager (need root). */
export type LinuxSystemPackage = 'deb' | 'rpm';

export function chooseLinuxUpdateMode(input: {
  platform: string;
  isPackaged: boolean;
  /** Contents of resources/package-type (written by electron-builder, corrected by the postinst), or null. */
  packageType: string | null;
  /** process.env.APPIMAGE, set by the AppImage runtime. */
  appImagePath?: string | null;
}): LinuxUpdateMode {
  if (input.platform !== 'linux' || !input.isPackaged) return 'off';
  // APPIMAGE wins: the AppImage packs the same resources dir as the deb and
  // rpm, so its package-type file can say either of them.
  if (input.appImagePath?.trim()) return 'appimage';
  const type = input.packageType?.trim();
  if (type === 'deb' || type === 'rpm') return type;
  return 'unsupported';
}

/** Whether installing an update for this mode needs root (and so a password prompt). */
export function needsElevation(mode: LinuxUpdateMode): boolean {
  return mode === 'deb' || mode === 'rpm';
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

export type LinuxFeed =
  | { provider: 'github'; owner: string; repo: string }
  | { provider: 'generic'; url: string };

/** The production feed, or a generic http(s) feed when the override env var is set. */
export function resolveLinuxFeed(override: string | undefined | null): { feed: LinuxFeed; ignoredOverride?: string } {
  const github: LinuxFeed = { provider: 'github', owner: DESKTOP_RELEASE_OWNER, repo: DESKTOP_RELEASE_REPO };
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

export type LinuxFallbackReason = 'no-elevation' | 'install-failed';

/** How to install the downloaded file by hand, per package type. */
export function manualInstallHint(mode: LinuxUpdateMode): string {
  switch (mode) {
    case 'rpm':
      return 'Click to download the .rpm, then run: sudo dnf install ./<file>.rpm (openSUSE: sudo zypper install ./<file>.rpm)';
    case 'appimage':
      return 'Click to download the new AppImage, make it executable (chmod +x <file>.AppImage) and run it in place of this one.';
    default:
      return 'Click to download the .deb, then run: sudo apt install ./<file>.deb';
  }
}

/** The notification shown when the update cannot install itself. */
export function fallbackNotice(reason: LinuxFallbackReason, version?: string | null, mode: LinuxUpdateMode = 'deb'): {
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
  if (mode === 'appimage') {
    return {
      title: `${name} could not be installed`,
      body: `The AppImage could not be replaced (is its folder read-only?). ${hint}`,
      url,
    };
  }
  return {
    title: `${name} could not be installed`,
    body: `The update needs your password to install. ${hint}`,
    url,
  };
}
