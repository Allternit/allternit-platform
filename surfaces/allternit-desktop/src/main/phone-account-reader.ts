/**
 * Settings → Connections, phone ways: read which account an AI chat app on a
 * linked phone is signed in to, its plan, and its usage windows (5-hour,
 * Weekly, …), the same facts the browser sign-ins show
 * (IPC `phone-link:read-account`).
 *
 * Read-only by design. The reader opens the app, walks the vendor's
 * navigation spec (phone-account-specs.ts) by tapping only elements whose
 * whole label matches a spec candidate and passes the deny-list, reads the
 * screen text, and goes Home at the end. It never types, never toggles,
 * never taps anything that signs out, deletes, pays or changes a plan.
 *
 * Parsing is evidence-only: a field the screen didn't plainly show is null.
 * Results are saved on the phone's app record (phone-link:devices) and a
 * background scheduler refreshes each signed-in AI chat app every 6 h on a
 * healthy, unlocked phone that no bot is using, one app at a time.
 */
import { catalogApp, PROBE_CATEGORY } from './phone-app-catalog.js';
import {
  launch,
  readScreen,
  resolve as resolveLinkedApp,
  screenLabels,
  SETTLE_MS,
  toolError,
  visibleAccount,
  type LinkedAppLane,
  type Resolved,
} from './phone-app-signin.js';
import { accountSpec, deniedElement, type AccountTapStep, type PhoneAccountSpec } from './phone-account-specs.js';
import type { AppAccountInfo } from './phone-link-backend.js';
import type { LinkedPhone } from './phone-link-health.js';
import type { PhoneLinkAccountResult, PhoneLinkError, PhoneLinkHealth, PhoneLinkUsageWindow } from './phone-link-types.js';
import type { ToolResult } from './phone-tools.js';

export type { PhoneLinkAccountResult };

// ── Parsing (pure) ──────────────────────────────────────────────────────────

export interface ParsedAccountScreen {
  account: string | null;
  plan: string | null;
  windows: PhoneLinkUsageWindow[];
}

function norm(label: string): string {
  return label.toLowerCase().replace(/[’]/g, "'").replace(/\s+/g, ' ').replace(/[,:;·•]+$/, '').trim();
}

function escapeRe(value: string): string {
  return value.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
}

/** Labels that put a bare plan name in context ("Plan" then "Pro" on the next line). */
const PLAN_CONTEXT = /^(your |current )?(plan|subscription|membership|account type|tier)$/;
/** A label with one of these is an offer, not the person's plan. */
const OFFER = /\b(upgrade|get|try|buy|subscribe|switch|compare|see|explore|unlock|start|join)\b|\$|€|£|\/ ?(mo|month|yr|year)\b|per month/;

/**
 * The plan the screen plainly shows, else null. Accepted forms: a whole
 * label "<Vendor> <Plan>", "<Plan> plan", "Plan: <Plan>", a multi-word plan
 * name ("Google AI Pro"), or a one-word "<Plan>" within two labels after
 * "Plan" / "Subscription" / "Current plan". Two
 * different plans on one screen (a plan comparison) → null.
 */
export function parsePlan(labels: string[], spec: Pick<PhoneAccountSpec, 'plans' | 'vendorWords'>): string | null {
  const plans = [...spec.plans].sort((a, b) => b.length - a.length);
  const vendor = spec.vendorWords.map((w) => escapeRe(w.toLowerCase())).join('|');
  const found = new Set<string>();
  const normed = labels.map(norm);
  normed.forEach((label, i) => {
    if (OFFER.test(label)) return;
    for (const plan of plans) {
      const p = escapeRe(plan.toLowerCase());
      const framed = new RegExp(
        `^(?:(?:your |current )?(?:plan|subscription|membership)\\s*[:\\-–]\\s*)?(?:(?:${vendor}) )?${p}(?: (?:plan|subscription|membership|member|subscriber))?(?: \\(current\\)| · current| current plan)?$`,
      );
      if (!framed.test(label)) continue;
      // A one-word bare name ("Pro", "SuperGrok") is also an upsell tile's title, so it needs a plan label nearby.
      const bare = label === plan.toLowerCase() && !plan.includes(' ');
      const inContext = normed.slice(Math.max(0, i - 2), i).some((before) => PLAN_CONTEXT.test(before));
      if (!bare || inContext) {
        found.add(plan);
        return;
      }
    }
  });
  return found.size === 1 ? [...found][0] : null;
}

type WindowKind = { id: string; label: string };

/** Which usage window a header label names, else null. */
function windowHeader(label: string): WindowKind | null {
  if (/^(resets?|renews?)\b/.test(label)) return null;
  if (label.length > 90) return null;
  const model = /\b(opus|sonnet|haiku)\b/.exec(label)?.[1];
  const withModel = (id: string, name: string): WindowKind =>
    model ? { id: `${id}_${model}`, label: `${name} (${model[0].toUpperCase()}${model.slice(1)})` } : { id, label: name };
  if (/\bcurrent session\b|\b5[- ]?(hour|hr|h)\b|\bfive[- ]hour\b/.test(label)) return withModel('five_hour', '5-hour');
  if (/\bweekly\b|\bthis week\b|\b7[- ]day\b|\bper week\b/.test(label)) return withModel('weekly', 'Weekly');
  if (/\bdaily\b|\btoday'?s? (limit|usage|messages|queries)\b|\b24[- ]hour\b|\bper day\b/.test(label)) return withModel('daily', 'Daily');
  if (/\bmonthly\b|\bthis month\b|\bper month\b/.test(label)) return withModel('monthly', 'Monthly');
  return null;
}

const num = (s: string) => Number(s.replace(/,/g, ''));
const clampPct = (n: number) => Math.round(Math.min(100, Math.max(0, n)));

/** Percent left from "34% used", "66% left", "12 of 40 messages left", "28/40 used"; null when not shown. */
export function remainingPct(text: string): number | null {
  const t = text.toLowerCase();
  let m = /(\d{1,3}(?:\.\d+)?)\s*%\s*(?:used|consumed)/.exec(t);
  if (m) return clampPct(100 - Number(m[1]));
  m = /(\d{1,3}(?:\.\d+)?)\s*%\s*(?:left|remaining|available)/.exec(t);
  if (m) return clampPct(Number(m[1]));
  m = /(\d[\d,]*)\s*(?:of|\/)\s*(\d[\d,]*)\s*(?:[a-z]+\s*){0,3}?(left|remaining|available|used)\b/.exec(t);
  if (m && num(m[2]) > 0) {
    const share = (num(m[1]) / num(m[2])) * 100;
    return clampPct(m[3] === 'used' ? 100 - share : share);
  }
  m = /used\s*(\d[\d,]*)\s*(?:of|\/)\s*(\d[\d,]*)/.exec(t);
  if (m && num(m[2]) > 0) return clampPct(100 - (num(m[1]) / num(m[2])) * 100);
  return null;
}

const WEEKDAYS = ['sun', 'mon', 'tue', 'wed', 'thu', 'fri', 'sat'];
const MONTHS = ['jan', 'feb', 'mar', 'apr', 'may', 'jun', 'jul', 'aug', 'sep', 'oct', 'nov', 'dec'];

/** "9:00 AM", "9 pm", "21:30"; null when the text has no clear time (a bare "21" isn't one). */
function clockOf(text: string): { h: number; m: number } | null {
  const m = /\b(\d{1,2})(?::(\d{2}))?\s*(a\.?m\.?|p\.?m\.?)(?![a-z])/.exec(text) ?? /\b(\d{1,2}):(\d{2})\b/.exec(text);
  if (!m) return null;
  let h = Number(m[1]);
  const min = m[2] ? Number(m[2]) : 0;
  const mer = m[3]?.replace(/\./g, '');
  if (mer) {
    if (h < 1 || h > 12) return null;
    if (mer === 'pm' && h !== 12) h += 12;
    if (mer === 'am' && h === 12) h = 0;
  }
  if (h > 23 || min > 59) return null;
  return { h, m: min };
}

/**
 * The reset time as ISO from what follows "Resets" ("in 2 hr 10 min",
 * "Thu 9:00 AM", "Oct 21, 9:00 AM", "at 3 PM", "tomorrow at 9 AM"), in the
 * computer's local time (the phone's, for a phone on the same person's desk);
 * null when unparseable. Relative times don't depend on the time zone.
 */
export function parseResetTime(raw: string, now: Date): string | null {
  const text = raw.toLowerCase().replace(/[’]/g, "'").replace(/\s+/g, ' ').trim();
  if (/^in\b/.test(text)) {
    const re = /(\d+(?:\.\d+)?)\s*(d|days?|h|hrs?|hours?|m|mins?|minutes?)\b/g;
    let ms = 0;
    let hit = false;
    for (let m = re.exec(text); m; m = re.exec(text)) {
      hit = true;
      const n = Number(m[1]);
      const unit = m[2][0];
      ms += unit === 'd' ? n * 86_400_000 : unit === 'h' ? n * 3_600_000 : n * 60_000;
    }
    return hit ? new Date(now.getTime() + ms).toISOString() : null;
  }
  const clock = clockOf(text);
  const at = (d: Date, c: { h: number; m: number } | null) => new Date(d.getFullYear(), d.getMonth(), d.getDate(), c?.h ?? 0, c?.m ?? 0, 0, 0);

  const month = new RegExp(`\\b(${MONTHS.join('|')})[a-z]*\\.? (\\d{1,2})(?:st|nd|rd|th)?\\b(?:,? (\\d{4}))?`).exec(text);
  if (month) {
    const mi = MONTHS.indexOf(month[1]);
    const day = Number(month[2]);
    let year = month[3] ? Number(month[3]) : now.getFullYear();
    let when = at(new Date(year, mi, day), clock);
    if (!month[3] && when.getTime() < now.getTime() - 86_400_000) when = at(new Date(++year, mi, day), clock);
    return when.getMonth() === mi ? when.toISOString() : null;
  }
  if (/\btomorrow\b/.test(text)) {
    if (!clock) return null;
    return at(new Date(now.getFullYear(), now.getMonth(), now.getDate() + 1), clock).toISOString();
  }
  // Whole weekday words only ("mon" in "month" is not Monday).
  const weekday = new RegExp(`\\b(${WEEKDAYS.join('|')})(?:day|sday|nesday|rsday|urday|s|rs)?\\b`).exec(text);
  if (weekday) {
    if (!clock) return null;
    const target = WEEKDAYS.indexOf(weekday[1]);
    let add = (target - now.getDay() + 7) % 7;
    let when = at(new Date(now.getFullYear(), now.getMonth(), now.getDate() + add), clock);
    if (when.getTime() <= now.getTime()) {
      add += 7;
      when = at(new Date(now.getFullYear(), now.getMonth(), now.getDate() + add), clock);
    }
    return when.toISOString();
  }
  if (clock && /^(at |today )?(at )?\d/.test(text)) {
    let when = at(now, clock);
    if (when.getTime() <= now.getTime()) when = at(new Date(now.getFullYear(), now.getMonth(), now.getDate() + 1), clock);
    return when.toISOString();
  }
  return null;
}

/** What follows "Resets" / "Renews" in a text, else null. */
function resetText(text: string): string | null {
  const m = /\b(?:resets?|renews?|refreshes|available again)\b:?\s*(.+)$/i.exec(text);
  return m ? m[1].trim() : null;
}

/**
 * Usage windows from a screen's labels: a header ("Current session",
 * "Weekly limits", "Daily messages") followed (in the same label or the next
 * few) by "34% used" / "12 of 40 left" and "Resets …". A header-less
 * "12 of 40 messages left" becomes a "Messages" window. Pure.
 */
export function parseUsageWindows(labels: string[], now: Date): PhoneLinkUsageWindow[] {
  const normed = labels.map(norm);
  const windows: PhoneLinkUsageWindow[] = [];
  const used = new Set<number>();
  for (let i = 0; i < normed.length; i++) {
    const kind = windowHeader(normed[i]);
    if (!kind || windows.some((w) => w.id === kind.id)) continue;
    let remaining: number | null = null;
    let resets: string | null = null;
    for (let j = i; j < Math.min(normed.length, i + 6); j++) {
      if (j > i && windowHeader(normed[j])) break;
      const text = normed[j];
      if (remaining === null) remaining = remainingPct(text);
      if (resets === null) {
        const after = resetText(text);
        if (after) resets = parseResetTime(after, now);
      }
      used.add(j);
    }
    if (remaining !== null || resets !== null) windows.push({ ...kind, remaining_pct: remaining, resets_at: resets });
  }
  if (!windows.length) {
    normed.forEach((text, i) => {
      if (used.has(i) || windows.length) return;
      if (!/\b(messages?|queries|prompts|requests|uses)\b/.test(text)) return;
      const remaining = remainingPct(text);
      if (remaining === null) return;
      const after = resetText(text) ?? (normed[i + 1] ? resetText(normed[i + 1]) : null);
      windows.push({ id: 'messages', label: 'Messages', remaining_pct: remaining, resets_at: after ? parseResetTime(after, now) : null });
    });
  }
  return windows;
}

/** Account, plan and usage windows from one screen's text (screen_text / read_ui output). Pure; unit-tested. */
export function parseAccountScreen(text: string, spec: Pick<PhoneAccountSpec, 'plans' | 'vendorWords'>, now: Date): ParsedAccountScreen {
  const labels = screenLabels(text);
  return { account: visibleAccount(labels), plan: parsePlan(labels, spec), windows: parseUsageWindows(labels, now) };
}

/** First non-null account/plan wins; windows are united by id (first seen wins). */
export function mergeParsed(a: ParsedAccountScreen, b: ParsedAccountScreen): ParsedAccountScreen {
  return {
    account: a.account ?? b.account,
    plan: a.plan ?? b.plan,
    windows: [...a.windows, ...b.windows.filter((w) => !a.windows.some((had) => had.id === w.id))],
  };
}

// ── Screen elements (for picking what to tap) ───────────────────────────────

export interface ScreenElement {
  className: string;
  label: string;
  id: string;
}

/** read_ui lines (`class "label" [id] {{x, y}, {w, h}}`) or screen_text lines (`label @ (x, y)`). */
export function screenElements(text: string): ScreenElement[] {
  const out: ScreenElement[] = [];
  for (const raw of text.split('\n')) {
    const line = raw.trim();
    if (!line) continue;
    const quoted = /^(\S*)\s*"([^"]*)"\s*(?:\[([^\]]*)\])?/.exec(line);
    if (quoted) {
      out.push({ className: quoted[1] ?? '', label: quoted[2], id: quoted[3] ?? '' });
      continue;
    }
    const idOnly = /^(\S+)\s+\[([^\]]+)\]/.exec(line);
    if (idOnly) {
      out.push({ className: idOnly[1], label: '', id: idOnly[2] });
      continue;
    }
    const label = screenLabels(line)[0];
    if (label) out.push({ className: '', label, id: '' });
  }
  return out;
}

/** The element to tap for a step: the first candidate (in the spec's order) on screen that isn't denied. */
export function pickTap(elements: ScreenElement[], step: Pick<AccountTapStep, 'tap'>): { element_id: string } | null {
  for (const candidate of step.tap) {
    if (candidate.startsWith('id:')) {
      const want = candidate.slice(3).toLowerCase();
      const hit = elements.find((el) => {
        const id = el.id.toLowerCase();
        return id && (id === want || id.endsWith(`:id/${want}`)) && !deniedElement(el.label, el.className);
      });
      if (hit) return { element_id: hit.id };
      continue;
    }
    const want = norm(candidate);
    const hit = elements.find((el) => el.label && norm(el.label) === want && !deniedElement(el.label, el.className));
    if (hit) return { element_id: hit.label };
  }
  return null;
}

// ── Reader ──────────────────────────────────────────────────────────────────

/** What the reader needs from the Phone Link lane (PhoneLinkBackend implements it). */
export interface AccountLane extends LinkedAppLane {
  rememberAppAccount(deviceId: string, appId: string, info: AppAccountInfo): void;
}

export interface AccountReaderDeps {
  lane: AccountLane;
  /** True while a bot is using the phone (or someone is watching its live screen): the read stops. */
  busy?: (deviceId: string) => boolean;
  sleep?: (ms: number) => Promise<void>;
  now?: () => Date;
  logger?: (message: string) => void;
}

/** Pause after a tap for the next screen to draw. */
export const TAP_SETTLE_MS = 1500;

const fail = (code: string, message: string, retryable = false): PhoneLinkAccountResult => ({
  ok: false,
  account: null,
  plan: null,
  usage: null,
  error: { code, message, retryable },
});

const fromError = (error: PhoneLinkError | undefined): PhoneLinkAccountResult =>
  fail(error?.code ?? 'phone_failed', error?.message ?? 'The phone did not answer.', error?.retryable ?? false);

class Stop extends Error {
  constructor(readonly result: PhoneLinkAccountResult) {
    super(result.error?.message ?? 'stopped');
  }
}

const STOPPING = new Set(['device_offline', 'device_locked', 'pairing_invalid', 'accessibility_off']);

/** One read at a time across all phones (the scheduler and the IPC share it). */
let queue: Promise<unknown> = Promise.resolve();

/** phone-link:read-account — open the app's account screen read-only and report account, plan, usage. */
export function readLinkedAccount(deps: AccountReaderDeps, opts: unknown): Promise<PhoneLinkAccountResult> {
  const run = queue.then(() => readOnce(deps, opts));
  queue = run.catch(() => undefined);
  return run;
}

async function readOnce(deps: AccountReaderDeps, opts: unknown): Promise<PhoneLinkAccountResult> {
  const r = resolveLinkedApp(deps.lane, opts);
  if ('ok' in r) return fromError(r.error);
  const spec = accountSpec(r.app.id);
  if (!spec) return fail('unsupported_app', `Reading the account isn't available for ${r.app.name} yet.`);
  if (deps.busy?.(r.phone.deviceId)) return fail('phone_busy', `A bot is using ${r.phone.name ?? 'the phone'} right now. Try again when it's done.`, true);
  const sleep = deps.sleep ?? ((ms: number) => new Promise<void>((done) => setTimeout(done, ms)));
  const now = deps.now ?? (() => new Date());
  const log = deps.logger ?? (() => {});

  let parsed: ParsedAccountScreen = { account: null, plan: null, windows: [] };
  try {
    for (const path of spec.paths) {
      const got = await walkPath(deps, r, spec, path.steps, sleep, now);
      parsed = mergeParsed(parsed, got);
      if (parsed.plan && parsed.windows.length > 0 && parsed.account) {
        log(`[PhoneAccount] ${r.app.id}: read via "${path.name}"`);
        break;
      }
    }
  } catch (error) {
    if (error instanceof Stop) return error.result;
    return fail('phone_failed', error instanceof Error ? error.message : String(error), true);
  } finally {
    // Always leave the app, even after a failure.
    await deps.lane.call('phone.home', r.serial, {}).catch(() => null);
  }

  const observedAt = now().toISOString();
  const usage = parsed.windows.length ? { windows: parsed.windows, observedAt } : null;
  if (parsed.account || parsed.plan || usage) {
    deps.lane.rememberAppAccount(r.phone.deviceId, r.app.id, { account: parsed.account, plan: parsed.plan, usage, accountCheckedAt: observedAt });
  }
  return { ok: true, account: parsed.account, plan: parsed.plan, usage };
}

/** One candidate path from a fresh launch. Taps only spec labels that pass the deny-list. */
async function walkPath(
  deps: AccountReaderDeps,
  r: Resolved,
  spec: PhoneAccountSpec,
  steps: PhoneAccountSpec['paths'][number]['steps'],
  sleep: (ms: number) => Promise<void>,
  now: () => Date,
): Promise<ParsedAccountScreen> {
  const lane = deps.lane;
  const checkBusy = () => {
    if (deps.busy?.(r.phone.deviceId)) throw new Stop(fail('phone_busy', 'A bot started using the phone; the account read stopped.', true));
  };
  const stopOn = (res: ToolResult) => {
    if (!res.ok && STOPPING.has(res.error)) throw new Stop(fromError(toolError(res).error));
  };

  const launched = await launch(lane, r);
  if (launched) throw new Stop(fromError(launched.error));
  await sleep(SETTLE_MS[0]);

  let parsed: ParsedAccountScreen = { account: null, plan: null, windows: [] };
  let taps = 0;
  try {
    for (const step of steps) {
      checkBusy();
      if ('read' in step) {
        const screen = await readScreen(lane, r.serial);
        stopOn(screen);
        if (screen.ok) parsed = mergeParsed(parsed, parseAccountScreen(String(screen.text ?? ''), spec, now()));
        if (step.scroll && screen.ok) {
          const swiped = await lane.call('phone.swipe', r.serial, { direction: 'up' });
          stopOn(swiped);
          if (swiped.ok) {
            await sleep(800);
            const more = await readScreen(lane, r.serial);
            stopOn(more);
            if (more.ok) parsed = mergeParsed(parsed, parseAccountScreen(String(more.text ?? ''), spec, now()));
          }
        }
        continue;
      }
      // The element tree has classes (to skip toggles) and ids; screen_text is the fallback.
      let ui = await lane.call('phone.read_ui', r.serial, { visible_only: true, max_elements: 400 });
      stopOn(ui);
      if (!ui.ok) ui = await readScreen(lane, r.serial);
      stopOn(ui);
      const target = ui.ok ? pickTap(screenElements(String(ui.text ?? '')), step) : null;
      if (!target) {
        if (step.optional) continue;
        return parsed;
      }
      const tapped = await lane.call('phone.find_and_tap', r.serial, { element_id: target.element_id, match: 'exact' });
      stopOn(tapped);
      if (!tapped.ok) {
        if (step.optional) continue;
        return parsed;
      }
      taps++;
      await sleep(TAP_SETTLE_MS);
    }
    return parsed;
  } finally {
    // Android: step back out so the next path starts from the app's main screen (iPhone has no Back; Home ends the read).
    if (r.phone.platform === 'android') {
      for (let i = 0; i < taps; i++) {
        const back = await lane.call('phone.back', r.serial, {}).catch(() => null);
        if (!back?.ok) break;
      }
    }
  }
}

// ── Scheduler ───────────────────────────────────────────────────────────────

/** How often each signed-in AI chat app's account is re-read. */
export const ACCOUNT_REFRESH_MS = 6 * 3600_000;
/** How often the scheduler looks for a due app (one app per look). */
export const ACCOUNT_TICK_MS = 5 * 60_000;
/** After a phone links (or an app signs in), wait this long before the first read. */
export const ACCOUNT_KICK_DELAY_MS = 60_000;

export interface AccountSchedulerDeps {
  phones: () => LinkedPhone[];
  health: (deviceId: string) => PhoneLinkHealth | null;
  busy: (deviceId: string) => boolean;
  read: (opts: { deviceId: string; app: string }) => Promise<PhoneLinkAccountResult>;
  now?: () => number;
  refreshMs?: number;
  tickMs?: number;
  kickDelayMs?: number;
  logger?: (message: string) => void;
}

/**
 * Refreshes account reads in the background: every 6 h per signed-in AI chat
 * app, and soon after a phone links or an app signs in. Only on a HEALTHY
 * phone (so never locked, offline or with accessibility off) that no bot is
 * using; one app per tick, so reads never pile up.
 */
export class PhoneAccountScheduler {
  /** Last attempt per `deviceId:app`, so a failing read waits a full period too. */
  private readonly attempted = new Map<string, number>();
  /** Due now regardless of the saved read time: `deviceId:app`, a deviceId, or '*'. */
  private readonly force = new Set<string>();
  private timer: ReturnType<typeof setInterval> | null = null;
  private running = false;

  constructor(private readonly deps: AccountSchedulerDeps) {}

  private now(): number {
    return (this.deps.now ?? Date.now)();
  }

  start(): void {
    if (this.timer) return;
    this.timer = setInterval(() => void this.tick(), this.deps.tickMs ?? ACCOUNT_TICK_MS);
    this.timer.unref?.();
  }

  stop(): void {
    if (this.timer) clearInterval(this.timer);
    this.timer = null;
  }

  /** A phone just linked or an app just signed in: make its apps due and look again shortly. */
  kick(deviceId?: string, app?: string): void {
    for (const key of [...this.attempted.keys()]) {
      if (!deviceId || key === `${deviceId}:${app}` || (!app && key.startsWith(`${deviceId}:`))) this.attempted.delete(key);
    }
    this.force.add(deviceId && app ? `${deviceId}:${app}` : deviceId ?? '*');
    const t = setTimeout(() => void this.tick(), this.deps.kickDelayMs ?? ACCOUNT_KICK_DELAY_MS);
    t.unref?.();
  }

  /** The next due `{deviceId, app}`, or null. */
  due(): { deviceId: string; app: string } | null {
    const period = this.deps.refreshMs ?? ACCOUNT_REFRESH_MS;
    for (const phone of this.deps.phones()) {
      if (this.deps.health(phone.deviceId)?.state !== 'HEALTHY') continue;
      if (this.deps.busy(phone.deviceId)) continue;
      for (const app of phone.apps ?? []) {
        if (app.signedIn !== true) continue;
        const entry = catalogApp(app.vendor);
        if ((app.category ?? entry?.category) !== PROBE_CATEGORY || !accountSpec(app.vendor)) continue;
        const key = `${phone.deviceId}:${app.vendor}`;
        const forced = this.force.has(key) || this.force.has(phone.deviceId) || this.force.has('*');
        const checked = app.accountCheckedAt ? Date.parse(app.accountCheckedAt) : 0;
        const last = Math.max(forced ? 0 : Number.isFinite(checked) ? checked : 0, this.attempted.get(key) ?? 0);
        if (this.now() - last >= period) return { deviceId: phone.deviceId, app: app.vendor };
      }
    }
    return null;
  }

  /** Read one due app. Returns what it read (for tests), or null when nothing was due. */
  async tick(): Promise<{ deviceId: string; app: string; result: PhoneLinkAccountResult } | null> {
    if (this.running) return null;
    const next = this.due();
    if (!next) {
      this.force.clear();
      return null;
    }
    this.running = true;
    const key = `${next.deviceId}:${next.app}`;
    this.attempted.set(key, this.now());
    this.force.delete(key);
    try {
      const result = await this.deps.read(next);
      if (!result.ok) this.deps.logger?.(`[PhoneAccount] ${key}: ${result.error?.code ?? 'failed'}`);
      return { ...next, result };
    } catch (error) {
      this.deps.logger?.(`[PhoneAccount] ${key}: ${error instanceof Error ? error.message : String(error)}`);
      return null;
    } finally {
      this.running = false;
    }
  }
}
