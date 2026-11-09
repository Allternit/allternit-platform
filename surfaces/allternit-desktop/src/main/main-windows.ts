/**
 * Allternit Desktop — registry of main (workspace) windows.
 *
 * New Window (Cmd/Ctrl+Shift+N) opens another workspace window on the same
 * account and the same stores (one session, one set of local servers); only
 * the window itself is new. Each window gets a number and a tint dot so the
 * Window menu can tell them apart.
 *
 * `getFocusedMainWindow()` is the window the user is in (else the last one
 * focused). Code that acts on "the" main window uses it; code that reports
 * state (engine status, tunnel, permissions) uses `broadcastToMainWindows`.
 */
import type { BrowserWindow } from 'electron';

/** Tint dots, in the order windows take them. Emoji so a native menu shows the colour. */
export const WINDOW_TINTS = ['\u{1F535}', '\u{1F7E2}', '\u{1F7E3}', '\u{1F7E0}', '\u{1F534}', '\u{1F7E1}'] as const;

export interface MainWindowEntry {
  window: BrowserWindow;
  /** 1-based; the lowest number not in use. */
  number: number;
  tint: string;
}

const entries: MainWindowEntry[] = [];
const listeners = new Set<() => void>();
let lastFocused: BrowserWindow | null = null;

function notify(): void {
  for (const listener of listeners) listener();
}

function live(entry: MainWindowEntry): boolean {
  return !entry.window.isDestroyed();
}

export function registerMainWindow(window: BrowserWindow): MainWindowEntry {
  const used = new Set(entries.filter(live).map((e) => e.number));
  let number = 1;
  while (used.has(number)) number += 1;
  const entry: MainWindowEntry = { window, number, tint: WINDOW_TINTS[(number - 1) % WINDOW_TINTS.length] };
  entries.push(entry);
  lastFocused = window;
  notify();
  return entry;
}

export function unregisterMainWindow(window: BrowserWindow): void {
  const index = entries.findIndex((e) => e.window === window);
  if (index >= 0) entries.splice(index, 1);
  if (lastFocused === window) lastFocused = null;
  notify();
}

export function noteMainWindowFocused(window: BrowserWindow): void {
  lastFocused = window;
  notify();
}

export function listMainWindows(): MainWindowEntry[] {
  return entries.filter(live);
}

export function getMainWindowEntry(window: BrowserWindow | null | undefined): MainWindowEntry | undefined {
  return window ? entries.find((e) => e.window === window) : undefined;
}

/** The main window the user is in; else the one focused last; else the newest. */
export function getFocusedMainWindow(): BrowserWindow | null {
  const open = listMainWindows();
  const focused = open.find((e) => e.window.isFocused());
  if (focused) return focused.window;
  if (lastFocused && !lastFocused.isDestroyed() && open.some((e) => e.window === lastFocused)) return lastFocused;
  return open.length ? open[open.length - 1].window : null;
}

/** Send one IPC message to every open main window. */
export function broadcastToMainWindows(channel: string, ...args: unknown[]): void {
  for (const entry of listMainWindows()) {
    if (!entry.window.webContents.isDestroyed()) entry.window.webContents.send(channel, ...args);
  }
}

/** Called when the set, the focus or a title of main windows changes. */
export function onMainWindowsChanged(listener: () => void): () => void {
  listeners.add(listener);
  return () => listeners.delete(listener);
}
