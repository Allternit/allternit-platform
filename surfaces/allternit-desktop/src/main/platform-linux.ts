/**
 * Allternit Desktop — Linux Platform Integration
 *
 * Linux-specific functionality: AppIndicator/system tray,
 * XDG autostart, desktop notifications, and xdg-open integration.
 */

import { app, Tray, Menu, Notification, nativeImage, type NativeImage, type Rectangle } from 'electron';
import { execFile } from 'node:child_process';
import * as fs from 'node:fs';
import * as path from 'node:path';
import log from 'electron-log';

const AUTOSTART_DIR = path.join(
  process.env.XDG_CONFIG_HOME ?? path.join(process.env.HOME ?? '~', '.config'),
  'autostart',
);
const DESKTOP_ENTRY_NAME = 'allternit-desktop.desktop';

let tray: Tray | null = null;

/**
 * Create and configure the Linux system tray icon.
 */
export function setupLinuxTray(
  handlers: {
    onShow: () => void;
    onQuit: () => void;
    onOpenSettings: () => void;
  },
): Tray {
  if (tray) return tray;

  const iconPath = resolveTrayIcon();
  tray = new Tray(iconPath);
  tray.setToolTip('Allternit Desktop');

  const contextMenu = Menu.buildFromTemplate([
    { label: 'Show Allternit', click: handlers.onShow },
    { type: 'separator' },
    { label: 'Settings', click: handlers.onOpenSettings },
    { type: 'separator' },
    { label: 'Quit', click: handlers.onQuit },
  ]);

  tray.setContextMenu(contextMenu);
  tray.on('click', handlers.onShow);

  log.info('[PlatformLinux] System tray initialized');
  return tray;
}

/**
 * Register protocol handler via xdg-mime on Linux.
 */
export async function registerProtocolHandler(): Promise<void> {
  const desktopFilePath = resolveDesktopFile();
  if (!desktopFilePath) {
    log.warn('[PlatformLinux] Desktop file not found, skipping protocol registration');
    return;
  }

  await new Promise<void>((resolve) => {
    execFile(
      'xdg-mime',
      ['default', DESKTOP_ENTRY_NAME, 'x-scheme-handler/allternit'],
      { timeout: 5000 },
      (error) => {
        if (error) {
          log.warn('[PlatformLinux] xdg-mime failed:', error.message);
        }
        resolve();
      },
    );
  });

  log.info('[PlatformLinux] Protocol handler registered');
}

/**
 * Create an XDG autostart desktop entry.
 */
export async function setupAutoLaunch(): Promise<void> {
  fs.mkdirSync(AUTOSTART_DIR, { recursive: true });

  const exePath = process.execPath;
  const content = [
    '[Desktop Entry]',
    'Type=Application',
    'Name=Allternit Desktop',
    `Exec=${exePath} --minimized`,
    'Terminal=false',
    'X-GNOME-Autostart-enabled=true',
    'Comment=Allternit AI Platform',
  ].join('\n');

  const filePath = path.join(AUTOSTART_DIR, DESKTOP_ENTRY_NAME);
  fs.writeFileSync(filePath, content, 'utf-8');

  log.info('[PlatformLinux] Auto-launch enabled via XDG autostart');
}

/**
 * Remove the XDG autostart desktop entry.
 */
export async function disableAutoLaunch(): Promise<void> {
  const filePath = path.join(AUTOSTART_DIR, DESKTOP_ENTRY_NAME);
  if (fs.existsSync(filePath)) {
    fs.unlinkSync(filePath);
  }
  log.info('[PlatformLinux] Auto-launch disabled');
}

/**
 * Show a desktop notification via Electron (libnotify on Linux).
 */
export function showLinuxNotification(title: string, body: string): void {
  if (!Notification.isSupported()) return;

  const notification = new Notification({ title, body, silent: false });
  notification.show();
}

/**
 * Returns true if the current process is running on Linux.
 */
export function isLinuxPlatform(): boolean {
  return process.platform === 'linux';
}

/**
 * Clean up Linux-specific resources.
 */
export function cleanupLinuxPlatform(): void {
  if (tray) {
    tray.destroy();
    tray = null;
  }
}

// ── Internal helpers ────────────────────────────────────────────────────

function resolveTrayIcon(): NativeImage {
  const iconPaths = [
    path.join(app.getAppPath(), 'build', 'icon.png'),
    path.join(app.getAppPath(), 'static', 'icon.png'),
  ];

  for (const iconPath of iconPaths) {
    if (fs.existsSync(iconPath)) {
      return nativeImage.createFromPath(iconPath);
    }
  }

  log.warn('[PlatformLinux] No tray icon found');
  return nativeImage.createEmpty();
}

function resolveDesktopFile(): string | null {
  const candidates = [
    path.join(app.getAppPath(), 'build', DESKTOP_ENTRY_NAME),
    `/usr/share/applications/${DESKTOP_ENTRY_NAME}`,
    path.join(process.env.HOME ?? '~', '.local', 'share', 'applications', DESKTOP_ENTRY_NAME),
  ];

  for (const candidate of candidates) {
    if (fs.existsSync(candidate)) return candidate;
  }
  return null;
}

// ── Island pill placement (program "Island bar + Team map", lane 29) ────

/**
 * `window`: a top-centre always-on-top pill window (X11, and XWayland via
 * `--ozone-platform=x11`). `tray`: native Wayland, where the compositor
 * ignores client placement and most ignore always-on-top, so the job shows
 * as a dot in the tray menu with "Open Island bar" instead (PLAN D14).
 * `backend` is hud-windowing.ts `linuxOzoneBackend()`.
 */
export function linuxIslandPillMode(backend: 'wayland' | 'x11'): 'window' | 'tray' {
  return backend === 'wayland' ? 'tray' : 'window';
}

/** Gap between the top of the work area (GNOME/KDE top panels end there) and the pill. */
const LINUX_PILL_GAP = 10;

/** Top-centre of the active display's work area (X11 only; see linuxIslandPillMode). */
export function linuxIslandPillBounds(
  display: { workArea: Rectangle },
  size: { width: number; height: number },
): Rectangle {
  const { workArea } = display;
  const width = Math.max(1, Math.min(size.width, workArea.width - 32));
  return {
    x: Math.round(workArea.x + (workArea.width - width) / 2),
    y: Math.round(workArea.y + LINUX_PILL_GAP),
    width: Math.round(width),
    height: Math.round(Math.min(size.height, workArea.height - LINUX_PILL_GAP)),
  };
}
