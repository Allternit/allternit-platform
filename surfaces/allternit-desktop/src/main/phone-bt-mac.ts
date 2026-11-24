/**
 * Phone Use route `bt-mac` (docs/contracts/CONTRACT-phone-use-2026-11-16.md
 * §1 row 2b, §5, §6): this Mac shows up as a Bluetooth keyboard and mouse
 * called "Allternit Keyboard". The person pairs it in their own iPhone's
 * Settings > Bluetooth and turns on AssistiveTouch; the assistant then moves
 * the pointer, clicks and types on that iPhone.
 *
 *   hands: `allternit-bthid` (cmd/allternit-bthid, Swift, shipped in
 *          Resources/bin on macOS): newline-delimited JSON, protocol in
 *          cmd/allternit-bthid/Sources/BTHIDTool/Protocol.swift (keep in sync)
 *   eyes:  the iPhone's Allternit screen broadcast (phone-app-store.ts stills)
 *   aim:   the shared keyboard/mouse core (phone-hid-core.ts), through
 *          senderFromTransport, like the Android sender (`bt-android`)
 *
 * The helper process lives while the person has the keyboard turned on
 * (Settings → Computers → Phones → "Control my iPhone by Bluetooth"):
 * the iPhone stays connected only while the Mac keeps the keyboard
 * published. Turned off, it closes after a quiet minute. If it stops or is
 * killed, the next call starts it again and pairs again.
 *
 * Health is built from three things: the helper's `state` (Bluetooth power
 * and permission, refused, connected), whether the iPhone's broadcast is
 * running, and the core's pointer-seen signal (calibration found the
 * AssistiveTouch pointer). Every acting call has already crossed guardCheck
 * in phone-tools.ts before the router picks this lane.
 *
 * UNDOCUMENTED (accepted by Eoj 2026-11-17): the keyboard works only because
 * CoreBluetooth accepts the HID service under its 128-bit UUID form. If macOS
 * refuses it, the route is "Not available: macOS refused the keyboard
 * service; Apple may have changed this." macOS only; Windows and Linux use
 * `bt-android`, `bt-apple` or `usb-agent`. NOT CHECKED ON HARDWARE (P2-P4).
 */

import { spawn as nodeSpawn } from 'node:child_process';
import * as fs from 'node:fs';
import * as os from 'node:os';
import * as path from 'node:path';
import { HidCore, HidError, hidRouteHealth, senderFromTransport, type Calibration, type HidTransport, type Point } from './phone-hid-core.js';
import { needsYou, type NeedsYou, type RouteHealth } from './phone-link-health.js';
import type { Capability, LaneContext, LaneTool, PhoneLane } from './phone-route.js';
import type { ToolErrorCode, ToolResult } from './phone-tools.js';
import type { Gray } from './phone-usb-agent-vision.js';
import { BroadcastStills } from './phone-broadcast-stills.js';
import { frameOf, type VerifyFrame } from './phone-verify.js';

export const BTHID_BINARY = 'allternit-bthid';
/** What the iPhone lists in Settings > Bluetooth. */
export const BT_MAC_DEVICE_NAME = 'Allternit Keyboard';

export const BT_MAC_REFUSED = 'Not available: macOS refused the keyboard service; Apple may have changed this.';
export const BT_MAC_NOT_ON_THIS_OS = 'Bluetooth keyboard and mouse from a computer needs a Mac. On Windows and Linux, use the Allternit Android app or the USB Agent.';
export const BT_MAC_NOT_IN_BUILD = 'Bluetooth keyboard and mouse from this Mac is not in this build of Allternit Desktop.';
export const BT_MAC_OFF = 'Turn on Control my iPhone by Bluetooth in Settings → Computers → Phones, then pair your iPhone with "Allternit Keyboard".';
/** The System Settings > Privacy & Security pane macOS keeps the Bluetooth permission in. */
export const BT_MAC_PERMISSION_PANE = 'Bluetooth';
/** Opens that pane (System Settings > Privacy & Security > Bluetooth). */
export const BT_MAC_PERMISSION_URL = 'x-apple.systempreferences:com.apple.preference.security?Privacy_Bluetooth';
const RADIO_OFF = 'Turn on Bluetooth on this Mac.';
const NO_BLE = 'This Mac has no Bluetooth Low Energy.';

/** Where the binary is: an override, Resources/bin in a packaged app, or a local SwiftPM build in a checkout. */
export function btHidBinaryPath(opts: {
  platform?: NodeJS.Platform;
  packaged: boolean;
  resourcesPath?: string;
  appPath?: string;
  env?: NodeJS.ProcessEnv;
  exists?: (p: string) => boolean;
}): string | null {
  if ((opts.platform ?? process.platform) !== 'darwin') return null;
  const exists = opts.exists ?? ((p: string) => fs.existsSync(p));
  const override = (opts.env ?? process.env).ALLTERNIT_BTHID_BIN;
  if (override) return exists(override) ? override : null;
  const candidates = opts.packaged
    ? [opts.resourcesPath ? path.join(opts.resourcesPath, 'bin', BTHID_BINARY) : null]
    : [
        opts.appPath ? path.join(opts.appPath, 'resources', 'bin', BTHID_BINARY) : null,
        opts.appPath ? path.join(opts.appPath, '..', '..', 'cmd', 'allternit-bthid', '.build', 'release', BTHID_BINARY) : null,
      ];
  return candidates.find((p): p is string => !!p && exists(p)) ?? null;
}

// ── The helper's state ──────────────────────────────────────────────────────

export type BtRadio = 'not_started' | 'unknown' | 'resetting' | 'unsupported' | 'unauthorized' | 'off' | 'on';
export type BtAuthorization = 'not_determined' | 'restricted' | 'denied' | 'allowed';

export interface BtHidState {
  bluetooth: BtRadio;
  authorization: BtAuthorization;
  servicesReady: boolean;
  advertising: boolean;
  /** An iPhone is subscribed to the keyboard's input: connected and bonded. */
  connected: boolean;
  /** macOS refused the keyboard service. */
  refused: boolean;
  /** A Bluetooth peripheral is not told its host's name; null today. */
  name: string | null;
  lastError: { code: string; message: string } | null;
}

const RADIOS: readonly BtRadio[] = ['not_started', 'unknown', 'resetting', 'unsupported', 'unauthorized', 'off', 'on'];
const AUTHS: readonly BtAuthorization[] = ['not_determined', 'restricted', 'denied', 'allowed'];

/** A `state` reply or `event:"state"` line → the state. Unknown values read as not ready. */
export function readBtHidState(r: Record<string, unknown>): BtHidState {
  const err = r.last_error && typeof r.last_error === 'object' ? (r.last_error as Record<string, unknown>) : null;
  return {
    bluetooth: RADIOS.includes(r.bluetooth as BtRadio) ? (r.bluetooth as BtRadio) : 'unknown',
    authorization: AUTHS.includes(r.authorization as BtAuthorization) ? (r.authorization as BtAuthorization) : 'not_determined',
    servicesReady: r.services_ready === true,
    advertising: r.advertising === true,
    connected: r.connected === true,
    refused: r.refused === true,
    name: typeof r.name === 'string' && r.name ? r.name : null,
    lastError: err && typeof err.code === 'string' ? { code: err.code, message: typeof err.message === 'string' ? err.message : err.code } : null,
  };
}

// ── The process ─────────────────────────────────────────────────────────────

export interface BtHidProcess {
  write(line: string): void;
  onLine(handler: (line: string) => void): void;
  onExit(handler: (code: number | null) => void): void;
  /** Close stdin: the binary lets go of every key, removes the keyboard and exits. */
  close(): void;
  kill(): void;
}

export type BtHidSpawn = (bin: string) => BtHidProcess;

export const spawnBtHidProcess: BtHidSpawn = (bin) => {
  // Plain spawn: it exits by itself when stdin closes (and so when Desktop exits). Nothing secret in argv or env.
  const child = nodeSpawn(bin, [], { stdio: ['pipe', 'pipe', 'pipe'], env: { PATH: '/usr/bin:/bin', HOME: os.homedir() } });
  const lines: Array<(line: string) => void> = [];
  const exits: Array<(code: number | null) => void> = [];
  let pending = '';
  let exited = false;
  const exit = (code: number | null) => {
    if (exited) return;
    exited = true;
    exits.forEach((h) => h(code));
  };
  child.stdout.on('data', (chunk: Buffer) => {
    pending += chunk.toString('utf8');
    const parts = pending.split('\n');
    pending = parts.pop() ?? '';
    for (const line of parts) if (line.trim()) lines.forEach((h) => h(line));
  });
  child.stderr.resume();
  child.stdin.on('error', () => {});
  child.once('error', () => exit(null));
  child.once('exit', (code) => exit(code));
  return {
    write: (line) => void child.stdin.write(`${line}\n`),
    onLine: (h) => void lines.push(h),
    onExit: (h) => void exits.push(h),
    close: () => void child.stdin.end(),
    kill: () => void child.kill('SIGKILL'),
  };
};

/** A failure from the helper (its own code) or around it: no_binary, exited, timeout. */
export class BtHidError extends Error {
  constructor(
    readonly code: string,
    message: string,
  ) {
    super(message);
    this.name = 'BtHidError';
  }
}

/** Mouse units per move command: the helper splits it into ±127 reports itself (Protocol.swift maxMove). */
export const BT_MAC_MAX_MOVE = 4_000;
/** Text per `type` command (the helper takes up to 500). */
const TYPE_CHUNK = 200;

/** How long Desktop waits for each command: the helper's own limit (Protocol.swift Command.timeLimit) plus 2 s. */
export function bthidTimeoutMs(cmd: string, args: Record<string, unknown> = {}): number {
  const own = (() => {
    switch (cmd) {
      case 'version':
      case 'state':
        return 2;
      case 'pair':
      case 'reset':
        return 20;
      case 'type':
        return 5 + (typeof args.text === 'string' ? args.text.length : 0) * 0.1;
      case 'move': {
        const d = Math.max(Math.abs(Number(args.dx) || 0), Math.abs(Number(args.dy) || 0));
        return 5 + Math.ceil(d / 127) * 0.05;
      }
      default:
        return 5;
    }
  })();
  return Math.ceil(own * 1000) + 2000;
}

export interface BtHidClientDeps {
  binary: () => string | null;
  spawn?: BtHidSpawn;
  /** The person has the keyboard turned on (persisted by phone-service). */
  wanted?: boolean;
  /** With the keyboard off, close the process after this long with nothing asked (default 60 s). */
  idleMs?: number;
  timeoutMs?: (cmd: string, args: Record<string, unknown>) => number;
}

/**
 * One allternit-bthid process. Commands go one at a time in order; each has
 * a time limit, past which the process is killed and the next command starts
 * a new one. While the keyboard is wanted, a fresh process is told to pair
 * before anything else, so the iPhone can reconnect.
 */
export class BtHidClient {
  private proc: BtHidProcess | null = null;
  private seq = 0;
  private readonly pending = new Map<string, { resolve: (r: Record<string, unknown>) => void; reject: (e: Error) => void; timer: NodeJS.Timeout }>();
  private idle: NodeJS.Timeout | null = null;
  private queue: Promise<unknown> = Promise.resolve();
  private wanted: boolean;
  /** The running process has published the keyboard. */
  private published = false;
  private latest: BtHidState | null = null;
  private readonly listeners = new Set<(s: BtHidState) => void>();

  constructor(private readonly deps: BtHidClientDeps) {
    this.wanted = deps.wanted ?? false;
  }

  get running(): boolean {
    return this.proc !== null;
  }

  get isWanted(): boolean {
    return this.wanted;
  }

  /** The newest state the helper reported (reply or event), or null before it ran. */
  lastState(): BtHidState | null {
    return this.latest;
  }

  onState(listener: (s: BtHidState) => void): () => void {
    this.listeners.add(listener);
    return () => this.listeners.delete(listener);
  }

  /** One command; a failed reply rejects with its code and sentence. */
  call(cmd: string, args: Record<string, unknown> = {}): Promise<Record<string, unknown>> {
    const run = this.queue.then(() => this.run(cmd, args));
    this.queue = run.catch(() => undefined);
    return run;
  }

  /** The helper's state now (starting it, and pairing again when wanted). */
  async status(): Promise<BtHidState> {
    return readBtHidState(await this.call('state'));
  }

  /** Turn the keyboard on: publish and advertise as "Allternit Keyboard". */
  async pair(): Promise<BtHidState> {
    this.wanted = true;
    return readBtHidState(await this.call('pair'));
  }

  /** Turn the keyboard off: the iPhone disconnects, and the process ends. */
  async unpair(): Promise<void> {
    this.wanted = false;
    if (!this.proc) return;
    try {
      await this.call('unpair');
    } finally {
      this.close();
    }
  }

  /** End the process (stdin closes; it lets go of every key and exits). */
  close(): void {
    this.clearIdle();
    const proc = this.proc;
    this.proc = null;
    this.published = false;
    proc?.close();
  }

  private async run(cmd: string, args: Record<string, unknown>): Promise<Record<string, unknown>> {
    if (this.wanted && !this.published && cmd !== 'pair' && cmd !== 'unpair' && cmd !== 'version') {
      // A fresh process (or one whose pair failed): publish first, then answer. A failure here is
      // reported by the command itself (state shows it; input fails as not_connected).
      await this.send('pair', {}).catch(() => undefined);
    }
    const reply = await this.send(cmd, args);
    if (reply.ok !== true) {
      throw new BtHidError(typeof reply.error === 'string' ? reply.error : 'failed', typeof reply.message === 'string' ? reply.message : 'The Bluetooth keyboard could not do that.');
    }
    return reply;
  }

  private send(cmd: string, args: Record<string, unknown>): Promise<Record<string, unknown>> {
    this.clearIdle();
    const proc = this.ensure();
    if (!proc) return Promise.reject(new BtHidError('no_binary', BT_MAC_NOT_IN_BUILD));
    const id = String(++this.seq);
    const limit = (this.deps.timeoutMs ?? bthidTimeoutMs)(cmd, args);
    return new Promise<Record<string, unknown>>((resolve, reject) => {
      const timer = setTimeout(() => {
        this.pending.delete(id);
        // A hung process is not trusted again: kill it, the next command starts a fresh one.
        if (this.proc === proc) {
          this.proc = null;
          this.published = false;
        }
        proc.kill();
        reject(new BtHidError('timeout', `The Bluetooth keyboard did not answer ${cmd} in time.`));
        this.armIdle();
      }, limit);
      this.pending.set(id, {
        resolve: (r) => {
          if (cmd === 'pair') this.published = r.ok === true;
          if (cmd === 'unpair') this.published = false;
          resolve(r);
        },
        reject,
        timer,
      });
      proc.write(JSON.stringify({ ...args, id, cmd }));
    });
  }

  private ensure(): BtHidProcess | null {
    if (this.proc) return this.proc;
    const bin = this.deps.binary();
    if (!bin) return null;
    const proc = (this.deps.spawn ?? spawnBtHidProcess)(bin);
    proc.onLine((line) => this.onLine(line));
    proc.onExit(() => this.onExit(proc));
    this.proc = proc;
    this.published = false;
    return proc;
  }

  private onLine(line: string): void {
    let msg: Record<string, unknown>;
    try {
      msg = JSON.parse(line);
    } catch {
      return;
    }
    if (msg.event === 'state' || (typeof msg.bluetooth === 'string' && msg.ok === true)) this.setState(readBtHidState(msg));
    const id = typeof msg.id === 'string' ? msg.id : null;
    const waiter = id ? this.pending.get(id) : undefined;
    if (!waiter || !id) return;
    this.pending.delete(id);
    clearTimeout(waiter.timer);
    waiter.resolve(msg);
    this.armIdle();
  }

  private setState(s: BtHidState): void {
    this.latest = s;
    for (const l of this.listeners) l(s);
  }

  private onExit(proc: BtHidProcess): void {
    if (this.proc === proc) {
      this.proc = null;
      this.published = false;
      if (this.latest) this.setState({ ...this.latest, advertising: false, connected: false, servicesReady: false });
    }
    for (const [id, waiter] of this.pending) {
      clearTimeout(waiter.timer);
      waiter.reject(new BtHidError('exited', 'The Bluetooth keyboard helper stopped. It starts again on the next call.'));
      this.pending.delete(id);
    }
  }

  /** Only while the keyboard is off: a wanted keyboard must stay published for the iPhone to stay connected. */
  private armIdle(): void {
    if (this.wanted || this.pending.size > 0 || !this.proc) return;
    this.clearIdle();
    this.idle = setTimeout(() => this.close(), this.deps.idleMs ?? 60_000);
    this.idle.unref?.();
  }

  private clearIdle(): void {
    if (this.idle) clearTimeout(this.idle);
    this.idle = null;
  }
}

/** The contract's §6 transport over the helper. */
export function btMacTransport(client: BtHidClient): HidTransport {
  return {
    status: async () => {
      const s = await client.status();
      // A bonded iPhone shows only while connected; paired = connected (a peripheral can't list its bonds).
      return { paired: s.connected, connected: s.connected, ...(s.name ? { name: s.name } : {}) };
    },
    pointerMove: async (dx, dy) => void (await client.call('move', { dx, dy })),
    pointerClick: async (button, down = true, up = true) => void (await client.call('button', { button, down, up })),
    pointerReset: async () => void (await client.call('reset')),
    key: async (usage, modifiers, down) => void (await client.call('key', { usage, modifiers, down })),
    typeText: async (text) => {
      for (let i = 0; i < text.length; i += TYPE_CHUNK) await client.call('type', { text: text.slice(i, i + TYPE_CHUNK) });
    },
    scroll: async (lines) => {
      let left = Math.trunc(lines);
      while (left !== 0) {
        const step = Math.max(-100, Math.min(100, left));
        await client.call('scroll', { lines: step });
        left -= step;
      }
    },
  };
}

// ── Health (contract §5) and the settings row ───────────────────────────────

export interface BtMacHealthInput {
  platform: NodeJS.Platform;
  /** The person turned the keyboard on. */
  enabled: boolean;
  /** The helper's state, or null when it could not be read (`reason` says why). */
  state: BtHidState | null;
  reason?: string;
  broadcast: boolean;
  pointerSeen: boolean | null;
  locked: boolean | null;
}

/** macOS has not given Allternit the Bluetooth permission (denied, or blocked by a profile). */
function permissionMissing(s: BtHidState): boolean {
  return s.authorization === 'denied' || s.authorization === 'restricted' || s.bluetooth === 'unauthorized';
}

/** MAC_PERMISSION_MISSING naming the Bluetooth pane (contract §5). */
export function btMacPermissionNeed(): NeedsYou {
  return needsYou('MAC_PERMISSION_MISSING', { name: BT_MAC_PERMISSION_PANE });
}

/** Why the Mac's radio or permission stops the keyboard, or null when they are fine. */
function blocked(s: BtHidState): string | null {
  if (s.refused) return BT_MAC_REFUSED;
  if (permissionMissing(s)) return btMacPermissionNeed().message;
  if (s.bluetooth === 'unsupported') return NO_BLE;
  if (s.bluetooth === 'off') return RADIO_OFF;
  if (s.bluetooth !== 'on') return s.lastError?.message ?? 'Bluetooth on this Mac is still starting. Try again in a moment.';
  if (!s.advertising && !s.connected && s.lastError) return s.lastError.message;
  return null;
}

/**
 * The route's health, most blocking first: not a Mac / turned off / helper
 * unreadable / refused → unavailable (with the sentence); the Bluetooth
 * permission denied → MAC_PERMISSION_MISSING naming the Bluetooth pane;
 * radio → unavailable; then the shared Bluetooth rules (phone-hid-core.ts
 * hidRouteHealth): no iPhone connected → BT_NOT_PAIRED, locked →
 * unavailable, no broadcast → BROADCAST_STOPPED, pointer missing →
 * ASSISTIVETOUCH_OFF.
 */
export function btMacHealth(i: BtMacHealthInput): RouteHealth {
  if (i.platform !== 'darwin') return { state: 'unavailable', reason: BT_MAC_NOT_ON_THIS_OS };
  if (!i.enabled) return { state: 'unavailable', reason: BT_MAC_OFF };
  if (!i.state) return { state: 'unavailable', reason: i.reason ?? 'The Bluetooth keyboard helper did not answer.' };
  if (!i.state.refused && permissionMissing(i.state)) return { state: 'needs_you', needsYou: btMacPermissionNeed() };
  const why = blocked(i.state);
  if (why) return { state: 'unavailable', reason: why };
  const link = { paired: i.state.connected, connected: i.state.connected, ...(i.state.name ? { name: i.state.name } : {}) };
  return hidRouteHealth({ link, broadcast: i.broadcast, pointerSeen: i.pointerSeen, locked: i.locked });
}

/** The "Control my iPhone by Bluetooth" row in Settings → Computers → Phones. */
/**
 * At Desktop launch: if the person left "Control my iPhone by Bluetooth" on,
 * start the helper and publish "Allternit Keyboard" now, so the iPhone can
 * reconnect before the first call (it reconnects to a bonded keyboard only
 * while that keyboard advertises). Mac only; never throws. Turning it on the
 * first time is still the person's Pair click, which is when macOS asks for
 * Bluetooth.
 */
export async function resumeBtMacAtLaunch(i: {
  platform: NodeJS.Platform;
  enabled: boolean;
  client: () => Pick<BtHidClient, 'pair'> | null;
  log?: (message: string) => void;
}): Promise<'started' | 'off' | 'unavailable' | 'failed'> {
  if (i.platform !== 'darwin') return 'unavailable';
  if (!i.enabled) return 'off';
  const client = i.client();
  if (!client) return 'unavailable';
  try {
    await client.pair();
    return 'started';
  } catch (error) {
    i.log?.(`Bluetooth keyboard did not start at launch: ${error instanceof Error ? error.message : String(error)}`);
    return 'failed';
  }
}

export interface BtMacSettings {
  /** This Mac and this build can do it; when false, the row shows `message` and no buttons. */
  available: boolean;
  /** The person turned it on (Pair); Disconnect turns it off. */
  enabled: boolean;
  status: 'unavailable' | 'off' | 'blocked' | 'refused' | 'waiting' | 'connected';
  connected: boolean;
  /** What the iPhone lists in Settings > Bluetooth. */
  deviceName: string;
  /** One plain sentence for the row. */
  message: string;
  /** Contract §5 code when the person must act on the Mac: MAC_PERMISSION_MISSING (Bluetooth). */
  needs_you?: 'MAC_PERMISSION_MISSING';
  /** The System Settings pane to open for `needs_you`. */
  settingsUrl?: string;
}

export function btMacSettings(i: { platform: NodeJS.Platform; hasBinary: boolean; enabled: boolean; state: BtHidState | null; error?: string }): BtMacSettings {
  const base = { deviceName: BT_MAC_DEVICE_NAME, enabled: i.enabled, connected: false };
  if (i.platform !== 'darwin') return { ...base, available: false, enabled: false, status: 'unavailable', message: BT_MAC_NOT_ON_THIS_OS };
  if (!i.hasBinary) return { ...base, available: false, enabled: false, status: 'unavailable', message: BT_MAC_NOT_IN_BUILD };
  if (!i.enabled) {
    return { ...base, available: true, status: 'off', message: `Off. Choose Pair, then tap "${BT_MAC_DEVICE_NAME}" in your iPhone's Settings > Bluetooth.` };
  }
  if (!i.state) return { ...base, available: true, status: 'blocked', message: i.error ?? 'The Bluetooth keyboard helper did not answer.' };
  if (i.state.refused) return { ...base, available: true, status: 'refused', message: BT_MAC_REFUSED };
  if (permissionMissing(i.state)) {
    return { ...base, available: true, status: 'blocked', message: btMacPermissionNeed().message, needs_you: 'MAC_PERMISSION_MISSING', settingsUrl: BT_MAC_PERMISSION_URL };
  }
  const why = blocked(i.state);
  if (why) return { ...base, available: true, status: 'blocked', message: why };
  if (i.state.connected) {
    return {
      ...base,
      available: true,
      status: 'connected',
      connected: true,
      message: 'Connected to your iPhone. Keep AssistiveTouch on (Settings > Accessibility > Touch) so the pointer shows.',
    };
  }
  return { ...base, available: true, status: 'waiting', message: `Waiting for your iPhone: open Settings > Bluetooth on it and tap "${BT_MAC_DEVICE_NAME}".` };
}

// ── The lane ────────────────────────────────────────────────────────────────

/** What the lane needs from the app-phone backend (AppPhoneBackend satisfies it). */
export interface BtMacPhones {
  refresh(): Promise<void>;
  devices(): Array<{ serial: string; platform: 'ios' | 'android'; name: string; screen: boolean }>;
  /** phone.screenshot of an app iPhone: a still of its broadcast. */
  call(name: string, serial: string, args: Record<string, unknown>): Promise<ToolResult>;
}

export interface BtMacDeps {
  platform?: NodeJS.Platform;
  /** The helper client, or null when this build has no binary (and on Windows/Linux). */
  client: () => BtHidClient | null;
  /** The person turned the keyboard on. */
  enabled: () => boolean;
  phones: () => BtMacPhones | null;
  /** JPEG → grayscale at about `width` px wide. */
  decode: (jpeg: Buffer, width: number) => Gray | null;
  /** Pointer calibration per iPhone serial. */
  load: (serial: string) => Calibration | null;
  save: (serial: string, c: Calibration) => void;
  now?: () => number;
  sleep?: (ms: number) => Promise<void>;
}

const COVERS: readonly Capability[] = ['tap', 'type', 'swipe', 'key'];
const DECODE_WIDTH = 360;
/**
 * After calibration missed the pointer, health says ASSISTIVETOUCH_OFF for
 * this long, then lets the next tap look again (the router only calls act on
 * a healthy lane, so without this the route could never recover).
 */
export const POINTER_RECHECK_MS = 30_000;

const fail = (error: ToolErrorCode, message: string): ToolResult => ({ ok: false, error, message });

interface IphoneState {
  core: HidCore;
  screen: BroadcastStills;
  pointerSeen: boolean | null;
  missedAt: number;
  size: { width: number; height: number } | null;
}

export class BtMacLane implements PhoneLane {
  readonly id = 'bt-mac' as const;
  readonly covers = COVERS;
  readonly verify = 'router' as const;
  private readonly iphones = new Map<string, IphoneState>();
  private readonly now: () => number;
  /** Bumps when the iPhone reconnects: a gesture may have been cut off (the core re-homes and lets go). */
  private gen = 0;
  private wasConnected = false;

  constructor(private readonly deps: BtMacDeps) {
    this.now = deps.now ?? Date.now;
  }

  // ── health (contract §5) ──────────────────────────────────────────────────

  async health(ctx: LaneContext): Promise<RouteHealth> {
    const platform = this.deps.platform ?? process.platform;
    if (platform !== 'darwin') return { state: 'unavailable', reason: BT_MAC_NOT_ON_THIS_OS };
    const client = this.deps.client();
    if (!client) return { state: 'unavailable', reason: BT_MAC_NOT_IN_BUILD };
    const phones = this.deps.phones();
    if (!phones) return { state: 'unavailable', reason: 'Phones are not set up yet.' };
    await phones.refresh();
    const iphones = phones.devices().filter((d) => d.platform === 'ios');
    const iphone = iphones.find((d) => d.serial === ctx.serial);
    if (!iphone) return { state: 'unavailable', reason: 'Bluetooth from this Mac drives iPhones connected with the Allternit app (its screen broadcast is how I see).' };
    if (iphones.length > 1) {
      return { state: 'unavailable', reason: 'More than one iPhone is connected with the Allternit app, and this Mac’s Bluetooth keyboard can’t tell which one it is paired with.' };
    }
    const enabled = this.deps.enabled();
    let state: BtHidState | null = null;
    let reason: string | undefined;
    if (enabled) {
      try {
        state = await client.status();
        if (state.connected && !this.wasConnected) this.gen += 1;
        this.wasConnected = state.connected;
      } catch (error) {
        reason = error instanceof Error ? error.message : String(error);
      }
    }
    const st = this.state(ctx.serial);
    if (st.pointerSeen === false && this.now() - st.missedAt >= POINTER_RECHECK_MS) st.pointerSeen = null;
    return btMacHealth({ platform, enabled, state, reason, broadcast: iphone.screen, pointerSeen: st.pointerSeen, locked: ctx.locked });
  }

  // ── acting ────────────────────────────────────────────────────────────────

  async act(tool: LaneTool, serial: string, a: Record<string, unknown>): Promise<ToolResult> {
    if (!this.deps.client()) return fail('route_unavailable', BT_MAC_NOT_IN_BUILD);
    const st = this.state(serial);
    try {
      switch (tool) {
        case 'phone.tap': {
          const p = await this.fraction(serial, a.x, a.y);
          const blocked = await this.calibrated(serial);
          if (blocked) return blocked;
          await st.core.tap(p[0], p[1]);
          return { ok: true };
        }
        case 'phone.swipe': {
          let pts: Point[];
          if (a.direction !== undefined) {
            const d = ({ up: [[0.5, 0.7], [0.5, 0.3]], down: [[0.5, 0.3], [0.5, 0.7]], left: [[0.8, 0.5], [0.2, 0.5]], right: [[0.2, 0.5], [0.8, 0.5]] } as Record<string, Point[]>)[String(a.direction)];
            if (!d) return fail('invalid_args', 'direction must be up, down, left or right.');
            pts = d;
          } else {
            pts = [await this.fraction(serial, a.x1, a.y1), await this.fraction(serial, a.x2, a.y2)];
          }
          const blocked = await this.calibrated(serial);
          if (blocked) return blocked;
          await st.core.drag(pts);
          return { ok: true };
        }
        case 'phone.type':
          if (typeof a.text !== 'string' || !a.text.length || a.text.length > 500) return fail('invalid_args', 'text must be 1–500 characters.');
          await st.core.type(a.text);
          return { ok: true };
        case 'phone.key':
          if (typeof a.combo !== 'string') return fail('invalid_args', 'combo is required, e.g. "enter" or "cmd+space".');
          await st.core.key(a.combo);
          return { ok: true };
        case 'phone.home':
          await st.core.pressHome();
          return { ok: true };
        case 'phone.back': {
          const blocked = await this.calibrated(serial);
          if (blocked) return blocked;
          await st.core.goBack();
          return { ok: true };
        }
        case 'phone.open_app':
          if (typeof a.app !== 'string' || !a.app.trim() || a.app.length > 60) return fail('invalid_args', 'app must be an app name, e.g. "Notes".');
          await st.core.openApp(a.app.trim());
          return { ok: true };
        default:
          return fail('not_supported', `${tool} does not run on the bt-mac route.`);
      }
    } catch (error) {
      return failure(error);
    }
  }

  /** One still of the iPhone for act-then-verify (and the size taps are measured against). */
  async frame(serial: string): Promise<VerifyFrame | null> {
    const shot = await this.still(serial);
    return shot ? frameOf(shot.jpeg, this.deps.decode(shot.jpeg, DECODE_WIDTH)) : null;
  }

  /** Corner reset before a retried tap (contract §4.1 step 5). */
  async rehome(serial: string): Promise<void> {
    await this.state(serial).core.reset();
  }

  // ── internals ─────────────────────────────────────────────────────────────

  private state(serial: string): IphoneState {
    const known = this.iphones.get(serial);
    if (known) return known;
    const client = (): BtHidClient => {
      const c = this.deps.client();
      if (!c) throw new BtHidError('no_binary', BT_MAC_NOT_IN_BUILD);
      return c;
    };
    // The transport looks the client up on every call, so a restarted helper is picked up.
    const transport: HidTransport = {
      status: () => btMacTransport(client()).status(),
      pointerMove: (dx, dy) => btMacTransport(client()).pointerMove(dx, dy),
      pointerClick: (b, d, u) => btMacTransport(client()).pointerClick(b, d, u),
      pointerReset: () => btMacTransport(client()).pointerReset(),
      key: (usage, mods, down) => btMacTransport(client()).key(usage, mods, down),
      typeText: (text) => btMacTransport(client()).typeText(text),
      scroll: (lines) => btMacTransport(client()).scroll!(lines),
    };
    const screen = new BroadcastStills(
      async () => {
        const s = await this.still(serial);
        return s ? { jpeg: s.jpeg, ageMs: s.ageMs } : null;
      },
      () => this.deps.phones()?.devices().find((d) => d.serial === serial)?.screen === true,
      (jpeg) => this.deps.decode(jpeg, DECODE_WIDTH),
      { now: this.deps.now, sleep: this.deps.sleep },
    );
    const core = new HidCore({
      sender: senderFromTransport('bt-mac', transport, { maxStep: BT_MAC_MAX_MOVE, linkGeneration: () => this.gen }),
      screen,
      load: () => this.deps.load(serial),
      save: (c) => this.deps.save(serial, c),
      sleep: this.deps.sleep,
      now: this.deps.now,
    });
    const st: IphoneState = { core, screen, pointerSeen: null, missedAt: 0, size: null };
    this.iphones.set(serial, st);
    return st;
  }

  private async still(serial: string): Promise<{ jpeg: Buffer; ageMs: number; width: number; height: number } | null> {
    const res = await this.deps.phones()?.call('phone.screenshot', serial, {});
    if (!res?.ok || typeof res.dataBase64 !== 'string') return null;
    const width = Number(res.width);
    const height = Number(res.height);
    if (!(width > 0 && height > 0)) return null;
    const st = this.iphones.get(serial);
    if (st) st.size = { width, height };
    return { jpeg: Buffer.from(res.dataBase64, 'base64'), ageMs: Number(res.ageMs) || 0, width, height };
  }

  /** Pixels of the latest still → screen fractions. */
  private async fraction(serial: string, x: unknown, y: unknown): Promise<Point> {
    const isCoord = (v: unknown): v is number => Number.isInteger(v) && (v as number) >= 0 && (v as number) <= 20_000;
    if (!isCoord(x) || !isCoord(y)) throw new HidError('bad_params', 'Coordinates must be non-negative integers from phone.screenshot.');
    const size = this.state(serial).size ?? (await this.still(serial)) ?? null;
    if (!size) throw new BtHidError('no_screen', needsYou('BROADCAST_STOPPED').message);
    if (x >= size.width || y >= size.height) throw new HidError('bad_params', `Coordinates are outside the ${size.width}×${size.height} screenshot.`);
    return [(x + 0.5) / size.width, (y + 0.5) / size.height];
  }

  /** Calibrates on first pointer use. Not found = the AssistiveTouch pointer is not showing. */
  private async calibrated(serial: string): Promise<ToolResult | null> {
    const st = this.state(serial);
    if (st.core.calibrated && st.pointerSeen !== false) return null;
    const ok = await st.core.calibrate();
    st.pointerSeen = ok;
    if (ok) return null;
    st.missedAt = this.now();
    const need = needsYou('ASSISTIVETOUCH_OFF');
    return { ok: false, error: 'needs_you', message: need.message, needs_you: need } as ToolResult;
  }
}

/** A thrown failure → the tool result; a missing iPhone connection is BT_NOT_PAIRED. */
function failure(error: unknown): ToolResult {
  if (error instanceof HidError) {
    const map: Record<HidError['code'], ToolErrorCode> = { bad_params: 'invalid_args', not_calibrated: 'not_calibrated', unsupported: 'unsupported' };
    return fail(map[error.code], error.message);
  }
  if (error instanceof BtHidError) {
    if (error.code === 'not_connected') {
      const need = needsYou('BT_NOT_PAIRED');
      return { ok: false, error: 'needs_you', message: need.message, needs_you: need } as ToolResult;
    }
    const map: Record<string, ToolErrorCode> = {
      bad_request: 'invalid_args',
      no_screen: 'no_screen',
      timeout: 'timeout',
      send_timeout: 'timeout',
      no_binary: 'route_unavailable',
      refused: 'route_unavailable',
      unauthorized: 'route_unavailable',
      bluetooth_off: 'route_unavailable',
      unsupported: 'route_unavailable',
      bluetooth_unknown: 'route_unavailable',
    };
    return fail(map[error.code] ?? 'phone_failed', error.message);
  }
  return fail('phone_failed', error instanceof Error ? error.message : String(error));
}

/** The `bt-mac` lane for the router (registered in phone-service.ts). */
export function btMacLane(deps: BtMacDeps): BtMacLane {
  return new BtMacLane(deps);
}
