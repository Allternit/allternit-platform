/**
 * Island pill window (program "Island bar + Team map", lane 29, PLAN D13).
 *
 * A small transparent, frameless, always-on-top, non-focusable window that
 * loads the ai pill route (`/bar/pill`, ai `src/pages/PillWindowPage.tsx`,
 * lane 30). It shows only while a bar job is live and hides RECEIPT_MS after
 * the last live job ends (the receipt stays on screen that long). Clicking the
 * pill opens the bar: the page calls `window.allternit.islandBar.open()`.
 *
 * Job state comes from the renderers, not the runtime: the main process does
 * not hold the bar job stream. Each ai window that follows bar jobs (main
 * window, bar window, the pill page itself) sends its current list through
 * `islandBar.reportJobs(jobs)` (IPC `island-bar:jobs`); this module unions
 * them per sender and drops a sender's list when its page goes away.
 *
 * Placement is per OS (platform-macos.ts, platform-windows.ts,
 * platform-linux.ts). On native Wayland there is no pill window at all: the
 * job shows as a dot in the tray menu with "Open Island bar" (PLAN D14).
 */
import { BrowserWindow, screen, type Rectangle, type WebContents } from 'electron';
import { ISLAND_PILL_ROUTE, floatingPanelWindowOptions, pinAboveEverything } from './island-bar-window.js';
import { macIslandPillBounds } from './platform-macos.js';
import { windowsIslandPillBounds } from './platform-windows.js';
import { linuxIslandPillBounds, linuxIslandPillMode } from './platform-linux.js';

/** Mockup A-pill-options.jsx option A: the capsule is 190–320 px wide; the needs-input state is the tallest. */
export const ISLAND_PILL_WIDTH = 360;
export const ISLAND_PILL_HEIGHT = 168;
const PILL_MIN = { width: 120, height: 32 };
const PILL_MAX = { width: 420, height: 240 };
/** How long the receipt stays after the last live job ends. */
export const ISLAND_PILL_RECEIPT_MS = 4000;

/** ai `src/lib/bar/types.ts` BarJobState. */
export type IslandBarJobState =
  | 'review'
  | 'waiting_approval'
  | 'working'
  | 'input_required'
  | 'completed'
  | 'failed'
  | 'cancelled';

export interface IslandBarJobReport {
  id: string;
  state: IslandBarJobState;
  /** Short line for the tray menu ("Refunding €320"). */
  title: string;
}

const JOB_STATES: ReadonlySet<string> = new Set<IslandBarJobState>([
  'review', 'waiting_approval', 'working', 'input_required', 'completed', 'failed', 'cancelled',
]);
/** The states that keep the pill up. `review` is still the bar's own card. */
const LIVE_STATES: ReadonlySet<IslandBarJobState> = new Set<IslandBarJobState>([
  'waiting_approval', 'working', 'input_required',
]);

export function isLiveBarJob(job: IslandBarJobReport): boolean {
  return LIVE_STATES.has(job.state);
}

/** A renderer's list reduced to known states and short text (at most 10 jobs). */
export function sanitizeBarJobs(value: unknown): IslandBarJobReport[] {
  if (!Array.isArray(value)) return [];
  const out: IslandBarJobReport[] = [];
  for (const raw of value.slice(0, 10)) {
    const job = raw as { id?: unknown; state?: unknown; title?: unknown } | null;
    if (typeof job?.id !== 'string' || !/^[A-Za-z0-9_-]{1,80}$/.test(job.id)) continue;
    if (typeof job.state !== 'string' || !JOB_STATES.has(job.state)) continue;
    const title = typeof job.title === 'string' ? job.title.replace(/\s+/g, ' ').trim().slice(0, 60) : '';
    out.push({ id: job.id, state: job.state as IslandBarJobState, title });
  }
  return out;
}

/** Tray menu line for a live job, with a coloured dot like the Meetings rows. */
export function barJobTrayLabel(job: IslandBarJobReport): string {
  const dot = job.state === 'working' ? '🟣' : '🟠';
  const what = job.state === 'waiting_approval' ? 'Waiting on approval'
    : job.state === 'input_required' ? 'Needs your answer'
      : 'Running';
  return `${dot} ${what}${job.title ? `: ${job.title}` : ''}`;
}

/** Pill window bounds for this OS, or null when the pill lives in the tray (native Wayland). */
export function islandPillBounds(
  platform: string,
  linuxBackend: 'wayland' | 'x11',
  display: { bounds: Rectangle; workArea: Rectangle },
  size: { width: number; height: number },
): Rectangle | null {
  if (platform === 'darwin') return macIslandPillBounds(display, size);
  if (platform === 'win32') return windowsIslandPillBounds(display, size);
  if (linuxIslandPillMode(linuxBackend) === 'tray') return null;
  return linuxIslandPillBounds(display, size);
}

/** A size from the renderer, clamped; null when it is not two numbers. */
export function clampIslandPillSize(value: unknown): { width: number; height: number } | null {
  const v = value as { width?: unknown; height?: unknown } | null;
  const w = v?.width;
  const h = v?.height;
  if (typeof w !== 'number' || typeof h !== 'number' || !Number.isFinite(w) || !Number.isFinite(h)) return null;
  return {
    width: Math.round(Math.min(PILL_MAX.width, Math.max(PILL_MIN.width, w))),
    height: Math.round(Math.min(PILL_MAX.height, Math.max(PILL_MIN.height, h))),
  };
}

/**
 * When the pill shows. Pure, with injected timers, so the 4 s receipt hold is
 * testable: `update(live)` after every report; `onChange(visible)` fires on
 * each change.
 */
export function createIslandPillVisibility(deps: {
  onChange: (visible: boolean) => void;
  setTimer: (fn: () => void, ms: number) => unknown;
  clearTimer: (handle: unknown) => void;
  receiptMs?: number;
}) {
  let visible = false;
  let hold: unknown = null;
  const cancelHold = () => {
    if (hold !== null) deps.clearTimer(hold);
    hold = null;
  };
  const set = (next: boolean) => {
    if (next === visible) return;
    visible = next;
    deps.onChange(next);
  };
  return {
    update(live: number): void {
      if (live > 0) {
        cancelHold();
        set(true);
        return;
      }
      // Last live job ended: keep the receipt up, then hide.
      if (visible && hold === null) {
        hold = deps.setTimer(() => { hold = null; set(false); }, deps.receiptMs ?? ISLAND_PILL_RECEIPT_MS);
      }
    },
    /** Flag off / signed out: hide now. */
    reset(): void {
      cancelHold();
      set(false);
    },
    visible: () => visible,
  };
}

export interface IslandPillWindowDeps {
  isMac: boolean;
  platform: string;
  /** hud-windowing.ts linuxOzoneBackend(process.env, process.argv). */
  linuxBackend: 'wayland' | 'x11';
  preload: string;
  platformUrl: () => string;
  /** Flag `bar.island` on and the computer paired. */
  enabled: () => boolean;
  guardNavigation: (wc: WebContents) => void;
  openExternal: (url: string) => void;
  isQuitting: () => boolean;
  /** Live jobs changed: the tray menu redraws (dot + "Open Island bar"). */
  onJobsChanged: (live: IslandBarJobReport[]) => void;
  log: { info: (...a: unknown[]) => void; warn: (...a: unknown[]) => void; error: (...a: unknown[]) => void };
}

export interface IslandPillWindow {
  /** IPC `island-bar:jobs`: `sender`'s whole current list. */
  report: (sender: WebContents, jobs: unknown) => void;
  /** IPC `island-bar:set-pill-size` from the pill page. */
  setSize: (sender: WebContents, size: unknown) => void;
  liveJobs: () => IslandBarJobReport[];
  /** False on native Wayland: the tray carries the job instead. */
  hasWindow: () => boolean;
  isFrom: (sender: WebContents) => boolean;
  /** Flag off: hide and forget every report. */
  reset: () => void;
}

export function createIslandPillWindow(deps: IslandPillWindowDeps): IslandPillWindow {
  const reports = new Map<number, IslandBarJobReport[]>();
  let win: BrowserWindow | null = null;
  let size = { width: ISLAND_PILL_WIDTH, height: ISLAND_PILL_HEIGHT };
  let lastLiveKey = '';

  const alive = (): boolean => win !== null && !win.isDestroyed();
  const pillUrl = () => new URL(ISLAND_PILL_ROUTE, deps.platformUrl()).toString();

  function bounds(): Rectangle | null {
    const display = screen.getDisplayNearestPoint(screen.getCursorScreenPoint());
    return islandPillBounds(deps.platform, deps.linuxBackend, display, size);
  }

  function ensure(at: Rectangle): BrowserWindow {
    if (alive()) return win!;
    const next = new BrowserWindow(floatingPanelWindowOptions({
      isMac: deps.isMac,
      preload: deps.preload,
      bounds: at,
      title: 'Allternit Island pill',
      // Never takes focus from the app the person is in.
      focusable: false,
    }));
    win = next;
    pinAboveEverything(next, deps.isMac);
    deps.guardNavigation(next.webContents);
    next.webContents.setWindowOpenHandler(({ url }) => {
      deps.openExternal(url);
      return { action: 'deny' };
    });
    next.webContents.on('did-fail-load', (_e, code, description) => {
      deps.log.error('[IslandPill] Failed to load the pill page:', code, description);
    });
    next.on('close', (event) => {
      if (deps.isQuitting()) return;
      event.preventDefault();
      next.hide();
    });
    next.on('closed', () => { if (win === next) win = null; });
    void next.loadURL(pillUrl());
    return next;
  }

  const visibility = createIslandPillVisibility({
    setTimer: (fn, ms) => setTimeout(fn, ms),
    clearTimer: (handle) => clearTimeout(handle as ReturnType<typeof setTimeout>),
    onChange: (visible) => {
      if (!visible) {
        if (alive()) win!.hide();
        return;
      }
      const at = bounds();
      if (!at) return; // Native Wayland: tray only.
      const w = ensure(at);
      const current = w.webContents.getURL();
      if (current && new URL(current).origin !== new URL(pillUrl()).origin) void w.loadURL(pillUrl());
      w.setBounds(at);
      pinAboveEverything(w, deps.isMac);
      w.showInactive();
    },
  });

  function liveJobs(): IslandBarJobReport[] {
    const byId = new Map<string, IslandBarJobReport>();
    for (const list of reports.values()) for (const job of list) if (isLiveBarJob(job)) byId.set(job.id, job);
    return [...byId.values()];
  }

  function recompute(): void {
    const live = deps.enabled() ? liveJobs() : [];
    if (deps.enabled()) visibility.update(live.length);
    else visibility.reset();
    const key = JSON.stringify(live);
    if (key !== lastLiveKey) {
      lastLiveKey = key;
      deps.onJobsChanged(live);
    }
  }

  return {
    report(sender, jobs) {
      const id = sender.id;
      if (!reports.has(id)) sender.once('destroyed', () => { reports.delete(id); recompute(); });
      reports.set(id, sanitizeBarJobs(jobs));
      recompute();
    },
    setSize(sender, value) {
      if (!alive() || sender !== win!.webContents) return;
      const next = clampIslandPillSize(value);
      if (!next) return;
      size = next;
      const at = bounds();
      if (at && win!.isVisible()) win!.setBounds(at);
    },
    liveJobs,
    hasWindow: () => deps.platform !== 'linux' || linuxIslandPillMode(deps.linuxBackend) === 'window',
    isFrom: (sender) => alive() && sender === win!.webContents,
    reset() {
      reports.clear();
      recompute();
    },
  };
}
