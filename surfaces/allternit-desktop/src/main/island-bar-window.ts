/**
 * Island bar window (program "Island bar + Team map", lane 28, PLAN D13).
 *
 * One frameless, transparent, always-on-top window that loads the ai app's
 * bar route (`/bar`, ai `src/pages/BarWindowPage.tsx`, lane 30). It opens
 * near the top of the display under the cursor, hides on blur or Escape,
 * and keeps its page (draft, recents) between opens.
 *
 * Lane 29's pill window reuses `floatingPanelWindowOptions` and
 * `pinAboveEverything` so both windows float the same way.
 */
import { BrowserWindow, screen, type BrowserWindowConstructorOptions, type Rectangle, type WebContents } from 'electron';

/** The ai route the bar window loads. Lane 30 mounts BarWindowPage here. */
export const ISLAND_BAR_ROUTE = '/bar';
/** Mockup A-desktop-bar.jsx: a 660 px card near the top of the screen. */
export const ISLAND_BAR_WIDTH = 660;
export const ISLAND_BAR_MIN_HEIGHT = 56;
export const ISLAND_BAR_MAX_HEIGHT = 560;
/** Until the page reports its height (`island-bar:set-height`). */
export const ISLAND_BAR_INITIAL_HEIGHT = 360;

export function islandBarUrl(platformUrl: string): string {
  return new URL(ISLAND_BAR_ROUTE, platformUrl).toString();
}

/** Centered, 16% down the work area (at least 48 px), never wider than the display. */
export function islandBarBounds(workArea: Rectangle, height: number = ISLAND_BAR_INITIAL_HEIGHT): Rectangle {
  const width = Math.min(ISLAND_BAR_WIDTH, Math.max(320, workArea.width - 32));
  const h = Math.min(clampIslandBarHeight(height) ?? ISLAND_BAR_INITIAL_HEIGHT, workArea.height);
  return {
    x: Math.round(workArea.x + (workArea.width - width) / 2),
    y: Math.round(workArea.y + Math.max(48, workArea.height * 0.16)),
    width,
    height: h,
  };
}

/** A height from the renderer, clamped; null when it is not a number. */
export function clampIslandBarHeight(height: unknown): number | null {
  if (typeof height !== 'number' || !Number.isFinite(height)) return null;
  return Math.round(Math.min(ISLAND_BAR_MAX_HEIGHT, Math.max(ISLAND_BAR_MIN_HEIGHT, height)));
}

/**
 * Frameless, transparent, always-on-top, out of the taskbar and Mission
 * Control. Same set as the HUD (`unified-main.ts` createHudWindow) and the
 * companion bubble (`desktop-companion.ts`).
 */
export function floatingPanelWindowOptions(opts: {
  isMac: boolean;
  preload: string;
  bounds: Rectangle;
  title: string;
  focusable?: boolean;
}): BrowserWindowConstructorOptions {
  return {
    ...opts.bounds,
    title: opts.title,
    frame: false,
    transparent: true,
    backgroundColor: '#00000000',
    // A transparent resizable window grows an edge hot-zone on Windows.
    resizable: false,
    movable: false,
    minimizable: false,
    maximizable: false,
    fullscreenable: false,
    skipTaskbar: true,
    hasShadow: false,
    alwaysOnTop: true,
    focusable: opts.focusable ?? true,
    // NSPanel: no cmd-tab anchor, floats over fullscreen apps.
    type: opts.isMac ? 'panel' : undefined,
    roundedCorners: true,
    hiddenInMissionControl: opts.isMac,
    show: false,
    webPreferences: {
      preload: opts.preload,
      contextIsolation: true,
      nodeIntegration: false,
      sandbox: true,
    },
  };
}

/** Above normal windows on every Space, fullscreen included; not above system UI on Windows/Linux. */
export function pinAboveEverything(win: BrowserWindow, isMac: boolean): void {
  win.setAlwaysOnTop(true, isMac ? 'floating' : 'pop-up-menu');
  try {
    win.setVisibleOnAllWorkspaces?.(true, { visibleOnFullScreen: true, skipTransformProcessType: true });
  } catch {
    // Not supported on every platform/compositor.
  }
}

export interface IslandBarWindowDeps {
  isMac: boolean;
  preload: string;
  /** Current ai origin (activePlatformUrl). */
  platformUrl: () => string;
  /** False before this computer is paired (mayOpenHud). */
  mayOpen: () => boolean;
  /** installWillNavigateGuard (security.ts). */
  guardNavigation: (wc: WebContents) => void;
  /** Links from the bar open in the browser. */
  openExternal: (url: string) => void;
  isQuitting: () => boolean;
  log: { info: (...a: unknown[]) => void; warn: (...a: unknown[]) => void; error: (...a: unknown[]) => void };
}

export interface IslandBarWindow {
  open: () => void;
  hide: () => void;
  toggle: () => void;
  isOpen: () => boolean;
  /** True when an IPC event came from the bar window. */
  isFrom: (sender: WebContents) => boolean;
  /** The page fits the window to its content. */
  setHeight: (height: unknown) => void;
  window: () => BrowserWindow | null;
}

export function createIslandBarWindow(deps: IslandBarWindowDeps): IslandBarWindow {
  let win: BrowserWindow | null = null;
  let shownAt = 0;
  let height = ISLAND_BAR_INITIAL_HEIGHT;

  const alive = (): boolean => win !== null && !win.isDestroyed();

  function cursorWorkArea(): Rectangle {
    return screen.getDisplayNearestPoint(screen.getCursorScreenPoint()).workArea;
  }

  function ensure(): BrowserWindow {
    if (alive()) return win!;
    const next = new BrowserWindow(floatingPanelWindowOptions({
      isMac: deps.isMac,
      preload: deps.preload,
      bounds: islandBarBounds(cursorWorkArea(), height),
      title: 'Allternit Island bar',
    }));
    win = next;
    pinAboveEverything(next, deps.isMac);
    deps.guardNavigation(next.webContents);
    next.webContents.setWindowOpenHandler(({ url }) => {
      deps.openExternal(url);
      return { action: 'deny' };
    });
    next.webContents.on('did-fail-load', (_e, code, description) => {
      deps.log.error('[IslandBar] Failed to load the bar page:', code, description);
    });
    // Escape hides even before the page handles it (or if it fails to load).
    next.webContents.on('before-input-event', (event, input) => {
      if (input.type === 'keyDown' && input.key === 'Escape') {
        event.preventDefault();
        hide();
      }
    });
    next.on('blur', () => {
      // A blur right after show is the OS settling focus, not the user leaving.
      if (Date.now() - shownAt < 200) return;
      if (next.webContents.isDevToolsFocused()) return;
      hide();
    });
    // ⌘W / Alt+F4 hide; only quitting closes it (keeps the draft).
    next.on('close', (event) => {
      if (deps.isQuitting()) return;
      event.preventDefault();
      hide();
    });
    next.on('closed', () => { if (win === next) win = null; });
    void next.loadURL(islandBarUrl(deps.platformUrl()));
    return next;
  }

  function open(): void {
    if (!deps.mayOpen()) {
      deps.log.info('[IslandBar] Not opening before pairing');
      return;
    }
    const w = ensure();
    // The app may have switched platform origins since the page loaded.
    const want = new URL(islandBarUrl(deps.platformUrl()));
    const current = w.webContents.getURL();
    if (current && new URL(current).origin !== want.origin) void w.loadURL(want.toString());
    if (!w.isVisible()) w.setBounds(islandBarBounds(cursorWorkArea(), height));
    w.show();
    w.focus();
    w.moveTop();
    w.webContents.focus();
    shownAt = Date.now();
    const notify = () => { if (!w.isDestroyed()) w.webContents.send('island-bar:shown'); };
    if (w.webContents.isLoading()) w.webContents.once('did-finish-load', notify);
    else notify();
  }

  function hide(): void {
    if (alive() && win!.isVisible()) win!.hide();
  }

  return {
    open,
    hide,
    toggle() {
      if (alive() && win!.isVisible() && win!.isFocused()) hide();
      else open();
    },
    isOpen: () => alive() && win!.isVisible(),
    isFrom: (sender) => alive() && sender === win!.webContents,
    setHeight(next) {
      const h = clampIslandBarHeight(next);
      if (h === null) return;
      height = h;
      if (!alive()) return;
      const b = win!.getBounds();
      if (b.height !== h) win!.setBounds({ ...b, height: h });
    },
    window: () => (alive() ? win : null),
  };
}
