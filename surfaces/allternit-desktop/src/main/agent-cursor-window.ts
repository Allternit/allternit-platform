import { app, BrowserWindow, ipcMain, screen, type IpcMainEvent, type IpcMainInvokeEvent } from 'electron';

/** What the overlay page needs to map native screen pixels to its CSS pixels. */
export interface AgentCursorDisplay {
  width: number;
  height: number;
  scaleFactor: number;
}

/**
 * The real-screen agent cursor badge on this Mac: one transparent,
 * click-through window over the whole primary display (not the work area,
 * so the menu bar and Dock are covered too) that draws which bot is moving
 * the pointer and what it is doing. It never takes clicks or focus, and
 * content protection keeps it out of the agent's own screenshots.
 *
 * Main only owns the window. The page (`companion.html?surface=agent-cursor`)
 * subscribes to this device's `computer.action` events, draws the badge and
 * decides when to show or hide it.
 */
export function installAgentCursorWindow(options: { origin: () => string; preload: string }) {
  let win: BrowserWindow | null = null;
  let ready = false;
  let quitting = false;
  const owned = (event: IpcMainEvent | IpcMainInvokeEvent) =>
    !!win && !win.isDestroyed() && event.sender === win.webContents && event.senderFrame === event.sender.mainFrame;
  const display = (): AgentCursorDisplay => {
    const d = screen.getPrimaryDisplay();
    return { width: d.bounds.width, height: d.bounds.height, scaleFactor: d.scaleFactor || 1 };
  };

  function makeWindow() {
    const bounds = screen.getPrimaryDisplay().bounds;
    const next = new BrowserWindow({
      ...bounds,
      title: 'Allternit Agent Cursor',
      frame: false, transparent: true, backgroundColor: '#00000000', hasShadow: false,
      resizable: false, movable: false, minimizable: false, maximizable: false, fullscreenable: false,
      focusable: false, skipTaskbar: true, alwaysOnTop: true, show: false, enableLargerThanScreen: true,
      type: process.platform === 'darwin' ? 'panel' : undefined,
      webPreferences: { preload: options.preload, sandbox: true, contextIsolation: true, nodeIntegration: false },
    });
    next.setAlwaysOnTop(true, 'screen-saver');
    next.setVisibleOnAllWorkspaces(true, { visibleOnFullScreen: true, skipTransformProcessType: true });
    next.setIgnoreMouseEvents(true);
    next.setContentProtection(true);
    const target = new URL('/companion.html?surface=agent-cursor', options.origin()).toString();
    next.webContents.setWindowOpenHandler(() => ({ action: 'deny' }));
    next.webContents.on('will-navigate', (event, url) => { if (url !== target) event.preventDefault(); });
    next.webContents.on('did-start-navigation', () => { ready = false; next.hide(); });
    next.webContents.on('did-finish-load', () => {
      void next.webContents.executeJavaScript('document.documentElement.dataset.desktopCompanion === "true"')
        .then((valid: boolean) => {
          if (next.isDestroyed()) return;
          if (!valid) { console.error(`[agent-cursor] ${target} did not render the companion entry`); return; }
          ready = true;
        })
        .catch(error => { console.error('[agent-cursor] renderer validation failed', error); });
    });
    next.on('close', event => { if (!quitting) { event.preventDefault(); next.hide(); } });
    next.on('closed', () => { if (win === next) { win = null; ready = false; } });
    void next.loadURL(target).catch(error => console.warn('[agent-cursor] overlay page did not load', error));
    return next;
  }
  function ensure() {
    if (quitting) return null;
    if (!win || win.isDestroyed()) { ready = false; win = makeWindow(); }
    return win;
  }
  /** Cover the primary display again and tell the page its new size. */
  function repair() {
    if (!win || win.isDestroyed()) return;
    win.setBounds(screen.getPrimaryDisplay().bounds);
    win.webContents.send('agent-cursor:display-changed', display());
  }

  ipcMain.handle('agent-cursor:display', event => {
    if (!owned(event)) throw new Error('Untrusted agent cursor sender');
    return display();
  });
  // show = showInactive: the badge never takes focus from the app the agent is using.
  ipcMain.on('agent-cursor:show', event => {
    if (!owned(event) || quitting || !ready || !win) return;
    win.setBounds(screen.getPrimaryDisplay().bounds);
    win.showInactive();
  });
  ipcMain.on('agent-cursor:hide', event => {
    if (owned(event) && win) win.hide();
  });
  app.whenReady().then(() => {
    screen.on('display-removed', repair); screen.on('display-metrics-changed', repair);
  });
  app.on('before-quit', () => { quitting = true; if (win && !win.isDestroyed()) win.hide(); });
  return { start: () => { ensure(); }, window: () => win };
}
