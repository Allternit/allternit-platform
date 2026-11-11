/**
 * Test kit for the native board: the unchanged USB Agent firmware, built as a
 * Mac program (hardware/usb-agent/native), driven over its control lines and
 * its pty, with the board's events recorded. The board process is the only
 * thing faked; every other piece the tests use is the real Desktop code.
 *
 * Control and event contract: hardware/usb-agent/native/README.md.
 *
 * - `buildNative()` builds the binaries once per test run into a temporary
 *   folder. A failed build throws with the compiler output; nothing skips.
 * - `NativeBoard.start()` runs one binary. Every started board is recorded so
 *   `NativeBoard.stopAll()` (called from afterEach) stops them all, even when
 *   a test failed half-way.
 * - `RawDesk` is a hand-written desk for tests where the desk must be silent
 *   or hostile. It is built from the functions in phone-usb-agent-protocol.ts.
 * - `feedPhone()` turns the board's HID reports into SimPhone input.
 */

import { EventEmitter } from 'node:events';
import { spawn, type ChildProcessWithoutNullStreams } from 'node:child_process';
import * as fs from 'node:fs';
import * as net from 'node:net';
import * as os from 'node:os';
import * as path from 'node:path';
import { fileURLToPath } from 'node:url';
import { provision, serialPort, type ProvisionInput } from './phone-usb-agent-provision.js';
import {
  boardProof,
  commandLine,
  newNonceHex,
  parseAuth,
  parseHello,
  parseReply,
  parseState,
  serverProof,
  sessionKey,
  textKey,
  type BoardState,
  type Hello,
  type Reply,
} from './phone-usb-agent-protocol.js';
import type { SimPhone } from './phone-usb-agent.testkit.js';
import { until } from './phone-usb-agent.testkit.js';

export type Variant = 'link-board' | 'link-board-dock';

const HERE = path.dirname(fileURLToPath(import.meta.url));
const REPO_ROOT = path.resolve(HERE, '../../../..');
const NATIVE_DIR = path.join(REPO_ROOT, 'hardware/usb-agent/native');

// ── build ───────────────────────────────────────────────────────────────────

let build: Promise<string> | null = null;
let buildDir: string | null = null;

/** Builds the board variants once per run. Rejects with the compiler output when the build fails. */
export function buildNative(): Promise<string> {
  if (build) return build;
  const out = fs.mkdtempSync(path.join(os.tmpdir(), 'link-native-build-'));
  buildDir = out;
  process.once('exit', () => removeBuild());
  build = new Promise<string>((resolve, reject) => {
    const child = spawn('make', ['-C', NATIVE_DIR, `OUT=${out}`, '-j3', 'link-board', 'link-board-dock'], { stdio: ['ignore', 'pipe', 'pipe'] });
    let log = '';
    child.stdout.on('data', (d: Buffer) => (log += d.toString()));
    child.stderr.on('data', (d: Buffer) => (log += d.toString()));
    child.once('error', (e) => reject(new Error(`native board build could not start make: ${e.message}`)));
    child.once('exit', (code) => {
      if (code === 0) return resolve(out);
      reject(new Error(`native board build failed (make exit ${code}). Compiler output:\n${log.slice(-6000)}`));
    });
  });
  return build;
}

export function removeBuild(): void {
  if (!buildDir) return;
  try {
    fs.rmSync(buildDir, { recursive: true, force: true });
  } catch {
    /* best effort */
  }
  buildDir = null;
  build = null;
}

// ── the board ───────────────────────────────────────────────────────────────

export interface BoardEvent {
  ev: string;
  t?: number;
  [key: string]: unknown;
}

export interface HidReport {
  dev: 'mouse' | 'keyboard';
  t: number;
  sent: number;
  buttons?: number;
  x?: number;
  y?: number;
  wheel?: number;
  pan?: number;
  call?: string;
  ch?: number;
}

export interface StartOptions {
  variant?: Variant;
  /** 12 hex characters; the board id is `usba-<mac>`. Default `aabbccddeeff`. */
  mac?: string;
  env?: Record<string, string>;
}

const started = new Set<NativeBoard>();
const toHex = (s: string) => Buffer.from(s, 'utf8').toString('hex');

export class NativeBoard extends EventEmitter {
  readonly events: BoardEvent[] = [];
  stderr = '';
  exitCode: number | null = null;
  private readonly child: ChildProcessWithoutNullStreams;
  private buf = '';
  private stopped = false;
  private readonly exited: Promise<void>;

  private constructor(bin: string, env: NodeJS.ProcessEnv, readonly mac: string, readonly variant: Variant) {
    super();
    this.child = spawn(bin, [], { env, stdio: ['pipe', 'pipe', 'pipe'] });
    this.child.stdin.on('error', () => undefined);
    this.child.stdout.on('data', (d: Buffer) => this.onData(d));
    this.child.stderr.on('data', (d: Buffer) => (this.stderr += d.toString()));
    this.exited = new Promise((resolve) => {
      this.child.once('exit', (code) => {
        this.exitCode = code ?? -1;
        resolve();
      });
    });
  }

  static async start(opts: StartOptions = {}): Promise<NativeBoard> {
    const dir = await buildNative();
    const variant = opts.variant ?? 'link-board';
    const mac = opts.mac ?? 'aabbccddeeff';
    const env = { ...process.env, LINK_NATIVE_MAC: mac, ...(opts.env ?? {}) };
    const board = new NativeBoard(path.join(dir, variant), env, mac, variant);
    started.add(board);
    try {
      await until(() => board.boots().length > 0 || board.exitCode !== null, 10_000);
    } catch {
      throw new Error(`the native board printed no boot event. stderr: ${board.stderr}`);
    }
    if (board.exitCode !== null) throw new Error(`the native board exited at start (${board.exitCode}). stderr: ${board.stderr}`);
    return board;
  }

  /** Stops every board a test started. Call from afterEach. */
  static async stopAll(): Promise<void> {
    await Promise.all([...started].map((b) => b.stop()));
  }

  get id(): string {
    return `usba-${this.mac}`;
  }

  get ptyPath(): string {
    const boot = this.boots().at(-1);
    if (!boot) throw new Error('the board has not booted');
    return String(boot.pty);
  }

  private onData(chunk: Buffer): void {
    this.buf += chunk.toString('utf8');
    let i: number;
    while ((i = this.buf.indexOf('\n')) >= 0) {
      const line = this.buf.slice(0, i);
      this.buf = this.buf.slice(i + 1);
      if (!line.trim()) continue;
      let ev: BoardEvent;
      try {
        ev = JSON.parse(line) as BoardEvent;
      } catch {
        ev = { ev: 'unparsed', line };
      }
      this.events.push(ev);
      this.emit('event', ev);
    }
  }

  // ── control lines ─────────────────────────────────────────────────────────

  control(line: string): void {
    if (this.exitCode !== null) throw new Error(`cannot send "${line}": the board has exited (${this.exitCode}). stderr: ${this.stderr}`);
    this.child.stdin.write(`${line}\n`);
  }

  pin(n: number, v: 0 | 1): void {
    this.control(`pin ${n} ${v}`);
  }

  /** Holds the arm button (pin 0, active low) for `ms` of real time, then lets go. */
  async pressButton(ms = 300): Promise<void> {
    this.pin(0, 0);
    await new Promise((r) => setTimeout(r, ms));
    this.pin(0, 1);
  }

  setUsb(state: 'mounted' | 'asleep' | 'none'): void {
    this.control(`usb ${state}`);
  }

  /** Jumps the board's millis() forward. The watchdog keeps real time. */
  advance(ms: number): void {
    this.control(`clock +${ms}`);
  }

  wifiAir(ssid: string, password = ''): void {
    this.control(`wifi air ${toHex(ssid)} ${password ? toHex(password) : '-'}`);
  }

  mdns(ip: string, port: number, deskId: string): void {
    this.control(`mdns set ${ip} ${port} ${deskId}`);
  }

  mdnsStall(ms: number): void {
    this.control(`mdns stall ${ms}`);
  }

  /** Restarts with reason `power-on`. Resolves at the new boot event. */
  async powerCycle(): Promise<void> {
    const before = this.boots().length;
    this.control('power-cycle');
    await until(() => this.boots().length > before, 10_000);
  }

  /** Runs the real `prov` lines over the board's pty (the production serialPort), then waits for the restart. */
  async provision(input: ProvisionInput): Promise<void> {
    const before = this.boots().length;
    const port = await serialPort(this.ptyPath);
    try {
      await provision(port, input);
    } finally {
      port.close();
    }
    await until(() => this.boots().length > before, 10_000);
  }

  // ── what the board did ────────────────────────────────────────────────────

  boots(): BoardEvent[] {
    return this.events.filter((e) => e.ev === 'boot');
  }

  /** Index to pass to hid() or hidTotals() so they only see what happens after now. */
  mark(): number {
    return this.events.length;
  }

  /** Every HID report or keyboard call, in order, including the ones the phone would not get (`sent: 0`). */
  hid(from = 0): HidReport[] {
    return this.events.slice(from).filter((e) => e.ev === 'hid') as unknown as HidReport[];
  }

  /** Totals of what the phone would get (`sent: 1` only): pointer movement, button presses, typed text. */
  hidTotals(from = 0): { dx: number; dy: number; clicks: number; typed: string } {
    let dx = 0;
    let dy = 0;
    let clicks = 0;
    let typed = '';
    let down = false;
    for (const r of this.hid(from)) {
      if (r.sent !== 1) continue;
      if (r.dev === 'mouse') {
        dx += r.x ?? 0;
        dy += r.y ?? 0;
        const left = ((r.buttons ?? 0) & 1) === 1;
        if (left && !down) clicks += 1;
        down = left;
      } else if (r.call === 'write' && r.ch !== undefined) {
        typed += String.fromCharCode(r.ch);
      }
    }
    return { dx, dy, clicks, typed };
  }

  /** Panel refreshes (epd variant only; this kit builds none). */
  frames(): BoardEvent[] {
    return this.events.filter((e) => e.ev === 'frame');
  }

  /** The latest level of an output pin; the armed LED is pin 21, active low. Null before the board set it. */
  led(pin = 21): number | null {
    for (let i = this.events.length - 1; i >= 0; i -= 1) {
      const e = this.events[i];
      if (e.ev === 'pin' && e.n === pin) return Number(e.v);
    }
    return null;
  }

  wifiEvents(): BoardEvent[] {
    return this.events.filter((e) => e.ev === 'wifi');
  }

  async stop(): Promise<void> {
    if (this.stopped) return;
    this.stopped = true;
    started.delete(this);
    if (this.exitCode === null) {
      this.child.stdin.write('quit\n');
      this.child.stdin.end();
      const timer = setTimeout(() => this.child.kill('SIGKILL'), 3_000);
      await this.exited;
      clearTimeout(timer);
    }
  }
}

/** Feeds the board's HID reports to a SimPhone: moves, taps (a press then a release), typed text. */
export function feedPhone(board: NativeBoard, phone: SimPhone): () => void {
  let down = false;
  const onEvent = (e: BoardEvent) => {
    if (e.ev !== 'hid' || e.sent !== 1) return;
    const r = e as unknown as HidReport;
    if (r.dev === 'keyboard') {
      if (r.call === 'write' && r.ch !== undefined) phone.apply(`type ${Buffer.from([r.ch]).toString('hex')}`);
      return;
    }
    if (r.x || r.y) phone.apply(`move ${r.x ?? 0} ${r.y ?? 0}`);
    const left = ((r.buttons ?? 0) & 1) === 1;
    if (left && !down) phone.apply('press');
    if (!left && down) {
      phone.apply('release');
      phone.apply('click');
    }
    down = left;
  };
  board.on('event', onEvent);
  return () => board.off('event', onEvent);
}

// ── a hand-written desk, for silent or hostile peers ────────────────────────

export class RawConn {
  readonly lines: string[] = [];
  hello: Hello | null = null;
  closed = false;
  sk = '';
  ek = '';
  seq = 0;
  private sn = '';
  private buf = '';
  private readonly waiters = new Set<() => void>();

  constructor(readonly socket: net.Socket, private readonly key: string) {
    socket.setNoDelay(true);
    socket.on('data', (d) => {
      this.buf += d.toString('latin1');
      let i: number;
      while ((i = this.buf.indexOf('\n')) >= 0) {
        const line = this.buf.slice(0, i).replace(/\r$/, '');
        this.buf = this.buf.slice(i + 1);
        if (!this.hello && line.startsWith('hello ')) this.hello = parseHello(line);
        this.lines.push(line);
      }
      this.wake();
    });
    socket.on('error', () => undefined);
    socket.on('close', () => {
      this.closed = true;
      this.wake();
    });
  }

  private wake(): void {
    for (const w of [...this.waiters]) w();
  }

  private wait<T>(probe: () => T | undefined, what: string, ms: number): Promise<T> {
    return new Promise<T>((resolve, reject) => {
      const check = () => {
        const v = probe();
        if (v === undefined) return false;
        cleanup();
        resolve(v);
        return true;
      };
      const timer = setTimeout(() => {
        cleanup();
        reject(new Error(`timed out waiting for ${what}. Lines so far: ${JSON.stringify(this.lines)}`));
      }, ms);
      const cleanup = () => {
        clearTimeout(timer);
        this.waiters.delete(check);
      };
      if (!check()) this.waiters.add(check);
    });
  }

  /** Sends `challenge`. With `wrongProof` the proof is made under another key, so a real board must hang up. */
  challenge(opts: { wrongProof?: boolean } = {}): void {
    const hello = this.hello;
    if (!hello) throw new Error('no hello yet');
    this.sn = newNonceHex();
    const proofKey = opts.wrongProof ? 'ee'.repeat(32) : this.key;
    this.socket.write(`challenge ${this.sn} ${serverProof(proofKey, hello.boardId, hello.boardNonce, this.sn, hello.version)}\n`);
    this.sk = sessionKey(this.key, hello.boardNonce, this.sn, hello.version);
    this.ek = textKey(this.key, hello.boardNonce, this.sn);
  }

  /** Waits for the board's `auth` and returns whether its proof is right. */
  async auth(ms = 5_000): Promise<boolean> {
    const hello = this.hello!;
    const line = await this.wait(() => this.lines.find((l) => l.startsWith('auth ')), 'the board\'s auth line', ms);
    const proof = parseAuth(line);
    return proof === boardProof(this.key, hello.boardId, hello.boardNonce, this.sn, hello.version);
  }

  /** Challenge, check the board's proof, send `ready`. */
  async handshake(): Promise<void> {
    await this.waitHello();
    this.challenge();
    if (!(await this.auth())) throw new Error('the board sent a wrong auth proof');
    this.socket.write('ready\n');
  }

  waitHello(ms = 15_000): Promise<Hello> {
    return this.wait(() => this.hello ?? undefined, 'the board\'s hello', ms);
  }

  /** Sends one command line with a valid tag (or a flipped one) and returns the sequence number it used. */
  command(cmd: string | ((seq: number) => string), opts: { seq?: number; badTag?: boolean } = {}): number {
    const seq = opts.seq ?? ++this.seq;
    this.seq = Math.max(this.seq, seq);
    const text = typeof cmd === 'function' ? cmd(seq) : cmd;
    let line = commandLine(this.sk, seq, text);
    if (opts.badTag) line = `${line.slice(0, -1)}${line.endsWith('0') ? '1' : '0'}`;
    this.socket.write(`${line}\n`);
    return seq;
  }

  write(raw: string): void {
    this.socket.write(raw);
  }

  /** Sends `ping` every `ms` so the board's 10 s lease does not run out on a slow Mac. Stops when the socket closes. */
  keepAlive(ms = 2_000): void {
    const timer = setInterval(() => {
      if (this.closed || this.socket.destroyed) return clearInterval(timer);
      this.command('ping');
    }, ms);
    timer.unref?.();
    this.socket.once('close', () => clearInterval(timer));
  }

  /** The reply to `seq`, checked under the session key. Throws when the tag is wrong. */
  async reply(seq: number, ms = 8_000): Promise<Reply> {
    const line = await this.wait(() => this.lines.find((l) => l.startsWith(`r ${seq} `)), `a reply to seq ${seq}`, ms);
    const parsed = parseReply(line, 2, this.sk);
    if (!parsed) throw new Error(`reply to seq ${seq} did not parse or has a bad tag: ${line}`);
    return parsed;
  }

  /** Every state line so far, checked under the session key. */
  states(): Array<BoardState & { n?: number }> {
    return this.lines.filter((l) => l.startsWith('s ')).map((l) => {
      const s = parseState(l, 2, this.sk);
      if (!s) throw new Error(`state line has a bad tag or shape: ${l}`);
      return s;
    });
  }

  waitState(pred: (s: BoardState & { n?: number }) => boolean, ms = 8_000): Promise<BoardState & { n?: number }> {
    return this.wait(() => this.states().find(pred), 'a matching state line', ms);
  }

  waitClosed(ms = 5_000): Promise<void> {
    return this.wait(() => (this.closed ? true : undefined), 'the board to hang up', ms).then(() => undefined);
  }
}

export class RawDesk {
  readonly conns: RawConn[] = [];
  private cursor = 0;
  private readonly server: net.Server;
  port = 0;

  private constructor(private readonly key: string) {
    this.server = net.createServer((socket) => this.conns.push(new RawConn(socket, this.key)));
  }

  static async listen(keyHex: string, port = 0): Promise<RawDesk> {
    const desk = new RawDesk(keyHex);
    await new Promise<void>((resolve, reject) => {
      desk.server.once('error', reject);
      desk.server.listen(port, '127.0.0.1', () => resolve());
    });
    desk.port = (desk.server.address() as net.AddressInfo).port;
    openDesks.add(desk);
    return desk;
  }

  /** The next connection the board makes that has sent its hello. */
  async accept(ms = 20_000): Promise<RawConn> {
    await until(() => this.conns.length > this.cursor, ms);
    const conn = this.conns[this.cursor++];
    await conn.waitHello(ms);
    return conn;
  }

  close(): void {
    openDesks.delete(this);
    for (const c of this.conns) c.socket.destroy();
    this.server.close();
  }
}

const openDesks = new Set<RawDesk>();

/** Closes every RawDesk a test opened. Call from afterEach. */
export function closeRawDesks(): void {
  for (const d of [...openDesks]) d.close();
}

/** A TCP port with nothing listening on it, for a board whose desk address is wrong. */
export async function deadPort(): Promise<number> {
  const server = net.createServer();
  await new Promise<void>((resolve) => server.listen(0, '127.0.0.1', () => resolve()));
  const port = (server.address() as net.AddressInfo).port;
  await new Promise<void>((resolve) => server.close(() => resolve()));
  return port;
}
