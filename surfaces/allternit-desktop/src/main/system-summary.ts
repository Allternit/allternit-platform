/**
 * The system drawer's summary as Desktop sees it. The app window reports it
 * (`companion:system-report`) while Settings → Developer → System drawer is
 * on; the Gizzi pet draws a dot from it, hovering that dot shows `bubbleText`
 * above the pet, and the menu bar menu lists `items` in a System section.
 * Nothing here opens anything by itself.
 */

export type SystemTab = 'needs-you' | 'bots' | 'running' | 'problems';

export interface SystemItem {
  tab: SystemTab;
  text: string;
  tone: 'warning' | 'error';
}

export interface SystemSummary {
  enabled: boolean;
  needsYou: number;
  problems: number;
  running: number;
  items: SystemItem[];
}

export const NO_SYSTEM: SystemSummary = { enabled: false, needsYou: 0, problems: 0, running: 0, items: [] };

const TABS: readonly SystemTab[] = ['needs-you', 'bots', 'running', 'problems'];
const MAX_ITEMS = 5;
const MAX_TEXT = 80;

const count = (v: unknown) => (typeof v === 'number' && Number.isFinite(v) && v > 0 ? Math.min(Math.floor(v), 999) : 0);

/** The renderer's report, checked: anything malformed reads as "off". */
export function sanitizeSummary(raw: unknown): SystemSummary {
  if (!raw || typeof raw !== 'object') return NO_SYSTEM;
  const r = raw as Record<string, unknown>;
  if (r.enabled !== true) return NO_SYSTEM;
  const items = (Array.isArray(r.items) ? r.items : [])
    .map((i): SystemItem | null => {
      if (!i || typeof i !== 'object') return null;
      const { tab, text, tone } = i as Record<string, unknown>;
      if (!TABS.includes(tab as SystemTab) || typeof text !== 'string' || !text.trim()) return null;
      const clean = text.replace(/\s+/g, ' ').trim();
      return { tab: tab as SystemTab, text: clean.length > MAX_TEXT ? `${clean.slice(0, MAX_TEXT - 1)}…` : clean, tone: tone === 'error' ? 'error' : 'warning' };
    })
    .filter((i): i is SystemItem => i !== null)
    .slice(0, MAX_ITEMS);
  return { enabled: true, needsYou: count(r.needsYou), problems: count(r.problems), running: count(r.running), items };
}

/** The pet's dot: amber when something needs you, red when only problems, none otherwise. */
export function dotTone(s: SystemSummary): 'warning' | 'error' | null {
  if (!s.enabled) return null;
  if (s.needsYou > 0) return 'warning';
  return s.problems > 0 ? 'error' : null;
}

/** The tab a click on the dot opens: what needs you first, then what broke. */
export function dotTab(s: SystemSummary): SystemTab {
  return s.needsYou > 0 ? 'needs-you' : s.problems > 0 ? 'problems' : 'bots';
}

/** What hovering the dot says, in two short lines at most. */
export function bubbleText(s: SystemSummary): string {
  const parts = [
    s.needsYou ? `${s.needsYou} need${s.needsYou === 1 ? 's' : ''} you` : '',
    s.problems ? `${s.problems} problem${s.problems === 1 ? '' : 's'}` : '',
    s.running ? `${s.running} running` : '',
  ].filter(Boolean);
  const first = s.items[0]?.text;
  const head = parts.join(' · ') || 'Nothing waiting';
  return first ? `${head}: ${first}. Click the dot to open.` : `${head}. Click the dot to open.`;
}
