/**
 * Allternit USB Agent: a bot operates a real iPhone (or anything that takes a
 * USB mouse and keyboard) with no app on the phone, no Mac-only mirroring and
 * no developer account.
 *
 *   hands: the USB Agent board in the phone's USB-C port (USB mouse +
 *          keyboard; iOS AssistiveTouch shows the pointer), commanded over
 *          Wi-Fi through the authenticated link (phone-usb-agent-link.ts)
 *   eyes:  the phone's AirPlay screen mirror, received here by UxPlay
 *          (phone-airplay.ts), one PIN-protected receiver per board; or, for
 *          a board paired to an iPhone with the Allternit app (Allternit
 *          Link, phone-link-board.ts), that app's shared screen
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
import { USBA_DEFAULT_PORT, newKeyHex, type BoardCommand, type UsbaVersion } from './phone-usb-agent-protocol.js';
import { BoardError, UsbPointer, type Calibration, type PointerScreen } from './phone-usb-agent-pointer.js';
import { changed, type Gray } from './phone-usb-agent-vision.js';
import { compareVersions } from './phone-usb-agent-firmware.js';
import { provision, readIdentity, validateProvision, type SerialPort } from './phone-usb-agent-provision.js';
import type { ToolErrorCode, ToolResult } from './phone-tools.js';
import { APP_FRAME_TIMEOUT_MS, APP_SERIAL_PREFIX, APP_SHARE_SCREEN_MESSAGE } from './phone-app-store.js';
import type { LinkBoardStatus, LinkFrameSource } from './phone-link-board.js';

export const USB_SERIAL_PREFIX = 'usb:';

export interface StoredUsbAgent {
  boardId: string;
  name: string;
  /** The board key, sealed with the OS keychain (safeStorage) in production. */
  sealedKey: string;
  addedAt: number;
  /** Index into the AirPlay port range; stable so the phone's PIN trust survives restarts. */
  slot: number;
  /** Allternit Link: the app iPhone (`app:<uuid>`) whose shared screen is this board's eyes. Set on Desktop only. */
  pairedPhone?: string;
  /** The board has a cloud relay row (hardware/link/README.md section 4.5). */
  relay?: 'registered';
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

/** One board as Settings shows it: the shared status shape plus the Desktop-only fields. */
export interface UsbAgentInfo extends LinkBoardStatus {
  address: string | null;
  /** The eyes' state (the AirPlay mirror, or the paired app phone's shared screen). */
  mirroring: AirplayState;
  receiverName: string;
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
  /** Firmware version this Desktop ships, for the update hint. */
  bundledFirmware?: () => string | null;
  /** Allternit Link: the shared screen of a paired app phone (AppPhoneBackend.frameSource); null when that lane is off. */
  eyes?: (pairedPhone: string) => LinkFrameSource | null;
}

interface Board {
  stored: StoredUsbAgent;
  keyHex: string;
  /** The eyes: an AirPlay receiver, or the paired app phone's shared screen. */
  screen: LinkFrameSource;
  pointer: UsbPointer;
  generation: number;
  /** Last armed state seen (link loss counts as disarmed), for the `armed` event. */
  armed: boolean;
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
        if (b) {
          // 'armed' fires on every change, link loss included (phone-link-board.ts stops a run on it).
          const armed = this.link.session(b.stored.boardId)?.state.armed ?? false;
          if (armed !== b.armed) {
            b.armed = armed;
            this.emit('armed', { boardId: b.stored.boardId, armed });
          }
        }
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
      ...(old?.stored.pairedPhone ? { pairedPhone: old.stored.pairedPhone } : {}),
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
    if (b.screen.wake && b.screen.state() !== 'mirroring') await b.screen.wake(APP_FRAME_TIMEOUT_MS);
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

  /**
   * Allternit Link: pairs a board with an app iPhone (`app:<uuid>`), or unpairs
   * it (null). One board per phone: pairing a second board to the phone
   * unpairs the first. Either way the board's calibration is cleared, since
   * nothing proves which phone it is in until calibration finds the pointer
   * in that phone's shared screen. A paired board's eyes are the app's shared
   * screen and its AirPlay receiver is not started.
   */
  pair(boardId: string, serial: string | null): UsbAgentInfo {
    const b = this.boards.get(boardId);
    if (!b) throw new Error('Unknown USB Agent.');
    if (serial !== null && (!serial.startsWith(APP_SERIAL_PREFIX) || serial.length <= APP_SERIAL_PREFIX.length || serial.length > 120)) {
      throw new Error('Pair the board with an iPhone that has the Allternit app (serial "app:…").');
    }
    if (serial !== null) {
      for (const other of this.boards.values()) {
        if (other !== b && other.stored.pairedPhone === serial) this.setEyes(other, undefined);
      }
    }
    this.setEyes(b, serial ?? undefined);
    this.persist();
    this.emit('devices');
    return this.info(b);
  }

  private setEyes(b: Board, pairedPhone: string | undefined): void {
    b.screen.stop();
    if (pairedPhone) b.stored.pairedPhone = pairedPhone;
    else delete b.stored.pairedPhone;
    b.screen = this.eyesFor(b.stored);
    this.deps.store.saveCalibration(b.stored.boardId, null);
    b.pointer.forget();
    if (this.listening) b.screen.start();
  }

  /** The app phone a board is paired with, or null. */
  pairedPhone(boardId: string): string | null {
    return this.boards.get(boardId)?.stored.pairedPhone ?? null;
  }

  /** The board paired with an app phone, or null. */
  boardForPhone(serial: string): string | null {
    for (const b of this.boards.values()) if (b.stored.pairedPhone === serial) return b.stored.boardId;
    return null;
  }

  status(boardId: string): UsbAgentInfo | null {
    const b = this.boards.get(boardId);
    return b ? this.info(b) : null;
  }

  /**
   * Faces and progress (USBA/2 display commands). Best effort: sent only on a
   * USBA/2 session, failures ignored. A USBA/1 board never receives them, since
   * its firmware resets the idle timer on any command once armed.
   */
  async display(boardId: string, cmds: BoardCommand[]): Promise<void> {
    for (const cmd of cmds) {
      if (cmd.op !== 'face' && cmd.op !== 'prog' && cmd.op !== 'label') continue;
      if (protocolOf(this.link.session(boardId)) !== 2) return;
      try {
        await this.link.send(boardId, cmd);
      } catch {
        // best effort
      }
    }
  }

  // ── queries ──────────────────────────────────────────────────────────────

  list(): UsbAgentInfo[] {
    return [...this.boards.values()].map((b) => this.info(b));
  }

  /** Online boards that are phones of their own. A paired board is part of its app phone. */
  serials(): string[] {
    return [...this.boards.values()].filter((b) => !b.stored.pairedPhone && this.link.session(b.stored.boardId)).map((b) => serialOf(b.stored.boardId));
  }

  has(serial: string): boolean {
    if (!serial.startsWith(USB_SERIAL_PREFIX)) return false;
    const b = this.boards.get(serial.slice(USB_SERIAL_PREFIX.length));
    return Boolean(b && !b.stored.pairedPhone);
  }

  private info(b: Board): UsbAgentInfo {
    const session = this.link.session(b.stored.boardId);
    const mirroring = b.screen.state();
    const state = session?.state;
    return {
      boardId: b.stored.boardId,
      serial: serialOf(b.stored.boardId),
      name: b.stored.name,
      online: session !== null,
      armed: state?.armed ?? false,
      status: statusOf(session, mirroring, b.pointer.calibrated),
      link: state?.link ?? null,
      usb: state?.usb ?? null,
      hid: state?.hid ?? null,
      dock: state?.dock ?? null,
      face: state?.face ?? null,
      prog: state?.prog ?? null,
      eyes: b.stored.pairedPhone ? 'app' : 'airplay',
      phone: b.stored.pairedPhone ?? null,
      screen: mirroring === 'mirroring',
      calibrated: b.pointer.calibrated,
      firmware: session?.firmware ?? null,
      protocol: protocolOf(session),
      firmwareUpdate: Boolean(session && compareVersions(session.firmware, this.deps.bundledFirmware?.() ?? null) < 0),
      relay: b.stored.relay === 'registered' ? 'registered' : 'none',
      address: session?.address ?? null,
      mirroring,
      receiverName: receiverNameFor(b.stored.slot),
    };
  }

  /** What to tell the bot when the eyes have no picture. */
  private noScreen(b: Board): string {
    return b.stored.pairedPhone
      ? `${APP_SHARE_SCREEN_MESSAGE} The Allternit Link board sees through the app's shared screen.`
      : `The iPhone isn’t mirroring. On the phone: Control Center → Screen Mirroring → ${receiverNameFor(b.stored.slot)}.`;
  }

  /**
   * A paired board's eyes join the app stream on demand: before a screenshot,
   * and before an action the board could otherwise run, wait for a picture.
   */
  private async wakeEyes(b: Board, name: string): Promise<void> {
    if (!b.screen.wake || b.screen.state() === 'mirroring') return;
    if (name !== 'phone.screenshot') {
      const s = this.link.session(b.stored.boardId);
      if (!s || !s.state.armed || s.state.usb !== 'mounted') return;
    }
    await b.screen.wake(APP_FRAME_TIMEOUT_MS);
  }

  // ── bot tools ────────────────────────────────────────────────────────────

  async call(name: string, serial: string, a: Record<string, unknown>): Promise<ToolResult> {
    const b = this.boards.get(serial.slice(USB_SERIAL_PREFIX.length));
    if (!b) return fail('device_offline', `Phone ${serial} is not connected.`);
    try {
      await this.wakeEyes(b, name);
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
        case 'phone.read_ui':
        case 'phone.screen_text':
        case 'phone.find_and_tap':
          return fail('unsupported', 'Reading the screen as text needs the Phone Link helper or an Android phone. On a USB-Agent iPhone, use phone.screenshot and tap from what you see.');
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
    if (!frame || b.screen.state() !== 'mirroring') return fail('no_screen', this.noScreen(b));
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
    if ((opts.needScreen ?? true) && b.screen.state() !== 'mirroring') return fail('no_screen', this.noScreen(b));
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

  /** The eyes for a stored board: the paired app phone's shared screen, else its AirPlay receiver. */
  private eyesFor(stored: StoredUsbAgent): LinkFrameSource {
    if (stored.pairedPhone) return this.deps.eyes?.(stored.pairedPhone) ?? NO_EYES;
    const opts: UxplayOptions = {
      name: receiverNameFor(stored.slot),
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
    return screen;
  }

  private addBoard(stored: StoredUsbAgent, keyHex: string): Board {
    const board: Board = { stored, keyHex, screen: this.eyesFor(stored), pointer: null as unknown as UsbPointer, generation: 0, armed: false };
    // Read board.screen on every call: pairing swaps the eyes under the same pointer.
    const view: PointerScreen = {
      live: () => board.screen.state() === 'mirroring',
      latestAt: () => board.screen.latest()?.at ?? null,
      latestGray: () => {
        const f = board.screen.latest();
        return f ? this.deps.decode(f.jpeg, DECODE_WIDTH) : null;
      },
      nextGray: async (since, timeoutMs) => {
        const f = await board.screen.nextFrame(since, timeoutMs);
        return f ? this.deps.decode(f.jpeg, DECODE_WIDTH) : null;
      },
      settled: (q, l, m) => board.screen.settled(q, l, m),
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

/** Eyes for a paired board when the app phone lane is off: never a picture. */
const NO_EYES: LinkFrameSource = {
  state: () => 'unavailable',
  latest: () => null,
  nextFrame: async () => null,
  settled: async () => undefined,
  start: () => undefined,
  stop: () => undefined,
};

function receiverNameFor(slot: number): string {
  return slot === 0 ? AIRPLAY_NAME : `${AIRPLAY_NAME} ${slot + 1}`;
}

/** The session's USBA version (lane F adds `version` to BoardSessionInfo; a session without it is USBA/1). */
function protocolOf(session: BoardSessionInfo | null): UsbaVersion | null {
  if (!session) return null;
  return (session as BoardSessionInfo & { version?: UsbaVersion }).version === 2 ? 2 : 1;
}

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
