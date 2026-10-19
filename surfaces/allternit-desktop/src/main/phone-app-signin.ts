/**
 * Connections wizard (Settings → Connections): open a catalog app on a
 * linked phone so the person can sign in there, then read its screen and
 * tell whether it is signed in (IPC `phone-link:open-app` / `phone-link:check-app`).
 *
 * Both go through the Phone Link lane's own phone tools (phone.open_app,
 * phone.screen_text / phone.read_ui in phone-link-backend.ts), so there is
 * no second transport. Read-only by design: this never types, never taps a
 * sign-in button and never reads a password field — it only reads the labels
 * the app already shows.
 *
 * signedIn is decided conservatively from the screen's labels:
 *   false  a sign-in gate is on screen ("Sign in", "Log in", "Create account",
 *          "Continue with Google", …) as a whole label
 *   true   the app's main UI is on screen (for the AI chat apps — the catalog's
 *          PROBE_CATEGORY — their chat composer; for others the category's
 *          main screen labels), or a "Sign out" / "Log out" control
 *   null   neither: the screen didn't show either way
 */
import { catalogApp, PROBE_CATEGORY, type PhoneCatalogApp } from './phone-app-catalog.js';
import { phoneWorks, type LinkedPhone } from './phone-link-health.js';
import type { PhoneLinkAppResult, PhoneLinkHealth } from './phone-link-types.js';
import type { ToolResult } from './phone-tools.js';

export type { PhoneLinkAppResult };

/** What the wizard needs from the Phone Link lane (PhoneLinkBackend implements it). */
export interface LinkedAppLane {
  linkedPhone(serial: string): LinkedPhone | null;
  /** The phone's latest health, or null when it hasn't been checked. */
  healthOf(deviceId: string): PhoneLinkHealth | null;
  call(name: string, serial: string, args: Record<string, unknown>): Promise<ToolResult>;
  /** Save the app's sign-in state into the phone's cached app list (phone-link:devices). */
  rememberAppSignIn(deviceId: string, appId: string, signedIn: boolean | null): void;
}

export interface LinkedAppDeps {
  lane: LinkedAppLane;
  /** Defaults to a real timer; tests pass an instant one. */
  sleep?: (ms: number) => Promise<void>;
}

/** How long an app gets to draw its first screen after launch, then one more look. */
export const SETTLE_MS = [2500, 2000] as const;

const SERIAL_PREFIX = 'link:';

// ── Screen classification (pure) ────────────────────────────────────────────

/** Whole-label sign-in gates (EN + common variants). Matched against one label at a time, so "Sign in to another account" inside a settings list doesn't count. */
const GATE_LABELS: RegExp[] = [
  /^(sign|log)\s?(in|on)( now| here)?[.!]?$/,
  /^(sign|log)\s?(in|up)( with| using) (google|apple|email|e-mail|phone|facebook|microsoft|sso|your [a-z ]+)$/,
  /^continue with (google|apple|email|e-mail|phone|phone number|facebook|microsoft|sso|x)$/,
  /^(sign|log)\s?up( for free| free)?[.!]?$/,
  /^create (an |your |a new )?account$/,
  /^(sign|log) in to (continue|get started|your account)$/,
  /^already have an account\??( (sign|log)\s?in)?$/,
  /^(don['’]t|do not) have an account\??( (sign|create|join)[a-z ]*)?$/,
  /^forgot (your )?password\??$/,
  /^(enter|use) (your )?(email|e-mail|phone number|password)( address)?$/,
  // Kimi iOS when its token lapsed: "Session expired. Please log in again."
  /^(your )?session (has )?expired\b/,
];

/** "Sign out" / "Log out": only signed-in apps show these. */
const SIGNED_IN_LABELS: RegExp[] = [/^(sign|log)\s?(out|off)$/, /^switch accounts?$/, /^manage (your )?account$/];

/** The AI chat apps' composer and history (PROBE_CATEGORY). */
const AI_CHAT_MARKERS: RegExp[] = [
  /^(message|ask|chat with) (chatgpt|claude|gemini|grok|kimi|copilot|perplexity)$/,
  /^(reply to|talk to) (claude|chatgpt|gemini)$/,
  /^ask anything$/,
  /^how can i help( you)?( today)?\??$/,
  /^new chat$/,
  /^chats?$/,
  /^recents?$/,
  /^history$/,
];

/** Main-screen labels by catalog category. Only labels a signed-out app doesn't show. */
const CATEGORY_MARKERS: Record<string, RegExp[]> = {
  Communication: [/^(chats|messages|calls|direct messages|dms|activity|channels|updates|communities)$/, /^new (message|chat|group)$/],
  Social: [/^(home|reels|explore|notifications|your story|create post|what['’]s on your mind\??)$/],
  Email: [/^(inbox|primary|compose|new mail|new message|focused)$/],
  Storage: [/^(my drive|my files|files|recent|shared with me|starred)$/],
  Productivity: [/^(new page|private|teamspace|teamspaces|recent documents|owned by me)$/],
  Spreadsheets: [/^(recent spreadsheets|owned by me|new spreadsheet|blank workbook)$/],
  Presentations: [/^(recent presentations|owned by me|new presentation)$/],
  'Project management': [/^(my tasks|my work|my issues|inbox|boards|projects|workspaces?)$/],
  Tasks: [/^(today|inbox|upcoming|my tasks)$/],
  Calendar: [/^(today|schedule|new event)$/],
  Meetings: [/^(new meeting|join a meeting|meetings|schedule)$/],
  Video: [/^(subscriptions|you|library|your channel)$/],
  Media: [/^(your library|library|photos|memories)$/],
  Developer: [/^(my work|repositories|pull requests|issues|notifications)$/],
  Design: [/^(recent|drafts|your designs|projects)$/],
};

/** One label per line: strips the lane's "label @ (x, y)" suffix and read_ui's quoting. */
export function screenLabels(text: string): string[] {
  const out: string[] = [];
  for (const raw of text.split('\n')) {
    let line = raw.trim();
    // The iPhone runner's XCUI tree: `StaticText, label: 'Usage', id: 'x', {{x, y}, {w, h}}`.
    const xcui = /^[A-Za-z]+,\s*(.*?)\s*\{\{[-\d.]+,\s*[-\d.]+\},\s*\{[-\d.]+,\s*[-\d.]+\}\}\s*$/.exec(line);
    if (xcui) {
      const label = /(?:^|, )label: '(.*?)'(?=, (?:label|id|identifier|value|placeholderValue): '|,?\s*$)/.exec(xcui[1])?.[1];
      if (label) out.push(label.replace(/[\u200b-\u200f\u202a-\u202e\u2060-\u2069\ufeff]/g, '').replace(/[\u00a0\u202f]/g, ' '));
      continue;
    }
    const quoted = /"([^"]*)"/.exec(line);
    if (quoted) line = quoted[1];
    line = line.replace(/\s*@\s*\(\s*-?[\d.]+,\s*-?[\d.]+\s*\)\s*$/, '').trim();
    if (line) out.push(line);
  }
  return out;
}

function norm(label: string): string {
  return label.toLowerCase().replace(/[’]/g, "'").replace(/\s+/g, ' ').replace(/[,:;]+$/, '').trim();
}

const EMAIL_RE = /\b[A-Z0-9._%+-]+@[A-Z0-9.-]+\.[A-Z]{2,}\b/i;
const HANDLE_RE = /^@[A-Za-z0-9_.]{2,30}$/;

/** An email or @handle the screen plainly shows, else null. */
export function visibleAccount(labels: string[]): string | null {
  for (const label of labels) {
    const email = EMAIL_RE.exec(label);
    if (email) return email[0];
  }
  for (const label of labels) {
    if (HANDLE_RE.test(label.trim())) return label.trim();
  }
  return null;
}

/**
 * Decide signedIn + account from a screen's text. Pure; unit-tested. The
 * account is only taken from an AI chat app's screen: a social or video
 * app's first screen is a feed full of other people's @handles and emails
 * (its account comes from the account reader's per-app rule instead).
 */
export function classifySignIn(app: Pick<PhoneCatalogApp, 'name' | 'category'>, screenText: string): { signedIn: boolean | null; account: string | null } {
  const labels = screenLabels(screenText);
  const normed = labels.map(norm);
  const has = (patterns: RegExp[]) => normed.some((label) => patterns.some((re) => re.test(label)));
  // A gate wins: ChatGPT and others show a composer AND "Log in" when signed out.
  if (has(GATE_LABELS)) return { signedIn: false, account: null };
  const name = app.name.toLowerCase();
  const markers = [
    ...SIGNED_IN_LABELS,
    ...(app.category === PROBE_CATEGORY ? [...AI_CHAT_MARKERS, new RegExp(`^(message|ask) ${escapeRe(name)}$`)] : []),
    ...(CATEGORY_MARKERS[app.category] ?? []),
  ];
  if (has(markers)) return { signedIn: true, account: app.category === PROBE_CATEGORY ? visibleAccount(labels) : null };
  return { signedIn: null, account: null };
}

function escapeRe(value: string): string {
  return value.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
}

// ── IPC handlers ────────────────────────────────────────────────────────────

const err = (code: string, message: string, retryable = false): PhoneLinkAppResult => ({
  ok: false,
  signedIn: null,
  account: null,
  error: { code, message, retryable },
});

/** Health states the wizard can't work through, worded like Settings › Linked phones. */
function healthError(health: PhoneLinkHealth | null): PhoneLinkAppResult | null {
  if (!health || health.state === 'HEALTHY' || health.state === 'UNKNOWN') return null;
  const retryable = health.recovery === 'automatic';
  switch (health.state) {
    case 'DEVICE_LOCKED':
      return err('device_locked', health.action ?? 'Unlock your phone.', true);
    case 'TRANSPORT_OFFLINE':
      return err('device_offline', health.detail ?? 'The phone is not answering. Check it is on the same Wi-Fi as this computer.', true);
    case 'PAIRING_INVALID':
      return err('pairing_invalid', health.action ?? health.detail ?? 'Pair this phone again in Settings › Connections › Setup.', false);
    default:
      return err(health.state.toLowerCase(), health.action ?? health.detail ?? 'The phone is not ready.', retryable);
  }
}

/** A failed lane call as the wizard's error. */
export function toolError(res: ToolResult): PhoneLinkAppResult {
  if (res.ok) return err('phone_failed', 'The phone did not answer.');
  const retryable = res.error === 'device_offline' || res.error === 'device_locked' || res.error === 'phone_failed';
  return err(res.error, res.message, retryable);
}

export type Resolved = { serial: string; phone: LinkedPhone; app: PhoneCatalogApp };

export function resolve(lane: LinkedAppLane, opts: unknown): Resolved | PhoneLinkAppResult {
  const o = (opts ?? {}) as { deviceId?: unknown; app?: unknown };
  if (typeof o.deviceId !== 'string' || !o.deviceId) return err('invalid_args', 'deviceId is required.');
  if (typeof o.app !== 'string' || !o.app) return err('invalid_args', 'app is required.');
  const serial = SERIAL_PREFIX + o.deviceId;
  const phone = lane.linkedPhone(serial);
  if (!phone) return err('device_not_found', 'That phone is not linked to this computer.');
  const app = catalogApp(o.app);
  if (!app) return err('unknown_app', `"${o.app}" is not in the phone app catalog.`);
  if ((phone.platform === 'ios' ? app.ios : app.android).length === 0) {
    return err('unknown_app', `${app.name} has no ${phone.platform === 'ios' ? 'iPhone' : 'Android'} app.`);
  }
  const health = lane.healthOf(phone.deviceId);
  // A renewal warning while the signature is still valid: the phone works.
  const unhealthy = phoneWorks(health, phone.expiresAt, Date.now()) ? null : healthError(health);
  if (unhealthy) return unhealthy;
  return { serial, phone, app };
}

export async function launch(lane: LinkedAppLane, r: Resolved): Promise<PhoneLinkAppResult | null> {
  // The catalog id: the lane maps it to the bundle ids / packages itself.
  const res = await lane.call('phone.open_app', r.serial, { app: r.app.id });
  if (res.ok) return null;
  if (res.error === 'unknown_app') return err('not_installed', `${r.app.name} is not installed on ${r.phone.name ?? 'the phone'}.`);
  return toolError(res);
}

/** phone-link:open-app — bring the app to the front so the person can sign in there. */
export async function openLinkedApp(deps: LinkedAppDeps, opts: unknown): Promise<PhoneLinkAppResult> {
  const r = resolve(deps.lane, opts);
  if ('ok' in r) return r;
  const failed = await launch(deps.lane, r);
  if (failed) return failed;
  return { ok: true, signedIn: null, account: null };
}

export async function readScreen(lane: LinkedAppLane, serial: string): Promise<ToolResult> {
  const text = await lane.call('phone.screen_text', serial, {});
  if (text.ok || text.error === 'device_offline' || text.error === 'device_locked' || text.error === 'pairing_invalid') return text;
  // Older helpers without screen_text: the element tree has the same labels.
  return lane.call('phone.read_ui', serial, { visible_only: true, max_elements: 400 });
}

/** phone-link:check-app — open (or foreground) the app, read its screen, decide signedIn. */
export async function checkLinkedApp(deps: LinkedAppDeps, opts: unknown): Promise<PhoneLinkAppResult> {
  const r = resolve(deps.lane, opts);
  if ('ok' in r) return r;
  const sleep = deps.sleep ?? ((ms: number) => new Promise<void>((done) => setTimeout(done, ms)));
  const failed = await launch(deps.lane, r);
  if (failed) return failed;
  let verdict: { signedIn: boolean | null; account: string | null } = { signedIn: null, account: null };
  for (const wait of SETTLE_MS) {
    await sleep(wait);
    const screen = await readScreen(deps.lane, r.serial);
    if (!screen.ok) return toolError(screen);
    verdict = classifySignIn(r.app, typeof screen.text === 'string' ? screen.text : '');
    if (verdict.signedIn !== null) break;
  }
  deps.lane.rememberAppSignIn(r.phone.deviceId, r.app.id, verdict.signedIn);
  return { ok: true, signedIn: verdict.signedIn, account: verdict.account };
}
