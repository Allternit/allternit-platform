/**
 * Bot-facing phone tools: screenshot, tap, type, swipe, home, back, key,
 * open_app, task, sms.send, call.dial. Android phones (Lane 2a) run them over
 * adb; iPhones on an Allternit USB Agent (phone-usb-agent.ts) run the same
 * names through the board, so a bot uses one set of tools for both.
 *
 * Everything here is transport-agnostic: the adb runner, approval gate, rate
 * store and ARTEMIS client are injected, so the argument validation and the
 * safety rules (approval on every send/dial, person-to-person rate limits,
 * no emergency numbers) are unit-tested with fakes and no phone.
 */

import { createHash } from 'node:crypto';
import type { PhoneLinkHealth } from './phone-link-health.js';

// ── Injected collaborators ──────────────────────────────────────────────────

export interface AdbResult {
  stdout: string;
  stderr: string;
  code: number;
}

export interface AdbCall {
  (args: string[], opts?: { serial?: string; timeoutMs?: number }): Promise<AdbResult>;
  /** Raw stdout bytes (screencap). */
  binary(args: string[], opts?: { serial?: string; timeoutMs?: number }): Promise<Buffer>;
}

export interface ApprovalRequest {
  kind: 'sms' | 'dial';
  serial: string;
  /** Human-readable, shown verbatim to the user. */
  summary: string;
  /** SHA-256 of the canonical action; the gate decision is bound to it. */
  actionHash: string;
}

export type ApprovalDecision = 'approved' | 'denied' | 'timeout';

export interface ApprovalGate {
  request(req: ApprovalRequest): Promise<ApprovalDecision>;
}

export interface RateStore {
  /** Epoch-ms timestamps of prior sends of this kind on this device. */
  load(key: string): number[];
  save(key: string, timestamps: number[]): void;
}

export interface ArtemisClient {
  runTask(serial: string, task: string, model: 'Flash' | 'Pro'): Promise<unknown>;
}

/** iPhones on a USB Agent. Serials start with "usb:". */
export interface UsbPhoneBackend {
  has(serial: string): boolean;
  serials(): string[];
  call(name: string, serial: string, args: Record<string, unknown>): Promise<ToolResult>;
}

/** A Phone-Linked phone as phone.devices lists it, health included. */
export interface LinkedPhoneDevice {
  serial: string;
  platform: 'ios' | 'android';
  via: 'phone-link';
  route: 'iphone-app-ictrl' | 'android-app';
  name?: string;
  health?: PhoneLinkHealth;
}

/** Phone-Linked phones (iPhone helper + Android app, one iCTRL lane). Serials start with "link:". */
export interface PhoneLinkBackend extends UsbPhoneBackend {
  linkedDevices(): Promise<LinkedPhoneDevice[]>;
}

export interface PhoneToolDeps {
  adb: AdbCall;
  gate: ApprovalGate;
  rates: RateStore;
  /** Resolved per call: ARTEMIS can be installed after Desktop starts. */
  artemis: () => ArtemisClient | null;
  now?: () => number;
  /** Serials of phones that are online (for default-device resolution). */
  onlineSerials: () => string[];
  /** The phone's default SMS subscription id, or null when it can't be read. */
  defaultSmsSubscription?: (serial: string) => Promise<number | null>;
  /** iPhones on a USB Agent, when that lane is running. */
  usb?: () => UsbPhoneBackend | null;
  /** Phone-Linked phones, when any are linked. Consulted exactly like usb. */
  links?: () => PhoneLinkBackend | null;
}

// ── Result shapes ───────────────────────────────────────────────────────────

export type ToolErrorCode =
  | 'unknown_tool'
  | 'invalid_args'
  | 'device_required'
  | 'device_offline'
  | 'approval_denied'
  | 'approval_timeout'
  | 'rate_limited'
  | 'blocked_number'
  | 'unknown_app'
  | 'artemis_unavailable'
  | 'adb_failed'
  // USB Agent (iPhone) states
  | 'unsupported'
  | 'disarmed'
  | 'device_locked'
  | 'no_screen'
  | 'not_calibrated'
  | 'calibration_failed'
  | 'uncertain'
  // Phone Link lane states
  | 'pairing_invalid'
  // element lookup (phone.find_and_tap)
  | 'not_found';

export type ToolResult =
  | { ok: true; [key: string]: unknown }
  | { ok: false; error: ToolErrorCode; message: string; retryAfterSec?: number };

const fail = (error: ToolErrorCode, message: string, extra: { retryAfterSec?: number } = {}): ToolResult => ({
  ok: false,
  error,
  message,
  ...extra,
});

// ── Tool catalogue (what the MCP shim advertises) ───────────────────────────

const serialProp = { type: 'string', description: 'Phone serial from phone.devices: Android phones as "host:port", iPhones on a USB Agent as "usb:…", Phone-Linked phones as "link:…". Optional when exactly one phone is online.' };

export const PHONE_TOOLS = [
  {
    name: 'phone.devices',
    description: 'List connected phones: Android phones (adb), iPhones on an Allternit USB Agent (serial "usb:…"), and Phone-Linked phones with the AI apps signed in (serial "link:…", with per-phone health).',
    inputSchema: { type: 'object', properties: {}, additionalProperties: false },
  },
  {
    name: 'phone.screenshot',
    description: 'Take a screenshot of the phone. Returns a base64 image (PNG on Android, JPEG of the AirPlay mirror on USB-Agent iPhones, JPEG on Phone-Linked phones) with its width and height. Tap coordinates are pixels of this image; on Phone-Linked phones coordinate_space says the space taps use ("pixels", or "points" when the image is 1:1 points).',
    inputSchema: { type: 'object', properties: { serial: serialProp }, additionalProperties: false },
  },
  {
    name: 'phone.tap',
    description: 'Tap at pixel coordinates from the latest screenshot. On iPhone the result says whether the screen changed; take a new screenshot before the next tap.',
    inputSchema: {
      type: 'object',
      properties: { serial: serialProp, x: { type: 'integer', minimum: 0 }, y: { type: 'integer', minimum: 0 } },
      required: ['x', 'y'],
      additionalProperties: false,
    },
  },
  {
    name: 'phone.type',
    description: 'Type ASCII text into the focused field (max 500 chars).',
    inputSchema: {
      type: 'object',
      properties: { serial: serialProp, text: { type: 'string', maxLength: 500 } },
      required: ['text'],
      additionalProperties: false,
    },
  },
  {
    name: 'phone.read_ui',
    description: 'Read the screen as text: one line per element (class, label, id, bounds). On Phone-Linked phones it uses the helper\'s own read; on Android it parses the accessibility dump; on a USB-Agent iPhone use phone.screenshot instead.',
    inputSchema: {
      type: 'object',
      properties: {
        serial: serialProp,
        filter: { type: 'object', properties: { label: { type: 'string' }, type: { type: 'string' } }, additionalProperties: false },
        visible_only: { type: 'boolean' },
        max_elements: { type: 'integer', minimum: 1, maximum: 1000 },
      },
      additionalProperties: false,
    },
  },
  {
    name: 'phone.screen_text',
    description: 'All readable text on the screen, one line per element. Works on Phone-Linked phones and Android over adb; not on a USB-Agent iPhone.',
    inputSchema: { type: 'object', properties: { serial: serialProp }, additionalProperties: false },
  },
  {
    name: 'phone.find_and_tap',
    description: 'Find an element on the screen and tap its centre. element_id is a resource id (Android), accessibility id or label; match narrows by label text. Works on Phone-Linked phones and Android over adb; not on a USB-Agent iPhone.',
    inputSchema: {
      type: 'object',
      properties: { serial: serialProp, element_id: { type: 'string', maxLength: 200 }, match: { type: 'string', maxLength: 200 } },
      required: ['element_id'],
      additionalProperties: false,
    },
  },
  {
    name: 'phone.swipe',
    description: 'Swipe between two points, or in a direction (up/down/left/right) from screen centre.',
    inputSchema: {
      type: 'object',
      properties: {
        serial: serialProp,
        x1: { type: 'integer', minimum: 0 },
        y1: { type: 'integer', minimum: 0 },
        x2: { type: 'integer', minimum: 0 },
        y2: { type: 'integer', minimum: 0 },
        direction: { enum: ['up', 'down', 'left', 'right'] },
        durationMs: { type: 'integer', minimum: 50, maximum: 5000 },
      },
      additionalProperties: false,
    },
  },
  {
    name: 'phone.home',
    description: 'Go to the home screen.',
    inputSchema: { type: 'object', properties: { serial: serialProp }, additionalProperties: false },
  },
  {
    name: 'phone.back',
    description: 'Go back (Android back button; on iPhone, the swipe from the left edge).',
    inputSchema: { type: 'object', properties: { serial: serialProp }, additionalProperties: false },
  },
  {
    name: 'phone.key',
    description: 'Press a key or shortcut: enter, esc, tab, backspace, space, up/down/left/right, or a combo like cmd+space (iPhone hardware-keyboard shortcuts).',
    inputSchema: {
      type: 'object',
      properties: { serial: serialProp, combo: { type: 'string', maxLength: 40 } },
      required: ['combo'],
      additionalProperties: false,
    },
  },
  {
    name: 'phone.open_app',
    description: 'Open an app. Android: package name (com.android.settings) or common name (settings, chrome, camera…). iPhone: the app name as shown on the home screen (Spotlight).',
    inputSchema: {
      type: 'object',
      properties: { serial: serialProp, app: { type: 'string', maxLength: 128 } },
      required: ['app'],
      additionalProperties: false,
    },
  },
  {
    name: 'phone.task',
    description: 'Do a plain-English task on the phone through Google ARTEMIS (e.g. "open Settings and turn on dark mode").',
    inputSchema: {
      type: 'object',
      properties: { serial: serialProp, task: { type: 'string', maxLength: 2000 }, model: { enum: ['Flash', 'Pro'] } },
      required: ['task'],
      additionalProperties: false,
    },
  },
  {
    name: 'phone.sms.send',
    description:
      'Send one text from the phone\'s own SIM to one person. The user must approve every message on their screen; person-to-person only, rate limited, never bulk.',
    inputSchema: {
      type: 'object',
      properties: {
        serial: serialProp,
        to: { type: 'string', description: 'Phone number, digits with optional leading +.' },
        text: { type: 'string', maxLength: 1000 },
        subscription_id: { type: 'integer', minimum: 0, description: 'SIM subscription id from phone.devices. Default SIM if omitted.' },
      },
      required: ['to', 'text'],
      additionalProperties: false,
    },
  },
  {
    name: 'phone.call.dial',
    description:
      'Place a call from the phone\'s own SIM. Dial only — the bot cannot speak on a carrier call. The user must approve every call.',
    inputSchema: {
      type: 'object',
      properties: { serial: serialProp, number: { type: 'string' }, subscription_id: { type: 'integer', minimum: 0 } },
      required: ['number'],
      additionalProperties: false,
    },
  },
] as const;

// ── Rate limits (person-to-person only) ─────────────────────────────────────

export const SMS_LIMITS = { minGapMs: 15_000, perHour: 10, perDay: 30 } as const;
export const DIAL_LIMITS = { minGapMs: 10_000, perHour: 10, perDay: 30 } as const;

type Limits = { minGapMs: number; perHour: number; perDay: number };

export function checkRate(
  history: number[],
  now: number,
  limits: Limits,
): { ok: true } | { ok: false; retryAfterSec: number; reason: string } {
  const day = history.filter((t) => now - t < 86_400_000);
  const hour = day.filter((t) => now - t < 3_600_000);
  const last = day.length ? Math.max(...day) : 0;
  if (last && now - last < limits.minGapMs) {
    return { ok: false, retryAfterSec: Math.ceil((limits.minGapMs - (now - last)) / 1000), reason: 'too soon after the previous one' };
  }
  if (hour.length >= limits.perHour) {
    const oldest = Math.min(...hour);
    return { ok: false, retryAfterSec: Math.ceil((3_600_000 - (now - oldest)) / 1000), reason: `hourly limit of ${limits.perHour} reached` };
  }
  if (day.length >= limits.perDay) {
    const oldest = Math.min(...day);
    return { ok: false, retryAfterSec: Math.ceil((86_400_000 - (now - oldest)) / 1000), reason: `daily limit of ${limits.perDay} reached` };
  }
  return { ok: true };
}

// ── Validation helpers ──────────────────────────────────────────────────────

const EMERGENCY = new Set(['911', '112', '999', '110', '000', '988', '933']);
const COORD_MAX = 20_000;

export function normalizeNumber(raw: unknown): string | null {
  if (typeof raw !== 'string') return null;
  const stripped = raw.replace(/[\s().-]/g, '');
  return /^\+?[0-9]{3,15}$/.test(stripped) ? stripped : null;
}

export function isEmergencyNumber(normalized: string): boolean {
  return EMERGENCY.has(normalized.replace(/^\+/, '')) || EMERGENCY.has(normalized.replace(/^\+1/, ''));
}

const APP_ALIASES: Record<string, string> = {
  settings: 'com.android.settings',
  chrome: 'com.android.chrome',
  camera: 'com.android.camera2',
  clock: 'com.google.android.deskclock',
  calculator: 'com.google.android.calculator',
  contacts: 'com.google.android.contacts',
  messages: 'com.google.android.apps.messaging',
  phone: 'com.google.android.dialer',
  maps: 'com.google.android.apps.maps',
  gmail: 'com.google.android.gm',
  photos: 'com.google.android.apps.photos',
  'play store': 'com.android.vending',
  youtube: 'com.google.android.youtube',
};

const ANDROID_KEYS: Record<string, string> = {
  enter: 'KEYCODE_ENTER',
  esc: 'KEYCODE_ESCAPE',
  tab: 'KEYCODE_TAB',
  backspace: 'KEYCODE_DEL',
  space: 'KEYCODE_SPACE',
  up: 'KEYCODE_DPAD_UP',
  down: 'KEYCODE_DPAD_DOWN',
  left: 'KEYCODE_DPAD_LEFT',
  right: 'KEYCODE_DPAD_RIGHT',
};

export const PACKAGE_RE = /^[a-zA-Z][a-zA-Z0-9_]*(\.[a-zA-Z][a-zA-Z0-9_]*)+$/;

export function resolveApp(app: unknown): string | null {
  if (typeof app !== 'string') return null;
  const trimmed = app.trim();
  if (PACKAGE_RE.test(trimmed)) return trimmed;
  return APP_ALIASES[trimmed.toLowerCase()] ?? null;
}

/** Quote for the on-device `sh`: adb joins argv into one string for the remote shell. */
export function shQuote(value: string): string {
  return `'${value.replace(/'/g, `'\\''`)}'`;
}

/** `input text` argument: spaces become %s. Printable ASCII only; "%s" literal is ambiguous so refused. */
export function encodeInputText(text: string): string | null {
  if (!/^[\x20-\x7E]*$/.test(text) || text.includes('%s')) return null;
  return text.replace(/ /g, '%s');
}

export function canonicalHash(value: Record<string, unknown>): string {
  const sorted = Object.keys(value)
    .sort()
    .reduce<Record<string, unknown>>((acc, key) => {
      acc[key] = value[key];
      return acc;
    }, {});
  return createHash('sha256').update(JSON.stringify(sorted)).digest('hex');
}

// ── Screen reading (phone.read_ui / phone.screen_text / phone.find_and_tap) ──
// The text form is shared with the Phone Link lane's page_source: one line
// per element — class, label (text || content-desc), id (resource-id), bounds
// as {{x, y}, {w, h}}.

export interface UiElement {
  className: string;
  /** text || content-desc. */
  label: string;
  /** resource-id. */
  id: string;
  bounds: string;
  rect: { x: number; y: number; w: number; h: number };
}

const XML_ENTITIES: Record<string, string> = { quot: '"', amp: '&', lt: '<', gt: '>', apos: "'" };

function unescapeXml(value: string): string {
  return value.replace(/&(#x?[0-9a-fA-F]+|\w+);/g, (all, entity: string) => {
    if (entity.startsWith('#x') || entity.startsWith('#X')) return String.fromCodePoint(parseInt(entity.slice(2), 16));
    if (entity.startsWith('#')) return String.fromCodePoint(parseInt(entity.slice(1), 10));
    return XML_ENTITIES[entity] ?? all;
  });
}

/** uiautomator dump XML (binary-clean, from `exec-out … /dev/tty`) → elements. */
export function parseUiDump(xml: string): UiElement[] {
  const out: UiElement[] = [];
  for (const tag of xml.matchAll(/<node\b[^>]*?>/g)) {
    const attrs: Record<string, string> = {};
    for (const m of tag[0].matchAll(/([\w:.-]+)="([^"]*)"/g)) attrs[m[1]] = unescapeXml(m[2]);
    if (!attrs.class) continue;
    const bounds = /^\[(\d+),(\d+)\]\[(\d+),(\d+)\]$/.exec(attrs.bounds ?? '');
    if (!bounds) continue;
    const [x1, y1, x2, y2] = [Number(bounds[1]), Number(bounds[2]), Number(bounds[3]), Number(bounds[4])];
    out.push({
      className: attrs.class,
      label: attrs.text || attrs['content-desc'] || '',
      id: attrs['resource-id'] || '',
      bounds: `{{${x1}, ${y1}}, {${x2 - x1}, ${y2 - y1}}}`,
      rect: { x: x1, y: y1, w: x2 - x1, h: y2 - y1 },
    });
  }
  return out;
}

/** The shared text form: `class "label" [id] {{x, y}, {w, h}}` (empty fields omitted). */
export function formatUiElement(el: UiElement): string {
  return [el.className, el.label ? `"${el.label}"` : '', el.id ? `[${el.id}]` : '', el.bounds].filter(Boolean).join(' ');
}

function parseFilter(a: Record<string, unknown>): { label?: string; type?: string } | ToolResult {
  if (a.filter === undefined) return {};
  if (typeof a.filter !== 'object' || a.filter === null) return fail('invalid_args', 'filter must be an object like { label, type }.');
  const f = a.filter as { label?: unknown; type?: unknown };
  if (f.label !== undefined && typeof f.label !== 'string') return fail('invalid_args', 'filter.label must be a string.');
  if (f.type !== undefined && typeof f.type !== 'string') return fail('invalid_args', 'filter.type must be a string.');
  return { label: f.label, type: f.type };
}

const isInt = (v: unknown, max = COORD_MAX): v is number => Number.isInteger(v) && (v as number) >= 0 && (v as number) <= max;

/**
 * `service call isms` recipe for sendTextForSubscriber (Android 11–14 layout:
 * subId, callingPkg, attributionTag, dest, scAddr, text, sentIntent,
 * deliveryIntent, persist, messageId). The transaction number is 5 on the
 * AOSP builds this was verified against; OEM builds can differ, which is why
 * sms.send reports "submitted", never "delivered".
 */
export function buildIsmsSend(subscriptionId: number, to: string, text: string): string[] {
  return [
    'shell',
    'service',
    'call',
    'isms',
    '5',
    'i32',
    String(subscriptionId),
    's16',
    'com.android.mms.service',
    's16',
    'null',
    's16',
    shQuote(to),
    's16',
    'null',
    's16',
    shQuote(text),
    's16',
    'null',
    's16',
    'null',
    'i32',
    '0',
    'i64',
    '0',
  ];
}

// ── The runner ──────────────────────────────────────────────────────────────

export class PhoneToolRunner {
  private readonly now: () => number;

  constructor(private readonly deps: PhoneToolDeps) {
    this.now = deps.now ?? Date.now;
  }

  async call(name: string, args: unknown): Promise<ToolResult> {
    const a = (args && typeof args === 'object' ? args : {}) as Record<string, unknown>;
    const usb = this.deps.usb?.() ?? null;
    const links = this.deps.links?.() ?? null;
    if (name === 'phone.devices') {
      const android = this.deps.onlineSerials();
      const iphones = usb?.serials() ?? [];
      const linked = links ? await links.linkedDevices() : [];
      return {
        ok: true,
        serials: [...android, ...iphones, ...linked.map((d) => d.serial)],
        devices: [...android.map((serial) => ({ serial, platform: 'android' })), ...iphones.map((serial) => ({ serial, platform: 'ios', via: 'usb-agent' })), ...linked],
      };
    }

    const known = PHONE_TOOLS.some((t) => t.name === name);
    if (!known) return fail('unknown_tool', `No phone tool named ${name}.`);

    const serial = this.resolveSerial(a.serial, usb, links);
    if (typeof serial !== 'string') return serial;
    if (usb?.has(serial)) return usb.call(name, serial, a);
    if (links?.has(serial)) return links.call(name, serial, a);

    try {
      switch (name) {
        case 'phone.screenshot':
          return await this.screenshot(serial);
        case 'phone.tap':
          return await this.tap(serial, a);
        case 'phone.type':
          return await this.type(serial, a);
        case 'phone.swipe':
          return await this.swipe(serial, a);
        case 'phone.home':
          return (await this.sh(serial, ['shell', 'input', 'keyevent', 'KEYCODE_HOME'])) ?? { ok: true };
        case 'phone.back':
          return (await this.sh(serial, ['shell', 'input', 'keyevent', 'KEYCODE_BACK'])) ?? { ok: true };
        case 'phone.key':
          return await this.key(serial, a);
        case 'phone.open_app':
          return await this.openApp(serial, a);
        case 'phone.task':
          return await this.task(serial, a);
        case 'phone.sms.send':
          return await this.smsSend(serial, a);
        case 'phone.call.dial':
          return await this.callDial(serial, a);
        case 'phone.read_ui':
          return await this.readUi(serial, a);
        case 'phone.screen_text':
          return await this.screenText(serial);
        case 'phone.find_and_tap':
          return await this.findAndTap(serial, a);
        default:
          return fail('unknown_tool', `No phone tool named ${name}.`);
      }
    } catch (error) {
      return fail('adb_failed', error instanceof Error ? error.message : String(error));
    }
  }

  private resolveSerial(raw: unknown, usb: UsbPhoneBackend | null, links: PhoneLinkBackend | null): string | ToolResult {
    const online = [...this.deps.onlineSerials(), ...(usb?.serials() ?? []), ...(links?.serials() ?? [])];
    if (raw !== undefined) {
      if (typeof raw !== 'string' || !raw) return fail('invalid_args', 'serial must be a non-empty string.');
      if (!online.includes(raw)) return fail('device_offline', `Phone ${raw} is not connected.`);
      return raw;
    }
    if (online.length === 1) return online[0];
    if (online.length === 0) return fail('device_offline', 'No phone is connected. Connect one in Settings → Computers → Phones.');
    return fail('device_required', `Several phones are connected (${online.join(', ')}); pass serial.`);
  }

  private async sh(serial: string, args: string[], timeoutMs = 15_000): Promise<ToolResult | null> {
    const res = await this.deps.adb(args, { serial, timeoutMs });
    return res.code === 0 ? null : fail('adb_failed', (res.stderr || res.stdout || `adb exited ${res.code}`).trim().slice(0, 300));
  }

  private async screenshot(serial: string): Promise<ToolResult> {
    const png = await this.deps.adb.binary(['exec-out', 'screencap', '-p'], { serial, timeoutMs: 20_000 });
    if (png.length < 8 || png[0] !== 0x89 || png[1] !== 0x50) return fail('adb_failed', 'Screenshot did not return a PNG.');
    return { ok: true, mimeType: 'image/png', dataBase64: png.toString('base64') };
  }

  private async tap(serial: string, a: Record<string, unknown>): Promise<ToolResult> {
    if (!isInt(a.x) || !isInt(a.y)) return fail('invalid_args', 'x and y must be non-negative integers.');
    return (await this.sh(serial, ['shell', 'input', 'tap', String(a.x), String(a.y)])) ?? { ok: true };
  }

  private async type(serial: string, a: Record<string, unknown>): Promise<ToolResult> {
    if (typeof a.text !== 'string' || a.text.length === 0 || a.text.length > 500) {
      return fail('invalid_args', 'text must be 1–500 characters.');
    }
    const encoded = encodeInputText(a.text);
    if (encoded === null) return fail('invalid_args', 'text must be printable ASCII and must not contain "%s".');
    return (await this.sh(serial, ['shell', 'input', 'text', shQuote(encoded)])) ?? { ok: true };
  }

  private async swipe(serial: string, a: Record<string, unknown>): Promise<ToolResult> {
    const duration = a.durationMs === undefined ? 300 : a.durationMs;
    if (!Number.isInteger(duration) || (duration as number) < 50 || (duration as number) > 5000) {
      return fail('invalid_args', 'durationMs must be 50–5000.');
    }
    let coords: number[];
    if (a.direction !== undefined) {
      const size = await this.deps.adb(['shell', 'wm', 'size'], { serial });
      const m = /(\d+)x(\d+)/.exec(size.stdout);
      if (!m) return fail('adb_failed', 'Could not read the screen size.');
      const [w, h] = [Number(m[1]), Number(m[2])];
      const [cx, cy] = [Math.floor(w / 2), Math.floor(h / 2)];
      const dx = Math.floor(w / 3);
      const dy = Math.floor(h / 3);
      const map: Record<string, number[]> = {
        up: [cx, cy + dy, cx, cy - dy],
        down: [cx, cy - dy, cx, cy + dy],
        left: [cx + dx, cy, cx - dx, cy],
        right: [cx - dx, cy, cx + dx, cy],
      };
      const picked = typeof a.direction === 'string' ? map[a.direction] : undefined;
      if (!picked) return fail('invalid_args', 'direction must be up, down, left or right.');
      coords = picked;
    } else {
      coords = [a.x1, a.y1, a.x2, a.y2] as number[];
      if (!coords.every((c) => isInt(c))) return fail('invalid_args', 'Give x1,y1,x2,y2 as non-negative integers, or a direction.');
    }
    return (await this.sh(serial, ['shell', 'input', 'swipe', ...coords.map(String), String(duration)])) ?? { ok: true };
  }

  private async key(serial: string, a: Record<string, unknown>): Promise<ToolResult> {
    const code = typeof a.combo === 'string' ? ANDROID_KEYS[a.combo.toLowerCase()] : undefined;
    if (!code) return fail('invalid_args', `On Android, combo is one of: ${Object.keys(ANDROID_KEYS).join(', ')}.`);
    return (await this.sh(serial, ['shell', 'input', 'keyevent', code])) ?? { ok: true };
  }

  /** `uiautomator dump` to stdout, binary so CRLF never corrupts the XML. */
  private async uiDump(serial: string): Promise<string | ToolResult> {
    const buf = await this.deps.adb.binary(['exec-out', 'uiautomator', 'dump', '/dev/tty'], { serial, timeoutMs: 15_000 });
    const xml = buf.toString('utf8');
    if (!xml.includes('<node')) return fail('adb_failed', 'Could not read the screen.');
    return xml;
  }

  private async readUi(serial: string, a: Record<string, unknown>): Promise<ToolResult> {
    const filter = parseFilter(a);
    if ('ok' in filter) return filter;
    const max = a.max_elements === undefined ? 200 : a.max_elements;
    if (!Number.isInteger(max) || (max as number) < 1 || (max as number) > 1000) {
      return fail('invalid_args', 'max_elements must be 1–1000.');
    }
    const xml = await this.uiDump(serial);
    if (typeof xml !== 'string') return xml;
    let els = parseUiDump(xml);
    if (filter.label) els = els.filter((e) => e.label.toLowerCase().includes(filter.label!.toLowerCase()));
    if (filter.type) els = els.filter((e) => e.className.toLowerCase().includes(filter.type!.toLowerCase()));
    return { ok: true, text: els.slice(0, max as number).map(formatUiElement).join('\n') };
  }

  private async screenText(serial: string): Promise<ToolResult> {
    const xml = await this.uiDump(serial);
    if (typeof xml !== 'string') return xml;
    const lines = [...new Set(parseUiDump(xml).map((e) => e.label).filter(Boolean))];
    return { ok: true, text: lines.join('\n') };
  }

  private async findAndTap(serial: string, a: Record<string, unknown>): Promise<ToolResult> {
    if (typeof a.element_id !== 'string' || !a.element_id.trim()) return fail('invalid_args', 'element_id is required.');
    if (a.match !== undefined && typeof a.match !== 'string') return fail('invalid_args', 'match must be a string.');
    const xml = await this.uiDump(serial);
    if (typeof xml !== 'string') return xml;
    const id = a.element_id.trim().toLowerCase();
    const match = typeof a.match === 'string' ? a.match.toLowerCase() : undefined;
    const found = parseUiDump(xml).find(
      (e) =>
        (e.id.toLowerCase() === id || e.id.toLowerCase().endsWith(`:${id}`) || e.id.toLowerCase().endsWith(`/${id}`)) &&
        (!match || e.label.toLowerCase().includes(match)),
    );
    if (!found) {
      return fail('not_found', `No element matching "${a.element_id}"${match ? ` with label containing "${a.match}"` : ''} is on the screen.`);
    }
    const x = found.rect.x + Math.floor(found.rect.w / 2);
    const y = found.rect.y + Math.floor(found.rect.h / 2);
    return (await this.sh(serial, ['shell', 'input', 'tap', String(x), String(y)])) ?? { ok: true, element: a.element_id };
  }

  private async openApp(serial: string, a: Record<string, unknown>): Promise<ToolResult> {
    const pkg = resolveApp(a.app);
    if (!pkg) return fail('unknown_app', `"${String(a.app)}" is not a package name or a known app name. Use the package, e.g. com.android.settings.`);
    const res = await this.deps.adb(['shell', 'monkey', '-p', pkg, '-c', 'android.intent.category.LAUNCHER', '1'], { serial });
    if (res.code !== 0 || /No activities found|monkey aborted/i.test(res.stdout + res.stderr)) {
      return fail('unknown_app', `${pkg} is not installed or has no launcher activity.`);
    }
    return { ok: true, package: pkg };
  }

  private async task(serial: string, a: Record<string, unknown>): Promise<ToolResult> {
    if (typeof a.task !== 'string' || !a.task.trim() || a.task.length > 2000) {
      return fail('invalid_args', 'task must be 1–2000 characters.');
    }
    const model = a.model === undefined ? 'Flash' : a.model;
    if (model !== 'Flash' && model !== 'Pro') return fail('invalid_args', 'model must be Flash or Pro.');
    const artemis = this.deps.artemis();
    if (!artemis) {
      return fail('artemis_unavailable', 'Plain-English tasks need ARTEMIS. Enable it in Settings → Computers → Phones.');
    }
    const result = await artemis.runTask(serial, a.task.trim(), model);
    return { ok: true, result };
  }

  private async gated(
    kind: 'sms' | 'dial',
    serial: string,
    limits: Limits,
    summary: string,
    payload: Record<string, unknown>,
  ): Promise<ToolResult | null> {
    const rateKey = `${serial}:${kind}`;
    const rate = checkRate(this.deps.rates.load(rateKey), this.now(), limits);
    if (!rate.ok) {
      return fail('rate_limited', `Not sent: ${rate.reason}. Texts and calls are person-to-person only.`, { retryAfterSec: rate.retryAfterSec });
    }
    const decision = await this.deps.gate.request({ kind, serial, summary, actionHash: canonicalHash({ kind, serial, ...payload }) });
    if (decision === 'denied') return fail('approval_denied', 'The user declined.');
    if (decision === 'timeout') return fail('approval_timeout', 'The user did not respond in time.');
    // Count the attempt before sending: a failed send still used the quota.
    const history = this.deps.rates.load(rateKey).filter((t) => this.now() - t < 86_400_000);
    this.deps.rates.save(rateKey, [...history, this.now()]);
    return null;
  }

  private async smsSend(serial: string, a: Record<string, unknown>): Promise<ToolResult> {
    const to = normalizeNumber(a.to);
    if (!to) return fail('invalid_args', 'to must be a phone number (3–15 digits, optional leading +).');
    if (typeof a.text !== 'string' || a.text.length === 0 || a.text.length > 1000) {
      return fail('invalid_args', 'text must be 1–1000 characters.');
    }
    if (a.subscription_id !== undefined && !isInt(a.subscription_id, 1000)) {
      return fail('invalid_args', 'subscription_id must be a non-negative integer.');
    }
    if (isEmergencyNumber(to)) return fail('blocked_number', 'Emergency numbers are not allowed.');
    const sub = (a.subscription_id as number | undefined) ?? (await this.deps.defaultSmsSubscription?.(serial)) ?? null;
    if (sub === null) {
      return fail('invalid_args', 'Could not tell which SIM to send from; pass subscription_id from phone.devices.');
    }

    const blocked = await this.gated('sms', serial, SMS_LIMITS, `Send a text to ${to}:\n\n${a.text}`, { to, text: a.text, sub });
    if (blocked) return blocked;

    const res = await this.deps.adb(buildIsmsSend(sub, to, a.text), { serial, timeoutMs: 15_000 });
    // `service call` prints a Parcel; "Result: Parcel(00000000 ..." with a leading zero word is the success shape.
    const accepted = res.code === 0 && /Result: Parcel\(\s*00000000/.test(res.stdout) && !/Exception/i.test(res.stdout);
    if (!accepted) return fail('adb_failed', 'The phone did not accept the text. (Android build may use a different SMS service layout.)');
    return { ok: true, status: 'submitted', note: 'Handed to the phone\'s SMS service; delivery is not confirmed.' };
  }

  private async callDial(serial: string, a: Record<string, unknown>): Promise<ToolResult> {
    const number = normalizeNumber(a.number);
    if (!number) return fail('invalid_args', 'number must be a phone number (3–15 digits, optional leading +).');
    if (a.subscription_id !== undefined && !isInt(a.subscription_id, 1000)) {
      return fail('invalid_args', 'subscription_id must be a non-negative integer.');
    }
    if (isEmergencyNumber(number)) return fail('blocked_number', 'Emergency numbers are not allowed.');

    const blocked = await this.gated('dial', serial, DIAL_LIMITS, `Call ${number} from the phone`, { number, sub: a.subscription_id ?? -1 });
    if (blocked) return blocked;

    const args = ['shell', 'am', 'start', '-a', 'android.intent.action.CALL', '-d', shQuote(`tel:${number.replace('+', '%2B')}`)];
    if (a.subscription_id !== undefined) args.push('--ei', 'android.telecom.extra.PHONE_ACCOUNT_SUBSCRIPTION_ID', String(a.subscription_id));
    const res = await this.deps.adb(args, { serial });
    if (res.code !== 0 || /Error|Exception|Permission Denial/i.test(res.stdout + res.stderr)) {
      return fail('adb_failed', (res.stderr || res.stdout).trim().slice(0, 300) || 'The phone refused to place the call.');
    }
    return { ok: true, status: 'dialing', note: 'Call placed. The bot cannot speak or listen on a carrier call.' };
  }
}
