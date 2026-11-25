/**
 * "Open as app": one BrowserWindow per installed app inside the running
 * Desktop (D18: one Desktop runtime). Used on Windows and Linux, and on
 * macOS when the per-app helper is missing. It loads the app shell route
 * the app link it is given (else /app/<slug> on the origin the Desktop UI
 * uses), with a one-time handoff token when cloud-api minted one. On Windows the window takes the app's own AppUserModelID,
 * so it groups under the app's Start menu shortcut, not under Allternit.
 */
import { BrowserWindow, type WebContents } from 'electron';
import { appShellUrl } from './app-link.js';
import { appUserModelId } from './app-launchers.js';

export interface AppWindowDeps {
  preloadPath: string;
  platformUrl: () => string;
  installNavigationGuard: (contents: WebContents) => void;
  openExternal: (url: string) => void;
  execPath: string;
  execArgs: string[];
}

export interface AppWindowTarget {
  slug: string;
  /** Window title. Falls back to the page title when unknown. */
  name?: string | null;
  /** Windows .ico for the taskbar group. */
  iconPath?: string | null;
  /** Full URL to load (the app link, with ?handoff= when minted). Falls back to <platform>/app/<slug>. */
  url?: string | null;
}

const windows = new Map<string, BrowserWindow>();

export function openAppWindow(target: AppWindowTarget, deps: AppWindowDeps): BrowserWindow {
  const url = target.url || appShellUrl(deps.platformUrl(), target.slug);
  const existing = windows.get(target.slug);
  if (existing && !existing.isDestroyed()) {
    if (existing.isMinimized()) existing.restore();
    existing.show();
    existing.focus();
    return existing;
  }

  const isMac = process.platform === 'darwin';
  const win = new BrowserWindow({
    width: 1100,
    height: 760,
    minWidth: 480,
    minHeight: 360,
    title: target.name || 'Allternit App',
    titleBarStyle: isMac ? 'hiddenInset' : 'default',
    trafficLightPosition: { x: 16, y: 16 },
    show: false,
    backgroundColor: '#FFFFFF',
    webPreferences: {
      preload: deps.preloadPath,
      contextIsolation: true,
      nodeIntegration: false,
      sandbox: true,
    },
  });
  windows.set(target.slug, win);

  if (process.platform === 'win32') {
    const relaunch = [deps.execPath, ...deps.execArgs].map((a) => `"${a}"`).join(' ');
    win.setAppDetails({
      appId: appUserModelId(target.slug),
      ...(target.iconPath ? { appIconPath: target.iconPath, appIconIndex: 0 } : {}),
      relaunchCommand: `${relaunch} allternit://app/${target.slug}`,
      relaunchDisplayName: target.name || target.slug,
    });
  }

  if (target.name) {
    const title = target.name;
    win.on('page-title-updated', (event) => {
      event.preventDefault();
      win.setTitle(title);
    });
  }

  deps.installNavigationGuard(win.webContents);
  win.webContents.setWindowOpenHandler(({ url: next }) => {
    deps.openExternal(next);
    return { action: 'deny' };
  });
  win.once('ready-to-show', () => win.show());
  win.on('closed', () => {
    if (windows.get(target.slug) === win) windows.delete(target.slug);
  });
  void win.loadURL(url);
  return win;
}

/** Closes the app's window, if open (used on uninstall). */
export function closeAppWindow(slug: string): void {
  const win = windows.get(slug);
  if (win && !win.isDestroyed()) win.close();
  windows.delete(slug);
}
