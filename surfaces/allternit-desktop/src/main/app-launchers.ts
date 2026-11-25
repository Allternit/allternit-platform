/**
 * Per-OS launchers for installed Allternit apps (plan v3 lane 38, E1 = B, D18).
 *
 *   macOS    ~/Applications/Allternit Apps/<Name>.app: the prebuilt native
 *            helper (native/app-helper) + Info.plist (com.allternit.app.<slug>)
 *            + AppIcon.icns, signed ad hoc. Its own Dock tile and process.
 *            Written locally, so no quarantine flag and no Gatekeeper prompt.
 *   Windows  Start menu shortcut with its own AppUserModelID that runs
 *            Allternit with allternit://app/<slug>; the app window sets the
 *            same id (app-window.ts) so it gets its own taskbar icon.
 *   Linux    ~/.local/share/applications/allternit-app-<slug>.desktop with
 *            its own icon, Exec=<allternit> allternit://app/<slug>.
 *
 * The pure text builders (Info.plist, .desktop entry, ids, ICO wrapper) are
 * exported for tests. Everything that touches the disk or runs a tool goes
 * through createAppLaunchers().
 */
import { execFile } from 'node:child_process';
import * as fs from 'node:fs/promises';
import * as path from 'node:path';

export type AppTier = 'browser' | 'app' | 'full';

export interface AppInstallRequest {
  slug: string;
  name: string;
  /** https URL or data:image/png|jpeg URL of the app icon. Optional. */
  iconUrl?: string | null;
  tier: AppTier;
  /** Keep a macOS menu bar item so the app runs after its window closes. Only when the app's permissions include it. */
  menuBar?: boolean;
}

export type LauncherKind = 'mac-dock' | 'windows-start-menu' | 'linux-desktop-entry';

export interface InstalledApp {
  slug: string;
  name: string;
  tier: AppTier;
  menuBar: boolean;
  kind: LauncherKind;
  /** The .app bundle, .lnk or .desktop file. */
  launcherPath: string;
  iconPath: string | null;
  installedAt: string;
}

export type AppInstallResult =
  | { ok: true; app: InstalledApp }
  | { ok: false; error: 'bad_slug' | 'bad_name' | 'unsupported_platform' | 'helper_missing' | 'install_failed'; message: string };

// ── Pure parts ────────────────────────────────────────────────────────────────

/** Same rule as the cloud-api app_slug column (migration 161): letter first, [a-z0-9], 16 max. */
const SLUG = /^[a-z][a-z0-9]{0,15}$/;

export function isValidAppSlug(slug: unknown): slug is string {
  return typeof slug === 'string' && SLUG.test(slug);
}

/** macOS CFBundleIdentifier and Windows AppUserModelID for one app. */
export function appBundleId(slug: string): string {
  if (!isValidAppSlug(slug)) throw new Error(`bad app slug: ${slug}`);
  return `com.allternit.app.${slug}`;
}

export const appUserModelId = appBundleId;

export function appDeepLink(slug: string): string {
  if (!isValidAppSlug(slug)) throw new Error(`bad app slug: ${slug}`);
  return `allternit://app/${slug}`;
}

/**
 * A display name safe as a file name on every OS: no path separators, no
 * characters Windows refuses, no control characters, no leading dot, 64 max.
 * Returns null when nothing usable is left.
 */
export function safeAppName(name: unknown): string | null {
  if (typeof name !== 'string') return null;
  const cleaned = name
    // eslint-disable-next-line no-control-regex
    .replace(/[\u0000-\u001f\u007f/\\:*?"<>|]/g, ' ')
    .replace(/\s+/g, ' ')
    .trim()
    .replace(/^\.+/, '')
    .slice(0, 64)
    .trim();
  if (!cleaned) return null;
  // Windows reserved device names.
  if (/^(con|prn|aux|nul|com\d|lpt\d)$/i.test(cleaned)) return `${cleaned} App`;
  return cleaned;
}

function xmlEscape(value: string): string {
  return value.replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;').replace(/"/g, '&quot;');
}

export const HELPER_EXECUTABLE = 'AllternitAppHelper';

export function macInfoPlist(opts: { slug: string; name: string; appUrl: string; menuBar: boolean; hasIcon: boolean }): string {
  const s = (v: string) => `<string>${xmlEscape(v)}</string>`;
  const entries: Array<[string, string]> = [
    ['CFBundleIdentifier', s(appBundleId(opts.slug))],
    ['CFBundleName', s(opts.name)],
    ['CFBundleDisplayName', s(opts.name)],
    ['CFBundleExecutable', s(HELPER_EXECUTABLE)],
    ['CFBundlePackageType', s('APPL')],
    ['CFBundleShortVersionString', s('1.0')],
    ['CFBundleVersion', s('1')],
    ['CFBundleInfoDictionaryVersion', s('6.0')],
    ['LSMinimumSystemVersion', s('12.0')],
    ['LSApplicationCategoryType', s('public.app-category.productivity')],
    ['NSHighResolutionCapable', '<true/>'],
    ['NSAppTransportSecurity', '<dict><key>NSAllowsLocalNetworking</key><true/></dict>'],
    ['AllternitAppSlug', s(opts.slug)],
    ['AllternitAppURL', s(opts.appUrl)],
    ['AllternitMenuBar', opts.menuBar ? '<true/>' : '<false/>'],
  ];
  if (opts.hasIcon) entries.push(['CFBundleIconFile', s('AppIcon')]);
  const body = entries.map(([k, v]) => `  <key>${k}</key>\n  ${v}`).join('\n');
  return (
    '<?xml version="1.0" encoding="UTF-8"?>\n' +
    '<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">\n' +
    `<plist version="1.0">\n<dict>\n${body}\n</dict>\n</plist>\n`
  );
}

/** One Exec= argument, quoted per the Desktop Entry spec when it needs it. */
export function desktopExecArg(arg: string): string {
  const escaped = arg.replace(/%/g, '%%');
  if (!/[\s"'\\><~|&;$*?#()`]/.test(escaped)) return escaped;
  return `"${escaped.replace(/(["`$\\])/g, '\\$1')}"`;
}

/** Desktop Entry string values: no newlines, backslash escaped. */
function desktopValue(value: string): string {
  return value.replace(/\\/g, '\\\\').replace(/[\r\n\t]/g, ' ');
}

export function linuxDesktopEntry(opts: { slug: string; name: string; execPath: string; execArgs?: string[]; iconPath: string | null }): string {
  const exec = [opts.execPath, ...(opts.execArgs ?? []), appDeepLink(opts.slug)].map(desktopExecArg).join(' ');
  const lines = [
    '[Desktop Entry]',
    'Type=Application',
    'Version=1.0',
    `Name=${desktopValue(opts.name)}`,
    `Comment=${desktopValue(`${opts.name} (Allternit app)`)}`,
    `Exec=${exec}`,
    ...(opts.iconPath ? [`Icon=${desktopValue(opts.iconPath)}`] : []),
    'Terminal=false',
    'Categories=Utility;',
    `X-Allternit-App=${opts.slug}`,
  ];
  return `${lines.join('\n')}\n`;
}

export function linuxDesktopFileName(slug: string): string {
  if (!isValidAppSlug(slug)) throw new Error(`bad app slug: ${slug}`);
  return `allternit-app-${slug}.desktop`;
}

/** A Windows .ico holding one PNG image (Vista and later read PNG entries). */
export function pngToIco(png: Buffer, size: number): Buffer {
  const header = Buffer.alloc(6);
  header.writeUInt16LE(0, 0); // reserved
  header.writeUInt16LE(1, 2); // type: icon
  header.writeUInt16LE(1, 4); // one image
  const entry = Buffer.alloc(16);
  const dim = size >= 256 ? 0 : size; // 0 means 256
  entry.writeUInt8(dim, 0);
  entry.writeUInt8(dim, 1);
  entry.writeUInt8(0, 2); // palette
  entry.writeUInt8(0, 3); // reserved
  entry.writeUInt16LE(1, 4); // planes
  entry.writeUInt16LE(32, 6); // bits per pixel
  entry.writeUInt32LE(png.length, 8);
  entry.writeUInt32LE(6 + 16, 12);
  return Buffer.concat([header, entry, png]);
}

/** iconutil's iconset file names and pixel sizes. */
export const MAC_ICONSET: ReadonlyArray<readonly [string, number]> = [
  ['icon_16x16.png', 16], ['icon_16x16@2x.png', 32],
  ['icon_32x32.png', 32], ['icon_32x32@2x.png', 64],
  ['icon_128x128.png', 128], ['icon_128x128@2x.png', 256],
  ['icon_256x256.png', 256], ['icon_256x256@2x.png', 512],
  ['icon_512x512.png', 512], ['icon_512x512@2x.png', 1024],
];

/** Only https icons or inline PNG/JPEG; never file:, http: or anything else. */
export function isAllowedIconUrl(url: unknown): url is string {
  if (typeof url !== 'string' || url.length > 2_000_000) return false;
  if (/^data:image\/(png|jpeg);base64,/i.test(url)) return true;
  try {
    return new URL(url).protocol === 'https:';
  } catch {
    return false;
  }
}

// ── Side effects ──────────────────────────────────────────────────────────────

export interface AppLauncherDeps {
  platform: NodeJS.Platform;
  home: string;
  /** Electron userData: the installed-apps registry and Windows icons live here. */
  userData: string;
  /** Windows Roaming AppData (Start menu lives under it). */
  appData: string;
  /** The packaged helper binary, or null when it is not in this build. */
  helperBinary: string | null;
  /** The Allternit executable a Windows/Linux launcher runs. */
  execPath: string;
  /** Extra args before the deep link (dev builds pass the app path). */
  execArgs: string[];
  /** Icon bytes → PNG resized to a square of `size` px; null if it cannot decode. */
  resizePng: (bytes: Buffer, size: number) => Buffer | null;
  writeShortcut: (lnkPath: string, opts: { target: string; args: string; appUserModelId: string; icon?: string; description: string }) => boolean;
  /** Called on Linux when no handler owns x-scheme-handler/allternit. */
  registerLinuxScheme: () => Promise<void>;
  fetchImpl?: typeof fetch;
  log?: { info: (...a: unknown[]) => void; warn: (...a: unknown[]) => void };
}

const MAX_ICON_BYTES = 2 * 1024 * 1024;
const REGISTRY_FILE = 'installed-apps.json';

function run(cmd: string, args: string[], timeout = 20_000): Promise<{ ok: boolean; stdout: string; stderr: string }> {
  return new Promise((resolve) => {
    execFile(cmd, args, { timeout }, (error, stdout, stderr) =>
      resolve({ ok: !error, stdout: String(stdout ?? ''), stderr: String(stderr ?? (error?.message ?? '')) }),
    );
  });
}

async function exists(p: string): Promise<boolean> {
  try {
    await fs.access(p);
    return true;
  } catch {
    return false;
  }
}

export function createAppLaunchers(deps: AppLauncherDeps) {
  const log = deps.log ?? { info: () => {}, warn: () => {} };
  const fetchImpl = deps.fetchImpl ?? fetch;
  const registryPath = path.join(deps.userData, REGISTRY_FILE);
  const macRoot = path.join(deps.home, 'Applications', 'Allternit Apps');
  const winStartMenu = path.join(deps.appData, 'Microsoft', 'Windows', 'Start Menu', 'Programs', 'Allternit Apps');
  const linuxApps = path.join(deps.home, '.local', 'share', 'applications');
  const linuxIcons = path.join(deps.home, '.local', 'share', 'icons', 'allternit-apps');
  const winIcons = path.join(deps.userData, 'app-icons');

  async function readRegistry(): Promise<Record<string, InstalledApp>> {
    try {
      const parsed = JSON.parse(await fs.readFile(registryPath, 'utf8')) as Record<string, InstalledApp>;
      return parsed && typeof parsed === 'object' ? parsed : {};
    } catch {
      return {};
    }
  }

  async function writeRegistry(reg: Record<string, InstalledApp>): Promise<void> {
    await fs.mkdir(path.dirname(registryPath), { recursive: true });
    const tmp = `${registryPath}.tmp`;
    await fs.writeFile(tmp, JSON.stringify(reg, null, 2));
    await fs.rename(tmp, registryPath);
  }

  async function loadIcon(iconUrl: string | null | undefined): Promise<Buffer | null> {
    if (!iconUrl || !isAllowedIconUrl(iconUrl)) return null;
    try {
      if (iconUrl.startsWith('data:')) {
        const bytes = Buffer.from(iconUrl.slice(iconUrl.indexOf(',') + 1), 'base64');
        return bytes.length <= MAX_ICON_BYTES ? bytes : null;
      }
      const res = await fetchImpl(iconUrl, { redirect: 'follow', signal: AbortSignal.timeout(10_000) });
      if (!res.ok) return null;
      const bytes = Buffer.from(await res.arrayBuffer());
      return bytes.length <= MAX_ICON_BYTES ? bytes : null;
    } catch (error) {
      log.warn('[AppLaunchers] icon fetch failed', error);
      return null;
    }
  }

  async function findMacBundle(slug: string): Promise<string | null> {
    let entries: string[];
    try {
      entries = await fs.readdir(macRoot);
    } catch {
      return null;
    }
    const marker = `<string>${appBundleId(slug)}</string>`;
    for (const entry of entries) {
      if (!entry.endsWith('.app') || entry.startsWith('.')) continue;
      try {
        const plist = await fs.readFile(path.join(macRoot, entry, 'Contents', 'Info.plist'), 'utf8');
        if (plist.includes(marker)) return path.join(macRoot, entry);
      } catch {
        /* not ours */
      }
    }
    return null;
  }

  const LSREGISTER = '/System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/Support/lsregister';

  async function installMac(req: AppInstallRequest, name: string, appUrl: string, icon: Buffer | null): Promise<InstalledApp> {
    if (!deps.helperBinary || !(await exists(deps.helperBinary))) {
      throw Object.assign(new Error('The app helper is not in this build.'), { code: 'helper_missing' });
    }
    await fs.mkdir(macRoot, { recursive: true });
    const finalPath = path.join(macRoot, `${name}.app`);
    const staging = path.join(macRoot, `.${req.slug}.staging.app`);
    await fs.rm(staging, { recursive: true, force: true });
    const contents = path.join(staging, 'Contents');
    await fs.mkdir(path.join(contents, 'MacOS'), { recursive: true });
    await fs.mkdir(path.join(contents, 'Resources'), { recursive: true });
    const exe = path.join(contents, 'MacOS', HELPER_EXECUTABLE);
    await fs.copyFile(deps.helperBinary, exe);
    await fs.chmod(exe, 0o755);
    await fs.writeFile(path.join(contents, 'PkgInfo'), 'APPL????');

    let hasIcon = false;
    if (icon) {
      const iconset = path.join(staging, '..', `.${req.slug}.iconset`);
      await fs.rm(iconset, { recursive: true, force: true });
      await fs.mkdir(iconset, { recursive: true });
      let complete = true;
      for (const [file, size] of MAC_ICONSET) {
        const png = deps.resizePng(icon, size);
        if (!png) { complete = false; break; }
        await fs.writeFile(path.join(iconset, file), png);
      }
      if (complete) {
        const r = await run('iconutil', ['-c', 'icns', iconset, '-o', path.join(contents, 'Resources', 'AppIcon.icns')]);
        hasIcon = r.ok;
        if (!r.ok) log.warn('[AppLaunchers] iconutil failed', r.stderr);
      }
      await fs.rm(iconset, { recursive: true, force: true });
    }
    await fs.writeFile(
      path.join(contents, 'Info.plist'),
      macInfoPlist({ slug: req.slug, name, appUrl, menuBar: req.menuBar === true, hasIcon }),
    );

    const sign = await run('codesign', ['--force', '--sign', '-', '--timestamp=none', staging], 60_000);
    if (!sign.ok) throw new Error(`codesign failed: ${sign.stderr.trim()}`);
    await run('xattr', ['-dr', 'com.apple.quarantine', staging]);

    // Replace the old bundle (it may have another name after a rename).
    const previous = await findMacBundle(req.slug);
    if (previous) {
      await run('osascript', ['-e', `quit app id "${appBundleId(req.slug)}"`], 5_000);
      await fs.rm(previous, { recursive: true, force: true });
    }
    await fs.rm(finalPath, { recursive: true, force: true });
    await fs.rename(staging, finalPath);
    await run(LSREGISTER, ['-f', finalPath]);
    return {
      slug: req.slug, name, tier: req.tier, menuBar: req.menuBar === true, kind: 'mac-dock',
      launcherPath: finalPath, iconPath: hasIcon ? path.join(finalPath, 'Contents', 'Resources', 'AppIcon.icns') : null,
      installedAt: new Date().toISOString(),
    };
  }

  async function installWindows(req: AppInstallRequest, name: string, icon: Buffer | null, previous?: InstalledApp): Promise<InstalledApp> {
    let iconPath: string | null = null;
    const png = icon ? deps.resizePng(icon, 256) : null;
    if (png) {
      await fs.mkdir(winIcons, { recursive: true });
      iconPath = path.join(winIcons, `${req.slug}.ico`);
      await fs.writeFile(iconPath, pngToIco(png, 256));
    }
    await fs.mkdir(winStartMenu, { recursive: true });
    const lnk = path.join(winStartMenu, `${name}.lnk`);
    if (previous && previous.launcherPath !== lnk) await fs.rm(previous.launcherPath, { force: true });
    const args = [...deps.execArgs.map((a) => `"${a}"`), appDeepLink(req.slug)].join(' ');
    const ok = deps.writeShortcut(lnk, {
      target: deps.execPath,
      args,
      appUserModelId: appUserModelId(req.slug),
      ...(iconPath ? { icon: iconPath } : {}),
      description: name,
    });
    if (!ok) throw new Error('Could not write the Start menu shortcut.');
    return {
      slug: req.slug, name, tier: req.tier, menuBar: false, kind: 'windows-start-menu',
      launcherPath: lnk, iconPath, installedAt: new Date().toISOString(),
    };
  }

  async function installLinux(req: AppInstallRequest, name: string, icon: Buffer | null): Promise<InstalledApp> {
    let iconPath: string | null = null;
    const png = icon ? deps.resizePng(icon, 256) : null;
    if (png) {
      await fs.mkdir(linuxIcons, { recursive: true });
      iconPath = path.join(linuxIcons, `allternit-app-${req.slug}.png`);
      await fs.writeFile(iconPath, png);
    }
    await fs.mkdir(linuxApps, { recursive: true });
    const file = path.join(linuxApps, linuxDesktopFileName(req.slug));
    await fs.writeFile(
      file,
      linuxDesktopEntry({ slug: req.slug, name, execPath: deps.execPath, execArgs: deps.execArgs, iconPath }),
      { mode: 0o755 },
    );
    await run('update-desktop-database', [linuxApps], 10_000);
    const handler = await run('xdg-mime', ['query', 'default', 'x-scheme-handler/allternit'], 5_000);
    if (!handler.ok || !handler.stdout.trim()) await deps.registerLinuxScheme();
    return {
      slug: req.slug, name, tier: req.tier, menuBar: false, kind: 'linux-desktop-entry',
      launcherPath: file, iconPath, installedAt: new Date().toISOString(),
    };
  }

  async function install(req: AppInstallRequest, appUrl: string): Promise<AppInstallResult> {
    if (!isValidAppSlug(req?.slug)) return { ok: false, error: 'bad_slug', message: 'App slug must be 1-16 letters or digits, letter first.' };
    const name = safeAppName(req.name);
    if (!name) return { ok: false, error: 'bad_name', message: 'App name is empty.' };
    const tier: AppTier = req.tier === 'full' || req.tier === 'app' ? req.tier : 'browser';
    const clean: AppInstallRequest = { slug: req.slug, name, tier, menuBar: req.menuBar === true, iconUrl: req.iconUrl ?? null };
    try {
      const icon = await loadIcon(clean.iconUrl);
      const reg = await readRegistry();
      let app: InstalledApp;
      if (deps.platform === 'darwin') app = await installMac(clean, name, appUrl, icon);
      else if (deps.platform === 'win32') app = await installWindows(clean, name, icon, reg[clean.slug]);
      else if (deps.platform === 'linux') app = await installLinux(clean, name, icon);
      else return { ok: false, error: 'unsupported_platform', message: `No launchers on ${deps.platform}.` };
      reg[clean.slug] = app;
      await writeRegistry(reg);
      log.info('[AppLaunchers] installed', clean.slug, app.launcherPath);
      return { ok: true, app };
    } catch (error) {
      const code = (error as { code?: string }).code === 'helper_missing' ? 'helper_missing' : 'install_failed';
      log.warn('[AppLaunchers] install failed', clean.slug, error);
      return { ok: false, error: code, message: error instanceof Error ? error.message : String(error) };
    }
  }

  async function uninstall(slug: string): Promise<{ ok: boolean; removed: boolean }> {
    if (!isValidAppSlug(slug)) return { ok: false, removed: false };
    const reg = await readRegistry();
    const entry = reg[slug];
    let removed = false;
    if (deps.platform === 'darwin') {
      const bundle = (await findMacBundle(slug)) ?? entry?.launcherPath ?? null;
      if (bundle && (await exists(bundle))) {
        await run('osascript', ['-e', `quit app id "${appBundleId(slug)}"`], 5_000);
        await run(LSREGISTER, ['-u', bundle]);
        await fs.rm(bundle, { recursive: true, force: true });
        removed = true;
      }
    } else if (deps.platform === 'win32') {
      if (entry?.launcherPath) { await fs.rm(entry.launcherPath, { force: true }); removed = true; }
      await fs.rm(path.join(winIcons, `${slug}.ico`), { force: true });
    } else if (deps.platform === 'linux') {
      const file = path.join(linuxApps, linuxDesktopFileName(slug));
      if (await exists(file)) { await fs.rm(file, { force: true }); removed = true; }
      await fs.rm(path.join(linuxIcons, `allternit-app-${slug}.png`), { force: true });
      await run('update-desktop-database', [linuxApps], 10_000);
    }
    if (entry) {
      delete reg[slug];
      await writeRegistry(reg);
      removed = true;
    }
    return { ok: true, removed };
  }

  /** The registry entry, but only while its launcher is still on disk. */
  async function get(slug: string): Promise<InstalledApp | null> {
    if (!isValidAppSlug(slug)) return null;
    const entry = (await readRegistry())[slug];
    if (!entry) return null;
    if (deps.platform === 'darwin') {
      const bundle = await findMacBundle(slug);
      return bundle ? { ...entry, launcherPath: bundle } : null;
    }
    return (await exists(entry.launcherPath)) ? entry : null;
  }

  /** macOS only: launch the app's own helper. False when it is not installed. */
  async function launchMacHelper(slug: string, appUrl: string, handoff?: string | null): Promise<boolean> {
    if (deps.platform !== 'darwin') return false;
    const bundle = await findMacBundle(slug);
    if (!bundle) return false;
    const args = ['-a', bundle, '--args', '--url', appUrl];
    if (handoff && /^[A-Za-z0-9._~-]{8,512}$/.test(handoff)) args.push('--token', handoff);
    const entry = (await readRegistry())[slug];
    if (entry?.menuBar) args.push('--menubar');
    const r = await run('open', args, 15_000);
    if (!r.ok) log.warn('[AppLaunchers] open failed', slug, r.stderr);
    return r.ok;
  }

  return { install, uninstall, get, launchMacHelper, paths: { macRoot, winStartMenu, linuxApps, linuxIcons, registryPath } };
}

export type AppLaunchers = ReturnType<typeof createAppLaunchers>;
