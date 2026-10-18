/**
 * User-comms core (Phones 2.0, Phase 3): the Desktop-main machinery that lets
 * the *user* text and call from their linked Android over their own SIM — no
 * approval gate (the user is the actor), looser rate limits than the bot lane,
 * and an opt-in SMS watcher for live thread updates.
 *
 * This module is transport- and glue-agnostic: the adb runner, rate store and
 * clock are injected, so everything is unit-tested with fixtures and no real
 * device. The orchestrator wires it into IPC/preload against the pinned
 * PhoneCommsBridge contract (smsList / smsMessages / smsSendUser /
 * callDialUser / callEnd / callState / onCommsEvent); the method names here
 * match that contract one-to-one.
 *
 * Everything is defensive: adb failures and parse failures become
 * `{ ok: false, message }` — this class never throws across its public API.
 *
 * ── Parsing assumptions (adb output is NOT escaped — verified formats) ──────
 * `content query --projection address:body:date:read` prints one `Row: N …`
 * line per message with values RAW: a body containing a comma, a colon, or a
 * literal newline lands in the output as-is (newlines physically break the
 * row across lines), and `date` is epoch milliseconds. We therefore split
 * fields only on the KNOWN projected keys, from the ends inward:
 *   address = first `address=…` … first `, body=` after it
 *   tail    = the LAST `, date=<digits>, read=0|1` that validates as the row
 *             suffix (a decoy `, date=…` inside a body is skipped by the
 *             backward scan)
 *   body    = everything in between (embedded commas, colons, newlines kept)
 * Malformed rows are counted and skipped; rows that exist but none parse is
 * a `{ ok: false, message: 'parse_failed' }`.
 *
 * ── callState heuristic — honest limits ─────────────────────────────────────
 * `dumpsys telephony.registry` reports mCallState per subscription (0 idle,
 * 1 ringing, 2 offhook) and `dumpsys activity activities` shows the in-call
 * UI (Google: …dialer/.incallui.InCallActivity; Samsung: …incallui…; anything
 * matching /incall/i). We map: offhook + in-call UI not yet foreground →
 * 'dialing'; offhook + in-call UI foreground → 'active'; ringing (1) →
 * 'active' (the 3-state contract has no 'ringing'; an inbound ring is a live
 * call session). LIMITS: (1) adb cannot see "connected vs dialing" audio
 * state — once the OEM in-call UI is foreground (which includes the "Calling…"
 * window on Pixel/Samsung) we report 'active', so 'dialing' is only seen in
 * the brief pre-UI window or with call screens that don't match /incall/i;
 * (2) some OEMs rename the telephony registry fields — if no mCallState line
 * parses we fall back to UI-presence (incall → 'active', else 'idle');
 * (3) the caller number is best-effort (mCallIncomingNumber, else a
 * `dat=tel:` in the activity dump, else omitted). Treat the state as a UI
 * hint, not ground truth. A ringing call also sets `ringing: true`; the call
 * bridge reads the ring directly through telephonyState().
 */

import {
  buildIsmsSend,
  checkRate,
  isEmergencyNumber,
  normalizeNumber,
  shQuote,
  type AdbCall,
  type RateStore,
} from './phone-tools.js';

// ── Result + entity shapes (aligned with the PhoneCommsBridge contract) ─────

export type CommsResult<T> = ({ ok: true } & T) | { ok: false; message: string; retryAfterSec?: number };

export interface PhoneSmsThread {
  /** Normalized address (see normalizeAddress); contact resolution is the UI's job. */
  address: string;
  /** Placeholder hook — Desktop populates it from contacts later; undefined for now. */
  displayName?: string;
  lastSnippet: string;
  lastTimestamp: number;
  unreadCount?: number;
}

export interface PhoneSmsMessage {
  /** Deterministic synthetic id (the pinned projection has no _id column):
   * direction:timestamp:hash(address+body). Stable across polls while the
   * message content is unchanged. */
  id: string;
  direction: 'in' | 'out';
  body: string;
  timestamp: number;
}

export type CommsEvent = { type: 'sms'; address: string };
export type CallStateName = 'idle' | 'dialing' | 'active';
/**
 * The carrier line as the call bridge (phone-call-bridge.ts) needs it: unlike
 * CallStateName it keeps an inbound ring apart from a live call, so the bridge
 * can wait one ring and answer. 'unknown' = no mCallState line parsed.
 */
export type TelephonyStateName = 'idle' | 'ringing' | 'offhook' | 'unknown';

export interface PhoneAppInfo {
  pkg: string;
  /** Application labels are NOT available via adb without an on-device app;
   * the UI maps known packages to labels (it already does). */
  versionName?: string;
}

// ── Limits and tunables ─────────────────────────────────────────────────────

/** User lane, per device: 1 send / 5 s, 100/day. No hourly cap (not in spec). */
export const USER_SMS_LIMITS = { minGapMs: 5_000, perHour: Number.POSITIVE_INFINITY, perDay: 100 } as const;
export const DEFAULT_POLL_INTERVAL_MS = 15_000;
export const APP_CACHE_TTL_MS = 600_000;
export const MAX_APP_CONCURRENCY = 8;

export interface PhoneUserCommsDeps {
  adb: AdbCall;
  /** Shared shape with the bot lane; user-lane sends live under their own key. */
  rates: RateStore;
  /** Online phone serials — the SMS watcher polls all of these. */
  onlineSerials: () => string[];
  now?: () => number;
  pollIntervalMs?: number;
  appCacheTtlMs?: number;
  maxAppConcurrency?: number;
  /** Optional error sink for swallowed background errors (watcher, per-app lookups). */
  log?: (message: string) => void;
}

// ── Parsing: `content query` Row format ─────────────────────────────────────

export interface RawSmsRow {
  address: string;
  body: string;
  timestamp: number;
  read: boolean;
  direction: 'in' | 'out';
}

/** Strip separators only; keeps `+` and alphanumeric shortcodes ("40404", "Uber"). */
export function normalizeAddress(raw: string): string {
  return raw.replace(/[\s().-]/g, '');
}

/**
 * Parse `content query` stdout into rows. Bodies keep their raw commas,
 * colons and embedded newlines (see the header comment for the field-splitting
 * rules). `malformed` counts Row: lines that did not match the pinned
 * projection's shape.
 */
export function parseContentQueryRows(stdout: string, direction: 'in' | 'out'): { rows: RawSmsRow[]; malformed: number } {
  const rows: RawSmsRow[] = [];
  let malformed = 0;
  // Rejoin physical lines into row blobs: a row continues until the next
  // `Row: N ` line (an embedded newline in a body produces continuation lines).
  const blobs: string[] = [];
  for (const line of stdout.split(/\r?\n/)) {
    if (!line) continue;
    if (/^Row: \d+\b/.test(line)) blobs.push(line);
    else if (blobs.length) blobs[blobs.length - 1] += '\n' + line;
    // Stray output before the first Row: line is ignored.
  }
  for (const blob of blobs) {
    const rest = blob.replace(/^Row: \d+\s*/, '');
    const bodyMarker = rest.indexOf(', body=');
    if (!rest.startsWith('address=') || bodyMarker === -1) {
      malformed++;
      continue;
    }
    const address = rest.slice('address='.length, bodyMarker);
    const afterBody = rest.slice(bodyMarker + ', body='.length);
    // Backward scan for the genuine tail: the last `, date=<ms>, read=0|1`
    // that is also the END of the row. A decoy inside a body fails the suffix
    // check and the scan moves on.
    let tailStart = -1;
    let timestamp = 0;
    let read = false;
    let searchFrom = afterBody.length;
    for (;;) {
      const idx = afterBody.lastIndexOf(', date=', searchFrom);
      if (idx === -1) break;
      const tail = afterBody.slice(idx + ', date='.length);
      const m = /^(\d{9,16}), read=([01])$/.exec(tail);
      if (m) {
        tailStart = idx;
        timestamp = Number(m[1]);
        read = m[2] === '1';
        break;
      }
      searchFrom = idx;
    }
    if (tailStart === -1 || !address) {
      malformed++;
      continue;
    }
    rows.push({ address, body: afterBody.slice(0, tailStart), timestamp, read, direction });
  }
  return { rows, malformed };
}

/** Deterministic message id (see PhoneSmsMessage.id). FNV-1a 32-bit, no deps. */
export function messageId(direction: 'in' | 'out', timestamp: number, address: string, body: string): string {
  let hash = 0x811c9dc5;
  const input = `${address}\u0000${body}`;
  for (let i = 0; i < input.length; i++) {
    hash ^= input.charCodeAt(i);
    hash = Math.imul(hash, 0x01000193) >>> 0;
  }
  return `${direction}:${timestamp}:${hash.toString(16).padStart(8, '0')}`;
}

/** Merge inbox+sent rows into threads keyed by normalized address, last activity desc. */
export function buildThreads(rows: RawSmsRow[]): PhoneSmsThread[] {
  const byAddress = new Map<string, { thread: PhoneSmsThread; lastSeen: number }>();
  for (const row of rows) {
    const key = normalizeAddress(row.address);
    if (!key) continue;
    const existing = byAddress.get(key);
    if (!existing || row.timestamp >= existing.lastSeen) {
      byAddress.set(key, {
        lastSeen: row.timestamp,
        thread: {
          address: key,
          lastSnippet: row.body,
          lastTimestamp: row.timestamp,
          unreadCount: existing?.thread.unreadCount ?? 0,
        },
      });
    }
    const entry = byAddress.get(key)!;
    if (row.direction === 'in' && !row.read) {
      entry.thread.unreadCount = (entry.thread.unreadCount ?? 0) + 1;
    }
  }
  return [...byAddress.values()]
    .map((e) => e.thread)
    .sort((a, b) => b.lastTimestamp - a.lastTimestamp || a.address.localeCompare(b.address));
}

/** One thread's messages, ascending; `afterTimestamp` (ms, exclusive) enables incremental sync. */
export function buildMessages(
  rows: RawSmsRow[],
  address: string,
  after?: number | { afterTimestamp?: number },
): PhoneSmsMessage[] {
  const afterTimestamp = typeof after === 'number' ? after : after?.afterTimestamp;
  const key = normalizeAddress(address);
  return rows
    .filter((r) => normalizeAddress(r.address) === key && (afterTimestamp === undefined || r.timestamp > afterTimestamp))
    .sort((a, b) => a.timestamp - b.timestamp)
    .map((r) => ({ id: messageId(r.direction, r.timestamp, r.address, r.body), direction: r.direction, body: r.body, timestamp: r.timestamp }));
}

// ── Parsing: packages / call state ──────────────────────────────────────────

export function parsePackageList(stdout: string): { pkgs: string[]; malformed: boolean } {
  const pkgs: string[] = [];
  for (const line of stdout.split(/\r?\n/)) {
    if (!line.trim()) continue;
    if (line.startsWith('package:')) {
      const pkg = line.slice('package:'.length).trim();
      if (pkg) pkgs.push(pkg);
    } else {
      return { pkgs, malformed: true };
    }
  }
  return { pkgs, malformed: false };
}

export function parseVersionName(dumpsysStdout: string): string | undefined {
  const m = /^\s*versionName=(\S.*)$/m.exec(dumpsysStdout);
  return m ? m[1].trim() : undefined;
}

export interface ParsedCallState {
  state: CallStateName;
  number?: string;
  /** True while an inbound call rings (state still reads 'active' for older consumers). */
  ringing?: true;
}

export interface ParsedTelephonyState {
  state: TelephonyStateName;
  number?: string;
}

const TEL_RE = /dat=tel:([^,\s)}]+)/i;
const INCOMING_NUMBER_RE = /mCallIncomingNumber=([+\d][^,\s]*)/;

/** Documented heuristic — see the file header for limits. */
export function parseCallState(activitiesStdout: string, registryStdout: string): ParsedCallState {
  const incallVisible = /incall/i.test(activitiesStdout);
  const states: number[] = [];
  for (const m of registryStdout.matchAll(/callState\s*[=:]\s*(\d)/gi)) {
    states.push(Number(m[1]));
  }
  const number =
    INCOMING_NUMBER_RE.exec(registryStdout)?.[1] ??
    (() => {
      const raw = TEL_RE.exec(activitiesStdout)?.[1];
      return raw ? raw.replace(/%2B/gi, '+') : undefined;
    })();

  const state: CallStateName = (() => {
    if (states.length) {
      const worst = Math.max(...states);
      if (worst === 0) return 'idle';
      // 1 = ringing (inbound). The 3-state contract has no 'ringing'; treat as live.
      // 2 = offhook: 'dialing' only before the in-call UI surfaces; once it is
      // foreground we cannot see "connected" from adb, so report 'active'.
      if (worst === 2) {
        const resumed = /(mResumedActivity|topResumedActivity)[^\n]*incall/i.test(activitiesStdout);
        return resumed || incallVisible ? 'active' : 'dialing';
      }
      return 'active';
    }
    // No mCallState lines parsed (some OEMs rename them): fall back to UI presence.
    return incallVisible ? 'active' : 'idle';
  })();

  const out: ParsedCallState = { state };
  if (number) out.number = number;
  if (states.length && Math.max(...states) === 1) out.ringing = true;
  return out;
}

/**
 * The registry dump alone (cheap enough to poll every second): 0 idle,
 * 1 ringing, 2 offhook, the highest state across subscriptions wins. A dual-SIM
 * phone ringing on one SIM while the other is idle reads 'ringing'.
 */
export function parseTelephonyState(registryStdout: string): ParsedTelephonyState {
  const states: number[] = [];
  for (const m of registryStdout.matchAll(/callState\s*[=:]\s*(\d)/gi)) states.push(Number(m[1]));
  const number = INCOMING_NUMBER_RE.exec(registryStdout)?.[1];
  const worst = states.length ? Math.max(...states) : -1;
  const state: TelephonyStateName = worst === 0 ? 'idle' : worst === 1 ? 'ringing' : worst === 2 ? 'offhook' : 'unknown';
  return number ? { state, number } : { state };
}

/** `dumpsys telephony.registry` filtered to the call state and caller number lines. */
const REGISTRY_CALL_GREP = "dumpsys telephony.registry | grep -iE 'callState|CallIncomingNumber'";

// ── Bounded concurrency helper ──────────────────────────────────────────────

export async function mapWithConcurrency<T, R>(items: T[], limit: number, fn: (item: T) => Promise<R>): Promise<R[]> {
  const results = new Array<R>(items.length);
  let next = 0;
  const workers = Array.from({ length: Math.max(1, Math.min(limit, items.length)) }, async () => {
    while (next < items.length) {
      const i = next++;
      results[i] = await fn(items[i]);
    }
  });
  await Promise.all(workers);
  return results;
}

// ── The class ───────────────────────────────────────────────────────────────

const SMS_PROJECTION = 'address:body:date:read';
const USER_RATE_KEY = (serial: string) => `${serial}:sms:user`;

export class PhoneUserComms {
  private readonly now: () => number;
  private readonly pollIntervalMs: number;
  private readonly appCacheTtlMs: number;
  private readonly maxAppConcurrency: number;
  private readonly listeners = new Set<(ev: CommsEvent) => void>();
  private pollTimer: ReturnType<typeof setTimeout> | null = null;
  private readonly threadStamp = new Map<string, Map<string, number>>();
  private readonly appCache = new Map<string, { at: number; apps: PhoneAppInfo[] }>();

  constructor(private readonly deps: PhoneUserCommsDeps) {
    this.now = deps.now ?? Date.now;
    this.pollIntervalMs = deps.pollIntervalMs ?? DEFAULT_POLL_INTERVAL_MS;
    this.appCacheTtlMs = deps.appCacheTtlMs ?? APP_CACHE_TTL_MS;
    this.maxAppConcurrency = deps.maxAppConcurrency ?? MAX_APP_CONCURRENCY;
  }

  // ── SMS read ──────────────────────────────────────────────────────────────

  /** Inbox + sent merged into threads. OEM-blocked reads are `read_blocked`, distinct from empty. */
  async smsList(serial: string): Promise<CommsResult<{ threads: PhoneSmsThread[] }>> {
    try {
      const [inbox, sent] = await Promise.all([
        this.querySmsBox(serial, 'content://sms/inbox', 'in'),
        this.querySmsBox(serial, 'content://sms/sent', 'out'),
      ]);
      if (!inbox.ok) return inbox;
      if (!sent.ok) return sent;
      return { ok: true, threads: buildThreads([...inbox.rows, ...sent.rows]) };
    } catch (error) {
      return this.unexpected(error);
    }
  }

  /** Ascending messages for one address; `afterTimestamp` (ms, exclusive) = incremental sync. */
  async smsMessages(
    serial: string,
    address: string,
    opts?: { afterTimestamp?: number },
  ): Promise<CommsResult<{ messages: PhoneSmsMessage[] }>> {
    try {
      if (!normalizeAddress(address)) return { ok: false, message: 'invalid_args: address is empty' };
      const [inbox, sent] = await Promise.all([
        this.querySmsBox(serial, 'content://sms/inbox', 'in'),
        this.querySmsBox(serial, 'content://sms/sent', 'out'),
      ]);
      if (!inbox.ok) return inbox;
      if (!sent.ok) return sent;
      return { ok: true, messages: buildMessages([...inbox.rows, ...sent.rows], address, opts?.afterTimestamp) };
    } catch (error) {
      return this.unexpected(error);
    }
  }

  /** `content query` on one SMS box; error text from restricted OEMs maps to `read_blocked`. */
  private async querySmsBox(
    serial: string,
    uri: string,
    direction: 'in' | 'out',
  ): Promise<{ ok: true; rows: RawSmsRow[]; malformed: number } | { ok: false; message: string }> {
    const res = await this.deps.adb(
      ['shell', 'content', 'query', '--uri', uri, '--projection', SMS_PROJECTION],
      { serial, timeoutMs: 15_000 },
    );
    const out = `${res.stdout}\n${res.stderr}`;
    // OEM/carrier builds that restrict the SMS provider throw
    // SecurityException / Permission Denial from this command; a plain
    // transport failure (device dropped) is adb_failed, not read_blocked.
    const restricted = /SecurityException|Permission Denial|not allowed|Unable to find provider|Operation not permitted/i.test(out);
    if (restricted || (res.code !== 0 && /Exception|Permission|denied|not allowed/i.test(out))) {
      return { ok: false, message: 'read_blocked' };
    }
    if (res.code !== 0) {
      return { ok: false, message: `adb_failed: ${(res.stderr || res.stdout || `adb exited ${res.code}`).trim().slice(0, 300)}` };
    }
    if (!res.stdout.trim()) return { ok: true, rows: [], malformed: 0 };
    // Non-empty output with no parseable rows = an output shape we don't
    // recognize (a genuinely empty box prints nothing at all).
    const { rows, malformed } = parseContentQueryRows(res.stdout, direction);
    if (rows.length === 0) {
      return { ok: false, message: 'parse_failed: content query returned unrecognized rows' };
    }
    return { ok: true, rows, malformed };
  }

  // ── SMS send (user lane) ──────────────────────────────────────────────────

  /**
   * Send a text from the phone's own SIM. Looser than the bot lane (no
   * approval gate — the user is the actor), still rate-limited
   * (1/5s, 100/day per device). Emergency destinations are the USER's own
   * call to make: they are sent, but flagged `{ ok: true, emergency: true }`
   * so the UI can require its own warn-confirm before invoking this.
   * Status is 'submitted' — never 'delivered'.
   */
  async smsSendUser(
    serial: string,
    to: string,
    text: string,
  ): Promise<CommsResult<{ status: 'submitted'; emergency?: true }>> {
    try {
      const dest = normalizeNumber(to);
      if (!dest) return { ok: false, message: 'invalid_args: to must be a phone number (3–15 digits, optional leading +).' };
      if (typeof text !== 'string' || text.length === 0 || text.length > 1000) {
        return { ok: false, message: 'invalid_args: text must be 1–1000 characters.' };
      }
      const emergency = isEmergencyNumber(dest);

      const rateKey = USER_RATE_KEY(serial);
      const rate = checkRate(this.deps.rates.load(rateKey), this.now(), USER_SMS_LIMITS);
      if (!rate.ok) {
        return { ok: false, message: `rate_limited: ${rate.reason}`, retryAfterSec: rate.retryAfterSec };
      }

      const subId = await this.resolveSmsSubId(serial);
      // Count the attempt before sending, mirroring the bot lane: a failed
      // send still used the quota.
      const day = this.deps.rates.load(rateKey).filter((t) => this.now() - t < 86_400_000);
      this.deps.rates.save(rateKey, [...day, this.now()]);

      const res = await this.deps.adb(buildIsmsSend(subId, dest, text), { serial, timeoutMs: 15_000 });
      const accepted = res.code === 0 && /Result: Parcel\(\s*00000000/.test(res.stdout) && !/Exception/i.test(res.stdout);
      if (!accepted) {
        return { ok: false, message: 'adb_failed: the phone did not accept the text. (Android build may use a different SMS service layout.)' };
      }
      return emergency ? { ok: true, status: 'submitted', emergency: true } : { ok: true, status: 'submitted' };
    } catch (error) {
      return this.unexpected(error);
    }
  }

  /**
   * Active SIM via the same `content://telephony/siminfo` shape phone.devices
   * uses: the slot-0 row's `_id` (falling back to the first populated slot).
   * When the query is unreadable we send with subscription id 0 — the
   * platform's DEFAULT_SUBSCRIPTION_ID, i.e. the same default-SIM behavior as
   * a send with no subscriber override.
   */
  private async resolveSmsSubId(serial: string): Promise<number> {
    const res = await this.deps.adb(
      ['shell', 'content', 'query', '--uri', 'content://telephony/siminfo', '--projection', '_id:sim_id'],
      { serial, timeoutMs: 10_000 },
    );
    if (res.code !== 0 || !res.stdout.trim()) return 0;
    let firstSlot: { slot: number; id: number } | null = null;
    for (const line of res.stdout.split(/\r?\n/)) {
      if (!line.startsWith('Row:')) continue;
      // Field keys are prefixed by the "Row: N " head or ", " separators;
      // `sim_id` must not match via the `_id` inside its own name.
      const id = /(?:^|[^a-zA-Z])_id=(\d+)/.exec(line)?.[1];
      const slot = /(?:^|[^a-zA-Z])sim_id=(-?\d+)/.exec(line)?.[1];
      if (id === undefined || slot === undefined) continue;
      const slotNum = Number(slot);
      if (slotNum < 0) continue; // sim_id -1 = no SIM in that slot
      if (slotNum === 0) return Number(id);
      firstSlot ??= { slot: slotNum, id: Number(id) };
    }
    return firstSlot?.id ?? 0;
  }

  // ── Calls (user lane) ─────────────────────────────────────────────────────

  /** Place a carrier call via the dialer. Emergency numbers require `confirmEmergency: true`
   * (the UI's warn-confirm); without it the dial is refused here in main. */
  async callDialUser(
    serial: string,
    number: string,
    opts?: { confirmEmergency?: boolean },
  ): Promise<CommsResult<{ status: 'dialing'; emergency?: true }>> {
    try {
      const dest = normalizeNumber(number);
      if (!dest) return { ok: false, message: 'invalid_args: number must be a phone number (3–15 digits, optional leading +).' };
      const emergency = isEmergencyNumber(dest);
      if (emergency && opts?.confirmEmergency !== true) {
        return { ok: false, message: 'emergency_confirm_required' };
      }
      const args = ['shell', 'am', 'start', '-a', 'android.intent.action.CALL', '-d', shQuote(`tel:${dest.replace('+', '%2B')}`)];
      const res = await this.deps.adb(args, { serial });
      if (res.code !== 0 || /Error|Exception|Permission Denial/i.test(res.stdout + res.stderr)) {
        const detail = (res.stderr || res.stdout).trim().slice(0, 300);
        return { ok: false, message: `adb_failed: ${detail || 'the phone refused to place the call.'}` };
      }
      return emergency ? { ok: true, status: 'dialing', emergency: true } : { ok: true, status: 'dialing' };
    } catch (error) {
      return this.unexpected(error);
    }
  }

  /** End the current call the way a headset button would. */
  async callEnd(serial: string): Promise<CommsResult<{}>> {
    try {
      const res = await this.deps.adb(['shell', 'input', 'keyevent', 'KEYCODE_ENDCALL'], { serial });
      if (res.code !== 0) {
        const detail = (res.stderr || res.stdout).trim().slice(0, 300);
        return { ok: false, message: `adb_failed: ${detail || `adb exited ${res.code}`}` };
      }
      return { ok: true };
    } catch (error) {
      return this.unexpected(error);
    }
  }

  /**
   * Best-effort call state — heuristic with documented limits (file header).
   * `number` is populated when the dumps happen to reveal it.
   */
  async callState(serial: string): Promise<CommsResult<{ state: CallStateName; number?: string; ringing?: true }>> {
    try {
      const [activities, registry] = await Promise.all([
        this.deps.adb(['shell', 'dumpsys activity activities | grep -i incall'], { serial, timeoutMs: 15_000 }),
        this.deps.adb(['shell', REGISTRY_CALL_GREP], { serial, timeoutMs: 15_000 }),
      ]);
      if (activities.code !== 0 && registry.code !== 0) {
        const detail = (activities.stderr || registry.stderr || 'dumpsys failed').trim().slice(0, 300);
        return { ok: false, message: `adb_failed: ${detail}` };
      }
      const parsed = parseCallState(activities.stdout, registry.stdout);
      return { ok: true, ...parsed };
    } catch (error) {
      return this.unexpected(error);
    }
  }

  /**
   * The carrier line for the call bridge: idle / ringing / offhook (+ the
   * caller number when Android reveals it). One dumpsys, so it is cheap to
   * poll every second.
   */
  async telephonyState(serial: string): Promise<CommsResult<ParsedTelephonyState>> {
    try {
      const res = await this.deps.adb(['shell', REGISTRY_CALL_GREP], { serial, timeoutMs: 5_000 });
      // grep exits 1 when nothing matched: that is an OEM rename, not a dead phone.
      if (res.code !== 0 && !(res.code === 1 && !res.stderr.trim())) {
        return { ok: false, message: `adb_failed: ${(res.stderr || res.stdout || `adb exited ${res.code}`).trim().slice(0, 300)}` };
      }
      return { ok: true, ...parseTelephonyState(res.stdout) };
    } catch (error) {
      return this.unexpected(error);
    }
  }

  /**
   * Answer a ringing call the way a headset button would: KEYCODE_CALL, which
   * the stock dialers take as "answer" while ringing.
   */
  async callAnswer(serial: string, key: 'KEYCODE_CALL' | 'KEYCODE_HEADSETHOOK' = 'KEYCODE_CALL'): Promise<CommsResult<{}>> {
    try {
      const res = await this.deps.adb(['shell', 'input', 'keyevent', key], { serial });
      if (res.code !== 0) {
        const detail = (res.stderr || res.stdout).trim().slice(0, 300);
        return { ok: false, message: `adb_failed: ${detail || `adb exited ${res.code}`}` };
      }
      return { ok: true };
    } catch (error) {
      return this.unexpected(error);
    }
  }

  // ── SMS watcher ───────────────────────────────────────────────────────────

  /**
   * Subscribe to SMS activity on any linked phone. While subscribers exist,
   * all online serials are polled every `pollIntervalMs` (default 15 s);
   * threads whose lastTimestamp advances emit `{ type: 'sms', address }`.
   * The first successful poll per phone only baselines (no emit storm).
   * Polling pauses when the last subscriber unsubscribes. Never auto-starts.
   */
  onSmsEvent(cb: (ev: CommsEvent) => void): () => void {
    this.listeners.add(cb);
    this.ensurePolling();
    return () => {
      this.listeners.delete(cb);
      if (this.listeners.size === 0) this.stopPolling();
    };
  }

  /** Alias matching the pinned bridge name; identical behavior. */
  onCommsEvent(cb: (ev: CommsEvent) => void): () => void {
    return this.onSmsEvent(cb);
  }

  get watcherActive(): boolean {
    return this.pollTimer !== null;
  }

  private ensurePolling(): void {
    if (this.pollTimer !== null || this.listeners.size === 0) return;
    this.pollTimer = setTimeout(() => {
      this.pollTimer = null;
      void this.pollTick();
    }, this.pollIntervalMs);
  }

  private stopPolling(): void {
    if (this.pollTimer !== null) {
      clearTimeout(this.pollTimer);
      this.pollTimer = null;
    }
  }

  private async pollTick(): Promise<void> {
    for (const serial of this.deps.onlineSerials()) {
      const res = await this.smsList(serial);
      if (!res.ok) {
        this.deps.log?.(`comms watcher: smsList(${serial}) failed: ${res.message}`);
        continue;
      }
      const seen = this.threadStamp.get(serial) ?? null;
      const next = new Map<string, number>();
      for (const thread of res.threads) {
        next.set(thread.address, thread.lastTimestamp);
        const prev = seen?.get(thread.address);
        if (seen !== null && (prev === undefined || thread.lastTimestamp > prev)) {
          this.emit({ type: 'sms', address: thread.address });
        }
      }
      this.threadStamp.set(serial, next);
    }
    this.ensurePolling();
  }

  private emit(ev: CommsEvent): void {
    for (const cb of this.listeners) {
      try {
        cb(ev);
      } catch (error) {
        this.deps.log?.(`comms watcher: listener threw: ${error instanceof Error ? error.message : String(error)}`);
      }
    }
  }

  // ── App inventory ─────────────────────────────────────────────────────────

  /** Third-party packages with versionName (bounded `dumpsys` fan-out, 10-min per-serial cache). */
  async listApps(serial: string): Promise<CommsResult<{ apps: PhoneAppInfo[] }>> {
    try {
      const cached = this.appCache.get(serial);
      if (cached && this.now() - cached.at < this.appCacheTtlMs) {
        return { ok: true, apps: cached.apps };
      }
      const res = await this.deps.adb(['shell', 'pm', 'list', 'packages', '-3'], { serial, timeoutMs: 20_000 });
      if (res.code !== 0) {
        const detail = (res.stderr || res.stdout).trim().slice(0, 300);
        return { ok: false, message: `adb_failed: ${detail || `adb exited ${res.code}`}` };
      }
      const { pkgs, malformed } = parsePackageList(res.stdout);
      if (malformed) return { ok: false, message: 'parse_failed: pm list packages returned unrecognized output' };
      const apps: PhoneAppInfo[] = await mapWithConcurrency(pkgs, this.maxAppConcurrency, async (pkg) => {
        try {
          const d = await this.deps.adb(['shell', 'dumpsys', 'package', pkg], { serial, timeoutMs: 10_000 });
          return { pkg, versionName: d.code === 0 ? parseVersionName(d.stdout) : undefined };
        } catch {
          return { pkg };
        }
      });
      this.appCache.set(serial, { at: this.now(), apps });
      return { ok: true, apps };
    } catch (error) {
      return this.unexpected(error);
    }
  }

  /** Drop the app inventory cache (e.g. after an app install/uninstall). */
  invalidateApps(serial?: string): void {
    if (serial === undefined) this.appCache.clear();
    else this.appCache.delete(serial);
  }

  // ── internals ─────────────────────────────────────────────────────────────

  private unexpected(error: unknown): { ok: false; message: string } {
    return { ok: false, message: `adb_failed: ${error instanceof Error ? error.message : String(error)}`.slice(0, 300) };
  }
}
