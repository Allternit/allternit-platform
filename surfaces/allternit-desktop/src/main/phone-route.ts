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
 * `official` is the Allternit iPhone app (officialLane: Shortcuts, the action
 * catalog, Home, and on-phone reads of the broadcast frame), through
 * allternit-api's `/phones/app/:id/rpc`. `mirror` is phone-mirror.ts (iPhone
 * Mirroring on a Mac) and `bt-mac` is phone-bt-mac.ts (this Mac as a Bluetooth
 * keyboard and mouse), both registered by phone-service.ts through
 * PhoneToolDeps.lanes; their stubs remain for runners built without them.
 * Routes not built yet are stubs that report "unavailable", so the router skips
 * them. The USB Agent and the old iCTRL helper are wrapped as the `usb-agent`
 * and `legacy-helper` lanes without changing them.
 */

import type { AppRpcResult } from './phone-app-store.js';
import { legacyHelperRouteHealth, needsYou, isNeedsYouCode, type NeedsYou, type NeedsYouCode, type RouteHealth } from './phone-link-health.js';
import type { AppPhoneLane, PhoneLinkBackend, ToolErrorCode, ToolResult, UsbPhoneBackend } from './phone-tools.js';
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

/** Tools that need a second capability as well: find_and_tap reads, then taps (the official route reads but cannot tap). */
const ALSO_NEEDS: Partial<Record<LaneTool, Capability>> = { 'phone.find_and_tap': 'tap' };

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
  /** The tool about to run; absent for phone.describe. The official route's health depends on it (a screen read needs the broadcast, the rest need the app). */
  tool?: LaneTool;
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
  official: 'Shortcuts, actions and Home need the next version of Allternit Desktop.',
  mirror: 'iPhone Mirroring is not available in this version of Allternit Desktop.',
  // The real lane is phone-bt-mac.ts, registered by phone-service.ts; this stub serves runners built without it.
  'bt-mac': 'Bluetooth keyboard and mouse from this Mac is not in this build of Allternit Desktop.',
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

// ── The official route: the Allternit iPhone app ────────────────────────────

/** Seconds the runtime and the phone wait by default (contract §3.1). */
const OFFICIAL_TIMEOUT_S = 30;
/** The runtime waits timeout_s + 5 s; this HTTP call waits 5 s longer so the runtime's answer arrives first. */
const OFFICIAL_HTTP_MARGIN_S = 10;
/** device.describe is asked again after this (phone.actions.* reuse it). */
const DESCRIBE_TTL_MS = 10_000;

const officialTimeoutMs = (timeoutS: unknown) => ((Number.isInteger(timeoutS) ? (timeoutS as number) : OFFICIAL_TIMEOUT_S) + OFFICIAL_HTTP_MARGIN_S) * 1000;

/** The Device Access switch a tool needs on the phone (Settings > Device Access in the Allternit app). */
function switchOf(tool: string): 'Shortcuts' | 'Home' | null {
  if (tool.startsWith('phone.home.')) return 'Home';
  if (tool === 'phone.shortcuts.run' || tool.startsWith('phone.actions.')) return 'Shortcuts';
  return null;
}

const asObj = (v: unknown): Record<string, unknown> => (v && typeof v === 'object' && !Array.isArray(v) ? (v as Record<string, unknown>) : {});

/**
 * The phone's answer as a lane result (contract §4/§5). A Phone Use reply's
 * `result` is the phone's own envelope `{ok, route, verified, detail, needs_you?}`:
 * its detail and needs_you are kept as they are. Runtime refusals become the
 * plain states the bot can explain: the app is closed (APP_NOT_FOREGROUND),
 * a switch is off, the phone did not answer in time.
 */
export function officialResult(tool: string, res: AppRpcResult): ToolResult {
  if (res.ok) {
    const env = res.result;
    const detail = asObj(env.detail);
    if (env.ok === true) return { ok: true, verified: env.verified === true, detail };
    const ny = asObj(env.needs_you);
    if (isNeedsYouCode(ny.code)) {
      const filled: NeedsYou = typeof ny.message === 'string' && ny.message ? { code: ny.code, message: ny.message } : needsYou(ny.code, { name: typeof detail.name === 'string' ? detail.name : 'Allternit Actions' });
      return { ok: false, error: 'needs_you', message: filled.message, needs_you: filled, detail } as ToolResult;
    }
    const code = typeof detail.error === 'string' ? detail.error : 'phone_failed';
    const message = typeof detail.message === 'string' && detail.message ? detail.message : 'The iPhone could not do that.';
    const error: ToolErrorCode = code === 'timeout' ? 'timeout' : code === 'bad_params' ? 'bad_params' : code === 'unknown_action' ? 'unknown_action' : code === 'busy' ? 'rate_limited' : 'phone_failed';
    return { ok: false, error, message, detail } as ToolResult;
  }
  const sw = switchOf(tool);
  switch (res.code) {
    case 'app_not_open': {
      const ny = needsYou('APP_NOT_FOREGROUND');
      return { ok: false, error: 'needs_you', message: ny.message, needs_you: ny, detail: { error: 'app_not_open' } } as ToolResult;
    }
    case 'device_access_off':
      return { ok: false, error: 'device_access_off', message: `Turn on ${sw ?? 'this'} in the Allternit app on your iPhone (Settings > Device Access). Do not retry until the person says it is on.`, detail: { error: 'device_access_off' } } as ToolResult;
    case 'permission_denied':
      return { ok: false, error: 'device_access_off', message: `iOS does not allow Allternit to use ${sw ?? 'this'}. Allow it for Allternit in the iPhone Settings app, then turn it on in Allternit Settings > Device Access.`, detail: { error: 'permission_denied' } } as ToolResult;
    case 'not_supported':
      return { ok: false, error: 'not_supported', message: res.message || 'This iPhone app version cannot do that yet. Update the Allternit app.', detail: { error: 'not_supported' } } as ToolResult;
    case 'app_timeout':
      return { ok: false, error: 'timeout', message: 'The iPhone did not answer in time. It may still have done it: check the phone before trying again.', detail: { error: 'timeout' } } as ToolResult;
    case 'busy':
      return { ok: false, error: 'rate_limited', message: 'Too many calls are waiting for this iPhone. Wait for them to finish.', detail: { error: 'busy' } } as ToolResult;
    case 'bad_request':
      return { ok: false, error: 'invalid_args', message: res.message || 'The iPhone call was not valid.', detail: { error: 'bad_request' } } as ToolResult;
    default:
      if (res.status === 409) return officialResult(tool, { ok: false, status: 409, code: 'app_not_open', message: '' });
      if (res.status === 504) return officialResult(tool, { ok: false, status: 504, code: 'app_timeout', message: '' });
      if (res.status === 404) return { ok: false, error: 'device_offline', message: 'This iPhone is not paired with this computer any more. Pair it again in the Allternit app (Settings > Phones).' };
      return { ok: false, error: 'phone_failed', message: res.message || `The Allternit app answered ${res.code || res.status}.`, detail: { error: res.code || 'phone_failed' } } as ToolResult;
  }
}

/** The bot's arguments as the phone's socket fields (contract §9.1): Home ids go as device_id / scene_id. */
export function officialRequest(tool: LaneTool, args: Record<string, unknown>): { method: string; params: Record<string, unknown> } | null {
  const pick = (...keys: string[]) => Object.fromEntries(keys.filter((k) => args[k] !== undefined).map((k) => [k, args[k]]));
  switch (tool) {
    case 'phone.shortcuts.run':
      return { method: 'device.shortcuts.run', params: pick('name', 'input', 'timeout_s') };
    case 'phone.home.list':
      return { method: 'device.home.list', params: pick('kind') };
    case 'phone.home.set':
      return { method: 'device.home.set', params: { device_id: args.id, state: args.state } };
    case 'phone.home.scene':
      return { method: 'device.home.scene', params: { scene_id: args.id } };
    case 'phone.read_ui':
      return { method: 'device.screen.read', params: pick('filter') };
    default:
      return null;
  }
}

interface ActionCatalog {
  built?: string;
  builtIn: { name: string; known: boolean; actions: string[] };
  apps: Array<{ id: string; name: string; category: string; shortcut: string; installed: boolean; link: string | null; actionCount: number }>;
}

/** The action catalog from device.describe's `catalog` (contract §7). */
function catalogOf(detail: Record<string, unknown>): ActionCatalog {
  const cat = asObj(detail.catalog);
  const built = asObj(cat.actions_shortcut);
  const str = (v: unknown, d = '') => (typeof v === 'string' ? v : d);
  return {
    built: typeof cat.built === 'string' ? cat.built : undefined,
    builtIn: {
      name: str(built.name, 'Allternit Actions'),
      known: built.known === true,
      actions: Array.isArray(built.actions) ? built.actions.filter((a): a is string => typeof a === 'string') : [],
    },
    apps: (Array.isArray(cat.apps) ? cat.apps : []).map((raw) => {
      const a = asObj(raw);
      return {
        id: str(a.id),
        name: str(a.name),
        category: str(a.category),
        shortcut: str(a.shortcut),
        installed: a.shortcut_known === true,
        link: typeof a.shortcut_link === 'string' ? a.shortcut_link : null,
        actionCount: (Number(a.actions) || 0) + (Number(a.web_actions) || 0),
      };
    }).filter((a) => a.id && a.shortcut),
  };
}

/**
 * phone.actions.list over device.describe. The built-in "Allternit Actions"
 * entries come with their ids, a derived kind and whether they are guarded;
 * a third-party app comes with its shortcut, whether it is added and how many
 * actions it has (device.describe sends the per-intent list only as a count).
 */
export function actionsListFromDescribe(detail: Record<string, unknown>, args: Record<string, unknown>): Record<string, unknown> {
  const cat = catalogOf(detail);
  const wantKind = typeof args.kind === 'string' ? args.kind : undefined;
  const wantApp = typeof args.app === 'string' ? args.app.toLowerCase() : undefined;
  const installedOnly = args.installed_only === true;
  const builtIn = cat.builtIn.actions
    .map((id) => {
      const kind = deriveActionKind(id);
      return { id, title: actionWords(id), kind, params: [] as string[], dispatch: 'actions_shortcut', installed: cat.builtIn.known, guarded: approvalClass('phone.actions.run', { app: 'allternit', action: id }, { action: { kind } }) !== null };
    })
    .filter((a) => !wantKind || a.kind === wantKind);
  const apps: Array<Record<string, unknown>> = [];
  if ((!wantApp || wantApp === 'allternit') && (!installedOnly || cat.builtIn.known)) {
    apps.push({ id: 'allternit', name: cat.builtIn.name, category: 'Built in', shortcut: cat.builtIn.name, installed: cat.builtIn.known, actions: builtIn });
  }
  if (!wantKind) {
    for (const a of cat.apps) {
      if (wantApp && a.id.toLowerCase() !== wantApp) continue;
      if (installedOnly && !a.installed) continue;
      apps.push({ id: a.id, name: a.name, category: a.category, shortcut: a.shortcut, installed: a.installed, action_count: a.actionCount, actions: [] });
    }
  }
  return {
    apps,
    ...(cat.built ? { built: cat.built } : {}),
    note: 'Third-party apps run through their own Allternit shortcut, which the person adds once in the Allternit app (Settings > Device Access > iPhone actions).',
  };
}

/** phone.actions.run as a device.shortcuts.run call (contract §3.1, §7.2), or a ToolResult when it cannot run. */
export function actionsRunRequest(detail: Record<string, unknown>, args: Record<string, unknown>): { name: string; input: Record<string, unknown>; dispatched_as: string } | ToolResult {
  const cat = catalogOf(detail);
  const app = String(args.app ?? '').toLowerCase();
  const action = String(args.action ?? '');
  const input = { action, params: asObj(args.params) };
  const unknown = (what: string): ToolResult => ({ ok: false, error: 'unknown_action', message: `${what} Call phone.actions.list to see what this iPhone can do.`, detail: { error: 'unknown_action' } } as ToolResult);
  if (app === 'allternit') {
    if (!cat.builtIn.actions.includes(action)) return unknown(`"${action}" is not an Allternit Actions action.`);
    return { name: cat.builtIn.name, input, dispatched_as: 'actions_shortcut' };
  }
  const entry = cat.apps.find((a) => a.id.toLowerCase() === app);
  if (!entry) return unknown(`No catalog app has the id "${String(args.app)}".`);
  return { name: entry.shortcut, input, dispatched_as: 'app_shortcut' };
}

/**
 * The `official` route: the Allternit iPhone app, through allternit-api's
 * `/phones/app/:id/rpc` (contract §9.1). Health comes from the app's last
 * state frame as the node lists it: the app must be open for shortcuts,
 * actions and Home; a screen read needs the broadcast instead. The approval
 * check (phone-tools.ts phoneUseApproval) runs before act is called.
 */
export function officialLane(apps: () => AppPhoneLane | null, now: () => number = Date.now): PhoneLane {
  const described = new Map<string, { at: number; detail: Record<string, unknown> }>();
  const live = (serial: string) => {
    const a = apps();
    if (!a?.has(serial) || !a.rpc || !a.officialState) return null;
    if (a.devices().find((d) => d.serial === serial)?.platform === 'android') return null;
    return a as AppPhoneLane & Required<Pick<AppPhoneLane, 'rpc' | 'officialState'>>;
  };
  const describe = async (a: NonNullable<ReturnType<typeof live>>, serial: string): Promise<ToolResult> => {
    const hit = described.get(serial);
    if (hit && now() - hit.at < DESCRIBE_TTL_MS) return { ok: true, detail: hit.detail };
    const res = officialResult('phone.describe', await a.rpc(serial, 'device.describe', {}, officialTimeoutMs(undefined)));
    if (res.ok) described.set(serial, { at: now(), detail: asObj(res.detail) });
    return res;
  };
  return {
    id: 'official',
    covers: [...OFFICIAL_CAPS, 'read_ui'],
    verify: 'self',
    health: async ({ serial, tool }) => {
      const a = live(serial);
      if (!a) {
        return apps()?.has(serial) && !apps()?.rpc
          ? { state: 'unavailable', reason: NOT_BUILT.official! }
          : { state: 'unavailable', reason: 'This iPhone is not connected with the Allternit app.' };
      }
      if (tool === 'phone.read_ui') {
        return a.officialState(serial)?.screen ? { state: 'healthy' } : { state: 'needs_you', needsYou: needsYou('BROADCAST_STOPPED') };
      }
      if (!a.officialState(serial)?.appOpen) await a.refresh(true);
      return a.officialState(serial)?.appOpen ? { state: 'healthy' } : { state: 'needs_you', needsYou: needsYou('APP_NOT_FOREGROUND') };
    },
    act: async (tool, serial, args) => {
      const a = live(serial);
      if (!a) return { ok: false, error: 'route_unavailable', message: 'This iPhone is not connected with the Allternit app.' };
      if (tool === 'phone.actions.list' || tool === 'phone.actions.run') {
        const d = await describe(a, serial);
        if (!d.ok) return d;
        const detail = asObj(d.detail);
        if (tool === 'phone.actions.list') return { ok: true, verified: true, detail: actionsListFromDescribe(detail, args) };
        const run = actionsRunRequest(detail, args);
        if ('ok' in run) return run;
        const params: Record<string, unknown> = { name: run.name, input: run.input, ...(args.timeout_s !== undefined ? { timeout_s: args.timeout_s } : {}) };
        const res = officialResult(tool, await a.rpc(serial, 'device.shortcuts.run', params, officialTimeoutMs(args.timeout_s)));
        return res.ok ? { ...res, detail: { output: asObj(res.detail).output ?? null, returned_via: asObj(res.detail).returned_via, dispatched_as: run.dispatched_as } } : res;
      }
      const req = officialRequest(tool, args);
      if (!req) return { ok: false, error: 'not_supported', message: `${tool} does not run on the official route.` };
      const res = officialResult(tool, await a.rpc(serial, req.method, req.params, officialTimeoutMs(args.timeout_s)));
      if (res.ok && tool === 'phone.read_ui') {
        // Existing readers take elements and text at the top level (phone-verify.ts PhoneUseResult).
        const detail = asObj(res.detail);
        return { ...res, text: detail.text ?? '', elements: detail.elements ?? [], width: detail.width, height: detail.height };
      }
      return res;
    },
    facts: async (serial) => {
      const a = live(serial);
      if (!a) return {};
      const d = await describe(a, serial);
      if (!d.ok) return {};
      const detail = asObj(d.detail);
      return {
        locked: typeof detail.locked === 'boolean' ? detail.locked : null,
        foreground_app: typeof detail.foreground_app === 'string' ? detail.foreground_app : null,
        shortcuts: Array.isArray(detail.shortcuts) ? detail.shortcuts.filter((x): x is string => typeof x === 'string') : [],
        home: { available: asObj(detail.home).available === true },
      };
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
  // A Device Access switch is off: the person has to turn it on, no other route has it.
  'device_access_off',
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
    const key = `${id}\u0000${ctx.serial}\u0000${ctx.tool ?? ''}`;
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
    const prefix = `${id}\u0000${serial}\u0000`;
    for (const key of [...this.cache.keys()]) if (key.startsWith(prefix)) this.cache.delete(key);
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
    const also = ALSO_NEEDS[tool];
    const order = (opts.route ? [opts.route] : ROUTE_ORDER).filter((id) => this.lanes[id].covers.includes(cap) && (!also || this.lanes[id].covers.includes(also)));
    if (opts.route && order.length === 0) {
      return failEnvelope(opts.route, 'not_supported', `${tool} does not run on the ${opts.route} route.`, { detail: { error: 'not_supported' } });
    }
    const ctx: LaneContext = { ...(await this.context(serial)), tool };
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
    // A lane that checked its own effect says so (the official route: the phone read the state back).
    const own = (raw as Record<string, unknown>).verified;
    return toEnvelope(lane.id, tool, raw, raw.ok && typeof own === 'boolean' ? { verified: own } : {});
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
