/**
 * Island bar global shortcut (program "Island bar + Team map", PLAN D14).
 *
 * Electron-free so it can be unit tested: the caller passes `register` /
 * `unregister` (globalShortcut) and the saved value. Shortcuts cross the
 * bridge in the Settings recorder's spelling ("alt+space", "mod+shift+k";
 * ai `shortcut-recorder.tsx`); this file turns them into Electron
 * accelerators ("Alt+Space": Option+Space on a Mac).
 *
 * Rules:
 * - Default ⌥Space (Mac) / Alt+Space (Windows, Linux).
 * - At startup the saved key is tried, then the default, then
 *   ⌥⇧Space / Alt+Shift+Space. A key held by another app is never taken.
 * - If the picked key is not the live one, one notice asks the user to pick
 *   a shortcut (once per picked key, remembered across launches).
 * - A key picked in Settings that cannot be registered is not saved: the old
 *   key stays live and the reply carries `rejected` so Settings says why.
 * - Linux Wayland: Electron has no GlobalShortcuts portal here, so the reason
 *   is `wayland` (Settings points at the tray and allternit://bar).
 * - Until `start()` (and after `stop()`) nothing is held and the reason is
 *   `off`: the Desktop flag `bar.island` is off, so ⌥Space keeps typing a
 *   non-breaking space for everyone else.
 */

export const ISLAND_HOTKEY_DEFAULT = 'alt+space';
export const ISLAND_HOTKEY_FALLBACK = 'alt+shift+space';

/** Why the shortcut is (or is not) live. Mirrored in ai `shell-shortcuts.ts`. */
export type IslandHotkeyReason = 'ok' | 'fallback' | 'taken' | 'wayland' | 'off';

export interface IslandHotkeyStatus {
  /** What the user picked (or the default), recorder spelling. */
  shortcut: string;
  /** What the OS actually gave us, or null when nothing is registered. */
  active: string | null;
  reason: IslandHotkeyReason;
  /** Only on a `setHotkey` reply: the key that could not be used. The old key stays live. */
  rejected?: { shortcut: string; why: 'taken' | 'invalid' };
}

const MODIFIERS: Record<string, string> = {
  mod: 'CommandOrControl',
  ctrl: 'Control',
  alt: 'Alt',
  shift: 'Shift',
  meta: 'Super',
};
const NAMED_KEYS: Record<string, string> = {
  space: 'Space', enter: 'Enter', escape: 'Escape', backspace: 'Backspace', delete: 'Delete', tab: 'Tab',
  up: 'Up', down: 'Down', left: 'Left', right: 'Right', home: 'Home', end: 'End', pageup: 'PageUp', pagedown: 'PageDown',
  '+': 'Plus',
};
const PUNCTUATION = new Set([',', '.', '/', ';', "'", '[', ']', '\\', '-', '=', '`']);

/**
 * Recorder spelling → Electron accelerator, or null when it cannot be a
 * global shortcut (no key, unknown key, or no modifier other than Shift).
 */
export function toAccelerator(shortcut: string): string | null {
  const parts = shortcut.trim().toLowerCase().split('+').filter(Boolean);
  // "mod++" spells the plus key.
  if (shortcut.trim().endsWith('++')) parts.push('+');
  const key = parts.pop();
  if (!key) return null;
  const mods: string[] = [];
  for (const part of parts) {
    const mod = MODIFIERS[part];
    if (!mod) return null;
    if (!mods.includes(mod)) mods.push(mod);
  }
  if (!mods.some((m) => m !== 'Shift')) return null;
  let electronKey: string | null = null;
  if (NAMED_KEYS[key]) electronKey = NAMED_KEYS[key];
  else if (/^[a-z0-9]$/.test(key)) electronKey = key.toUpperCase();
  else if (/^f([1-9]|1[0-9]|2[0-4])$/.test(key)) electronKey = key.toUpperCase();
  else if (PUNCTUATION.has(key)) electronKey = key;
  if (!electronKey) return null;
  return [...mods, electronKey].join('+');
}

/** The keys tried at startup, in order, without repeats. */
export function islandHotkeyCandidates(picked: string): string[] {
  return [...new Set([picked, ISLAND_HOTKEY_DEFAULT, ISLAND_HOTKEY_FALLBACK])];
}

/** Text for the one-time "pick your shortcut" notice. */
export function islandHotkeyNoticeBody(status: IslandHotkeyStatus, label: (shortcut: string) => string): string {
  const picked = label(status.shortcut);
  if (status.active) {
    return `${picked} is used by another app, so ${label(status.active)} opens the Island bar. Pick your shortcut in Settings > Shortcuts.`;
  }
  return `${picked} is used by another app, so the Island bar has no shortcut. Pick one in Settings > Shortcuts, or open it from the tray.`;
}

/** "alt+shift+space" → "⌥⇧Space" (Mac) or "Alt+Shift+Space". For notices and logs. */
export function islandHotkeyLabel(shortcut: string, platform: string): string {
  const mac = platform === 'darwin';
  const parts = shortcut.toLowerCase().split('+').filter(Boolean);
  const key = parts.pop() ?? '';
  const keyLabel = NAMED_KEYS[key] ?? key.toUpperCase();
  const macSym: Record<string, string> = { ctrl: '⌃', alt: '⌥', shift: '⇧', mod: '⌘', meta: '⌘' };
  const otherName: Record<string, string> = { mod: 'Ctrl', ctrl: 'Ctrl', alt: 'Alt', shift: 'Shift', meta: 'Win' };
  if (mac) {
    const order = ['ctrl', 'alt', 'shift', 'mod', 'meta'];
    return order.filter((m) => parts.includes(m)).map((m) => macSym[m]).join('') + keyLabel;
  }
  const order = ['mod', 'ctrl', 'alt', 'shift', 'meta'];
  return [...new Set(order.filter((m) => parts.includes(m)).map((m) => otherName[m]))].concat(keyLabel).join('+');
}

export interface IslandHotkeyDeps {
  platform: string;
  /** `XDG_SESSION_TYPE`. */
  sessionType: string | undefined;
  /** globalShortcut.register; may throw on a bad accelerator. */
  register: (accelerator: string, onPress: () => void) => boolean;
  unregister: (accelerator: string) => void;
  /** Saved pick (recorder spelling), or undefined for the default. */
  readSaved: () => string | undefined;
  /** Save a pick; undefined clears it (back to the default). */
  save: (shortcut: string | undefined) => void;
  /** Picked key the "pick your shortcut" notice was last shown for. */
  readNoticeShownFor: () => string | undefined;
  saveNoticeShownFor: (shortcut: string | undefined) => void;
  notify: (title: string, body: string) => void;
  /** The hotkey was pressed. */
  onPress: () => void;
  /** Status changed (push to every renderer). */
  onChange?: (status: IslandHotkeyStatus) => void;
  log?: (message: string) => void;
}

export interface IslandHotkey {
  /** Register (saved → default → fallback). Call when the bar turns on. */
  start: () => IslandHotkeyStatus;
  status: () => IslandHotkeyStatus;
  /** Settings picked a key; null/'' restores the default. */
  set: (shortcut: string | null) => IslandHotkeyStatus;
  /** Release the key while Settings records one, so pressing it does not open the bar. */
  suspend: (suspended: boolean) => void;
  /** Release the key; the reason becomes `off`. Call when the bar turns off. */
  stop: () => IslandHotkeyStatus;
}

export function createIslandHotkey(deps: IslandHotkeyDeps): IslandHotkey {
  const wayland = deps.platform === 'linux' && deps.sessionType === 'wayland';
  let picked = cleanSaved(deps.readSaved());
  let active: string | null = null;
  let suspended = false;
  let off = true;

  function tryRegister(shortcut: string): boolean {
    const accel = toAccelerator(shortcut);
    if (!accel) return false;
    try {
      return deps.register(accel, deps.onPress) === true;
    } catch (err) {
      deps.log?.(`[IslandBar] register ${accel} threw: ${String(err)}`);
      return false;
    }
  }

  function release(shortcut: string | null): void {
    const accel = shortcut ? toAccelerator(shortcut) : null;
    if (!accel) return;
    try { deps.unregister(accel); } catch { /* already gone */ }
  }

  function reasonFor(): IslandHotkeyReason {
    if (off) return 'off';
    if (wayland) return 'wayland';
    if (active === picked) return 'ok';
    return active ? 'fallback' : 'taken';
  }

  function status(): IslandHotkeyStatus {
    return { shortcut: picked, active, reason: reasonFor() };
  }

  function registerChain(): void {
    active = null;
    for (const candidate of islandHotkeyCandidates(picked)) {
      if (tryRegister(candidate)) { active = candidate; break; }
      deps.log?.(`[IslandBar] ${candidate} is taken or invalid`);
    }
  }

  /** One notice per picked key that could not be used. */
  function maybeNotify(s: IslandHotkeyStatus): void {
    if (s.reason !== 'fallback' && s.reason !== 'taken') return;
    if (deps.readNoticeShownFor() === s.shortcut) return;
    deps.saveNoticeShownFor(s.shortcut);
    deps.notify('Pick your Island bar shortcut', islandHotkeyNoticeBody(s, (k) => islandHotkeyLabel(k, deps.platform)));
  }

  function emit(): IslandHotkeyStatus {
    const s = status();
    deps.onChange?.(s);
    return s;
  }

  return {
    start() {
      if (!off) return status();
      off = false;
      suspended = false;
      registerChain();
      const s = status();
      maybeNotify(s);
      return emit();
    },
    status,
    set(next) {
      if (off) return status();
      const wanted = next && next.trim() ? next.trim().toLowerCase() : ISLAND_HOTKEY_DEFAULT;
      if (!toAccelerator(wanted)) return { ...status(), rejected: { shortcut: wanted, why: 'invalid' } };
      if (wanted === active) {
        picked = wanted;
        deps.save(wanted === ISLAND_HOTKEY_DEFAULT ? undefined : wanted);
        return emit();
      }
      // Try the new key while the old one is still held; swap only on success.
      if (!tryRegister(wanted)) return { ...status(), rejected: { shortcut: wanted, why: 'taken' } };
      if (!suspended) release(active);
      else release(wanted); // still recording: hold nothing until resume
      picked = wanted;
      active = wanted;
      deps.save(wanted === ISLAND_HOTKEY_DEFAULT ? undefined : wanted);
      deps.saveNoticeShownFor(undefined);
      return emit();
    },
    suspend(on) {
      if (off || on === suspended) return;
      suspended = on;
      if (on) {
        release(active);
        return;
      }
      if (active && !tryRegister(active)) {
        // Another app grabbed it while we were recording; run the chain again.
        registerChain();
        emit();
      }
    },
    stop() {
      if (off) return status();
      if (!suspended) release(active);
      active = null;
      suspended = false;
      off = true;
      return emit();
    },
  };
}

function cleanSaved(saved: string | undefined): string {
  const value = typeof saved === 'string' ? saved.trim().toLowerCase() : '';
  return value && toAccelerator(value) ? value : ISLAND_HOTKEY_DEFAULT;
}
