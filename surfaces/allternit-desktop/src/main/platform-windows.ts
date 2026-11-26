/**
 * Allternit Desktop — Windows Platform Integration
 *
 * Windows-specific functionality: system tray, protocol handler,
 * auto-launch, native notifications, and firewall helpers.
 */

import { app, Tray, Menu, Notification, nativeImage, BrowserWindow, type NativeImage, type Rectangle } from 'electron';
import { execFile } from 'node:child_process';
import * as fs from 'node:fs';
import * as path from 'node:path';
import log from 'electron-log';

const PROTOCOL_SCHEME = 'allternit';
const REGISTRY_KEY_PATH = 'Software\\Microsoft\\Windows\\CurrentVersion\\Run';
const APP_NAME = 'Allternit Desktop';

let tray: Tray | null = null;

/**
 * Create and configure the Windows system tray icon with a context menu.
 */
export function setupWindowsTray(
  handlers: {
    onShow: () => void;
    onQuit: () => void;
    onOpenSettings: () => void;
  },
): Tray {
  if (tray) return tray;

  const iconPath = resolveTrayIcon();
  tray = new Tray(iconPath);
  tray.setToolTip(APP_NAME);

  const contextMenu = Menu.buildFromTemplate([
    {
      label: 'Show Allternit',
      click: handlers.onShow,
    },
    { type: 'separator' },
    {
      label: 'Settings',
      click: handlers.onOpenSettings,
    },
    { type: 'separator' },
    {
      label: 'Quit',
      click: handlers.onQuit,
    },
  ]);

  tray.setContextMenu(contextMenu);
  tray.on('double-click', handlers.onShow);

  log.info('[PlatformWindows] System tray initialized');
  return tray;
}

/**
 * Register the allternit:// protocol handler in the Windows registry.
 * This allows deep links from browsers and other apps.
 */
export async function registerProtocolHandler(): Promise<void> {
  if (process.platform !== 'win32') return;

  const exePath = process.execPath;
  const regCommands = [
    // Protocol key
    `add "HKCU\\Software\\Classes\\${PROTOCOL_SCHEME}" /ve /d "URL:Allternit Protocol" /f`,
    `add "HKCU\\Software\\Classes\\${PROTOCOL_SCHEME}" /v "URL Protocol" /d "" /f`,
    // Default icon
    `add "HKCU\\Software\\Classes\\${PROTOCOL_SCHEME}\\DefaultIcon" /ve /d "${exePath},0" /f`,
    // Command
    `add "HKCU\\Software\\Classes\\${PROTOCOL_SCHEME}\\shell\\open\\command" /ve /d "\\"${exePath}\\" \\"%1\\"" /f`,
  ];

  for (const cmd of regCommands) {
    await runRegCommand(cmd);
  }

  log.info('[PlatformWindows] Protocol handler registered');
}

/**
 * Add a registry entry so Allternit Desktop launches at Windows startup.
 */
export async function setupAutoLaunch(): Promise<void> {
  if (process.platform !== 'win32') return;

  const exePath = process.execPath;
  const cmd = `add "HKCU\\${REGISTRY_KEY_PATH}" /v "${APP_NAME}" /d "\\"${exePath}\\" --minimized" /f`;
  await runRegCommand(cmd);

  log.info('[PlatformWindows] Auto-launch enabled');
}

/**
 * Remove the auto-launch registry entry.
 */
export async function disableAutoLaunch(): Promise<void> {
  if (process.platform !== 'win32') return;

  const cmd = `delete "HKCU\\${REGISTRY_KEY_PATH}" /v "${APP_NAME}" /f`;
  await runRegCommand(cmd);

  log.info('[PlatformWindows] Auto-launch disabled');
}

/**
 * Prompt the user to allow incoming connections through Windows Firewall
 * for the local backend server.
 */
export async function requestFirewallException(): Promise<boolean> {
  if (process.platform !== 'win32') return false;

  const ruleName = 'Allternit Backend';
  const exePath = process.execPath;

  return new Promise((resolve) => {
    execFile(
      'netsh',
      [
        'advfirewall', 'firewall', 'add', 'rule',
        `name=${ruleName}`,
        'dir=in',
        'action=allow',
        `program=${exePath}`,
        'profile=private',
      ],
      { windowsHide: true },
      (error) => {
        if (error) {
          log.warn('[PlatformWindows] Firewall exception request failed:', error.message);
          resolve(false);
        } else {
          log.info('[PlatformWindows] Firewall exception added');
          resolve(true);
        }
      },
    );
  });
}

/**
 * Show a native Windows toast notification.
 */
export function showWindowsNotification(title: string, body: string): void {
  if (!Notification.isSupported()) {
    log.warn('[PlatformWindows] Notifications not supported');
    return;
  }

  const notification = new Notification({
    title,
    body,
    icon: resolveTrayIcon(),
    silent: false,
  });

  notification.show();
}

/**
 * Returns true if the current process is running on Windows.
 */
export function isWindowsPlatform(): boolean {
  return process.platform === 'win32';
}

/**
 * Clean up Windows-specific resources (tray icon, registry watchers).
 */
export function cleanupWindowsPlatform(): void {
  if (tray) {
    tray.destroy();
    tray = null;
    log.info('[PlatformWindows] Tray destroyed');
  }
}

// ── Internal helpers ────────────────────────────────────────────────────

function resolveTrayIcon(): NativeImage {
  const iconPaths = [
    path.join(process.resourcesPath ?? '', 'build', 'icon.ico'),
    path.join(app.getAppPath(), 'build', 'icon.ico'),
    path.join(app.getAppPath(), 'static', 'icon.ico'),
  ];

  for (const iconPath of iconPaths) {
    if (fs.existsSync(iconPath)) {
      return nativeImage.createFromPath(iconPath);
    }
  }

  // Fallback: create a 16x16 empty image
  log.warn('[PlatformWindows] No tray icon found, using empty image');
  return nativeImage.createEmpty();
}

function runRegCommand(args: string): Promise<void> {
  return new Promise((resolve, reject) => {
    execFile('reg', args.split(/\s+/), { windowsHide: true }, (error) => {
      if (error) {
        log.warn(`[PlatformWindows] reg ${args} failed:`, error.message);
        reject(error);
      } else {
        resolve();
      }
    });
  });
}

// ── Island pill placement (program "Island bar + Team map", lane 29) ────

/** Gap between the top of the work area (a top taskbar ends there) and the pill. */
const WINDOWS_PILL_GAP = 12;

/**
 * The pill sits top-centre of the active display's work area (the display
 * under the cursor, picked by the caller). The window is `skipTaskbar`
 * (floatingPanelWindowOptions), so it never shows as a taskbar button.
 */
export function windowsIslandPillBounds(
  display: { workArea: Rectangle },
  size: { width: number; height: number },
): Rectangle {
  const { workArea } = display;
  const width = Math.max(1, Math.min(size.width, workArea.width - 32));
  return {
    x: Math.round(workArea.x + (workArea.width - width) / 2),
    y: Math.round(workArea.y + WINDOWS_PILL_GAP),
    width: Math.round(width),
    height: Math.round(Math.min(size.height, workArea.height - WINDOWS_PILL_GAP)),
  };
}
