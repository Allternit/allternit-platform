/**
 * Phone Use router: one `phone.*` contract over every way Allternit can reach
 * an iPhone (docs/contracts/CONTRACT-phone-use-2026-11-16.md §1, §1.1, §5).
 *
 * The iPhone runs only the App Store app. Apple's own actions (Shortcuts,
 * Home, calendar …) go through it as the `official` route; taps come from
 * outside the phone (Mirroring, a Bluetooth keyboard and mouse from a Mac, an
 * Android phone or a second Apple device, the USB Agent board). Each route is a
 * PhoneLane behind this router, so health, order and fallback exist once:
 *
 *   1. `official` first whenever it covers the call;
 *   2. otherwise the first healthy tap route in ROUTE_ORDER (lanes read the
 *      phone's lock state from the context and say needs_you when it rules them out);
 *   3. a shortcut that needs the Allternit app in front opens it through a
 *      healthy tap route (Spotlight), then runs;
 *   4. nothing healthy: ok:false with exactly one needs_you (the first one in
 *      order). A call the owner denied, or one held for approval, never moves
 *      to another route;
 *   5. a pinned route (`route` arg) never falls back: unhealthy = its needs_you.
 *
 * Routes not built yet (official, mirror, bt-*) are stubs that report
 * "unavailable", so the router skips them; later plan steps register real
 * lanes in their place. The USB Agent and the old iCTRL helper are wrapped
 * as the `usb-agent` and `legacy-helper` lanes without changing them.
 */

import { legacyHelperRouteHealth, type NeedsYou, type NeedsYouCode, type RouteHealth } from './phone-link-health.js';
import type { PhoneLinkBackend, ToolErrorCode, ToolResult, UsbPhoneBackend } from './phone-tools.js';
import { actThenVerify, failEnvelope, READ_TOOLS, toEnvelope, VERIFIED_TOOLS, type PhoneUseResult, type VerifyFrame } from './phone-verify.js';

export type { PhoneUseResult } from './phone-verify.js';

// ── Route ids and order ─────────────────────────────────────────────────────

export const ROUTE_IDS = ['official', 'mirror', 'bt-mac', 'bt-android', 'bt-apple', 'usb-agent', 'legacy-helper'] as const;
export type RouteId = (typeof ROUTE_IDS)[number];

export function isRouteId(value: unknown): value is RouteId {
  return typeof value === 'string' && (ROUTE_IDS as readonly string[]).includes(value);
}

/**
 * The order the router tries routes in. `official` is always first; the tap
 * routes follow (2a mirror, 2b bt-mac, then bt-apple moved up next to bt-mac
 * by the plan amendment of 2026-11-16 — two Apple devices and no Mac is the
 * common setup — then bt-android and the board), and the deprecated helper last.
 */
export const ROUTE_ORDER: readonly RouteId[] = ['official', 'mirror', 'bt-mac', 'bt-apple', 'bt-android', 'usb-agent', 'legacy-helper'];

// ── Tools and what they need ────────────────────────────────────────────────

export type Capability = 'shortcuts' | 'actions' | 'home' | 'tap' | 'type' | 'swipe' | 'key' | 'screenshot' | 'read_ui';

/** New tools, served by `official` only (contract §3.1). phone.describe is answered by the router itself. */
export const OFFICIAL_TOOLS = ['phone.shortcuts.run', 'phone.actions.list', 'phone.actions.run', 'phone.home.list', 'phone.home.set', 'phone.home.scene'] as const;
/** Existing tools that run on every tap route (contract §3.2). */
export const TAP_TOOLS = [
  'phone.screenshot',
  'phone.read_ui',
  'phone.find_and_tap',
  'phone.tap',
  'phone.type',
  'phone.swipe',
  'phone.home',
  'phone.back',
  'phone.key',
  'phone.open_app',
] as const;

export type OfficialTool = (typeof OFFICIAL_TOOLS)[number];
export type TapTool = (typeof TAP_TOOLS)[number];
/** Every tool a lane acts on. */
export type LaneTool = OfficialTool | TapTool;

const NEEDS: Readonly<Record<LaneTool, Capability>> = {
  'phone.shortcuts.run': 'shortcuts',
  'phone.actions.list': 'actions',
  'phone.actions.run': 'actions',
  'phone.home.list': 'home',
  'phone.home.set': 'home',
  'phone.home.scene': 'home',
  'phone.screenshot': 'screenshot',
  'phone.read_ui': 'read_ui',
  'phone.find_and_tap': 'read_ui',
  'phone.tap': 'tap',
  'phone.type': 'type',
  'phone.swipe': 'swipe',
  'phone.home': 'key',
  'phone.back': 'key',
  'phone.key': 'key',
  'phone.open_app': 'key',
};

export const isOfficialTool = (name: string): name is OfficialTool => (OFFICIAL_TOOLS as readonly string[]).includes(name);
export const isTapTool = (name: string): name is TapTool => (TAP_TOOLS as readonly string[]).includes(name);

const TAP_CAPS: readonly Capability[] = ['screenshot', 'read_ui', 'tap', 'type', 'swipe', 'key'];
const OFFICIAL_CAPS: readonly Capability[] = ['shortcuts', 'actions', 'home'];

// ── Lanes ───────────────────────────────────────────────────────────────────

/** What a lane is told about the phone when asked for its health. */
export interface LaneContext {
  serial: string;
  /** true = locked, false = unlocked, null = unknown. Mirroring needs it locked; Bluetooth needs it unlocked. */
  locked: boolean | null;
}

/** Facts only the official route (the Allternit app on the phone) knows, for phone.describe. */
export interface PhoneFacts {
  locked?: boolean | null;
  foreground_app?: string | null;
  shortcuts?: string[];
  home?: { available: boolean };
}

/** One route to a phone (contract §1). */
export interface PhoneLane {
  readonly id: RouteId;
  /** What this route can do (phone.describe's `covers`). */
  readonly covers: readonly Capability[];
  /**
   * 'router': phone-verify runs act-then-verify around acting tools (needs `frame`).
   * 'self': the lane checks its own effect (USB Agent: screenChanged) or keeps
   * its behaviour unchanged (the old helper).
   */
  readonly verify: 'router' | 'self';
  health(ctx: LaneContext): Promise<RouteHealth>;
  /** Run one tool. Args never carry `serial`, `route` or `_…` keys. */
  act(tool: LaneTool, serial: string, args: Record<string, unknown>): Promise<ToolResult>;
  /** Router-verified lanes: one frame of the screen now. */
  frame?(serial: string): Promise<VerifyFrame | null>;
  /** Pointer routes: drive the pointer to the corner (contract §6). */
  rehome?(serial: string): Promise<void>;
  /** OCR of a frame, for checking phone.type. */
  readText?(serial: string, frame: VerifyFrame): Promise<{ text: string; fieldVisible: boolean } | null>;
  /** Official route: lock state, foreground app, shortcuts, Home. */
  facts?(serial: string): Promise<PhoneFacts>;
}

const NOT_BUILT: Partial<Record<RouteId, string>> = {
  official: 'Shortcuts, actions and Home need the next Allternit iPhone app update.',
  mirror: 'iPhone Mirroring is not available in this version of Allternit Desktop.',
  'bt-mac': 'Bluetooth keyboard and mouse from this Mac is not available in this version of Allternit Desktop.',
  'bt-apple': 'Bluetooth keyboard and mouse from a second iPhone or iPad is not available yet.',
  'bt-android': 'Bluetooth keyboard and mouse from the Allternit Android app is not available yet.',
};

/** A route that is not built yet: always "unavailable", so the router falls through it. */
export function stubLane(id: RouteId): PhoneLane {
  const reason = NOT_BUILT[id] ?? `The ${id} route is not available.`;
  return {
    id,
    covers: id === 'official' ? OFFICIAL_CAPS : TAP_CAPS,
    verify: id === 'official' ? 'self' : 'router',
    health: async () => ({ state: 'unavailable', reason }),
    act: async () => ({ ok: false, error: 'route_unavailable', message: reason }),
  };
}

/** The USB Agent board (phone-usb-agent.ts), unchanged: it aims, acts and reports screenChanged itself. */
export function usbAgentLane(backend: () => UsbPhoneBackend | null): PhoneLane {
  return {
    id: 'usb-agent',
    // No element read on the board (phone.read_ui / find_and_tap answer unsupported there).
    covers: ['screenshot', 'tap', 'type', 'swipe', 'key'],
    verify: 'self',
    health: async ({ serial }) =>
      backend()?.has(serial) ? { state: 'healthy' } : { state: 'unavailable', reason: 'No USB Agent is attached to this phone.' },
    act: async (tool, serial, args) => {
      const b = backend();
      return b ? b.call(tool, serial, args) : { ok: false, error: 'device_offline', message: 'The USB Agent is not connected.' };
    },
  };
}

/**
 * True for a `link:` serial that is an iPhone. The lane interface has no
 * platform lookup; canText is defined as "this link is an Android phone"
 * (phone-link-backend.ts), so a backend without it is never treated as iOS
 * and Android links keep their path untouched.
 */
export function isIosLink(links: PhoneLinkBackend | null, serial: string): boolean {
  return !!links && links.has(serial) && typeof links.canText === 'function' && !links.canText(serial);
}

/**
 * The old iCTRL helper on a linked iPhone, unchanged. DEPRECATED: no new
 * setup (phone-link.ts refuses it), removed after plan step 16 and Eoj's
 * gate G6. `retiresAt` is the removal date once set; until then no notice.
 */
export function legacyHelperLane(links: () => PhoneLinkBackend | null, retiresAt: () => string | null = () => null): PhoneLane {
  return {
    id: 'legacy-helper',
    covers: TAP_CAPS,
    verify: 'self',
    health: async ({ serial }) => legacyHelperRouteHealth(isIosLink(links(), serial), retiresAt()),
    act: async (tool, serial, args) => {
      const l = links();
      if (!l) return { ok: false, error: 'device_offline', message: 'The phone link is not running.' };
      // Mark bot use so background account reads wait (phone-account-reader.ts), as before.
      l.noteActivity?.(serial, 1);
      try {
        return await l.call(tool, serial, args);
      } finally {
        l.noteActivity?.(serial, -1);
      }
    },
  };
}

// ── Approval classes (contract §8) ──────────────────────────────────────────

export type ActionKind = 'create' | 'send' | 'open' | 'search' | 'read' | 'control' | 'other';
export type ApprovalClass = 'send' | 'purchase' | 'security';

/** Catalog app ids whose acting actions reach another person (contract §8 `send`). */
const SEND_APPS = new Set(['messages', 'mail', 'slack', 'whatsapp', 'phone', 'facetime', 'telegram', 'signal', 'discord', 'teams', 'gmail', 'outlook', 'messenger', 'wechat', 'line', 'viber', 'skype']);
/** Catalog app ids where acting actions spend money (contract §8 `purchase`). */
const MONEY_APPS = new Set(['wallet', 'uber', 'lyft', 'doordash', 'ubereats', 'instacart', 'grubhub', 'amazon', 'venmo', 'cashapp', 'paypal', 'zelle', 'starbucks']);
const PURCHASE_RE = /\b(buy|order|pay|payment|purchase|checkout|subscribe|subscription|tip|donate|transfer|send money|request (a )?ride)\b/i;
const SECURITY_RE = /\b(passcode|find my|apple account|apple id|password|lock|unlock|garage|alarm|camera|security)\b/i;
const SECURITY_HOME_KINDS = new Set(['lock', 'garage', 'security', 'alarm', 'camera']);
const READ_KINDS = new Set<ActionKind>(['read', 'search', 'open']);

/** "com.slack.SendMessageIntent" / "send_message" → "Send Message Intent". */
export function actionWords(id: string): string {
  const last = id.split(/[./]/).filter(Boolean).pop() ?? id;
  return last.replace(/[_-]+/g, ' ').replace(/([a-z0-9])([A-Z])/g, '$1 $2').trim();
}

/** Contract §7.2: the kind from an intent title's first word. A hint; the guard decides on kind and app. */
export function deriveActionKind(title: string): ActionKind {
  const first = actionWords(title).split(/\s+/)[0]?.toLowerCase() ?? '';
  if (['send', 'reply', 'post', 'message', 'call'].includes(first)) return 'send';
  if (['create', 'add', 'new', 'log'].includes(first)) return 'create';
  if (first === 'open') return 'open';
  if (['find', 'search'].includes(first)) return 'search';
  if (['get', 'show', 'read'].includes(first)) return 'read';
  if (['play', 'pause', 'toggle', 'set', 'start', 'stop'].includes(first)) return 'control';
  return 'other';
}

/** What the router learned from phone.actions.list / phone.home.list for the approval check. */
export interface ApprovalHints {
  action?: { kind?: ActionKind; guarded?: boolean };
  homeKind?: string;
  sceneSecurity?: boolean;
}

/**
 * The approval class a Phone Use call needs (contract §8), or null when it
 * runs without asking. Read-only calls and create/open/control in the user's
 * own data are not guarded; an unknown shortcut always is (it could send).
 */
export function approvalClass(tool: string, args: Record<string, unknown>, hints: ApprovalHints = {}): ApprovalClass | null {
  const text = (v: unknown) => (typeof v === 'string' ? v : '');
  switch (tool) {
    case 'phone.shortcuts.run': {
      const name = text(args.name);
      if (SECURITY_RE.test(name)) return 'security';
      if (PURCHASE_RE.test(name)) return 'purchase';
      if (hints.action && hints.action.guarded === false && hints.action.kind && hints.action.kind !== 'send') return null;
      return 'send';
    }
    case 'phone.actions.run': {
      const app = text(args.app).toLowerCase();
      const words = actionWords(text(args.action));
      const kind = hints.action?.kind ?? deriveActionKind(words);
      if (SECURITY_RE.test(words)) return 'security';
      if (PURCHASE_RE.test(words) || (MONEY_APPS.has(app) && !READ_KINDS.has(kind))) return 'purchase';
      if (kind === 'send' || (SEND_APPS.has(app) && !READ_KINDS.has(kind))) return 'send';
      if (hints.action?.guarded === true) return 'send';
      return null;
    }
    case 'phone.home.set': {
      const state = args.state && typeof args.state === 'object' ? (args.state as Record<string, unknown>) : {};
      const kind = (hints.homeKind ?? '').toLowerCase();
      if (SECURITY_HOME_KINDS.has(kind)) return 'security';
      if ('locked' in state || 'armed' in state || 'alarm' in state || 'door' in state) return 'security';
      return SECURITY_RE.test(text(args.id)) ? 'security' : null;
    }
    case 'phone.home.scene':
      return hints.sceneSecurity === true || SECURITY_RE.test(text(args.id)) ? 'security' : null;
    default:
      return null;
  }
}

// ── The router ──────────────────────────────────────────────────────────────

/** How long a lane's health answer is reused (short: phones lock, windows hide). */
export const HEALTH_TTL_MS = 3_000;

/** Failures that end the call on the lane that gave them: never retried on another route. */
const FINAL_ERRORS = new Set<string>([
  'approval_required',
  'approval_denied',
  'approval_timeout',
  'guard_unavailable',
  'no_change',
  'invalid_args',
  'bad_params',
  'unknown_action',
  'rate_limited',
  'timeout',
]);

export interface PhoneRouterDeps {
  /** Lanes by route id; ids left out are stubs. */
  lanes: Partial<Record<RouteId, PhoneLane>>;
  /** The phone's lock state, when something knows it. */
  locked?: (serial: string) => Promise<boolean | null>;
  now?: () => number;
  sleep?: (ms: number) => Promise<void>;
  healthTtlMs?: number;
}

export interface RunOptions {
  /** Pin one route: no fallback (contract §1.1 step 5). */
  route?: RouteId;
  /**
   * The approval check, run once the route is chosen and before anything is
   * sent: null = go ahead, a result = stop with it (held, denied, refused).
   */
  approve?: (route: RouteId) => Promise<ToolResult | null>;
}

export class PhoneRouter {
  private readonly lanes: Record<RouteId, PhoneLane>;
  private readonly cache = new Map<string, { at: number; health: RouteHealth }>();
  private readonly actionHints = new Map<string, { kind?: ActionKind; guarded?: boolean }>();
  private readonly homeHints = new Map<string, { kind?: string; security?: boolean }>();
  private readonly now: () => number;
  private readonly ttl: number;

  constructor(private readonly deps: PhoneRouterDeps) {
    this.lanes = Object.fromEntries(ROUTE_IDS.map((id) => [id, deps.lanes[id] ?? stubLane(id)])) as Record<RouteId, PhoneLane>;
    this.now = deps.now ?? Date.now;
    this.ttl = deps.healthTtlMs ?? HEALTH_TTL_MS;
  }

  lane(id: RouteId): PhoneLane {
    return this.lanes[id];
  }

  /** A lane's health for a phone, cached for HEALTH_TTL_MS. A throw counts as unavailable. */
  async health(id: RouteId, ctx: LaneContext): Promise<RouteHealth> {
    const key = `${id}\u0000${ctx.serial}`;
    const hit = this.cache.get(key);
    if (hit && this.now() - hit.at < this.ttl) return hit.health;
    let health: RouteHealth;
    try {
      health = await this.lanes[id].health(ctx);
    } catch (error) {
      health = { state: 'unavailable', reason: error instanceof Error ? error.message : String(error) };
    }
    this.cache.set(key, { at: this.now(), health });
    return health;
  }

  invalidate(id: RouteId, serial: string): void {
    this.cache.delete(`${id}\u0000${serial}`);
  }

  /** Hints for the approval check, from the last phone.actions.list / phone.home.list on this phone. */
  hints(serial: string, tool: string, args: Record<string, unknown>): ApprovalHints {
    if (tool === 'phone.actions.run' || tool === 'phone.shortcuts.run') {
      const key = tool === 'phone.actions.run' ? `${String(args.app)}/${String(args.action)}` : `shortcut/${String(args.name)}`;
      const action = this.actionHints.get(`${serial}\u0000${key}`);
      return action ? { action } : {};
    }
    if (tool === 'phone.home.set' || tool === 'phone.home.scene') {
      const home = this.homeHints.get(`${serial}\u0000${String(args.id)}`);
      return home ? { homeKind: home.kind, sceneSecurity: home.security } : {};
    }
    return {};
  }

  /**
   * Run one tool on the best route. Returns null when no route serves this
   * phone at all and none was pinned: the caller keeps its old path.
   */
  async run(tool: LaneTool, serial: string, args: Record<string, unknown>, opts: RunOptions = {}): Promise<PhoneUseResult | null> {
    const cap = NEEDS[tool];
    const order = (opts.route ? [opts.route] : ROUTE_ORDER).filter((id) => this.lanes[id].covers.includes(cap));
    if (opts.route && order.length === 0) {
      return failEnvelope(opts.route, 'not_supported', `${tool} does not run on the ${opts.route} route.`, { detail: { error: 'not_supported' } });
    }
    const ctx = await this.context(serial);
    let firstNeed: { route: RouteId; needsYou: NeedsYou } | null = null;
    let lastFailure: PhoneUseResult | null = null;
    let approved = false;
    for (const id of order) {
      let health = await this.health(id, ctx);
      if (id === 'official' && !opts.route && health.state === 'needs_you' && health.needsYou.code === 'APP_NOT_FOREGROUND') {
        health = await this.bringAppForward(serial, ctx, health);
      }
      if (health.state === 'unavailable') continue;
      if (health.state === 'needs_you') {
        firstNeed ??= { route: id, needsYou: health.needsYou };
        continue;
      }
      if (!approved && opts.approve) {
        const stop = await opts.approve(id);
        if (stop) return toEnvelope(id, tool, stop, stop.ok ? {} : { detail: { error: stop.error === 'approval_denied' ? 'denied' : stop.error } });
        approved = true;
      }
      const res = await this.act(this.lanes[id], tool, serial, args);
      if (health.notice) res.detail.notice = health.notice;
      if (res.ok) {
        this.learn(serial, tool, res);
        return res;
      }
      if (res.needs_you || FINAL_ERRORS.has(res.error)) return res;
      // The lane failed. Reads move on; an action moves on only when the lane is no longer healthy
      // (it never reached the phone), so nothing is sent twice.
      this.invalidate(id, serial);
      if (opts.route) return res;
      if (!READ_TOOLS.has(tool) && (await this.health(id, ctx)).state === 'healthy') return res;
      lastFailure = res;
    }
    if (firstNeed) {
      return failEnvelope(firstNeed.route, 'needs_you', firstNeed.needsYou.message, { needsYou: firstNeed.needsYou, detail: { error: 'needs_you' } });
    }
    if (lastFailure) return lastFailure;
    if (opts.route) {
      const h = await this.health(opts.route, ctx);
      const reason = h.state === 'unavailable' ? h.reason : `The ${opts.route} route is not available for this phone.`;
      return failEnvelope(opts.route, 'route_unavailable', reason, { detail: { error: 'route_unavailable' } });
    }
    return null;
  }

  /** phone.describe (contract §3.1): every route's health for this phone, plus what the app knows. Never acts. */
  async describe(serial: string): Promise<PhoneUseResult> {
    const ctx = await this.context(serial);
    const routes: Array<Record<string, unknown>> = [];
    let first: RouteId | null = null;
    for (const id of ROUTE_ORDER) {
      const lane = this.lanes[id];
      const h = await this.health(id, ctx);
      const code: NeedsYouCode | undefined = h.state === 'needs_you' ? h.needsYou.code : h.state === 'healthy' ? h.notice?.code : undefined;
      if (h.state === 'healthy') first ??= id;
      routes.push({ route: id, healthy: h.state === 'healthy', available: h.state !== 'unavailable', ...(code ? { needs_you: code } : {}), covers: [...lane.covers] });
    }
    let facts: PhoneFacts = {};
    const official = this.lanes.official;
    if (official.facts && (await this.health('official', ctx)).state === 'healthy') {
      try {
        facts = await official.facts(serial);
      } catch {
        facts = {};
      }
    }
    return toEnvelope(first ?? 'official', 'phone.describe', {
      ok: true,
      detail: {
        routes,
        locked: facts.locked ?? ctx.locked,
        foreground_app: facts.foreground_app ?? null,
        shortcuts: facts.shortcuts ?? [],
        home: facts.home ?? { available: false },
      },
    });
  }

  private async context(serial: string): Promise<LaneContext> {
    let locked: boolean | null = null;
    try {
      locked = (await this.deps.locked?.(serial)) ?? null;
    } catch {
      locked = null;
    }
    return { serial, locked };
  }

  /** Contract §1.1 step 3: open Allternit with Spotlight on a healthy tap route, then ask official again. */
  private async bringAppForward(serial: string, ctx: LaneContext, health: RouteHealth): Promise<RouteHealth> {
    for (const id of ROUTE_ORDER) {
      if (id === 'official' || !this.lanes[id].covers.includes('key')) continue;
      if ((await this.health(id, ctx)).state !== 'healthy') continue;
      const opened = await this.lanes[id].act('phone.open_app', serial, { app: 'Allternit' });
      if (!opened.ok) return health;
      this.invalidate('official', serial);
      return this.health('official', ctx);
    }
    return health;
  }

  private async act(lane: PhoneLane, tool: LaneTool, serial: string, args: Record<string, unknown>): Promise<PhoneUseResult> {
    let raw: ToolResult;
    try {
      if (lane.verify === 'router' && VERIFIED_TOOLS.has(tool) && lane.frame) {
        return await actThenVerify(
          {
            frame: () => lane.frame!(serial),
            send: () => lane.act(tool, serial, args),
            ...(lane.rehome ? { rehome: () => lane.rehome!(serial) } : {}),
            ...(lane.readText ? { readText: (f: VerifyFrame) => lane.readText!(serial, f) } : {}),
          },
          { route: lane.id, tool, typed: tool === 'phone.type' && typeof args.text === 'string' ? args.text : undefined, now: this.deps.now, sleep: this.deps.sleep },
        );
      }
      raw = await lane.act(tool, serial, args);
    } catch (error) {
      raw = { ok: false, error: 'phone_failed' as ToolErrorCode, message: error instanceof Error ? error.message : String(error) };
    }
    return toEnvelope(lane.id, tool, raw);
  }

  /** Remember action kinds and Home device kinds from list results, for the approval check. */
  private learn(serial: string, tool: LaneTool, res: PhoneUseResult): void {
    if (tool === 'phone.actions.list' && Array.isArray(res.detail.apps)) {
      for (const app of res.detail.apps as Array<Record<string, unknown>>) {
        if (!Array.isArray(app.actions)) continue;
        for (const a of app.actions as Array<Record<string, unknown>>) {
          const kind = typeof a.kind === 'string' ? (a.kind as ActionKind) : undefined;
          const guarded = typeof a.guarded === 'boolean' ? a.guarded : undefined;
          this.actionHints.set(`${serial}\u0000${String(app.id)}/${String(a.id)}`, { kind, guarded });
        }
      }
    }
    if (tool === 'phone.home.list') {
      for (const d of (Array.isArray(res.detail.devices) ? res.detail.devices : []) as Array<Record<string, unknown>>) {
        this.homeHints.set(`${serial}\u0000${String(d.id)}`, { kind: typeof d.kind === 'string' ? d.kind : undefined });
      }
      for (const s of (Array.isArray(res.detail.scenes) ? res.detail.scenes : []) as Array<Record<string, unknown>>) {
        this.homeHints.set(`${serial}\u0000${String(s.id)}`, { security: s.security === true || s.guarded === true });
      }
    }
  }
}
