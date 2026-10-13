/**
 * Allternit USB Agent: a bot operates a real iPhone (or anything that takes a
 * USB mouse and keyboard) with no app on the phone, no Mac-only mirroring and
 * no developer account.
 *
 *   hands: the USB Agent board in the phone's USB-C port (USB mouse +
 *          keyboard; iOS AssistiveTouch shows the pointer), commanded over
 *          Wi-Fi through the authenticated link (phone-usb-agent-link.ts)
 *   eyes:  the phone's AirPlay screen mirror, received here by UxPlay
 *          (phone-airplay.ts), one PIN-protected receiver per board
 *   aim:   calibration and closed-loop aiming (phone-usb-agent-pointer.ts)
 *
 * The board starts disarmed and only a press of its button arms it, so
 * nothing on the network can start driving the phone on its own; Stop here
 * disarms it at once. Each board shows up next to the Android phones and is
 * reached through the same phone.* bot tools.
 */

import { EventEmitter } from 'node:events';
import { randomInt } from 'node:crypto';
import type { ChildProcess } from 'node:child_process';
import { AirplayScreen, AIRPLAY_BASE_PORT, AIRPLAY_NAME, type AirplayState, type UxplayOptions } from './phone-airplay.js';
import { UsbAgentLink, type BoardSessionInfo } from './phone-usb-agent-link.js';
import { USBA_DEFAULT_PORT, newKeyHex, type BoardCommand } from './phone-usb-agent-protocol.js';
import { BoardError, UsbPointer, type Calibration, type PointerScreen } from './phone-usb-agent-pointer.js';
import { changed, type Gray } from './phone-usb-agent-vision.js';
import { provision, readIdentity, validateProvision, type SerialPort } from './phone-usb-agent-provision.js';
import type { ToolErrorCode, ToolResult } from './phone-tools.js';

export const USB_SERIAL_PREFIX = 'usb:';

export interface StoredUsbAgent {
  boardId: string;
  name: string;
  /** The board key, sealed with the OS keychain (safeStorage) in production. */
  sealedKey: string;
  addedAt: number;
  /** Index into the AirPlay port range; stable so the phone's PIN trust survives restarts. */
  slot: number;
}

export interface UsbAgentConfig {
  deskId: string;
  airplayPin: string;
}

export interface UsbAgentStore {
  load(): StoredUsbAgent[];
  save(boards: StoredUsbAgent[]): void;
  loadConfig(): UsbAgentConfig | null;
  saveConfig(c: UsbAgentConfig): void;
  loadCalibration(boardId: string): Calibration | null;
  saveCalibration(boardId: string, c: Calibration | null): void;
}

export interface Sealer {
  seal(hex: string): string;
  open(sealed: string): string;
}

/** Where a board is in getting ready, most blocking first. */
export type UsbAgentStatus =
  | 'board_offline'
  | 'phone_not_connected'
  | 'phone_locked'
  | 'no_screen'
  | 'not_calibrated'
  | 'disarmed'
  | 'ready';

export const USB_STATUS_LABEL: Record<UsbAgentStatus, string> = {
  board_offline: 'USB Agent not on Wi-Fi',
  phone_not_connected: 'Plug the USB Agent into the iPhone',
  phone_locked: 'Unlock the iPhone',
  no_screen: 'Start Screen Mirroring to Allternit',
  not_calibrated: 'Calibrate the pointer',
  disarmed: 'Press the USB Agent button to arm it',
  ready: 'Ready',
};

export interface UsbAgentInfo {
  boardId: string;
  serial: string;
  name: string;
  status: UsbAgentStatus;
  armed: boolean;
  firmware: string | null;
  address: string | null;
  mirroring: AirplayState;
  receiverName: string;
  calibrated: boolean;
}

export interface UsbAgentDeps {
  store: UsbAgentStore;
  sealer: Sealer;
  /** Spawns uxplay with the frame pipe (phone-airplay FRAME_FD); null when uxplay isn't installed. */
  spawnUxplay: ((args: string[]) => ChildProcess) | null;
  /** Publishes the link over mDNS so boards find this computer when its IP changes; null where unsupported. */
  advertise: ((port: number, deskId: string) => ChildProcess | null) | null;
  /** JPEG → grayscale, downscaled to about `width` pixels wide. */
  decode: (jpeg: Buffer, width: number) => Gray | null;
  registerFile: (slot: number) => string;
  openSerial: (path: string) => Promise<SerialPort>;
  listSerial: () => string[];
  lanAddresses: () => string[];
  linkPort?: number;
  now?: () => number;
}

interface Board {
  stored: StoredUsbAgent;
  keyHex: string;
  screen: AirplayScreen;
  pointer: UsbPointer;
  generation: number;
}

type Point = [number, number];

const DECODE_WIDTH = 360;
const fail = (error: ToolErrorCode, message: string): ToolResult => ({ ok: false, error, message });

export class UsbAgentManager extends EventEmitter {
  private boards = new Map<string, Board>();
  private link: UsbAgentLink;
  private config: UsbAgentConfig;
  private advertiser: ChildProcess | null = null;
  private listening: Promise<number> | null = null;
  private readonly now: () => number;

  constructor(private readonly deps: UsbAgentDeps) {
    super();
    this.now = deps.now ?? Date.now;
    this.config = deps.store.loadConfig() ?? this.newConfig();
    this.link = new UsbAgentLink((boardId) => this.boards.get(boardId)?.keyHex ?? null);
    for (const ev of ['session', 'state', 'ended'] as const) {
      this.link.on(ev, (payload: { boardId: string }) => {
        const b = this.boards.get(payload.boardId);
        if (b && ev !== 'state') b.generation += 1;
        this.emit('devices');
      });
    }
    for (const stored of deps.store.load()) {
      try {
        this.addBoard(stored, deps.sealer.open(stored.sealedKey));
      } catch {
        // A key the keychain can't open (copied profile, reset keychain): keep the row so the user can forget it.
        this.addBoard(stored, '');
      }
    }
  }

  private newConfig(): UsbAgentConfig {
    const c = { deskId: `desk-${newKeyHex().slice(0, 16)}`, airplayPin: String(randomInt(0, 10_000)).padStart(4, '0') };
    this.deps.store.saveConfig(c);
    return c;
  }

  airplayAvailable(): boolean {
    return this.deps.spawnUxplay !== null;
  }

  airplayPin(): string {
    return this.config.airplayPin;
  }

  /** Starts the link, the mDNS advert and one AirPlay receiver per board. Safe to call again. */
  async start(): Promise<number> {
    if (!this.listening) {
      const port = this.deps.linkPort ?? USBA_DEFAULT_PORT;
      this.listening = this.link.listen(port).then(
        (bound) => {
          this.advertiser = this.deps.advertise?.(bound, this.config.deskId) ?? null;
          return bound;
        },
        (error: NodeJS.ErrnoException) => {
          if (error.code === 'EADDRINUSE') throw new Error(`Port ${port} is already in use on this computer, so USB Agents can’t connect. Quit the app using it and reopen Settings.`);
          throw error;
        },
      );
      this.listening.catch(() => {
        this.listening = null;
      });
    }
    for (const b of this.boards.values()) b.screen.start();
    return this.listening;
  }

  stopAll(): void {
    this.link.close();
    for (const b of this.boards.values()) b.screen.stop();
    if (this.advertiser && this.advertiser.exitCode === null) this.advertiser.kill('SIGTERM');
    this.advertiser = null;
    this.listening = null;
  }

  hasBoards(): boolean {
    return this.boards.size > 0;
  }

  // ── setup ────────────────────────────────────────────────────────────────

  serialPorts(): string[] {
    return this.deps.listSerial();
  }

  /** Reads which board is on a serial port (step 1 of setup). */
  async identify(path: string): Promise<{ boardId: string; firmware: string; provisioned: boolean }> {
    const port = await this.deps.openSerial(path);
    try {
      return await readIdentity(port);
    } finally {
      port.close();
    }
  }

  /** Writes Wi-Fi and a fresh key to the board, then remembers it. */
  async setUp(path: string, wifi: { ssid: string; password: string }, name: string): Promise<UsbAgentInfo> {
    const bound = await this.start();
    const host = this.deps.lanAddresses()[0];
    const keyHex = newKeyHex();
    const input = { ssid: wifi.ssid, password: wifi.password, host: host ?? '', port: bound, deskId: this.config.deskId, keyHex };
    const problem = validateProvision(input);
    if (problem) throw new Error(problem);
    const port = await this.deps.openSerial(path);
    let boardId: string;
    try {
      boardId = (await provision(port, input)).boardId;
    } finally {
      port.close();
    }
    const old = this.boards.get(boardId);
    if (old) this.removeBoard(boardId);
    const stored: StoredUsbAgent = {
      boardId,
      name: name.trim().slice(0, 40) || 'iPhone',
      sealedKey: this.deps.sealer.seal(keyHex),
      addedAt: this.now(),
      slot: old?.stored.slot ?? this.freeSlot(),
    };
    const board = this.addBoard(stored, keyHex);
    this.deps.store.saveCalibration(boardId, null);
    board.pointer.forget();
    this.persist();
    board.screen.start();
    this.emit('devices');
    return this.info(board);
  }

  forget(boardId: string): void {
    if (!this.boards.has(boardId)) return;
    void this.link.send(boardId, { op: 'disarm' });
    this.link.disconnect(boardId);
    this.removeBoard(boardId);
    this.deps.store.saveCalibration(boardId, null);
    this.persist();
    this.emit('devices');
  }

  /** Disarms the board: it lets go of everything and refuses input until its button is pressed again. */
  async stop(boardId: string): Promise<ToolResult> {
    const res = await this.link.send(boardId, { op: 'disarm' });
    return res.ok ? { ok: true } : fail(res.error === 'offline' ? 'device_offline' : 'adb_failed', res.message);
  }

  async calibrate(boardId: string): Promise<ToolResult> {
    const b = this.boards.get(boardId);
    if (!b) return fail('device_offline', 'Unknown USB Agent.');
    const blocked = this.blocker(b, { needCalibration: false });
    if (blocked) return blocked;
    try {
      const ok = await b.pointer.calibrate();
      this.emit('devices');
      return ok
        ? { ok: true }
        : fail('calibration_failed', 'Couldn’t find the pointer on the screen. Make sure AssistiveTouch is on, the phone is unlocked on a plain screen, and mirroring is running.');
    } catch (error) {
      return this.boardFailure(error);
    }
  }

  // ── queries ──────────────────────────────────────────────────────────────

  list(): UsbAgentInfo[] {
    return [...this.boards.values()].map((b) => this.info(b));
  }

  serials(): string[] {
    return [...this.boards.values()].filter((b) => this.link.session(b.stored.boardId)).map((b) => serialOf(b.stored.boardId));
  }

  has(serial: string): boolean {
    return serial.startsWith(USB_SERIAL_PREFIX) && this.boards.has(serial.slice(USB_SERIAL_PREFIX.length));
  }

  private info(b: Board): UsbAgentInfo {
    const session = this.link.session(b.stored.boardId);
    return {
      boardId: b.stored.boardId,
      serial: serialOf(b.stored.boardId),
      name: b.stored.name,
      status: statusOf(session, b.screen.state(), b.pointer.calibrated),
      armed: session?.state.armed ?? false,
      firmware: session?.firmware ?? null,
      address: session?.address ?? null,
      mirroring: b.screen.state(),
      receiverName: b.screen.receiverName(),
      calibrated: b.pointer.calibrated,
    };
  }

  // ── bot tools ────────────────────────────────────────────────────────────

  async call(name: string, serial: string, a: Record<string, unknown>): Promise<ToolResult> {
    const b = this.boards.get(serial.slice(USB_SERIAL_PREFIX.length));
    if (!b) return fail('device_offline', `Phone ${serial} is not connected.`);
    try {
      switch (name) {
        case 'phone.screenshot':
          return this.screenshot(b);
        case 'phone.tap':
          return await this.act(b, async (frame) => {
            const p = this.fraction(frame, a.x, a.y);
            await b.pointer.tap(p[0], p[1]);
            return [p];
          });
        case 'phone.swipe':
          return await this.act(b, async (frame) => {
            if (a.direction !== undefined) {
              const d = ({ up: [[0.5, 0.7], [0.5, 0.3]], down: [[0.5, 0.3], [0.5, 0.7]], left: [[0.8, 0.5], [0.2, 0.5]], right: [[0.2, 0.5], [0.8, 0.5]] } as Record<string, Point[]>)[String(a.direction)];
              if (!d) throw new ArgError('direction must be up, down, left or right.');
              await b.pointer.drag(d);
              return d;
            }
            const pts: Point[] = [this.fraction(frame, a.x1, a.y1), this.fraction(frame, a.x2, a.y2)];
            await b.pointer.drag(pts);
            return pts;
          });
        case 'phone.type':
          if (typeof a.text !== 'string' || !a.text.length || a.text.length > 500 || !/^[\x20-\x7E]*$/.test(a.text)) {
            return fail('invalid_args', 'text must be 1–500 printable ASCII characters (the USB Agent types with a US keyboard).');
          }
          return await this.act(b, async () => {
            for (let i = 0; i < (a.text as string).length; i += 200) await b.pointer.type((a.text as string).slice(i, i + 200));
            return [];
          }, { needCalibration: false });
        case 'phone.home':
          return await this.act(b, async () => {
            await b.pointer.goHome();
            return [[0.5, 0.995], [0.5, 0.6]];
          });
        case 'phone.back':
          return await this.act(b, async () => {
            await b.pointer.goBack();
            return [[0.005, 0.45], [0.6, 0.45]];
          });
        case 'phone.key':
          if (typeof a.combo !== 'string') return fail('invalid_args', 'combo is required, e.g. "enter" or "cmd+space".');
          return await this.act(b, async () => {
            await b.pointer.key(a.combo as string);
            return [];
          }, { needCalibration: false });
        case 'phone.open_app': {
          if (typeof a.app !== 'string' || !a.app.trim() || a.app.length > 60 || !/^[\x20-\x7E]+$/.test(a.app)) {
            return fail('invalid_args', 'app must be an app name, e.g. "Settings".');
          }
          // Spotlight from a hardware keyboard: ⌘Space, type the name, Return.
          return await this.act(b, async () => {
            await b.pointer.key('cmd+space');
            await sleep(600);
            await b.pointer.type((a.app as string).trim());
            await sleep(500);
            await b.pointer.key('enter');
            return [];
          }, { needCalibration: false });
        }
        case 'phone.task':
          return fail('unsupported', 'Plain-English tasks run through ARTEMIS, which is Android-only. On an iPhone, look with phone.screenshot and act with phone.tap / phone.swipe / phone.type.');
        case 'phone.sms.send':
        case 'phone.call.dial':
          return fail('unsupported', 'Texts and calls aren’t available through the USB Agent. Use the Allternit phone number, or an Android phone.');
        default:
          return fail('unknown_tool', `No phone tool named ${name}.`);
      }
    } catch (error) {
      if (error instanceof ArgError) return fail('invalid_args', error.message);
      return this.boardFailure(error);
    }
  }

  private screenshot(b: Board): ToolResult {
    const frame = b.screen.latest();
    if (!frame || b.screen.state() !== 'mirroring') {
      return fail('no_screen', `The iPhone isn’t mirroring. On the phone: Control Center → Screen Mirroring → ${b.screen.receiverName()}.`);
    }
    return { ok: true, mimeType: 'image/jpeg', dataBase64: frame.jpeg.toString('base64'), width: frame.width, height: frame.height, ageMs: this.now() - frame.at };
  }

  /** Pixel coordinates from the latest screenshot → screen fractions. */
  private fraction(frame: { width: number; height: number }, x: unknown, y: unknown): [number, number] {
    if (!isCoord(x) || !isCoord(y)) throw new ArgError('Coordinates must be non-negative integers from phone.screenshot.');
    if (x >= frame.width || y >= frame.height) throw new ArgError(`Coordinates are outside the ${frame.width}×${frame.height} screenshot.`);
    return [(x + 0.5) / frame.width, (y + 0.5) / frame.height];
  }

  private blocker(b: Board, opts: { needCalibration: boolean; needScreen?: boolean }): ToolResult | null {
    const session = this.link.session(b.stored.boardId);
    if (!session) return fail('device_offline', 'The USB Agent is not connected to Wi-Fi.');
    if (session.state.usb === 'none') return fail('device_offline', 'The USB Agent isn’t plugged into the iPhone.');
    if (session.state.usb === 'asleep') return fail('device_locked', 'The iPhone is locked or asleep. Unlock it to continue.');
    if (!session.state.armed) return fail('disarmed', 'The USB Agent is disarmed. Press its button to let the bot use the phone.');
    if ((opts.needScreen ?? true) && b.screen.state() !== 'mirroring') {
      return fail('no_screen', `The iPhone isn’t mirroring. On the phone: Control Center → Screen Mirroring → ${b.screen.receiverName()}.`);
    }
    if (opts.needCalibration && !b.pointer.calibrated) return fail('not_calibrated', 'Calibrate the pointer first (Settings → Computers → Phones).');
    return null;
  }

  /**
   * Runs one action and reports whether the screen changed afterwards, so the
   * bot verifies instead of assuming. Coordinates are read against the frame
   * the bot saw, which must not be stale.
   */
  private async act(
    b: Board,
    run: (frame: { width: number; height: number }) => Promise<Point[]>,
    opts: { needCalibration: boolean } = { needCalibration: true },
  ): Promise<ToolResult> {
    const blocked = this.blocker(b, { needCalibration: opts.needCalibration, needScreen: opts.needCalibration });
    if (blocked) return blocked;
    const frame = b.screen.latest();
    const beforeGray = frame ? this.deps.decode(frame.jpeg, DECODE_WIDTH) : null;
    const from = b.pointer.where();
    const visited = await run(frame ?? { width: 1, height: 1 });
    // The pointer's own trip (from where it was to where it went) is masked out of the comparison.
    const ignore: Point[] = [...(from ? [from] : []), ...visited];
    if (!frame) return { ok: true, screenChanged: null };
    await b.screen.settled(350, 300, 3_000);
    const after = b.screen.latest();
    const afterGray = after && after !== frame ? this.deps.decode(after.jpeg, DECODE_WIDTH) : beforeGray;
    return {
      ok: true,
      screenChanged: changed(beforeGray, afterGray, ignore),
      note: 'screenChanged=false usually means the action missed; look again before retrying. Coordinates are only valid for the frame they came from.',
    };
  }

  private boardFailure(error: unknown): ToolResult {
    if (error instanceof BoardError) {
      const map: Record<string, ToolErrorCode> = { offline: 'device_offline', disarmed: 'disarmed', usb_asleep: 'device_locked', uncertain: 'uncertain', invalid: 'invalid_args', rejected: 'adb_failed' };
      return fail(map[error.result.error] ?? 'adb_failed', error.message);
    }
    return fail('adb_failed', error instanceof Error ? error.message : String(error));
  }

  // ── internals ────────────────────────────────────────────────────────────

  private addBoard(stored: StoredUsbAgent, keyHex: string): Board {
    const opts: UxplayOptions = {
      name: stored.slot === 0 ? AIRPLAY_NAME : `${AIRPLAY_NAME} ${stored.slot + 1}`,
      basePort: AIRPLAY_BASE_PORT + stored.slot * 3,
      fps: 8,
      quality: 75,
      pin: this.config.airplayPin,
      registerFile: this.deps.registerFile(stored.slot),
    };
    const screen = new AirplayScreen({ spawn: this.deps.spawnUxplay, now: this.now }, opts);
    screen.on('started', () => this.emit('devices'));
    screen.on('exit', () => this.emit('devices'));
    let lastState: AirplayState = screen.state();
    screen.on('frame', () => {
      if (screen.state() !== lastState) {
        lastState = screen.state();
        this.emit('devices');
      }
    });
    const board: Board = { stored, keyHex, screen, pointer: null as unknown as UsbPointer, generation: 0 };
    const view: PointerScreen = {
      live: () => screen.state() === 'mirroring',
      latestAt: () => screen.latest()?.at ?? null,
      latestGray: () => {
        const f = screen.latest();
        return f ? this.deps.decode(f.jpeg, DECODE_WIDTH) : null;
      },
      nextGray: async (since, timeoutMs) => {
        const f = await screen.nextFrame(since, timeoutMs);
        return f ? this.deps.decode(f.jpeg, DECODE_WIDTH) : null;
      },
      settled: (q, l, m) => screen.settled(q, l, m),
    };
    board.pointer = new UsbPointer({
      send: (cmd: BoardCommand) => this.link.send(stored.boardId, cmd),
      screen: view,
      load: () => this.deps.store.loadCalibration(stored.boardId),
      save: (c) => this.deps.store.saveCalibration(stored.boardId, c),
      linkGeneration: () => board.generation,
      now: this.now,
    });
    this.boards.set(stored.boardId, board);
    return board;
  }

  private removeBoard(boardId: string): void {
    const b = this.boards.get(boardId);
    if (!b) return;
    b.screen.stop();
    this.boards.delete(boardId);
  }

  private freeSlot(): number {
    const used = new Set([...this.boards.values()].map((b) => b.stored.slot));
    let slot = 0;
    while (used.has(slot)) slot += 1;
    return slot;
  }

  private persist(): void {
    this.deps.store.save([...this.boards.values()].map((b) => b.stored));
  }
}

class ArgError extends Error {}

const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));
const isCoord = (v: unknown): v is number => Number.isInteger(v) && (v as number) >= 0 && (v as number) <= 20_000;

export function serialOf(boardId: string): string {
  return `${USB_SERIAL_PREFIX}${boardId}`;
}

export function statusOf(session: BoardSessionInfo | null, mirroring: AirplayState, calibrated: boolean): UsbAgentStatus {
  if (!session) return 'board_offline';
  if (session.state.usb === 'none') return 'phone_not_connected';
  if (session.state.usb === 'asleep') return 'phone_locked';
  if (mirroring !== 'mirroring') return 'no_screen';
  if (!calibrated) return 'not_calibrated';
  if (!session.state.armed) return 'disarmed';
  return 'ready';
}
