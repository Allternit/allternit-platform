/**
 * Allternit Desktop — Startup Window (pairing card + loading + folder grants)
 *
 * First launch / signed-out launches show the pairing card: the computer's
 * name, three steps (sign in, approve, start) and one "Sign in to pair"
 * button. auth-manager.ts re-renders the card on every pairing state change
 * (pairing:render). Returning signed-in launches go straight to the loading
 * step. The page itself is built in startup-page.ts.
 */

import { app, BrowserWindow, nativeTheme } from 'electron';
import log from 'electron-log';
import { fileURLToPath } from 'node:url';
import { dirname, join } from 'node:path';
import { openExternalAllowlisted } from './security.js';
import { pairingComputerInfo } from './auth-manager.js';
import { buildStartupHtml, STARTUP_BG_DARK, STARTUP_BG_LIGHT, type StartupInitialStep } from './startup-page.js';

const __dirname = dirname(fileURLToPath(import.meta.url));

export type { StartupInitialStep } from './startup-page.js';

export interface StartupWindowOptions {
  initialStep: StartupInitialStep;
}

/** Content size of the startup window (the pairing card's design size). */
export const STARTUP_WINDOW_WIDTH = 440;
export const STARTUP_WINDOW_HEIGHT = 540;

export function createStartupWindow(options: StartupWindowOptions): BrowserWindow {
  const window = new BrowserWindow({
    width: STARTUP_WINDOW_WIDTH,
    height: STARTUP_WINDOW_HEIGHT,
    useContentSize: true,
    resizable: false,
    alwaysOnTop: true,
    titleBarStyle: 'hiddenInset',
    // Linux and Windows draw a real title bar. Without a title it shows the
    // package name ("@allternit/desktop"); the app name itself can't change
    // because it names the userData folder.
    title: 'Allternit',
    autoHideMenuBar: true,
    // Match the page (app white / graphite) so dark mode doesn't flash white.
    backgroundColor: nativeTheme.shouldUseDarkColors ? STARTUP_BG_DARK : STARTUP_BG_LIGHT,
    show: false,
    webPreferences: {
      preload: join(__dirname, '../preload/startup.js'),
      contextIsolation: true,
      nodeIntegration: false,
      sandbox: true,
    },
  });

  window.once('ready-to-show', () => {
    window.show();
  });
  // The startup page and the auth renderer loaded into it later have no
  // product <title>; keep the window's own title.
  window.on('page-title-updated', (event) => event.preventDefault());
  if (process.platform !== 'darwin') window.setMenuBarVisibility(false);

  const syncBackground = () => {
    if (!window.isDestroyed()) window.setBackgroundColor(nativeTheme.shouldUseDarkColors ? STARTUP_BG_DARK : STARTUP_BG_LIGHT);
  };
  nativeTheme.on('updated', syncBackground);
  window.once('closed', () => nativeTheme.off('updated', syncBackground));

  window.webContents.on('console-message', (event) => {
    log.info(`[Startup] ${event.message} (${event.sourceId}:${event.lineNumber})`);
  });

  window.webContents.setWindowOpenHandler(({ url }) => {
    void openExternalAllowlisted(url);
    return { action: 'deny' };
  });

  const html = buildStartupHtml({
    initialStep: options.initialStep,
    version: app.getVersion(),
    ...pairingComputerInfo(),
  });
  window.loadURL(`data:text/html;charset=utf-8,${encodeURIComponent(html)}`);
  return window;
}
