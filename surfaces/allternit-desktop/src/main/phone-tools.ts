/**
 * Bot-facing phone tools (Lane 2a): screenshot, tap, type, swipe, open_app,
 * task, sms.send, call.dial.
 *
 * Everything here is transport-agnostic: the adb runner, approval gate, rate
 * store and ARTEMIS client are injected, so the argument validation and the
 * safety rules (approval on every send/dial, person-to-person rate limits,
 * no emergency numbers) are unit-tested with fakes and no phone.
 */

import { createHash } from 'node:crypto';

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
  | 'adb_failed';

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

const serialProp = { type: 'string', description: 'Phone serial from phone.devices. Optional when exactly one phone is online.' };

export const PHONE_TOOLS = [
  {
    name: 'phone.devices',
    description: 'List paired phones with model, Android version, battery, SIMs and connection state.',
    inputSchema: { type: 'object', properties: {}, additionalProperties: false },
  },
  {
    name: 'phone.screenshot',
    description: 'Take a screenshot of the phone. Returns a base64 PNG.',
    inputSchema: { type: 'object', properties: { serial: serialProp }, additionalProperties: false },
  },
  {
    name: 'phone.tap',
    description: 'Tap at pixel coordinates (from the screenshot).',
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
    name: 'phone.open_app',
    description: 'Open an app by Android package name (com.android.settings) or common name (settings, chrome, camera…).',
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

const PACKAGE_RE = /^[a-zA-Z][a-zA-Z0-9_]*(\.[a-zA-Z][a-zA-Z0-9_]*)+$/;

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
    if (name === 'phone.devices') return { ok: true, serials: this.deps.onlineSerials() };

    const known = PHONE_TOOLS.some((t) => t.name === name);
    if (!known) return fail('unknown_tool', `No phone tool named ${name}.`);

    const serial = this.resolveSerial(a.serial);
    if (typeof serial !== 'string') return serial;

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
        case 'phone.open_app':
          return await this.openApp(serial, a);
        case 'phone.task':
          return await this.task(serial, a);
        case 'phone.sms.send':
          return await this.smsSend(serial, a);
        case 'phone.call.dial':
          return await this.callDial(serial, a);
        default:
          return fail('unknown_tool', `No phone tool named ${name}.`);
      }
    } catch (error) {
      return fail('adb_failed', error instanceof Error ? error.message : String(error));
    }
  }

  private resolveSerial(raw: unknown): string | ToolResult {
    const online = this.deps.onlineSerials();
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
