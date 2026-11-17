/**
 * The `mirror` Phone Use route (docs/contracts/CONTRACT-phone-use-2026-11-16.md
 * §1 route 2a, §3.2, §5): Apple's iPhone Mirroring app shows the user's iPhone
 * in a window on their Mac, and the assistant sees that window and clicks and
 * types inside it, as the user would with their own mouse and keyboard.
 *
 * The work is done by `allternit-mirror` (cmd/allternit-mirror, Swift, shipped
 * in Resources/bin on macOS). One process is kept while the lane is in use and
 * closed after a quiet minute; it speaks newline-delimited JSON (protocol in
 * cmd/allternit-mirror/Sources/MirrorCore/Protocol.swift, keep both in sync).
 *
 * Health always comes from a real probe of the window, the Mac's lock state and
 * the Screen Recording and Accessibility permissions; nothing here assumes the
 * route is healthy. The router (phone-route.ts) runs act-then-verify around
 * every action with frames from `capture`, and every send has already crossed
 * the approval guard (phone-tools.ts guardCheck) before it gets here. A changed
 * screen only says the input landed, never that a message was sent.
 *
 * macOS only. Windows and Linux report the route unavailable. NOT CHECKED ON
 * HARDWARE (plan check P1): Mirroring's response to synthetic events, its
 * Cmd+1/2/3 shortcuts and its "iPhone in Use" wording are unverified.
 */

import { spawn as nodeSpawn } from 'node:child_process';
import * as fs from 'node:fs';
import * as os from 'node:os';
import * as path from 'node:path';
import { isNeedsYouCode, needsYou, type NeedsYou, type RouteHealth } from './phone-link-health.js';
import { isIosLink, type Capability, type ImageReader, type LaneContext, type LaneTool, type PhoneLane } from './phone-route.js';
import type { AppPhoneLane, PhoneLinkBackend, ToolErrorCode, ToolResult, UsbPhoneBackend } from './phone-tools.js';
import { frameOf, type VerifyFrame } from './phone-verify.js';

export const MIRROR_BINARY = 'allternit-mirror';

/** Where the binary is: an override, Resources/bin in a packaged app, or a local SwiftPM build in a checkout. */
export function mirrorBinaryPath(opts: {
  platform?: NodeJS.Platform;
  packaged: boolean;
  resourcesPath?: string;
  appPath?: string;
  env?: NodeJS.ProcessEnv;
  exists?: (p: string) => boolean;
}): string | null {
  if ((opts.platform ?? process.platform) !== 'darwin') return null;
  const exists = opts.exists ?? ((p: string) => fs.existsSync(p));
  const override = (opts.env ?? process.env).ALLTERNIT_MIRROR_BIN;
  if (override) return exists(override) ? override : null;
  const candidates = opts.packaged
    ? [opts.resourcesPath ? path.join(opts.resourcesPath, 'bin', MIRROR_BINARY) : null]
    : [
        opts.appPath ? path.join(opts.appPath, 'resources', 'bin', MIRROR_BINARY) : null,
        opts.appPath ? path.join(opts.appPath, '..', '..', 'cmd', 'allternit-mirror', '.build', 'release', MIRROR_BINARY) : null,
      ];
  return candidates.find((p): p is string => !!p && exists(p)) ?? null;
}

// ── The process ─────────────────────────────────────────────────────────────

export interface MirrorProcess {
  write(line: string): void;
  onLine(handler: (line: string) => void): void;
  onExit(handler: (code: number | null) => void): void;
  /** Close stdin: the binary finishes its command and exits. */
  close(): void;
  kill(): void;
}

export type MirrorSpawn = (bin: string) => MirrorProcess;

export const spawnMirrorProcess: MirrorSpawn = (bin) => {
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

/** The binary could not answer: missing, crashed, or too slow. The route is then unavailable for this call. */
export class MirrorUnavailable extends Error {
  constructor(
    readonly code: 'no_binary' | 'exited' | 'timeout',
    message: string,
  ) {
    super(message);
    this.name = 'MirrorUnavailable';
  }
}

export type MirrorReply = Record<string, unknown> & { ok: boolean };

/** How long Desktop waits for each command: the binary's own limit (MirrorCore Command.timeLimit) plus 2 s. */
export function commandTimeoutMs(cmd: string, args: Record<string, unknown> = {}): number {
  const own = (() => {
    switch (cmd) {
      case 'version':
        return 2;
      case 'probe':
        return args.ocr ? 6 : 3;
      case 'capture':
        return 6;
      case 'read':
      case 'read_image':
        return 10;
      case 'type':
        return 5 + (typeof args.text === 'string' ? args.text.length : 0) * 0.03;
      case 'swipe':
        return 5 + (typeof args.duration_ms === 'number' ? args.duration_ms : 300) / 1000;
      default:
        return 5;
    }
  })();
  return Math.ceil(own * 1000) + 2000;
}

export interface MirrorClientDeps {
  binary: () => string | null;
  spawn?: MirrorSpawn;
  /** Close the process after this long with nothing asked (default 60 s). */
  idleMs?: number;
  timeoutMs?: (cmd: string, args: Record<string, unknown>) => number;
}

/**
 * One allternit-mirror process, started on first use and kept while in use.
 * Commands go one at a time in order; each has a time limit, past which the
 * process is killed and the next command starts a new one.
 */
export class MirrorClient {
  private proc: MirrorProcess | null = null;
  private seq = 0;
  private readonly pending = new Map<string, { resolve: (r: MirrorReply) => void; reject: (e: Error) => void; timer: NodeJS.Timeout }>();
  private idle: NodeJS.Timeout | null = null;
  private queue: Promise<unknown> = Promise.resolve();

  constructor(private readonly deps: MirrorClientDeps) {}

  /** True while a process is running. */
  get running(): boolean {
    return this.proc !== null;
  }

  request(cmd: string, args: Record<string, unknown> = {}): Promise<MirrorReply> {
    const run = this.queue.then(() => this.send(cmd, args));
    this.queue = run.catch(() => undefined);
    return run;
  }

  /** End the process (stdin closes; it exits by itself). */
  close(): void {
    this.clearIdle();
    const proc = this.proc;
    this.proc = null;
    proc?.close();
  }

  private send(cmd: string, args: Record<string, unknown>): Promise<MirrorReply> {
    this.clearIdle();
    const proc = this.ensure();
    if (!proc) return Promise.reject(new MirrorUnavailable('no_binary', 'iPhone Mirroring support is not in this build of Allternit Desktop.'));
    const id = String(++this.seq);
    const limit = (this.deps.timeoutMs ?? commandTimeoutMs)(cmd, args);
    return new Promise<MirrorReply>((resolve, reject) => {
      const timer = setTimeout(() => {
        this.pending.delete(id);
        // A hung process is not trusted again: kill it, the next command starts a fresh one.
        if (this.proc === proc) this.proc = null;
        proc.kill();
        reject(new MirrorUnavailable('timeout', `iPhone Mirroring did not answer ${cmd} in time.`));
        this.armIdle();
      }, limit);
      this.pending.set(id, { resolve, reject, timer });
      proc.write(JSON.stringify({ ...args, id, cmd }));
    });
  }

  private ensure(): MirrorProcess | null {
    if (this.proc) return this.proc;
    const bin = this.deps.binary();
    if (!bin) return null;
    const proc = (this.deps.spawn ?? spawnMirrorProcess)(bin);
    proc.onLine((line) => this.onLine(line));
    proc.onExit(() => this.onExit(proc));
    this.proc = proc;
    return proc;
  }

  private onLine(line: string): void {
    let msg: Record<string, unknown>;
    try {
      msg = JSON.parse(line);
    } catch {
      return;
    }
    const id = typeof msg.id === 'string' ? msg.id : null;
    const waiter = id ? this.pending.get(id) : undefined;
    if (!waiter || !id) return;
    this.pending.delete(id);
    clearTimeout(waiter.timer);
    waiter.resolve({ ...msg, ok: msg.ok === true });
    this.armIdle();
  }

  private onExit(proc: MirrorProcess): void {
    if (this.proc === proc) this.proc = null;
    for (const [id, waiter] of this.pending) {
      clearTimeout(waiter.timer);
      waiter.reject(new MirrorUnavailable('exited', 'The iPhone Mirroring helper stopped. It starts again on the next call.'));
      this.pending.delete(id);
    }
  }

  private armIdle(): void {
    if (this.pending.size > 0 || !this.proc) return;
    this.clearIdle();
    this.idle = setTimeout(() => this.close(), this.deps.idleMs ?? 60_000);
    this.idle.unref?.();
  }

  private clearIdle(): void {
    if (this.idle) clearTimeout(this.idle);
    this.idle = null;
  }
}

// ── Health ──────────────────────────────────────────────────────────────────

const NOT_ON_THIS_OS = 'iPhone Mirroring works only on a Mac.';

/**
 * A probe reply → the route's health (contract §5). Only the probe's own
 * "healthy" makes the route healthy; anything unknown is unavailable.
 *
 * A missing Mac permission is MAC_PERMISSION_MISSING naming the System
 * Settings pane. Like every needs-you state the router moves on to the next
 * route and shows it only when no route is ready (contract §1.1 step 4).
 */
export function probeHealth(probe: Record<string, unknown> | null, ctx: Pick<LaneContext, 'locked'>): RouteHealth {
  if (!probe || probe.ok !== true) {
    const why = probe && typeof probe.message === 'string' ? ` ${probe.message}` : '';
    return { state: 'unavailable', reason: `iPhone Mirroring could not be checked.${why}` };
  }
  const pane = (() => {
    const n = probe.needs_you;
    return n && typeof n === 'object' && typeof (n as Record<string, unknown>).pane === 'string' ? String((n as Record<string, unknown>).pane) : null;
  })();
  switch (probe.health) {
    case 'healthy':
      // The phone is known to be unlocked: Mirroring will show "in use" however the window looks.
      return ctx.locked === false ? { state: 'needs_you', needsYou: needsYou('MIRROR_PHONE_IN_USE') } : { state: 'healthy' };
    case 'MAC_LOCKED':
    case 'MIRROR_WINDOW_HIDDEN':
    case 'MIRROR_PHONE_IN_USE':
      return { state: 'needs_you', needsYou: needsYou(probe.health) };
    case 'NEEDS_SCREEN_RECORDING':
    case 'NEEDS_ACCESSIBILITY':
      return { state: 'needs_you', needsYou: macPermissionNeed(probe.health, pane) };
    case 'MIRROR_WINDOW_AMBIGUOUS':
      return { state: 'unavailable', reason: 'More than one iPhone Mirroring window is open. Close the extra ones so I use the right phone.' };
    case 'MIRROR_NOT_INSTALLED':
      return { state: 'unavailable', reason: 'This Mac has no iPhone Mirroring. It needs macOS 15 or later and is not offered in every region.' };
    default:
      return { state: 'unavailable', reason: 'iPhone Mirroring gave no health answer.' };
  }
}

/**
 * True when `serial` is the only iPhone this Desktop knows. iPhone Mirroring
 * shows one phone and does not say which, so with two iPhones the mirrored
 * one is not certain and the route stays out of it.
 */
export function onlyIphone(serial: string, lanes: { usb: UsbPhoneBackend | null; links: PhoneLinkBackend | null; apps: AppPhoneLane | null }): boolean {
  const iphones = new Set<string>();
  for (const s of lanes.usb?.serials() ?? []) iphones.add(s);
  // A second iPhone or iPad in keyboard mode (bt-apple) is not a phone Mirroring could show.
  for (const d of lanes.apps?.devices() ?? []) if (d.platform !== 'android' && d.keyboard_for === undefined) iphones.add(d.serial);
  for (const s of lanes.links?.serials() ?? []) if (isIosLink(lanes.links, s)) iphones.add(s);
  return iphones.size === 1 && iphones.has(serial);
}

const PRIVACY_PANE = 'System Settings > Privacy & Security > ';
const MAC_PERMISSION_NAME: Readonly<Record<string, string>> = {
  NEEDS_SCREEN_RECORDING: 'Screen & System Audio Recording',
  NEEDS_ACCESSIBILITY: 'Accessibility',
};

/** MAC_PERMISSION_MISSING for the helper's NEEDS_SCREEN_RECORDING / NEEDS_ACCESSIBILITY, naming the pane it reported. */
export function macPermissionNeed(health: string, pane: string | null): NeedsYou {
  const name = pane?.startsWith(PRIVACY_PANE) ? pane.slice(PRIVACY_PANE.length) : MAC_PERMISSION_NAME[health] ?? 'Screen & System Audio Recording';
  return needsYou('MAC_PERMISSION_MISSING', { name });
}

// ── Results ─────────────────────────────────────────────────────────────────

const ERROR_CODES: Readonly<Record<string, ToolErrorCode>> = {
  bad_request: 'invalid_args',
  outside_window: 'invalid_args',
  window_changed: 'invalid_args',
  timeout: 'timeout',
  permission_missing: 'route_unavailable',
  not_ready: 'route_unavailable',
  no_window: 'route_unavailable',
};

/** A failed binary reply → a ToolResult; contract states become needs_you. */
export function replyFailure(reply: Record<string, unknown>): ToolResult & { needs_you?: NeedsYou } {
  const message = typeof reply.message === 'string' ? reply.message : 'iPhone Mirroring could not do that.';
  if (isNeedsYouCode(reply.health)) {
    const ny = needsYou(reply.health);
    return { ok: false, error: 'needs_you', message: ny.message, needs_you: ny };
  }
  const pane = typeof reply.pane === 'string' ? reply.pane : null;
  if (typeof reply.health === 'string' && reply.health in MAC_PERMISSION_NAME) {
    const ny = macPermissionNeed(reply.health, pane);
    return { ok: false, error: 'needs_you', message: ny.message, needs_you: ny };
  }
  const code = typeof reply.error === 'string' ? ERROR_CODES[reply.error] : undefined;
  return { ok: false, error: code ?? 'phone_failed', message: pane && !message.includes(pane) ? `${message} (${pane})` : message };
}

export interface OcrElement {
  class: string;
  label: string;
  id: string;
  bounds: [number, number, number, number];
}

function elementsOf(reply: Record<string, unknown>): OcrElement[] {
  if (!Array.isArray(reply.elements)) return [];
  return (reply.elements as Array<Record<string, unknown>>)
    .filter((e) => typeof e.label === 'string' && Array.isArray(e.bounds) && e.bounds.length === 4)
    .map((e) => ({ class: 'Text', label: String(e.label), id: '', bounds: (e.bounds as number[]).map((n) => Math.round(Number(n))) as OcrElement['bounds'] }));
}

/**
 * The helper's Vision OCR on an image (`read_image`): the iPhone broadcast's
 * fallback frame when the broadcast extension has no memory to read it
 * (phone-route.ts officialLane). Mac only; null when this build has no helper
 * or the read fails, so the caller keeps the phone's own answer.
 */
export function mirrorImageReader(client: () => MirrorClient | null): ImageReader {
  return async (imageBase64) => {
    const c = client();
    if (!c) return null;
    try {
      const reply = await c.request('read_image', { image_base64: imageBase64 });
      if (!reply.ok) return null;
      const elements = elementsOf(reply);
      return { elements, text: typeof reply.text === 'string' ? reply.text : elements.map((e) => e.label).join('\n'), width: Number(reply.width) || 0, height: Number(reply.height) || 0 };
    } catch {
      return null;
    }
  };
}

/** phone.find_and_tap on OCR text: an exact label first, then one that contains the text; `match` narrows. */
export function bestLabelMatch(elements: OcrElement[], elementId: string, match?: string): OcrElement | null {
  const want = elementId.trim().toLowerCase();
  const narrow = match?.trim().toLowerCase();
  const pool = narrow ? elements.filter((e) => e.label.toLowerCase().includes(narrow)) : elements;
  return pool.find((e) => e.label.trim().toLowerCase() === want) ?? pool.find((e) => e.label.toLowerCase().includes(want)) ?? null;
}

/** The iPhone keyboard is up (a text field has focus): its bar reads "space". */
export function keyboardShowing(lines: string[]): boolean {
  return lines.some((l) => l.trim().toLowerCase() === 'space');
}

// ── The lane ────────────────────────────────────────────────────────────────

export interface MirrorLaneDeps {
  /** The process client; null when this build has no binary. */
  client: () => MirrorClient | null;
  platform?: NodeJS.Platform;
  /** Whether this phone may be the one in the Mirroring window (default: yes). */
  serves?: (serial: string) => boolean;
  sleep?: (ms: number) => Promise<void>;
}

const COVERS: readonly Capability[] = ['screenshot', 'read_ui', 'tap', 'type', 'swipe', 'key'];

type Asked = { reply: MirrorReply; fail?: undefined } | { fail: ToolResult; reply?: undefined };

const ok = (extra: Record<string, unknown> = {}): ToolResult => ({ ok: true, ...extra });

export function mirrorLane(deps: MirrorLaneDeps): PhoneLane {
  const mac = (deps.platform ?? process.platform) === 'darwin';
  const sleep = deps.sleep ?? ((ms: number) => new Promise<void>((r) => setTimeout(r, ms)));

  /** One command: the binary's reply, or why it could not be asked. */
  const ask = async (cmd: string, args: Record<string, unknown> = {}): Promise<Asked> => {
    const client = deps.client();
    if (!client) return { fail: { ok: false, error: 'route_unavailable', message: 'iPhone Mirroring support is not in this build of Allternit Desktop.' } };
    try {
      return { reply: await client.request(cmd, args) };
    } catch (error) {
      const timedOut = error instanceof MirrorUnavailable && error.code === 'timeout';
      return { fail: { ok: false, error: timedOut ? 'timeout' : 'route_unavailable', message: error instanceof Error ? error.message : String(error) } };
    }
  };

  /** The failure in an answer, or null when the binary said ok. */
  const failureOf = (a: Asked): ToolResult | null => a.fail ?? (a.reply.ok ? null : replyFailure(a.reply));

  const input = async (cmd: string, args: Record<string, unknown> = {}): Promise<ToolResult> => failureOf(await ask(cmd, args)) ?? ok();

  const read = async (): Promise<{ elements: OcrElement[]; width: number; height: number } | ToolResult> => {
    const a = await ask('read');
    const failed = failureOf(a);
    if (failed) return failed;
    return { elements: elementsOf(a.reply!), width: Number(a.reply!.width) || 0, height: Number(a.reply!.height) || 0 };
  };
  const isFailure = (r: unknown): r is ToolResult => !!r && typeof r === 'object' && (r as { ok?: unknown }).ok === false;

  return {
    id: 'mirror',
    covers: COVERS,
    verify: 'router',

    async health(ctx) {
      if (!mac) return { state: 'unavailable', reason: NOT_ON_THIS_OS };
      if (deps.serves && !deps.serves(ctx.serial)) {
        return { state: 'unavailable', reason: 'iPhone Mirroring is used only when this computer knows one iPhone, so the mirrored phone is certain.' };
      }
      if (!deps.client()) return { state: 'unavailable', reason: 'iPhone Mirroring support is not in this build of Allternit Desktop.' };
      const a = await ask('probe', { ocr: true });
      return probeHealth(a.reply ?? { ok: false, message: a.fail?.ok === false ? a.fail.message : undefined }, ctx);
    },

    async act(tool: LaneTool, _serial: string, args: Record<string, unknown>): Promise<ToolResult> {
      if (!mac) return { ok: false, error: 'route_unavailable', message: NOT_ON_THIS_OS };
      switch (tool) {
        case 'phone.screenshot': {
          const a = await ask('capture', { png: true, gray: false });
          const failed = failureOf(a);
          if (failed) return failed;
          const r = a.reply!;
          return ok({ mimeType: 'image/png', dataBase64: r.png_base64, width: r.width, height: r.height, coordinate_space: 'pixels' });
        }
        case 'phone.read_ui': {
          const seen = await read();
          if (isFailure(seen)) return seen;
          const filter = args.filter && typeof args.filter === 'object' ? (args.filter as Record<string, unknown>) : {};
          const label = typeof filter.label === 'string' ? filter.label.toLowerCase() : '';
          const type = typeof filter.type === 'string' ? filter.type.toLowerCase() : '';
          const max = typeof args.max_elements === 'number' ? args.max_elements : 1000;
          const elements = seen.elements
            .filter((e) => (!label || e.label.toLowerCase().includes(label)) && (!type || e.class.toLowerCase() === type))
            .slice(0, max);
          const text = elements.map((e) => e.label).join('\n');
          return ok({ elements, text, width: seen.width, height: seen.height, coordinate_space: 'pixels', detail: { elements, text } });
        }
        case 'phone.find_and_tap': {
          const seen = await read();
          if (isFailure(seen)) return seen;
          const el = bestLabelMatch(seen.elements, String(args.element_id ?? ''), typeof args.match === 'string' ? args.match : undefined);
          if (!el) return { ok: false, error: 'not_found', message: `No text matching "${String(args.element_id ?? '')}" is on the screen.` };
          const x = Math.round((el.bounds[0] + el.bounds[2]) / 2);
          const y = Math.round((el.bounds[1] + el.bounds[3]) / 2);
          const tapped = await input('tap', { x, y });
          return tapped.ok ? ok({ x, y, element: el }) : tapped;
        }
        case 'phone.tap':
          return input('tap', { x: args.x, y: args.y });
        case 'phone.type':
          return input('type', { text: args.text });
        case 'phone.swipe':
          return input(
            'swipe',
            typeof args.direction === 'string'
              ? { direction: args.direction, ...(args.durationMs !== undefined ? { duration_ms: args.durationMs } : {}) }
              : { x1: args.x1, y1: args.y1, x2: args.x2, y2: args.y2, ...(args.durationMs !== undefined ? { duration_ms: args.durationMs } : {}) },
          );
        case 'phone.home':
          return input('home');
        case 'phone.back':
          return input('back');
        case 'phone.key':
          return input('key', { combo: args.combo });
        case 'phone.open_app': {
          // Spotlight by name (contract §3.2): open it, type the name, Return.
          const app = typeof args.app === 'string' ? args.app.trim() : '';
          if (!app) return { ok: false, error: 'invalid_args', message: 'app must be the app name as shown on the home screen.' };
          const steps: Array<[string, Record<string, unknown>, number]> = [
            ['spotlight', {}, 700],
            ['type', { text: app }, 600],
            ['key', { combo: 'enter' }, 0],
          ];
          for (const [cmd, a, wait] of steps) {
            const r = await input(cmd, a);
            if (!r.ok) return r;
            if (wait) await sleep(wait);
          }
          return ok();
        }
        default:
          return { ok: false, error: 'not_supported', message: `${tool} does not run on iPhone Mirroring.` };
      }
    },

    async frame(): Promise<VerifyFrame | null> {
      if (!mac) return null;
      const r = (await ask('capture', { png: false, gray: true })).reply;
      const g = r?.ok && r.gray && typeof r.gray === 'object' ? (r.gray as Record<string, unknown>) : null;
      if (!g || typeof g.base64 !== 'string') return null;
      const bytes = new Uint8Array(Buffer.from(g.base64, 'base64'));
      const width = Number(g.width);
      const height = Number(g.height);
      if (!(width > 0 && height > 0) || bytes.length !== width * height) return null;
      return frameOf(bytes, { width, height, data: bytes });
    },

    async readText(): Promise<{ text: string; fieldVisible: boolean } | null> {
      const seen = await read();
      if (isFailure(seen)) return null;
      const elements = seen.elements;
      return { text: elements.map((e) => e.label).join('\n'), fieldVisible: keyboardShowing(elements.map((e) => e.label)) };
    },
  };
}
