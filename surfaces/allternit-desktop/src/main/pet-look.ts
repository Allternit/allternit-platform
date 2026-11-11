// pet-look.ts — the Look step of the desktop pet's offer loop: read one other
// app's window through the bundled Allternit Driver (arc on macOS, UIA on
// Windows) and hand back its text for the Notice step (pet-notice.ts).
//
// Order of checks (plan decision 4). Each step stops the look:
//   1. App block list and mute list, from the OS window list alone. A blocked
//      app is never sent to the driver at all.
//   2. Driver `context`: the real app name, bundle id, window title and page URL.
//      Block list again on those, then private-window titles, banking hosts,
//      and a browser whose title cannot be read.
//   3. The same window and title read under 30 s ago: skip.
//   4. Driver `read_ui` (120 elements, vision off). Secure fields are dropped.
//
// Text is tail-biased (decision 10): the last tailChars of the largest text
// element (terminals keep their newest lines at the bottom) goes last, the
// rest goes before it, all cut to maxTextChars keeping the end.
//
// Privacy: nothing here logs, and the text only ever goes back to the caller.
// Platforms: macOS and Windows. Linux reads nothing: AT-SPI reports password
// fields as plain text fields, so they could not be dropped.

import {
  PET_OFFER_BLOCK_DEFAULTS,
  PET_OFFER_DIALOG_ROLES,
  PET_OFFER_LIMITS,
  type PetOfferSettings,
} from './pet-offers.gen.js';
import { listNamesApp, offerWindowKey } from './pet-notice.js';

/** One OS window, front-to-back order, from window-below.ts's enumerator. */
export interface LookWindow {
  app: string;
  /** Bundle id on macOS, exe file name on Windows, "" when unknown. */
  appId: string;
  pid: number;
  id: number;
  title: string;
  bounds: { x: number; y: number; width: number; height: number };
}

export interface LookResult {
  app: string;
  appId: string;
  pid: number;
  title: string;
  windowKey: string;
  bounds: LookWindow['bounds'];
  text: string;
  /** The text came from a window or element with a dialog role. */
  dialog: boolean;
}

export interface LookDeps {
  /** A driver call; rejects when the driver is not running. */
  driver: (method: string, params: Record<string, unknown>) => Promise<unknown>;
  /** Called right before the window is read (the pet's eye). */
  onRead?: () => void;
  platform?: NodeJS.Platform;
}

/** Why a look stopped, for tests and counters. Never carries screen text. */
export type LookSkip =
  | 'platform' | 'blocked_app' | 'muted' | 'no_driver' | 'private_window' | 'blocked_host'
  | 'browser_no_title' | 'recent' | 'no_text';

const BROWSERS = [
  'safari', 'google chrome', 'chrome', 'chromium', 'firefox', 'microsoft edge', 'msedge', 'arc', 'brave browser', 'brave',
  'opera', 'vivaldi', 'orion', 'duckduckgo', 'zen', 'zen browser', 'tor browser', 'sigmaos', 'dia',
];
const BROWSER_IDS = [
  'com.apple.safari', 'com.apple.safaritechnologypreview', 'com.google.chrome', 'org.chromium.chromium', 'org.mozilla.firefox',
  'com.microsoft.edgemac', 'company.thebrowser.browser', 'com.brave.browser', 'com.operasoftware.opera', 'com.vivaldi.vivaldi',
  'com.kagi.kagimacos', 'com.duckduckgo.macos.browser', 'app.zen-browser.zen', 'company.thebrowser.dia',
  'chrome.exe', 'msedge.exe', 'firefox.exe', 'brave.exe', 'opera.exe', 'vivaldi.exe', 'arc.exe',
];

const lower = (s: string) => s.trim().toLowerCase();
const wordIn = (haystack: string, word: string) => new RegExp(`(^|[^a-z0-9])${word.replace(/[.*+?^${}()|[\]\\]/g, '\\$&')}([^a-z0-9]|$)`).test(haystack);

/** Step 1 (and again after `context`): the user's block list plus the contract's defaults. */
export function appBlocked(app: string, appId: string, userBlockList: readonly string[], platform: NodeJS.Platform = process.platform): boolean {
  if (listNamesApp(userBlockList, app, appId)) return true;
  const d = PET_OFFER_BLOCK_DEFAULTS;
  const ids = [...d.passwordManagerAppIds, ...d.messagesMacAppIds, ...d.messagesAndroidPackages];
  if (listNamesApp(d.passwordManagerNames, app, '') || listNamesApp(ids, '', appId)) return true;
  // The contract lists Messages by bundle id only; its name blocks it too on a Mac.
  if (platform === 'darwin' && lower(app) === 'messages') return true;
  const name = lower(app);
  return name.length > 0 && d.bankingNameKeywords.some(k => wordIn(name, k.toLowerCase()));
}

/** Private and incognito windows, by their title. */
export function privateWindowTitle(title: string): boolean {
  const t = lower(title);
  return PET_OFFER_BLOCK_DEFAULTS.privateWindowTitleSubstrings.some(p => t.includes(p.toLowerCase()));
}

/** Banking sites: a listed host or any subdomain of it, or a host label with a banking word. */
export function bankingHost(host: string): boolean {
  const h = lower(host).replace(/\.$/, '');
  if (!h) return false;
  if (PET_OFFER_BLOCK_DEFAULTS.bankingHostSuffixes.some(s => h === s || h.endsWith(`.${s}`))) return true;
  const labels = h.split('.').slice(0, -1);
  return labels.some(label => PET_OFFER_BLOCK_DEFAULTS.bankingHostLabelKeywords.some(k => label.includes(k)));
}

export function isBrowser(app: string, appId: string): boolean {
  return BROWSERS.includes(lower(app)) || BROWSER_IDS.includes(lower(appId));
}

/** The window under the pet's centre if there is one, else the frontmost other-app window. */
export function pickLookTarget(windows: readonly LookWindow[], petCentre: { x: number; y: number } | null): LookWindow | null {
  const usable = windows.filter(w => w.bounds.width > 40 && w.bounds.height > 40);
  if (petCentre) {
    const under = usable.find(w => petCentre.x >= w.bounds.x && petCentre.x < w.bounds.x + w.bounds.width
      && petCentre.y >= w.bounds.y && petCentre.y < w.bounds.y + w.bounds.height);
    if (under) return under;
  }
  return usable[0] ?? null;
}

const normRole = (role: unknown) => (typeof role === 'string' ? role.replace(/^AX/, '').toLowerCase() : '');
const DIALOG_ROLES = new Set([
  ...PET_OFFER_DIALOG_ROLES.macRoles, ...PET_OFFER_DIALOG_ROLES.macSubroles, ...PET_OFFER_DIALOG_ROLES.windowsLocalizedControlTypes,
].map(normRole));
/** Values a secure field shows instead of its contents. */
const MASKED = /^[•●◦∙*]+$/;

interface UiElement { role?: unknown; name?: unknown; value?: unknown }

/** Tail-biased text from a read_ui answer; secure fields dropped. Pure. */
export function lookText(elements: readonly UiElement[], limits: { tailChars: number; maxTextChars: number }): { text: string; dialog: boolean } {
  let dialog = false;
  const parts: string[] = [];
  for (const el of elements) {
    const role = normRole(el.role);
    if (DIALOG_ROLES.has(role)) dialog = true;
    if (role.includes('secure') || role.includes('password')) continue;
    const name = typeof el.name === 'string' ? el.name.trim() : '';
    const value = typeof el.value === 'string' ? el.value : typeof el.value === 'number' ? String(el.value) : '';
    if (value && MASKED.test(value.trim())) continue;
    const text = value.trim() && value.trim() !== name ? (name ? `${name}\n${value}` : value) : name;
    if (text) parts.push(text);
  }
  if (!parts.length) return { text: '', dialog };
  let largest = 0;
  for (let i = 1; i < parts.length; i++) if (parts[i].length > parts[largest].length) largest = i;
  const big = parts[largest];
  const tail = big.length > limits.tailChars ? big.slice(big.length - limits.tailChars) : big;
  const rest = parts.filter((_, i) => i !== largest).join('\n');
  const joined = rest ? `${rest}\n${tail}` : tail;
  return { text: joined.length > limits.maxTextChars ? joined.slice(joined.length - limits.maxTextChars) : joined, dialog };
}

/** When each window (windowKey) was last read, to skip a re-read under 30 s. Bounded. */
export class LookMemo {
  private seen = new Map<string, number>();
  recent(key: string, now: number): boolean {
    const at = this.seen.get(key);
    return at !== undefined && now - at < PET_OFFER_LIMITS.sameWindowRereadMs;
  }
  mark(key: string, now: number): void {
    this.seen.delete(key);
    this.seen.set(key, now);
    while (this.seen.size > 200) this.seen.delete(this.seen.keys().next().value as string);
  }
}

const str = (v: unknown) => (typeof v === 'string' ? v : '');

/** One look at `target`. Resolves with the window's text, or the reason it stopped. Never throws. */
export async function lookAt(
  target: LookWindow,
  settings: PetOfferSettings,
  memo: LookMemo,
  deps: LookDeps,
  now = Date.now(),
): Promise<LookResult | { skip: LookSkip }> {
  const platform = deps.platform ?? process.platform;
  if (platform !== 'darwin' && platform !== 'win32') return { skip: 'platform' };
  // 1. Before any driver call.
  if (appBlocked(target.app, target.appId, settings.blockList, platform)) return { skip: 'blocked_app' };
  if (listNamesApp(settings.muted, target.app, target.appId)) return { skip: 'muted' };

  // 2. Title and URL.
  let ctx: Record<string, unknown>;
  try {
    const answer = await deps.driver('context', { pid: target.pid, url: true });
    ctx = answer && typeof answer === 'object' ? answer as Record<string, unknown> : {};
  } catch {
    return { skip: 'no_driver' };
  }
  // The pid must still be the app we checked; a reused pid could be anything.
  if (typeof ctx.pid === 'number' && ctx.pid !== target.pid) return { skip: 'no_driver' };
  const app = str(ctx.app) || target.app;
  const appId = str(ctx.bundle_id) || target.appId;
  if (appBlocked(app, appId, settings.blockList, platform)) return { skip: 'blocked_app' };
  if (listNamesApp(settings.muted, app, appId)) return { skip: 'muted' };
  const title = str(ctx.title) || target.title;
  if (privateWindowTitle(title)) return { skip: 'private_window' };
  const host = str(ctx.host);
  if (host && bankingHost(host)) return { skip: 'blocked_host' };
  if ((isBrowser(app, appId) || str(ctx.url)) && !title.trim()) return { skip: 'browser_no_title' };

  // 3. Same window, same title, under 30 s ago.
  const windowKey = offerWindowKey(appId || app, title);
  if (memo.recent(windowKey, now)) return { skip: 'recent' };
  memo.mark(windowKey, now);

  // 4. The tree.
  deps.onRead?.();
  let elements: UiElement[] = [];
  try {
    const answer = await deps.driver('read_ui', { pid: target.pid, max_elements: 120, vision: 'off' }) as { elements?: unknown };
    if (Array.isArray(answer?.elements)) elements = answer.elements as UiElement[];
  } catch {
    return { skip: 'no_driver' };
  }
  const { text, dialog } = lookText(elements, settings);
  if (!text) return { skip: 'no_text' };
  return { app, appId, pid: target.pid, title, windowKey, bounds: target.bounds, text, dialog };
}
