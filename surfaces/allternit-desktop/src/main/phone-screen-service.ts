/**
 * Keeps a linked Android's screen service running (the Phone Link app's shell
 * server, ShellServerMain — what `POST /stream` streams through).
 *
 * The app can't start that server itself without its own trusted adb key,
 * which setup never gives it. Desktop already has a trusted adb connection
 * (USB, or home Wireless debugging), so whenever a linked Android is online
 * over adb this asks the app (`screen_service` RPC) and, when the server is
 * down, runs the app's two launch lines through that connection. The server
 * lives until the phone restarts, so the phone keeps streaming over the mesh
 * when it leaves home.
 *
 * The lines come from the phone, so they are checked against their one exact
 * shape before anything runs; nothing else is ever passed to a shell.
 */
import type { AdbResult } from './phone-tools.js';

/** Re-check a phone that stays online this often. */
export const SCREEN_SERVICE_RECHECK_MS = 5 * 60_000;
/** Never more often than this per phone (connect flaps, `devices` bursts). */
export const SCREEN_SERVICE_MIN_GAP_MS = 30_000;
/** The keeper's own tick: notices adb connects the manager doesn't emit (USB). */
export const SCREEN_SERVICE_TICK_MS = 30_000;

export const SCREEN_SERVICE_KILL_LINE = "pkill -f 'phonelink[.]shell[.][S]hellServerMain'";
const START_LINE =
  /^setsid app_process -Djava\.class\.path=(\/data\/app\/[A-Za-z0-9._~=+-]+(?:\/[A-Za-z0-9._~=+-]+)*\/base\.apk) \/ com\.allternit\.phonelink\.shell\.ShellServerMain [0-9a-f]{64} (\d{1,5}) <\/dev\/null >\/data\/local\/tmp\/allternit-shell\.log 2>&1 &$/;

export interface ScreenServiceLaunch {
  kill: string;
  start: string;
}

export interface ScreenServiceStatus {
  running: boolean;
  state: string;
  lastError: string | null;
  launch: ScreenServiceLaunch | null;
}

/** The two launch lines if (and only if) they have the exact expected shape, else null. */
export function validateLaunch(launch: unknown): ScreenServiceLaunch | null {
  if (!launch || typeof launch !== 'object') return null;
  const { kill, start } = launch as Record<string, unknown>;
  if (kill !== SCREEN_SERVICE_KILL_LINE || typeof start !== 'string') return null;
  const m = START_LINE.exec(start);
  if (!m || m[1].split('/').some((part) => part === '..' || part === '.')) return null;
  const port = Number(m[2]);
  if (port < 1024 || port > 65535) return null;
  return { kill, start };
}

/** The `screen_service` result, or null when it isn't one. */
export function parseScreenServiceStatus(raw: unknown): ScreenServiceStatus | null {
  if (!raw || typeof raw !== 'object') return null;
  const r = raw as Record<string, unknown>;
  if (typeof r.running !== 'boolean') return null;
  return {
    running: r.running,
    state: typeof r.state === 'string' ? r.state : 'unknown',
    lastError: typeof r.lastError === 'string' ? r.lastError : null,
    launch: validateLaunch(r.launch),
  };
}

/** One phone online over adb. */
export interface AdbScreenPhone {
  /** adb serial: USB serial, `ip:port`, or `adb-<serialno>-<x>._adb-tls-connect._tcp`. */
  serial: string;
  /** `ro.serialno` when known (the stored id of a wireless phone, or read over adb). */
  hardwareSerial?: string | null;
  model?: string | null;
  name?: string | null;
}

/** A linked Phone Link Android (phone-link-android.json). */
export interface ScreenServiceLink {
  deviceId: string;
  /** The USB adb serial setup ran on (= ro.serialno on the phones we've seen). */
  serial: string;
  deviceName?: string;
}

/** `adb-R58N123-AbCd._adb-tls-connect._tcp` → `R58N123`. */
export function serialFromMdns(serial: string): string | null {
  const m = /^adb-(.+)-[^-]+\._adb-tls-connect\._tcp\.?$/.exec(serial);
  return m ? m[1] : null;
}

const norm = (s: string | null | undefined) => (s ?? '').toLowerCase().replace(/[\s_-]+/g, ' ').trim();

/**
 * The link record for an adb phone. By serial first (the USB serial setup
 * recorded vs the adb serial, the phone's ro.serialno, or the serial inside an
 * mDNS name); then by name/model, only when exactly one record matches.
 */
export function matchLinkForAdb(phone: AdbScreenPhone, links: readonly ScreenServiceLink[]): ScreenServiceLink | null {
  const ids = new Set([phone.serial, phone.hardwareSerial, serialFromMdns(phone.serial)].filter((v): v is string => !!v));
  const bySerial = links.find((l) => ids.has(l.serial));
  if (bySerial) return bySerial;
  const names = [norm(phone.model), norm(phone.name)].filter(Boolean);
  if (names.length === 0) return null;
  const byName = links.filter((l) => {
    const n = norm(l.deviceName);
    return !!n && names.some((x) => n === x || n.endsWith(` ${x}`));
  });
  return byName.length === 1 ? byName[0] : null;
}

export interface ScreenServiceKeeperDeps {
  /** Phones online over adb right now. */
  adbPhones: () => Promise<AdbScreenPhone[]>;
  /** Linked Androids. */
  links: () => ScreenServiceLink[];
  /** `ro.serialno` over adb (cached by the keeper per adb serial). */
  readHardwareSerial?: (serial: string) => Promise<string | null>;
  /** The helper's sealed RPC on that linked phone. */
  helperCall: (deviceId: string, method: string, params: Record<string, unknown>, timeoutMs: number) => Promise<unknown>;
  /** One `adb -s <serial> shell <line>` call (a short-lived exec, not a long-lived shell). */
  adbShell: (serial: string, line: string) => Promise<AdbResult>;
  now?: () => number;
  sleep?: (ms: number) => Promise<void>;
  log: (message: string) => void;
}

export type ScreenServiceOutcome = 'running' | 'started' | 'start_failed' | 'unreachable' | 'bad_reply' | 'no_launch';

export class ScreenServiceKeeper {
  private timer: ReturnType<typeof setInterval> | null = null;
  /** deviceId → last attempt time. */
  private lastAttempt = new Map<string, number>();
  /** deviceIds online over adb at the previous pass (a new one is checked right away). */
  private online = new Set<string>();
  private inFlight = new Set<string>();
  private hwSerials = new Map<string, string | null>();
  private passing = false;

  constructor(private readonly deps: ScreenServiceKeeperDeps) {}

  start(intervalMs = SCREEN_SERVICE_TICK_MS): void {
    if (this.timer) return;
    this.timer = setInterval(() => void this.pass(), intervalMs);
    this.timer.unref?.();
    void this.pass();
  }

  stop(): void {
    if (this.timer) clearInterval(this.timer);
    this.timer = null;
  }

  private now(): number {
    return (this.deps.now ?? Date.now)();
  }

  /** One pass: every linked Android online over adb that is due gets checked (and started). */
  async pass(): Promise<void> {
    if (this.passing) return;
    this.passing = true;
    try {
      const links = this.deps.links();
      if (links.length === 0) {
        this.online.clear();
        return;
      }
      const phones = await this.deps.adbPhones().catch(() => [] as AdbScreenPhone[]);
      const seen = new Set<string>();
      const online = new Set<string>();
      const work: Promise<unknown>[] = [];
      for (const phone of phones) {
        const hardwareSerial = phone.hardwareSerial ?? (await this.hardwareSerial(phone.serial));
        const link = matchLinkForAdb({ ...phone, hardwareSerial }, links);
        if (!link || seen.has(link.deviceId)) continue;
        seen.add(link.deviceId);
        const justConnected = !this.online.has(link.deviceId);
        const last = this.lastAttempt.get(link.deviceId);
        const since = last === undefined ? Infinity : this.now() - last;
        if (since < SCREEN_SERVICE_MIN_GAP_MS) {
          // A reconnect inside the gap stays "just connected", so the first pass after the gap checks it.
          if (!justConnected) online.add(link.deviceId);
          continue;
        }
        online.add(link.deviceId);
        if (!justConnected && since < SCREEN_SERVICE_RECHECK_MS) continue;
        work.push(this.ensure(phone.serial, link));
      }
      this.online = online;
      await Promise.all(work);
    } finally {
      this.passing = false;
    }
  }

  private async hardwareSerial(serial: string): Promise<string | null> {
    if (!this.deps.readHardwareSerial) return null;
    if (this.hwSerials.has(serial)) return this.hwSerials.get(serial) ?? null;
    const value = await this.deps.readHardwareSerial(serial).catch(() => null);
    if (value) this.hwSerials.set(serial, value);
    return value;
  }

  /** Checks one phone and starts its screen service when it's down. Logs one line. */
  async ensure(adbSerial: string, link: ScreenServiceLink): Promise<ScreenServiceOutcome> {
    if (this.inFlight.has(link.deviceId)) return 'running';
    this.inFlight.add(link.deviceId);
    this.lastAttempt.set(link.deviceId, this.now());
    const who = `${link.deviceName || 'Android'} (${link.deviceId.slice(0, 8)}, adb ${adbSerial})`;
    try {
      const first = await this.status(link.deviceId);
      if (!first) return this.done('unreachable', `${who}: Phone Link didn't answer screen_service`);
      if (first === 'bad') return this.done('bad_reply', `${who}: screen_service reply not understood`);
      if (first.running) return this.done('running', `${who}: screen service running`);
      if (!first.launch) return this.done('no_launch', `${who}: screen service ${first.state}; the app gave no valid launch lines`);
      const kill = await this.deps.adbShell(adbSerial, first.launch.kill).catch((e: unknown) => ({ code: -1, stdout: '', stderr: String(e) }));
      // pkill exits 1 when nothing matched: that's fine. Anything else is adb trouble.
      if (kill.code !== 0 && kill.code !== 1) {
        return this.done('start_failed', `${who}: screen service start failed at kill (${(kill.stderr || kill.stdout).trim().slice(0, 160)})`);
      }
      const start = await this.deps.adbShell(adbSerial, first.launch.start).catch((e: unknown) => ({ code: -1, stdout: '', stderr: String(e) }));
      if (start.code !== 0) {
        return this.done('start_failed', `${who}: screen service start failed (${(start.stderr || start.stdout).trim().slice(0, 160)})`);
      }
      const sleep = this.deps.sleep ?? ((ms: number) => new Promise<void>((r) => setTimeout(r, ms)));
      for (let i = 0; i < 4; i += 1) {
        await sleep(1_500);
        const after = await this.status(link.deviceId);
        if (after && after !== 'bad' && after.running) return this.done('started', `${who}: screen service started over adb`);
      }
      return this.done('start_failed', `${who}: screen service launched over adb but didn't come up (see /data/local/tmp/allternit-shell.log)`);
    } finally {
      this.inFlight.delete(link.deviceId);
    }
  }

  private async status(deviceId: string): Promise<ScreenServiceStatus | 'bad' | null> {
    let raw: unknown;
    try {
      raw = await this.deps.helperCall(deviceId, 'screen_service', {}, 10_000);
    } catch {
      return null;
    }
    return parseScreenServiceStatus(raw) ?? 'bad';
  }

  private done(outcome: ScreenServiceOutcome, line: string): ScreenServiceOutcome {
    this.deps.log(`[PhoneScreen] ${line}`);
    return outcome;
  }
}
