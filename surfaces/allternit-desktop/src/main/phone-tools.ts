/**
 * Bot-facing phone tools: screenshot, tap, type, swipe, home, back, key,
 * open_app, task, sms.send, sms.read, call.dial. Android phones (Lane 2a) run them over
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
import { appHints, catalogApp, catalogAppByName } from './phone-app-catalog.js';
import { PhoneApiUnavailable, type PhoneApi, type PhoneRunRecorder } from './phone-runs.js';

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
  kind: 'sms' | 'dial' | 'sms_read' | 'device_write';
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

/**
 * iPhones connected with the Allternit App Store app (phone-app-store.ts).
 * Serials start with "app:". The screen and the app's own browser only.
 */
export interface AppPhoneLane extends UsbPhoneBackend {
  /** Re-reads the node's phone list when stale. Never throws. */
  refresh(): Promise<void>;
  devices(): Array<{ serial: string; platform: 'ios'; via: 'allternit-app'; route: 'iphone-app-store'; name: string; screen: boolean; note: string }>;
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
  /**
   * Texts, calls and inbox reads on a linked phone (Android only: the
   * helper's shell service). The runner validates, applies the approval gate,
   * rate limits and emergency block, then calls these, so a linked phone
   * goes through exactly the checks the adb lane does.
   */
  canText?(serial: string): boolean;
  sendSms?(serial: string, to: string, text: string, subscriptionId: number | null): Promise<ToolResult>;
  dial?(serial: string, number: string, subscriptionId: number | null): Promise<ToolResult>;
  /** Raw `content query` rows of the inbox since sinceMs (parseSmsRows reads them). */
  readSms?(serial: string, sinceMs: number, from: string | null): Promise<{ ok: true; raw: string } | ToolResult>;
  /** A bot tool call started (+1) or ended (-1) on a linked phone; background account reads wait while it's busy. */
  noteActivity?(serial: string, delta: number): void;
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
  /** Notified as +/-1 while an acting tool call runs on a serial (takeover banner). */
  activity?: (serial: string, delta: number) => void;
  /** Phone-Linked phones, when any are linked. Consulted exactly like usb. */
  links?: () => PhoneLinkBackend | null;
  /** iPhones with the Allternit app (serial "app:…"): screen + the app's browser. */
  apps?: () => AppPhoneLane | null;
  /**
   * An approved dial went out (phone-call-bridge.ts): true when the phone has
   * Call audio set up, so the bot joins the call once it connects.
   */
  onDialPlaced?: (serial: string, number: string) => boolean;
  /** phone.sms.read's poll wait; tests pass a fake. */
  sleep?: (ms: number) => Promise<void>;
  /** Posts every call to the phone run history (phone-runs.ts). */
  recorder?: PhoneRunRecorder;
  /** The local allternit-api phone endpoints: phone.task on the bot's brain, phone skills. */
  api?: () => PhoneApi | null;
  /** Environment for the ARTEMIS fallback (GEMINI_API_KEY). Defaults to process.env. */
  env?: () => Record<string, string | undefined>;
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
  // A Phone-Linked phone answered with an error (iCTRL protocol)
  | 'phone_failed'
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
  // Android Phone Link: the accessibility service is off, so no screen read or tap works
  | 'accessibility_off'
  // element lookup (phone.find_and_tap)
  | 'not_found'
  // iPhone with the Allternit app (app:…): no taps on native apps without the Bluetooth board
  | 'not_supported'
  // phone.web.* / phone.calendar.* ...: the Allternit app is not open on the iPhone
  | 'app_not_open'
  // phone.calendar/reminders/contacts: the person has not turned that type on, or iOS access is denied
  | 'device_access_off';

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

const serialProp = { type: 'string', description: 'Phone serial from phone.devices: Android phones as "host:port", iPhones on a USB Agent as "usb:…", Phone-Linked phones as "link:…", iPhones with the Allternit app as "app:…". Optional when exactly one phone is online.' };
const appSerialProp = { type: 'string', description: 'An iPhone connected with the Allternit app ("app:…" from phone.devices). Optional when it is the only phone online.' };

export const PHONE_TOOLS = [
  {
    name: 'phone.devices',
    description: 'List connected phones: Android phones (adb), iPhones on an Allternit USB Agent (serial "usb:…"), Phone-Linked phones with the AI apps signed in (serial "link:…", with per-phone health), and iPhones with the Allternit app (serial "app:…", route "iphone-app-store": screen only plus the app\'s browser through phone.web.*; screen says whether the screen is shared now).',
    inputSchema: { type: 'object', properties: {}, additionalProperties: false },
  },
  {
    name: 'phone.screenshot',
    description: 'Take a screenshot of the phone. Returns a base64 image (PNG on Android, JPEG of the AirPlay mirror on USB-Agent iPhones, JPEG on Phone-Linked phones and on iPhones with the Allternit app, where it is the latest frame of the shared screen) with its width and height. Tap coordinates are pixels of this image; on Phone-Linked phones coordinate_space says the space taps use ("pixels", or "points" when the image is 1:1 points).',
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
    description: 'Press a key or shortcut: enter, esc, tab, backspace, space, up/down/left/right, or a combo like cmd+space (iPhone hardware-keyboard shortcuts). Phone-Linked Android phones also take home and back; a Phone-Linked iPhone takes only enter and home.',
    inputSchema: {
      type: 'object',
      properties: { serial: serialProp, combo: { type: 'string', maxLength: 40 } },
      required: ['combo'],
      additionalProperties: false,
    },
  },
  {
    name: 'phone.open_app',
    description: 'Open an app. Android: package name (com.android.settings) or common name (settings, chrome, camera…). iPhone: the app name as shown on the home screen (Spotlight). For common apps the result carries hints: short notes on where things are in that app.',
    inputSchema: {
      type: 'object',
      properties: { serial: serialProp, app: { type: 'string', maxLength: 128 } },
      required: ['app'],
      additionalProperties: false,
    },
  },
  {
    name: 'phone.task',
    description:
      'Do a plain-English task on the phone (e.g. "open Settings and turn on dark mode"). Runs on your own brain: each step reads the screen and picks one phone action, on every kind of phone. Pass bot_id (or brain as provider/model) so it uses your model. Every step lands in the phone run history; save a run that worked with phone.save_skill to replay it later with no model.',
    inputSchema: {
      type: 'object',
      properties: {
        serial: serialProp,
        task: { type: 'string', maxLength: 2000 },
        max_steps: { type: 'integer', minimum: 1, maximum: 40 },
        bot_id: { type: 'string', maxLength: 200, description: 'Your bot id: the task runs on your brain.' },
        brain: { type: 'string', maxLength: 200, description: 'A brain as provider/model, when bot_id is not known.' },
        model: { enum: ['Flash', 'Pro'], description: 'Only for the ARTEMIS fallback (Android over adb with GEMINI_API_KEY).' },
      },
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
    name: 'phone.sms.read',
    description:
      'Read recent texts in the phone\'s inbox, e.g. a sign-in code you just triggered. Android only (adb or Phone Link). The user approves this once for your bot on this phone (pass bot_id). Waits up to wait_s seconds for a text to arrive. Returns messages (newest first) and codes: 4–8 digit or letter-and-digit codes found in them.',
    inputSchema: {
      type: 'object',
      properties: {
        serial: serialProp,
        from: { type: 'string', description: 'Only texts from this number (digits, optional leading +).' },
        since_s: { type: 'integer', minimum: 1, maximum: 86400, description: 'How far back to look, in seconds (default 300).' },
        wait_s: { type: 'integer', minimum: 0, maximum: 120, description: 'Wait this long for a matching text to arrive (default 0).' },
        bot_id: { type: 'string', maxLength: 200, description: 'Your bot id. The user approves reading texts once per bot and phone; without it they are asked every time.' },
      },
      additionalProperties: false,
    },
  },
  {
    name: 'phone.call.dial',
    description:
      'Place a call from the phone\'s own SIM. The user must approve every call. When Call audio is set up for this phone in Allternit Desktop (audio cable + "Bot answers calls"), you talk on the call once it connects; otherwise this only dials and you can\'t speak or listen.',
    inputSchema: {
      type: 'object',
      properties: { serial: serialProp, number: { type: 'string' }, subscription_id: { type: 'integer', minimum: 0 } },
      required: ['number'],
      additionalProperties: false,
    },
  },
  {
    name: 'phone.screen.request',
    description: 'iPhone with the Allternit app ("app:…") only: ask the person to share the screen. The app shows the Share screen sheet (only the person can start it). Returns shown and live; once live, use phone.screenshot.',
    inputSchema: { type: 'object', properties: { serial: appSerialProp }, additionalProperties: false },
  },
  {
    name: 'phone.web.open',
    description: 'iPhone with the Allternit app ("app:…") only: open a web page in the app\'s own browser. The app must be open on the phone. Then phone.web.snapshot to see it.',
    inputSchema: {
      type: 'object',
      properties: { serial: appSerialProp, url: { type: 'string', maxLength: 2048, description: 'An http or https address.' } },
      required: ['url'],
      additionalProperties: false,
    },
  },
  {
    name: 'phone.web.snapshot',
    description: 'iPhone with the Allternit app only: the app browser\'s page as url, title, a JPEG and elements [{ref, role, name, text, value?, bounds}]. Refs are valid only until the next snapshot; act with phone.web.click / phone.web.type.',
    inputSchema: { type: 'object', properties: { serial: appSerialProp }, additionalProperties: false },
  },
  {
    name: 'phone.web.click',
    description: 'iPhone with the Allternit app only: click an element of the app\'s browser by its ref from the latest phone.web.snapshot.',
    inputSchema: {
      type: 'object',
      properties: { serial: appSerialProp, ref: { type: 'string', maxLength: 200 } },
      required: ['ref'],
      additionalProperties: false,
    },
  },
  {
    name: 'phone.web.type',
    description: 'iPhone with the Allternit app only: type into a field of the app\'s browser by its ref from the latest phone.web.snapshot (replaces its text). submit presses return after.',
    inputSchema: {
      type: 'object',
      properties: { serial: appSerialProp, ref: { type: 'string', maxLength: 200 }, text: { type: 'string', maxLength: 2000 }, submit: { type: 'boolean' } },
      required: ['ref', 'text'],
      additionalProperties: false,
    },
  },
  {
    name: 'phone.web.scroll',
    description: 'iPhone with the Allternit app only: scroll the app\'s browser page by dy points (positive = down).',
    inputSchema: {
      type: 'object',
      properties: { serial: appSerialProp, dy: { type: 'integer', minimum: -20000, maximum: 20000 } },
      required: ['dy'],
      additionalProperties: false,
    },
  },
  {
    name: 'phone.web.back',
    description: 'iPhone with the Allternit app only: go back one page in the app\'s browser.',
    inputSchema: { type: 'object', properties: { serial: appSerialProp }, additionalProperties: false },
  },
  {
    name: 'phone.web.wait',
    description: 'iPhone with the Allternit app only: wait up to ms milliseconds (max 10000) for the app\'s browser page to settle.',
    inputSchema: {
      type: 'object',
      properties: { serial: appSerialProp, ms: { type: 'integer', minimum: 0, maximum: 10000 } },
      required: ['ms'],
      additionalProperties: false,
    },
  },
  {
    name: 'phone.calendar.list',
    description: 'iPhone with the Allternit app only: list calendar events between two times (Device Access > Calendar must be on in the app, and the app must be open on the phone). Returns events [{id, title, start, end, allDay, location?, calendar}].',
    inputSchema: {
      type: 'object',
      properties: {
        serial: appSerialProp,
        from: { type: 'string', maxLength: 40, description: 'ISO 8601 start, e.g. 2026-11-01T00:00:00Z.' },
        to: { type: 'string', maxLength: 40, description: 'ISO 8601 end, at most 62 days after from.' },
        limit: { type: 'integer', minimum: 1, maximum: 200 },
      },
      required: ['from', 'to'],
      additionalProperties: false,
    },
  },
  {
    name: 'phone.calendar.create',
    description: 'iPhone with the Allternit app only: add one event to the default calendar. The person approves every event on the Desktop screen first. Device Access > Calendar must be on and the app open on the phone.',
    inputSchema: {
      type: 'object',
      properties: {
        serial: appSerialProp,
        title: { type: 'string', maxLength: 200 },
        start: { type: 'string', maxLength: 40, description: 'ISO 8601.' },
        end: { type: 'string', maxLength: 40, description: 'ISO 8601, after start. Optional: defaults to one hour.' },
        allDay: { type: 'boolean' },
        location: { type: 'string', maxLength: 200 },
        notes: { type: 'string', maxLength: 2000 },
      },
      required: ['title', 'start'],
      additionalProperties: false,
    },
  },
  {
    name: 'phone.reminders.list',
    description: 'iPhone with the Allternit app only: list reminders (Device Access > Reminders must be on and the app open). Returns reminders [{id, title, due?, completed, list}].',
    inputSchema: {
      type: 'object',
      properties: { serial: appSerialProp, completed: { type: 'boolean', description: 'true = completed only, false or omitted = open only.' }, limit: { type: 'integer', minimum: 1, maximum: 200 } },
      additionalProperties: false,
    },
  },
  {
    name: 'phone.reminders.create',
    description: 'iPhone with the Allternit app only: add one reminder to the default list. The person approves every reminder on the Desktop screen first. Device Access > Reminders must be on and the app open.',
    inputSchema: {
      type: 'object',
      properties: {
        serial: appSerialProp,
        title: { type: 'string', maxLength: 200 },
        due: { type: 'string', maxLength: 40, description: 'ISO 8601, optional.' },
        notes: { type: 'string', maxLength: 2000 },
      },
      required: ['title'],
      additionalProperties: false,
    },
  },
  {
    name: 'phone.contacts.search',
    description: 'iPhone with the Allternit app only: search contacts by name (read only; Device Access > Contacts must be on and the app open). Returns contacts [{id, name, phones[], emails[]}].',
    inputSchema: {
      type: 'object',
      properties: { serial: appSerialProp, query: { type: 'string', minLength: 1, maxLength: 100 }, limit: { type: 'integer', minimum: 1, maximum: 50 } },
      required: ['query'],
      additionalProperties: false,
    },
  },
  {
    name: 'phone.skills',
    description: 'List saved phone skills: recorded phone runs that replay with no model. Each lists its inputs (typed text is never stored, so each run passes it).',
    inputSchema: { type: 'object', properties: {}, additionalProperties: false },
  },
  {
    name: 'phone.save_skill',
    description:
      'Save a phone run from the run history as a named skill (record once, replay with no model). run_id comes from phone.task or the run history; from_step/to_step (0-based indexes in that run) save a part of it. Typed text becomes inputs (input_names names them, in order).',
    inputSchema: {
      type: 'object',
      properties: {
        run_id: { type: 'string', maxLength: 120 },
        name: { type: 'string', maxLength: 80 },
        serial: serialProp,
        from_step: { type: 'integer', minimum: 0 },
        to_step: { type: 'integer', minimum: 0 },
        description: { type: 'string', maxLength: 500 },
        input_names: { type: 'array', items: { type: 'string', maxLength: 60 }, maxItems: 20 },
      },
      required: ['run_id', 'name'],
      additionalProperties: false,
    },
  },
  {
    name: 'phone.run_skill',
    description:
      'Replay a saved phone skill with no model: each step finds its element again (id, label, resource id, then place and look) and checks what it changed. Only a step whose element moved asks your brain once. Returns per-step results and model_calls.',
    inputSchema: {
      type: 'object',
      properties: {
        serial: serialProp,
        name: { type: 'string', maxLength: 80 },
        inputs: { type: 'object', additionalProperties: { type: 'string', maxLength: 500 } },
        bot_id: { type: 'string', maxLength: 200 },
        brain: { type: 'string', maxLength: 200 },
      },
      required: ['name'],
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

export const ANDROID_KEYS: Record<string, string> = {
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

// ── Inbox reads (phone.sms.read) ────────────────────────────────────────────

export interface SmsRow {
  from: string;
  body: string;
  /** Epoch ms (the phone's clock). */
  date: number;
}

/**
 * `content query --projection address:body:date` output → rows. Each row is
 * `Row: N address=…, body=…, date=…`; a body can hold commas and newlines,
 * so address ends at the first ", body=" and body at the last ", date=".
 */
export function parseSmsRows(raw: string): SmsRow[] {
  const out: SmsRow[] = [];
  for (const chunk of raw.split(/^Row: \d+ /m).slice(1)) {
    const text = chunk.replace(/\r?\n$/, '');
    const m = /^address=([\s\S]*?), body=([\s\S]*), date=(\d+)\s*$/.exec(text);
    if (!m) continue;
    out.push({ from: m[1] === 'NULL' ? '' : m[1], body: m[2] === 'NULL' ? '' : m[2], date: Number(m[3]) });
  }
  return out.sort((a, b) => b.date - a.date);
}

/**
 * One-time codes in a text: first the ones right after a word like code,
 * OTP, PIN or verification (letters allowed, at least one digit), then any
 * other 4–8 digit run that isn't part of a longer number. "123-456" counts
 * as 123456.
 */
export function extractCodes(text: string): string[] {
  const found: string[] = [];
  const add = (code: string) => {
    const c = code.replace(/[- ]/g, '');
    if (!found.includes(c)) found.push(c);
  };
  const keyword = /\b(?:code|otp|passcode|pin|verification|password)\b(?:\s+(?:is|code|was))*\s*[:=#-]?\s*(\d{3}[- ]\d{3}|(?=[A-Za-z]*\d)[A-Za-z0-9]{4,8})\b/gi;
  for (const m of text.matchAll(keyword)) if (/\d/.test(m[1])) add(m[1]);
  // Not inside a longer number: no digit (or "5-", "5.") right before, none right after.
  for (const m of text.matchAll(/(?<![\d+.,:/]|\d-)(\d{3}[- ]\d{3}|\d{4,8})(?![\d.,:/-]?\d)/g)) add(m[1]);
  return found;
}

/** Digits only, for comparing a sender against `from` (+1 555… vs 555…). */
const digitsOf = (n: string) => n.replace(/\D/g, '');

// ── The runner ──────────────────────────────────────────────────────────────

export class PhoneToolRunner {
  private readonly now: () => number;
  /** Each call's `_source`, keyed by its args: "api" means a Platform API key, which always asks before reading texts. */
  private readonly sources = new WeakMap<Record<string, unknown>, string>();

  constructor(private readonly deps: PhoneToolDeps) {
    this.now = deps.now ?? Date.now;
  }

  async call(name: string, args: unknown): Promise<ToolResult> {
    const raw = (args && typeof args === 'object' ? args : {}) as Record<string, unknown>;
    // `_run_id` / `_source` (set by allternit-api's replay and task loops) are for the run history only.
    const a = Object.fromEntries(Object.entries(raw).filter(([k]) => !k.startsWith('_')));
    const started = this.now();
    if (typeof raw._source === 'string') this.sources.set(a, raw._source);
    const result = await this.dispatch(name, a);
    const recorder = this.deps.recorder;
    if (recorder && name !== 'phone.devices' && name !== 'phone.skills' && name !== 'phone.save_skill') {
      const serial = typeof a.serial === 'string' ? a.serial : this.onlyOnline();
      if (serial) recorder.record({ serial, tool: name, args: raw, result: result as Record<string, unknown>, durationMs: this.now() - started });
    }
    return result;
  }

  /** The one connected phone, when there is exactly one. */
  private onlyOnline(): string | null {
    const online = [
      ...this.deps.onlineSerials(),
      ...(this.deps.usb?.()?.serials() ?? []),
      ...(this.deps.links?.()?.serials() ?? []),
      ...(this.deps.apps?.()?.serials() ?? []),
    ];
    return online.length === 1 ? online[0] : null;
  }

  private async dispatch(name: string, a: Record<string, unknown>): Promise<ToolResult> {
    const usb = this.deps.usb?.() ?? null;
    const links = this.deps.links?.() ?? null;
    const apps = this.deps.apps?.() ?? null;
    await apps?.refresh();
    if (name === 'phone.devices') {
      const android = this.deps.onlineSerials();
      const iphones = usb?.serials() ?? [];
      const linked = links ? await links.linkedDevices() : [];
      const appPhones = apps?.devices() ?? [];
      return {
        ok: true,
        serials: [...android, ...iphones, ...linked.map((d) => d.serial), ...appPhones.map((d) => d.serial)],
        devices: [...android.map((serial) => ({ serial, platform: 'android' })), ...iphones.map((serial) => ({ serial, platform: 'ios', via: 'usb-agent' })), ...linked, ...appPhones],
      };
    }

    const known = PHONE_TOOLS.some((t) => t.name === name);
    if (!known) return fail('unknown_tool', `No phone tool named ${name}.`);

    if (name === 'phone.skills' || name === 'phone.save_skill') return this.skillCall(name, a);

    const serial = this.resolveSerial(a.serial, usb, links, apps);
    if (typeof serial !== 'string') return serial;
    // iPhones with the Allternit app: the screen and the app's browser, nothing that taps.
    if (apps?.has(serial)) return apps.call(name, serial, a);
    if (name.startsWith('phone.web.') || name === 'phone.screen.request' || name.startsWith('phone.calendar.') || name.startsWith('phone.reminders.') || name.startsWith('phone.contacts.')) {
      return fail('not_supported', `${name} works on an iPhone connected with the Allternit app (serial "app:…"); calendar, reminders and contacts are not available on other phones. On this phone use phone.screenshot, phone.open_app, phone.tap and phone.type.`);
    }
    // Every lane: phone.task runs on the bot's brain in allternit-api; skills replay there too.
    if (name === 'phone.task') return this.task(serial, a, usb, links);
    if (name === 'phone.run_skill') return this.skillCall(name, { ...a, serial });
    if (usb?.has(serial)) {
      if (name === 'phone.sms.read') return fail('unsupported', 'Reading texts needs an Android phone (adb or Phone Link).');
      return withHints(name, a, await usb.call(name, serial, a));
    }
    if (links?.has(serial)) {
      // Linked phones: mark bot use so background account reads wait (phone-account-reader.ts).
      links.noteActivity?.(serial, 1);
      try {
        return withHints(name, a, await this.linkedCall(links, name, serial, a));
      } finally {
        links.noteActivity?.(serial, -1);
      }
    }

    // Anything that operates the phone screen/SIM marks the phone busy, so
    // the UI can show a takeover banner while a bot is mid-action. Reads
    // (devices/screenshot) don't.
    const acting = name !== 'phone.devices' && name !== 'phone.screenshot' && name !== 'phone.sms.read';
    if (acting) this.deps.activity?.(serial, 1);
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
          return withHints(name, a, await this.openApp(serial, a));
        case 'phone.sms.send':
          return await this.smsSend(serial, a);
        case 'phone.sms.read':
          return await this.smsRead(serial, a, (sinceMs) => this.adbInbox(serial, sinceMs));
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
    } finally {
      if (acting) this.deps.activity?.(serial, -1);
    }
  }

  /**
   * A linked phone's call. Texts, calls and inbox reads stay here so they pass
   * the same validation, approval gate, rate limits and emergency block as
   * the adb lane; only the final send goes to the helper.
   */
  private async linkedCall(links: PhoneLinkBackend, name: string, serial: string, a: Record<string, unknown>): Promise<ToolResult> {
    if (name !== 'phone.sms.send' && name !== 'phone.call.dial' && name !== 'phone.sms.read') return links.call(name, serial, a);
    if (!links.canText?.(serial)) {
      return fail('unsupported', name === 'phone.call.dial'
        ? 'Calls need an Android phone (adb or Phone Link). An iPhone can\'t place calls for a bot.'
        : 'Texts need an Android phone (adb or Phone Link). An iPhone doesn\'t let apps send or read texts.');
    }
    if (name === 'phone.sms.send') {
      return this.smsSend(serial, a, (sub, to, text) => links.sendSms ? links.sendSms(serial, to, text, sub) : Promise.resolve(fail('unsupported', 'This Phone Link build can\'t send texts.')));
    }
    if (name === 'phone.call.dial') {
      return this.callDial(serial, a, (number, sub) => links.dial ? links.dial(serial, number, sub) : Promise.resolve(fail('unsupported', 'This Phone Link build can\'t place calls.')));
    }
    return this.smsRead(serial, a, async (sinceMs, from) => {
      if (!links.readSms) return fail('unsupported', 'This Phone Link build can\'t read texts.');
      const res = await links.readSms(serial, sinceMs, from);
      return res.ok ? String((res as { raw?: unknown }).raw ?? '') : res;
    });
  }

  private resolveSerial(raw: unknown, usb: UsbPhoneBackend | null, links: PhoneLinkBackend | null, apps: AppPhoneLane | null): string | ToolResult {
    const online = [...this.deps.onlineSerials(), ...(usb?.serials() ?? []), ...(links?.serials() ?? []), ...(apps?.serials() ?? [])];
    if (raw !== undefined) {
      if (typeof raw !== 'string' || !raw) return fail('invalid_args', 'serial must be a non-empty string.');
      // A paired app iPhone takes phone.web.* while the app is open, even when its screen isn't shared.
      if (apps?.has(raw)) return raw;
      if (!online.includes(raw)) return fail('device_offline', `Phone ${raw} is not connected.`);
      return raw;
    }
    if (online.length === 1) return online[0];
    if (online.length === 0) return fail('device_offline', 'No phone is connected. Link one in Settings → Connections › Setup, or connect an Android phone or USB Agent in Settings → Computers → Phones.');
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
    // A package, a catalog id or name ("whatsapp", "Google Drive"), or a built-in alias.
    const key = typeof a.app === 'string' ? a.app.trim() : '';
    const entry = key && !PACKAGE_RE.test(key) ? catalogApp(key.toLowerCase()) ?? catalogAppByName(key.toLowerCase()) : undefined;
    const alias = resolveApp(a.app);
    const candidates = [...(entry?.android ?? []), ...(alias ? [alias] : [])];
    if (candidates.length === 0) return fail('unknown_app', `"${String(a.app)}" is not a package name or a known app name. Use the package, e.g. com.android.settings.`);
    for (const pkg of candidates) {
      // A carrier stub (versionName "stub(…)") only opens an install page: not installed.
      const info = await this.deps.adb(['shell', 'dumpsys', 'package', pkg], { serial });
      if (/versionName=stub/i.test(info.stdout)) continue;
      const res = await this.deps.adb(['shell', 'monkey', '-p', pkg, '-c', 'android.intent.category.LAUNCHER', '1'], { serial });
      if (res.code === 0 && !/No activities found|monkey aborted/i.test(res.stdout + res.stderr)) return { ok: true, package: pkg };
    }
    return fail('unknown_app', `${entry?.name ?? candidates[0]} is not installed on the phone.`);
  }

  /**
   * phone.task: the subtask loop in allternit-api (POST /phones/:serial/task)
   * on the bot's brain, for every lane. ARTEMIS is only the fallback when that
   * endpoint can't be reached, for an Android phone over adb with GEMINI_API_KEY set.
   */
  private async task(serial: string, a: Record<string, unknown>, usb: UsbPhoneBackend | null, links: PhoneLinkBackend | null): Promise<ToolResult> {
    if (typeof a.task !== 'string' || !a.task.trim() || a.task.length > 2000) {
      return fail('invalid_args', 'task must be 1–2000 characters.');
    }
    const model = a.model === undefined ? 'Flash' : a.model;
    if (model !== 'Flash' && model !== 'Pro') return fail('invalid_args', 'model must be Flash or Pro.');
    if (a.max_steps !== undefined && (!isInt(a.max_steps, 40) || a.max_steps === 0)) return fail('invalid_args', 'max_steps must be 1–40.');
    const api = this.deps.api?.() ?? null;
    let unavailable = 'allternit-api is not running';
    if (api) {
      try {
        const out = await api.task(serial, {
          goal: a.task.trim(),
          max_steps: a.max_steps as number | undefined,
          bot_id: typeof a.bot_id === 'string' ? a.bot_id : undefined,
          brain: typeof a.brain === 'string' ? a.brain : undefined,
        });
        if (out.ok === false && typeof out.error === 'string' && typeof out.message === 'string' && !('status' in out)) {
          return fail(out.error === 'not_found' ? 'not_found' : 'invalid_args', out.message);
        }
        return { ...out, ok: true, done: out.ok === true };
      } catch (error) {
        if (!(error instanceof PhoneApiUnavailable)) throw error;
        unavailable = error.message;
      }
    }
    const adbLane = !usb?.has(serial) && !links?.has(serial);
    const env = this.deps.env?.() ?? process.env;
    const artemis = adbLane && env.GEMINI_API_KEY ? this.deps.artemis() : null;
    if (!artemis) {
      return fail('artemis_unavailable', `Plain-English phone tasks run in Allternit Desktop's local API, which isn't reachable (${unavailable}). Restart Allternit Desktop and try again.`);
    }
    const result = await artemis.runTask(serial, a.task.trim(), model);
    return { ok: true, result, via: 'artemis' };
  }

  /** phone.skills / phone.save_skill / phone.run_skill → allternit-api. */
  private async skillCall(name: string, a: Record<string, unknown>): Promise<ToolResult> {
    const api = this.deps.api?.() ?? null;
    if (!api) return fail('unsupported', 'Phone skills live in Allternit Desktop\'s local API, which isn\'t running.');
    try {
      if (name === 'phone.skills') return { ...(await api.skills()), ok: true };
      if (typeof a.name !== 'string' || !a.name.trim()) return fail('invalid_args', 'name is required.');
      if (name === 'phone.save_skill') {
        if (typeof a.run_id !== 'string' || !a.run_id.trim()) return fail('invalid_args', 'run_id is required.');
        const out = await api.saveSkill(a);
        return out.ok === false ? (out as ToolResult) : { ...out, ok: true };
      }
      if (a.inputs !== undefined && (typeof a.inputs !== 'object' || a.inputs === null)) return fail('invalid_args', 'inputs must be an object of name → text.');
      const out = await api.runSkill(a.name.trim(), { serial: a.serial, inputs: a.inputs ?? {}, bot_id: a.bot_id, brain: a.brain });
      if (out.ok === false && !('status' in out)) return out as ToolResult;
      return { ...out, ok: true, succeeded: out.ok === true };
    } catch (error) {
      if (error instanceof PhoneApiUnavailable) return fail('unsupported', `Phone skills need Allternit Desktop's local API (${error.message}).`);
      throw error;
    }
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

  /** `send` replaces the adb send for a linked phone (null subscription = the phone's default SIM). */
  private async smsSend(
    serial: string,
    a: Record<string, unknown>,
    send?: (sub: number | null, to: string, text: string) => Promise<ToolResult>,
  ): Promise<ToolResult> {
    const to = normalizeNumber(a.to);
    if (!to) return fail('invalid_args', 'to must be a phone number (3–15 digits, optional leading +).');
    if (typeof a.text !== 'string' || a.text.length === 0 || a.text.length > 1000) {
      return fail('invalid_args', 'text must be 1–1000 characters.');
    }
    if (a.subscription_id !== undefined && !isInt(a.subscription_id, 1000)) {
      return fail('invalid_args', 'subscription_id must be a non-negative integer.');
    }
    if (isEmergencyNumber(to)) return fail('blocked_number', 'Emergency numbers are not allowed.');
    const sub = (a.subscription_id as number | undefined) ?? (send ? null : (await this.deps.defaultSmsSubscription?.(serial)) ?? null);
    if (sub === null && !send) {
      return fail('invalid_args', 'Could not tell which SIM to send from; pass subscription_id from phone.devices.');
    }

    const blocked = await this.gated('sms', serial, SMS_LIMITS, `Send a text to ${to}:\n\n${a.text}`, { to, text: a.text, sub: sub ?? -1 });
    if (blocked) return blocked;
    if (send) return send(sub, to, a.text);
    if (sub === null) return fail('invalid_args', 'Could not tell which SIM to send from; pass subscription_id from phone.devices.');

    const res = await this.deps.adb(buildIsmsSend(sub, to, a.text), { serial, timeoutMs: 15_000 });
    // `service call` prints a Parcel; "Result: Parcel(00000000 ..." with a leading zero word is the success shape.
    const accepted = res.code === 0 && /Result: Parcel\(\s*00000000/.test(res.stdout) && !/Exception/i.test(res.stdout);
    if (!accepted) return fail('adb_failed', 'The phone did not accept the text. (Android build may use a different SMS service layout.)');
    return { ok: true, status: 'submitted', note: 'Handed to the phone\'s SMS service; delivery is not confirmed.' };
  }

  /** `dial` replaces the adb dial for a linked phone. */
  private async callDial(
    serial: string,
    a: Record<string, unknown>,
    dial?: (number: string, sub: number | null) => Promise<ToolResult>,
  ): Promise<ToolResult> {
    const number = normalizeNumber(a.number);
    if (!number) return fail('invalid_args', 'number must be a phone number (3–15 digits, optional leading +).');
    if (a.subscription_id !== undefined && !isInt(a.subscription_id, 1000)) {
      return fail('invalid_args', 'subscription_id must be a non-negative integer.');
    }
    if (isEmergencyNumber(number)) return fail('blocked_number', 'Emergency numbers are not allowed.');

    const blocked = await this.gated('dial', serial, DIAL_LIMITS, `Call ${number} from the phone`, { number, sub: a.subscription_id ?? -1 });
    if (blocked) return blocked;
    if (dial) {
      const placed = await dial(number, (a.subscription_id as number | undefined) ?? null);
      if (!placed.ok) return placed;
      return dialPlaced(this.deps.onDialPlaced?.(serial, number) === true);
    }

    const args = ['shell', 'am', 'start', '-a', 'android.intent.action.CALL', '-d', shQuote(`tel:${number.replace('+', '%2B')}`)];
    if (a.subscription_id !== undefined) args.push('--ei', 'android.telecom.extra.PHONE_ACCOUNT_SUBSCRIPTION_ID', String(a.subscription_id));
    const res = await this.deps.adb(args, { serial });
    if (res.code !== 0 || /Error|Exception|Permission Denial/i.test(res.stdout + res.stderr)) {
      return fail('adb_failed', (res.stderr || res.stdout).trim().slice(0, 300) || 'The phone refused to place the call.');
    }
    return dialPlaced(this.deps.onDialPlaced?.(serial, number) === true);
  }

  /** The inbox over adb: the shell user can query the SMS provider. */
  private async adbInbox(serial: string, sinceMs: number): Promise<string | ToolResult> {
    const res = await this.deps.adb(
      ['shell', 'content', 'query', '--uri', 'content://sms/inbox', '--projection', 'address:body:date', '--where', shQuote(`date>${sinceMs}`), '--sort', shQuote('date DESC')],
      { serial, timeoutMs: 15_000 },
    );
    if (res.code !== 0 || /Exception|Permission Denial/i.test(res.stderr) || /^Error/m.test(res.stdout)) {
      return fail('adb_failed', (res.stderr || res.stdout).trim().slice(0, 300) || 'Could not read the phone\'s texts.');
    }
    return res.stdout;
  }

  /**
   * Reading texts is approved once per bot and phone (remembered in the rate
   * store). A bot without an id, and any Platform API key, asks every time.
   */
  private async readApproval(serial: string, a: Record<string, unknown>): Promise<ToolResult | null> {
    if (a.bot_id !== undefined && (typeof a.bot_id !== 'string' || a.bot_id.length > 200)) return fail('invalid_args', 'bot_id must be a string.');
    const bot = typeof a.bot_id === 'string' && a.bot_id.trim() ? a.bot_id.trim() : null;
    const source = this.sources.get(a) ?? null;
    const remember = bot && source !== 'api' ? `sms_read_allowed:${serial}:${bot}` : null;
    if (remember && this.deps.rates.load(remember).length > 0) return null;
    const who = source === 'api' ? 'An API key' : bot ? `Bot ${bot}` : 'A bot';
    const summary = `${who} wants to read texts received on ${serial}${a.from !== undefined ? ` from ${String(a.from)}` : ''} (last ${a.since_s ?? 300} s, at most 20).${remember ? ' You will not be asked again for this bot on this phone.' : ''}`;
    const decision = await this.deps.gate.request({ kind: 'sms_read', serial, summary, actionHash: canonicalHash({ kind: 'sms_read', serial, bot, source }) });
    if (decision === 'denied') return fail('approval_denied', 'The user declined.');
    if (decision === 'timeout') return fail('approval_timeout', 'The user did not respond in time.');
    if (remember) this.deps.rates.save(remember, [this.now()]);
    return null;
  }

  /**
   * phone.sms.read on any Android lane: `readInbox` returns the raw inbox rows
   * since a time. Polls every 3 s until a matching text is there or wait_s
   * runs out.
   */
  private async smsRead(
    serial: string,
    a: Record<string, unknown>,
    readInbox: (sinceMs: number, from: string | null) => Promise<string | ToolResult>,
  ): Promise<ToolResult> {
    let from: string | null = null;
    if (a.from !== undefined) {
      from = normalizeNumber(a.from);
      if (!from) return fail('invalid_args', 'from must be a phone number (3–15 digits, optional leading +).');
    }
    const sinceS = a.since_s === undefined ? 300 : a.since_s;
    if (!Number.isInteger(sinceS) || (sinceS as number) < 1 || (sinceS as number) > 86_400) return fail('invalid_args', 'since_s must be 1–86400.');
    const waitS = a.wait_s === undefined ? 0 : a.wait_s;
    if (!Number.isInteger(waitS) || (waitS as number) < 0 || (waitS as number) > 120) return fail('invalid_args', 'wait_s must be 0–120.');
    const denied = await this.readApproval(serial, a);
    if (denied) return denied;
    const sleep = this.deps.sleep ?? ((ms: number) => new Promise<void>((r) => setTimeout(r, ms)));
    const start = this.now();
    const sinceMs = start - (sinceS as number) * 1000;
    const deadline = start + (waitS as number) * 1000;
    const want = from ? digitsOf(from).slice(-10) : null;
    // 3 s polls; the cap only guards against a clock that never moves.
    for (let polls = 0; ; polls++) {
      const raw = await readInbox(sinceMs, from);
      if (typeof raw !== 'string') return raw;
      const rows = parseSmsRows(raw).filter((r) => !want || digitsOf(r.from).endsWith(want) || r.from.toLowerCase() === from!.toLowerCase());
      if (rows.length > 0 || this.now() >= deadline || polls >= 45) {
        const messages = rows.slice(0, 20).map((r) => ({ from: r.from, body: r.body, date: new Date(r.date).toISOString(), codes: extractCodes(r.body) }));
        return {
          ok: true,
          messages,
          codes: [...new Set(messages.flatMap((m) => m.codes))],
          waited_s: Math.round((this.now() - start) / 1000),
          ...(messages.length === 0 ? { note: `No text arrived${from ? ` from ${from}` : ''} in the last ${sinceS} s${waitS ? ` (waited ${waitS} s)` : ''}.` } : {}),
        };
      }
      await sleep(Math.min(3000, Math.max(0, deadline - this.now())));
    }
  }
}

function dialPlaced(bridged: boolean): ToolResult {
  return {
    ok: true,
    status: 'dialing',
    bridged,
    note: bridged
      ? 'Call placed. You join the call through Call audio as soon as it connects.'
      : 'Call placed. Call audio is not set up for this phone, so you cannot speak or listen on this call.',
  };
}

/** phone.open_app on every lane: add the app's navigation hints (phone-app-catalog.json) to a successful open. */
function withHints(name: string, a: Record<string, unknown>, res: ToolResult): ToolResult {
  if (name !== 'phone.open_app' || !res.ok || res.hints) return res;
  const opened = [res.package, res.app, res.bundle_id, res.bundleId].find((v): v is string => typeof v === 'string');
  const hints = appHints(opened, typeof a.app === 'string' ? a.app : undefined);
  return hints ? { ...res, hints } : res;
}
