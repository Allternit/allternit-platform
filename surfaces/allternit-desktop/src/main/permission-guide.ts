/**
 * Allternit Permission Guide
 *
 * Cross-platform permission detection + native/Electron overlay guide.
 *
 * macOS:   Electron host requests; macOS attributes grants to Allternit Desktop
 * Windows: desktopCapturer real-capture test + Electron overlay
 * Linux:   desktopCapturer real-capture test + Electron overlay
 */

import { systemPreferences, desktopCapturer, app, BrowserWindow, shell, screen } from 'electron';
import { spawn, ChildProcess } from 'child_process';
import * as fs from 'node:fs';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { dirname } from 'node:path';
import log from 'electron-log';

const __dirname = dirname(fileURLToPath(import.meta.url));
const platform = process.platform;
const isMac = platform === 'darwin';
const isWin = platform === 'win32';
const isLinux = platform === 'linux';

// ─── Types ───────────────────────────────────────────────────────────────────

export interface PermissionStatus {
  accessibility: 'granted' | 'denied' | 'unknown' | 'not-applicable';
  screenRecording: 'granted' | 'denied' | 'unknown' | 'not-applicable';
}

export type PermissionPanel = 'accessibility' | 'screen-recording';

export interface PresentResult {
  success: boolean;
  alreadyGranted?: boolean;
  error?: string;
}

// ─── State ───────────────────────────────────────────────────────────────────

let activeProcess: ChildProcess | null = null;
let activePanel: PermissionPanel | null = null;
let activeOverlayWindow: BrowserWindow | null = null;
let overlayTrackingInterval: ReturnType<typeof setInterval> | null = null;

// In-memory cache so repeated callers (renderer hooks, tray refresh) don't
// hammer the system APIs / desktopCapturer every few seconds. The cache is
// intentionally short; permission changes are rare and detected on the next
// real user action or app activation.
let cachedStatus: PermissionStatus | null = null;
let cachedAt = 0;
const CACHE_TTL_MS = 30000;

export function invalidatePermissionCache(): void {
  cachedStatus = null;
  cachedAt = 0;
}

// ─── Detection ───────────────────────────────────────────────────────────────

export async function checkPermissions(): Promise<PermissionStatus> {
  const now = Date.now();
  if (cachedStatus && now - cachedAt < CACHE_TTL_MS) {
    log.debug('[PermissionGuide] Returning cached permission status:', cachedStatus);
    return cachedStatus;
  }

  log.info('[PermissionGuide] Checking permissions...');

  let status: PermissionStatus;
  // --- macOS ---
  if (isMac) {
    status = await checkMacOSPermissions();
  }
  // --- Windows ---
  else if (isWin) {
    status = await checkWindowsPermissions();
  }
  // --- Linux ---
  else if (isLinux) {
    status = await checkLinuxPermissions();
  }
  // Unknown platform
  else {
    log.warn('[PermissionGuide] Unknown platform:', platform);
    status = { accessibility: 'not-applicable', screenRecording: 'not-applicable' };
  }

  cachedStatus = status;
  cachedAt = now;
  return status;
}

async function checkMacOSPermissions(): Promise<PermissionStatus> {
  const accessibilityGranted = systemPreferences.isTrustedAccessibilityClient(false);
  const screenApiStatus = systemPreferences.getMediaAccessStatus('screen');
  let screenGranted = screenApiStatus === 'granted';

  if (!screenGranted) {
    screenGranted = await testScreenCapture();
  }

  return {
    screenRecording: screenGranted ? 'granted' : 'denied',
    accessibility: accessibilityGranted ? 'granted' : 'denied',
  };
}

async function checkWindowsPermissions(): Promise<PermissionStatus> {
  // Windows has no system-level Accessibility permission gate.
  // Screen recording works via desktopCapturer; some enterprise configs may block it.
  const screenGranted = await testScreenCapture();
  return {
    accessibility: 'not-applicable',
    screenRecording: screenGranted ? 'granted' : 'denied',
  };
}

async function checkLinuxPermissions(): Promise<PermissionStatus> {
  // Linux has no system-level Accessibility permission gate (AT-SPI2 is open).
  // Wayland may block screen capture via xdg-desktop-portal until user approves.
  const screenGranted = await testScreenCapture();
  return {
    accessibility: 'not-applicable',
    screenRecording: screenGranted ? 'granted' : 'denied',
  };
}

/**
 * Universal screen-capture test using Electron desktopCapturer.
 * Works on macOS, Windows, and Linux (X11 + Wayland with portal).
 */
async function testScreenCapture(): Promise<boolean> {
  try {
    const sources = await desktopCapturer.getSources({
      types: ['screen'],
      thumbnailSize: { width: 100, height: 100 },
    });

    if (sources.length === 0) {
      log.warn('[PermissionGuide] desktopCapturer returned zero sources');
      return false;
    }

    const thumb = sources[0].thumbnail;
    const size = thumb.getSize();

    if (size.width === 0 || size.height === 0) {
      log.warn('[PermissionGuide] Thumbnail has zero dimensions');
      return false;
    }

    const bitmap = thumb.toBitmap();
    let hasRGBContent = false;
    for (let i = 0; i < bitmap.length; i += 4) {
      if (bitmap[i] !== 0 || bitmap[i + 1] !== 0 || bitmap[i + 2] !== 0) {
        hasRGBContent = true;
        break;
      }
    }
    return hasRGBContent;
  } catch (err) {
    log.warn('[PermissionGuide] desktopCapturer test failed:', err);
    return false;
  }
}

// ─── Guide Orchestration ─────────────────────────────────────────────────────

export async function presentGuide(panel: PermissionPanel): Promise<PresentResult> {
  log.info(`[PermissionGuide] presentGuide called for panel: ${panel}`);

  // Headless computers (ALLTERNIT_HEADLESS=1) never open app windows; the
  // overlay would be the first thing a developer's screenshot shows.
  if (process.env.ALLTERNIT_HEADLESS === '1') {
    return { success: false, error: 'Permission guide is off on headless computers' };
  }

  // Idempotent: if already granted, don't spawn overlay
  log.info('[PermissionGuide] Checking current permission state before presenting...');
  const status = await checkPermissions();

  if (panel === 'accessibility' && status.accessibility === 'granted') {
    log.info('[PermissionGuide] Accessibility already granted — skipping overlay');
    return { success: true, alreadyGranted: true };
  }
  if (panel === 'screen-recording' && status.screenRecording === 'granted') {
    log.info('[PermissionGuide] Screen Recording already granted — skipping overlay');
    return { success: true, alreadyGranted: true };
  }

  // Dismiss any existing overlay first
  dismissGuide();

  if (isMac) {
    return presentMacOSGuide(panel);
  }

  if (isWin) {
    return presentWindowsGuide(panel);
  }

  if (isLinux) {
    return presentLinuxGuide(panel);
  }

  return { success: false, error: 'Permission guide not supported on this platform' };
}

// ─── macOS: Native Swift overlay ─────────────────────────────────────────────

async function presentMacOSGuide(panel: PermissionPanel): Promise<PresentResult> {
  try {
    activePanel = panel;
    if (panel === 'accessibility') {
      // `true` raises the native TCC prompt from the Electron main process, so
      // the privacy row belongs to com.allternit.desktop.
      systemPreferences.isTrustedAccessibilityClient(true);
      await shell.openExternal('x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility');
    } else {
      // A real host-owned capture attempt registers Allternit in the Screen
      // Recording pane. The embedded driver never prompts independently.
      await testScreenCapture();
      await shell.openExternal('x-apple.systempreferences:com.apple.preference.security?Privacy_ScreenCapture');
    }
    return { success: true };
  } catch (error) {
    log.error('[PermissionGuide] Failed to request host-owned permission:', error);
    activePanel = null;
    return { success: false, error: (error as Error).message };
  }
}

// ─── Windows: Electron overlay + ms-settings ─────────────────────────────────

async function presentWindowsGuide(panel: PermissionPanel): Promise<PresentResult> {
  log.info('[PermissionGuide] Presenting Windows guide via Electron overlay');

  // Open Windows Settings Privacy page
  const settingsUrl = panel === 'accessibility'
    ? 'ms-settings:easeofaccess'
    : 'ms-settings:privacy-apppermissions';
  shell.openExternal(settingsUrl);

  // Spawn Electron overlay window
  spawnElectronOverlay(panel, 'Windows Settings');
  return { success: true };
}

// ─── Linux: Electron overlay + xdg-open ──────────────────────────────────────

async function presentLinuxGuide(panel: PermissionPanel): Promise<PresentResult> {
  log.info('[PermissionGuide] Presenting Linux guide via Electron overlay');

  // Try to open the system settings
  const settingsUrls = panel === 'accessibility'
    ? ['settings://accessibility', 'gnome-control-center accessibility', 'xdg-open settings://accessibility']
    : ['settings://privacy', 'gnome-control-center privacy', 'xdg-open settings://privacy'];

  for (const url of settingsUrls) {
    if (!url.includes(' ')) {
      shell.openExternal(url);
      break;
    }
  }

  spawnElectronOverlay(panel, 'System Settings');
  return { success: true };
}

// ─── Electron Overlay (Windows/Linux) ────────────────────────────────────────

function spawnElectronOverlay(panel: PermissionPanel, settingsAppName: string) {
  const mainWindow = BrowserWindow.getAllWindows()[0];
  if (!mainWindow) {
    log.warn('[PermissionGuide] No main window found for overlay positioning');
    return;
  }

  const title = panel === 'accessibility' ? 'Accessibility' : 'Screen Recording';
  const htmlContent = `
    <!DOCTYPE html>
    <html>
    <head>
      <meta charset="UTF-8">
      <style>
        body {
          margin: 0; padding: 0; font-family: 'Allternit Sans', Inter, ui-sans-serif, system-ui, -apple-system, BlinkMacSystemFont, 'Segoe UI', Roboto, sans-serif;
          background: rgba(30, 30, 30, 0.95); color: #fff; border-radius: 12px;
          overflow: hidden; user-select: none; -webkit-app-region: drag;
          box-shadow: 0 20px 60px rgba(0,0,0,0.4);
        }
        .container { padding: 20px 24px; width: 460px; box-sizing: border-box; }
        .header { display: flex; align-items: center; gap: 10px; margin-bottom: 14px; }
        .icon { width: 32px; height: 32px; background: #f59e0b; border-radius: 8px;
          display: flex; align-items: center; justify-content: center; font-size: 16px; }
        h1 { margin: 0; font-size: 15px; font-weight: 600; }
        p { margin: 0 0 16px; font-size: 13px; line-height: 1.5; color: #ccc; }
        .steps { background: rgba(255,255,255,0.06); border-radius: 8px; padding: 14px 16px; margin-bottom: 16px; }
        .steps ol { margin: 0; padding-left: 18px; font-size: 12px; color: #ddd; line-height: 1.6; }
        .steps li { margin-bottom: 4px; }
        .back-btn {
          display: block; width: 100%; padding: 10px; border-radius: 8px; border: none;
          background: #3b82f6; color: #fff; font-size: 13px; font-weight: 500; cursor: pointer;
          -webkit-app-region: no-drag;
        }
        .back-btn:hover { background: #2563eb; }
      </style>
    </head>
    <body>
      <div class="container">
        <div class="header">
          <div class="icon">🔒</div>
          <h1>${title} Permission</h1>
        </div>
        <p>Allternit needs <strong>${title}</strong> permission to control your desktop and capture screenshots.</p>
        <div class="steps">
          <ol>
            <li>Open <strong>${settingsAppName}</strong> (already opened)</li>
            <li>Find the <strong>${title}</strong> section</li>
            <li>Enable permission for <strong>Allternit</strong></li>
            <li>Click <strong>Back</strong> below when done</li>
          </ol>
        </div>
        <button class="back-btn" onclick="window.close()">← Back to Allternit</button>
      </div>
    </body>
    </html>
  `;

  activeOverlayWindow = new BrowserWindow({
    width: 500,
    height: 280,
    frame: false,
    transparent: true,
    alwaysOnTop: true,
    skipTaskbar: true,
    resizable: false,
    minimizable: false,
    maximizable: false,
    closable: true,
    show: false,
    webPreferences: {
      contextIsolation: false,
      nodeIntegration: false,
    },
  });

  activeOverlayWindow.loadURL(`data:text/html;charset=utf-8,${encodeURIComponent(htmlContent)}`);
  activePanel = panel;

  activeOverlayWindow.once('ready-to-show', () => {
    positionOverlayNearMainWindow(mainWindow);
    activeOverlayWindow?.show();
  });

  activeOverlayWindow.on('closed', () => {
    activeOverlayWindow = null;
    activePanel = null;
    if (overlayTrackingInterval) {
      clearInterval(overlayTrackingInterval);
      overlayTrackingInterval = null;
    }
  });

  // Track main window position every 250ms
  overlayTrackingInterval = setInterval(() => {
    if (activeOverlayWindow && !activeOverlayWindow.isDestroyed() && mainWindow && !mainWindow.isDestroyed()) {
      positionOverlayNearMainWindow(mainWindow);
    }
  }, 250);
}

function positionOverlayNearMainWindow(mainWindow: BrowserWindow) {
  if (!activeOverlayWindow || activeOverlayWindow.isDestroyed()) return;

  const mainBounds = mainWindow.getBounds();
  const overlaySize = activeOverlayWindow.getSize();

  // Position to the right of the main window, vertically centered
  let x = mainBounds.x + mainBounds.width + 16;
  let y = mainBounds.y + (mainBounds.height - overlaySize[1]) / 2;

  // Keep within display bounds
  const display = screen.getDisplayMatching(mainBounds);
  const workArea = display.workArea;
  if (x + overlaySize[0] > workArea.x + workArea.width) {
    x = mainBounds.x - overlaySize[0] - 16; // flip to left side
  }
  if (x < workArea.x) x = workArea.x + 16;
  if (y < workArea.y) y = workArea.y + 16;
  if (y + overlaySize[1] > workArea.y + workArea.height) {
    y = workArea.y + workArea.height - overlaySize[1] - 16;
  }

  activeOverlayWindow.setPosition(Math.round(x), Math.round(y));
}

// ─── Dismiss ─────────────────────────────────────────────────────────────────

export function dismissGuide(): { success: boolean; error?: string } {
  let dismissed = false;

  if (activeProcess) {
    log.info(`[PermissionGuide] Dismissing native overlay (killing PID ${activeProcess.pid})`);
    activeProcess.kill('SIGTERM');
    activeProcess = null;
    activePanel = null;
    dismissed = true;
  }

  if (activeOverlayWindow && !activeOverlayWindow.isDestroyed()) {
    log.info('[PermissionGuide] Dismissing Electron overlay');
    activeOverlayWindow.close();
    activeOverlayWindow = null;
    activePanel = null;
    dismissed = true;
  }

  if (overlayTrackingInterval) {
    clearInterval(overlayTrackingInterval);
    overlayTrackingInterval = null;
  }

  if (dismissed) {
    return { success: true };
  }
  log.warn('[PermissionGuide] dismissGuide called but no active overlay');
  return { success: false, error: 'No active overlay' };
}

// ─── Status ──────────────────────────────────────────────────────────────────

export function getGuideStatus(): { active: boolean; panel: PermissionPanel | null } {
  const nativeActive = activeProcess !== null && !activeProcess.killed;
  const electronActive = activeOverlayWindow !== null && !activeOverlayWindow.isDestroyed();
  const active = nativeActive || electronActive;
  log.info(`[PermissionGuide] getGuideStatus — active: ${active}, panel: ${activePanel}`);
  return { active, panel: activePanel };
}

// ─── Wait for dismissal ──────────────────────────────────────────────────────

export function waitForGuideDismissed(timeoutMs = 120_000): Promise<void> {
  return new Promise((resolve) => {
    if (!activeProcess && !activeOverlayWindow) {
      log.info('[PermissionGuide] waitForGuideDismissed — no active guide, resolving immediately');
      resolve();
      return;
    }

    log.info(`[PermissionGuide] Waiting for guide dismissal (timeout: ${timeoutMs}ms)...`);
    const startTime = Date.now();

    const checkInterval = setInterval(() => {
      const nativeGone = !activeProcess;
      const electronGone = !activeOverlayWindow || activeOverlayWindow.isDestroyed();

      if (nativeGone && electronGone) {
        clearInterval(checkInterval);
        clearTimeout(timeoutTimer);
        log.info('[PermissionGuide] Guide dismissed by user');
        resolve();
        return;
      }

      if (Date.now() - startTime > timeoutMs) {
        clearInterval(checkInterval);
        clearTimeout(timeoutTimer);
        log.warn('[PermissionGuide] waitForGuideDismissed timed out');
        resolve();
      }
    }, 500);

    const timeoutTimer = setTimeout(() => {
      clearInterval(checkInterval);
      resolve();
    }, timeoutMs + 1000);
  });
}

// ─── Onboarding ──────────────────────────────────────────────────────────────

export interface OnboardingResult {
  accessibility: 'granted' | 'denied' | 'unknown' | 'not-applicable';
  screenRecording: 'granted' | 'denied' | 'unknown' | 'not-applicable';
  guidesPresented: number;
}

export async function runPermissionOnboarding(
  sendStatus: (status: PermissionStatus) => void
): Promise<OnboardingResult> {
  log.info('[PermissionGuide] Starting permission onboarding flow...');

  let guidesPresented = 0;

  const status = await checkPermissions();
  sendStatus(status);

  // Accessibility — only relevant on macOS
  if (status.accessibility === 'denied') {
    log.info('[PermissionGuide] Onboarding: presenting accessibility guide');
    const result = await presentGuide('accessibility');
    if (result.success && !result.alreadyGranted) {
      guidesPresented++;
      await waitForGuideDismissed();
      log.info('[PermissionGuide] Onboarding: accessibility guide dismissed');
      sendStatus(await checkPermissions());
    }
  }

  // Screen Recording — relevant on all desktop platforms
  const recheck = await checkPermissions();
  if (recheck.screenRecording === 'denied') {
    log.info('[PermissionGuide] Onboarding: presenting screen recording guide');
    const result = await presentGuide('screen-recording');
    if (result.success && !result.alreadyGranted) {
      guidesPresented++;
      await waitForGuideDismissed();
      log.info('[PermissionGuide] Onboarding: screen recording guide dismissed');
      sendStatus(await checkPermissions());
    }
  }

  const final = await checkPermissions();
  const result: OnboardingResult = {
    accessibility: final.accessibility,
    screenRecording: final.screenRecording,
    guidesPresented,
  };

  log.info('[PermissionGuide] Permission onboarding complete:', result);
  return result;
}

// ─── Helpers ─────────────────────────────────────────────────────────────────

function resolveBinaryPath(): string | null {
  const binaryName = 'permission-guide-cli';
  const candidates = [
    join(process.resourcesPath ?? '', 'bin', binaryName),
    join(__dirname, '..', '..', 'native', 'vm-manager', '.build', 'release', binaryName),
    join(__dirname, '..', '..', 'resources', 'bin', binaryName),
  ];

  log.info('[PermissionGuide] Resolving binary path...');
  for (const p of candidates) {
    const exists = fs.existsSync(p);
    log.info(`[PermissionGuide] Candidate: ${p} — ${exists ? 'FOUND' : 'not found'}`);
    if (exists) return p;
  }

  log.error('[PermissionGuide] permission-guide-cli binary not found. Searched:', candidates);
  return null;
}
