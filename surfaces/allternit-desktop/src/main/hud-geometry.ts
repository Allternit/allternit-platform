/**
 * HUD geometry — the one place the floating HUD's size, placement and
 * clamping are decided. unified-main.ts feeds it Electron `screen` work areas
 * and applies the result; everything here is pure so the multi-monitor cases
 * (a display above or left of the primary, i.e. negative coordinates) can be
 * tested without a screen.
 *
 * All values are DIPs, the unit Electron's `screen` and `getBounds()` use.
 */

/** Default bar: wide and short so the composer dominates. */
export const HUD_WIDTH = 620;
export const HUD_HEIGHT = 220;
/** Gap between the bar and the bottom of the work area on first open. */
export const HUD_BOTTOM_MARGIN = 72;
export const HUD_MIN_WIDTH = 380;
export const HUD_MIN_HEIGHT = 160;
export const HUD_MAX_WIDTH = 1600;
export const HUD_MAX_HEIGHT = 1000;
/** How much of the bar must stay on screen while it is being dragged. */
export const HUD_DRAG_KEEP_VISIBLE = 80;

export interface HudRect {
  x: number;
  y: number;
  width: number;
  height: number;
}

/** Saved bounds per display id (Electron `Display.id`, stringified). */
export type HudBoundsByDisplay = Record<string, HudRect>;

const clamp = (value: number, min: number, max: number): number => Math.min(Math.max(value, min), max);

const isRect = (value: unknown): value is HudRect => {
  if (!value || typeof value !== 'object') return false;
  const rect = value as Record<string, unknown>;
  return ['x', 'y', 'width', 'height'].every((key) => typeof rect[key] === 'number' && Number.isFinite(rect[key]));
};

/** Default bounds on `area`: centred, `HUD_BOTTOM_MARGIN` above its bottom. */
export function defaultHudBounds(area: HudRect): HudRect {
  const width = Math.min(HUD_WIDTH, area.width);
  const height = Math.min(HUD_HEIGHT, area.height);
  return {
    width,
    height,
    x: Math.round(area.x + (area.width - width) / 2),
    y: Math.round(Math.max(area.y, area.y + area.height - height - HUD_BOTTOM_MARGIN)),
  };
}

/**
 * Size within the HUD limits and fully inside `area`. The limits give way to
 * the area when a display is smaller than the minimum.
 */
export function clampHudBounds(rect: HudRect, area: HudRect): HudRect {
  const width = Math.round(Math.min(clamp(rect.width, HUD_MIN_WIDTH, HUD_MAX_WIDTH), area.width));
  const height = Math.round(Math.min(clamp(rect.height, HUD_MIN_HEIGHT, HUD_MAX_HEIGHT), area.height));
  return {
    width,
    height,
    x: Math.round(clamp(rect.x, area.x, area.x + area.width - width)),
    y: Math.round(clamp(rect.y, area.y, area.y + area.height - height)),
  };
}

/**
 * Lenient clamp used mid-drag: the bar may hang off an edge (or straddle two
 * displays) but its top strip stays reachable on `area`. The strict clamp runs
 * when the drag ends.
 */
export function keepHudReachable(rect: HudRect, area: HudRect, keep: number = HUD_DRAG_KEEP_VISIBLE): HudRect {
  const width = Math.round(clamp(rect.width, HUD_MIN_WIDTH, HUD_MAX_WIDTH));
  const height = Math.round(clamp(rect.height, HUD_MIN_HEIGHT, HUD_MAX_HEIGHT));
  const visible = Math.min(keep, width);
  return {
    width,
    height,
    x: Math.round(clamp(rect.x, area.x - width + visible, area.x + area.width - visible)),
    y: Math.round(clamp(rect.y, area.y, area.y + area.height - Math.min(keep, height))),
  };
}

/**
 * Grow or shrink to `requestedHeight` keeping the bottom edge where it is (the
 * bar grows upward), never above the top of `area`. The old code clamped the
 * top to y=0, which on a display above the primary (negative y) teleported the
 * bar onto the primary display.
 */
export function resizeHudFromBottom(current: HudRect, requestedHeight: number, area: HudRect): HudRect {
  const bottom = Math.min(current.y + current.height, area.y + area.height);
  const room = Math.max(0, bottom - area.y);
  const height = Math.round(Math.min(clamp(requestedHeight, HUD_MIN_HEIGHT, HUD_MAX_HEIGHT), room || HUD_MIN_HEIGHT));
  return clampHudBounds({ x: current.x, width: current.width, height, y: bottom - height }, area);
}

/** Merge a partial renderer request onto `current`, ignoring non-finite fields. */
export function mergeHudBounds(current: HudRect, request: unknown): HudRect {
  const partial = (request && typeof request === 'object' ? request : {}) as Partial<Record<keyof HudRect, unknown>>;
  const pick = (key: keyof HudRect) => {
    const value = Number(partial[key]);
    return partial[key] !== undefined && partial[key] !== null && Number.isFinite(value) ? Math.round(value) : current[key];
  };
  return { x: pick('x'), y: pick('y'), width: pick('width'), height: pick('height') };
}

/**
 * Read the persisted `hudBounds` store value. Older builds stored a single
 * `{ width, height }` default there; anything that is not a map of display id
 * to a full rect is dropped.
 */
export function parseHudBoundsByDisplay(value: unknown): HudBoundsByDisplay {
  if (!value || typeof value !== 'object' || Array.isArray(value)) return {};
  const out: HudBoundsByDisplay = {};
  for (const [key, rect] of Object.entries(value as Record<string, unknown>)) {
    if (isRect(rect)) out[key] = { x: rect.x, y: rect.y, width: rect.width, height: rect.height };
  }
  return out;
}

/** Where the HUD opens on a display: its saved spot there, else the default. */
export function hudBoundsForDisplay(area: HudRect, saved: HudRect | undefined): HudRect {
  return saved ? clampHudBounds(saved, area) : defaultHudBounds(area);
}

export interface HudBoundsWindow {
  isDestroyed(): boolean;
  isResizable(): boolean;
  setBounds(bounds: HudRect): void;
  setResizable(resizable: boolean): void;
}

/**
 * Apply bounds to the HUD window. It is created non-resizable (a transparent
 * frameless window must not expose a system resize hot-zone), which on
 * Windows/Linux also blocks programmatic setBounds sizing — flip it on
 * briefly.
 *
 * On native Wayland the compositor ignores the position half of setBounds
 * (clients cannot place themselves). Size still applies.
 */
export function applyHudBounds(win: HudBoundsWindow, bounds: HudRect): boolean {
  try {
    const wasResizable = win.isResizable();
    if (!wasResizable) win.setResizable(true);
    try {
      win.setBounds(bounds);
    } finally {
      if (!wasResizable && !win.isDestroyed()) win.setResizable(false);
    }
    return true;
  } catch {
    return false;
  }
}

/** Tiny debounce helper for persisting HUD geometry after a drag/resize. */
export function debounce<T extends () => void>(fn: T, delayMs: number): T & { flush: () => void; cancel: () => void } {
  let timer: ReturnType<typeof setTimeout> | null = null;

  const debounced = (() => {
    if (timer) clearTimeout(timer);
    timer = setTimeout(() => {
      timer = null;
      fn();
    }, delayMs);
  }) as T & { flush: () => void; cancel: () => void };

  debounced.flush = () => {
    if (timer) {
      clearTimeout(timer);
      timer = null;
      fn();
    }
  };
  debounced.cancel = () => {
    if (timer) clearTimeout(timer);
    timer = null;
  };

  return debounced;
}
