/**
 * iPhones connected with the Allternit App Store app (route `iphone-app-store`,
 * serials `app:<phoneId>`). The app has no XCTest runner and no Bluetooth
 * board, so a bot gets two things on these phones:
 *
 * - **The screen.** The app's broadcast extension streams iCTRL packets
 *   (phone-ios-video.ts framing, types 0–5) out to the account's node
 *   (allternit-api), which opens the sealed envelopes and fans the plaintext
 *   packets out to viewers of `app:<id>` on the usual
 *   `/api/v1/phones/:serial/ws-token` + `/ws/phones/:serial/stream`. Desktop
 *   watches as one more viewer (AppPhoneViewerSession), parsed with the same
 *   IosVideoPacketParser the linked-iPhone lane uses, so the in-app live view
 *   and phone.screenshot share one stream. A still for the bot is the last
 *   keyframe and the frames after it, decoded by Chromium's own H.264 decoder
 *   (decodeStill, injected; phone-service.ts runs it in a hidden window).
 * - **The app's own browser** (ACI browser): phone.web.* go to the node's
 *   `POST /api/v1/phones/app/:id/rpc`, which forwards them to the app's control
 *   socket while the app is open. 409 `app_not_open` when it isn't.
 *
 * Taps and swipes on other apps are not possible yet ("Taps on native apps
 * need the Bluetooth board"), so those tools answer `not_supported`.
 *
 * Everything that talks to the node is injected (AppPhoneNode), so the lane is
 * unit-tested with no phone and no node.
 */

import WebSocket from 'ws';
import { IosVideoPacketParser, type IosVideoConfig } from './phone-ios-video.js';
import type { PhoneAudioMeta, PhoneInputEvent, PhoneStreamMeta, PhoneStreamStatus, ScrcpyDeviceMessage } from './phone-scrcpy.js';
import type { StreamConsumer, StreamSessionHandle } from './phone-stream.js';
import type { ToolErrorCode, ToolResult, UsbPhoneBackend } from './phone-tools.js';

export const APP_SERIAL_PREFIX = 'app:';
export const APP_PHONE_ROUTE = 'iphone-app-store';
export const APP_TAPS_MESSAGE = 'Taps on native apps need the Bluetooth board';
export const APP_SHARE_SCREEN_MESSAGE = 'Open the Allternit app on your iPhone and tap Share screen.';
export const APP_NOT_OPEN_MESSAGE = 'Open the Allternit app on the iPhone. The bot can use its browser only while the app is open.';

/** The app's web.* / screen.prompt error codes (spec "Decisions fixed after the iOS build" 10) → tool errors. */
const APP_ERRORS: Record<string, { error: ToolErrorCode; message: string }> = {
  stopped_by_user: { error: 'approval_denied', message: 'The person stopped the bot in the Allternit app\'s browser. Don\'t retry unless they ask you to.' },
  browser_not_open: { error: 'app_not_open', message: 'The browser isn\'t open in the Allternit app. Ask the person to open it, or use phone.web.open.' },
  stale_ref: { error: 'not_found', message: 'That element ref is from an older page. Take a new phone.web.snapshot and use a ref from it.' },
  not_editable: { error: 'invalid_args', message: 'That element can\'t be typed into. Pick a text field from the latest phone.web.snapshot.' },
  bad_params: { error: 'invalid_args', message: 'The Allternit app refused the arguments.' },
  unknown_method: { error: 'not_supported', message: 'The Allternit app on this iPhone is too old for this. Update it from the App Store.' },
  script_error: { error: 'phone_failed', message: 'The page didn\'t let the Allternit app read or act on it.' },
  internal: { error: 'phone_failed', message: 'The Allternit app hit an error. Try again.' },
  app_not_open: { error: 'app_not_open', message: APP_NOT_OPEN_MESSAGE },
};

/** How long a viewer waits for the first CONFIG packet before giving up. */
export const APP_START_TIMEOUT_MS = 20_000;
/** How long phone.screenshot waits for a keyframe after joining. */
export const APP_FRAME_TIMEOUT_MS = 8_000;
/** A screenshot viewer that started the stream stops it after this long without another screenshot. */
export const APP_VIEWER_IDLE_MS = 45_000;
const LIST_TTL_MS = 5_000;
const RPC_TIMEOUT_MS = 20_000;
/** Frames kept after a keyframe for a still (a GOP longer than this decodes to an older picture). */
const MAX_GOP_UNITS = 240;
const MAX_GOP_BYTES = 24 * 1024 * 1024;

export const isAppSerial = (serial: unknown): boolean => typeof serial === 'string' && serial.startsWith(APP_SERIAL_PREFIX);
export const appPhoneId = (serial: string): string => serial.slice(APP_SERIAL_PREFIX.length);

// ── The node (allternit-api) ────────────────────────────────────────────────

/** One app phone as the node's `GET /api/v1/phones` lists it. */
export interface AppPhoneRow {
  serial: string;
  name: string;
  model: string;
  /** The broadcast extension's ingest socket is open (the screen is shared). */
  online: boolean;
}

export type AppRpcResult =
  | { ok: true; result: Record<string, unknown> }
  | { ok: false; status: number; code: string; message: string };

/** What a viewer socket needs (ws-style; tests fake it). */
export interface AppViewerSocket {
  send(data: string): void;
  close(): void;
  onMessage(cb: (data: Buffer | string, isBinary: boolean) => void): void;
  onClose(cb: (code: number, reason: string) => void): void;
  onError(cb: (error: Error) => void): void;
}

export interface AppPhoneNode {
  /** `GET /api/v1/phones`, only the `app:` rows. */
  listPhones(): Promise<AppPhoneRow[]>;
  /** `POST /api/v1/phones/app/:id/rpc` with `{method, params}`. */
  rpc(phoneId: string, method: string, params: Record<string, unknown>, timeoutMs: number): Promise<AppRpcResult>;
  /** Mints a read-only ws-token and opens `/ws/phones/:serial/stream`. */
  openViewer(serial: string): Promise<AppViewerSocket>;
  /** `POST /api/v1/phones/app/pair/:phoneId/decision` with `{approve}`. */
  decidePair(phoneId: string, approve: boolean): Promise<{ ok: boolean; message?: string }>;
}

// ── The viewer session (live view + screenshots) ────────────────────────────

interface ViewerEvents {
  meta(meta: PhoneStreamMeta): void;
  videoChunk(chunk: Buffer): void;
  audioMeta(meta: PhoneAudioMeta): void;
  audioChunk(chunk: Buffer): void;
  deviceMessage(message: ScrcpyDeviceMessage): void;
  ended(info: { reason: string }): void;
}

export interface AppViewerDeps {
  open: () => Promise<AppViewerSocket>;
  deviceName: string;
  log?: (message: string) => void;
  startTimeoutMs?: number;
}

/**
 * One viewer of an app phone's screen, in the PhoneStreamManager session
 * shape. Binary frames are plaintext iCTRL packets; CONFIG becomes the
 * stream meta (again on every change: rotation), FRAME the video chunks,
 * AUDIO_CONFIG/AUDIO the sound. ENDED (the person stopped sharing) and a
 * relay `{"type":"end"}` end the session. Input is refused: taps on other
 * apps need the Bluetooth board.
 */
export class AppPhoneViewerSession implements StreamSessionHandle {
  private listeners: { [K in keyof ViewerEvents]?: Array<ViewerEvents[K]> } = {};
  private readonly parser = new IosVideoPacketParser();
  private socket: AppViewerSocket | null = null;
  private config: IosVideoConfig | null = null;
  private startPromise: Promise<void> | null = null;
  private resolveLive: (() => void) | null = null;
  private rejectLive: ((error: Error) => void) | null = null;
  private timer: ReturnType<typeof setTimeout> | null = null;
  private ended = false;

  constructor(private readonly deps: AppViewerDeps) {}

  on<K extends keyof ViewerEvents>(event: K, cb: ViewerEvents[K]): () => void {
    const list = (this.listeners[event] ??= []) as Array<ViewerEvents[K]>;
    list.push(cb);
    return () => {
      const i = list.indexOf(cb);
      if (i >= 0) list.splice(i, 1);
    };
  }

  private emit<K extends keyof ViewerEvents>(event: K, ...args: Parameters<ViewerEvents[K]>): void {
    for (const cb of this.listeners[event] ?? []) (cb as (...a: Parameters<ViewerEvents[K]>) => void)(...args);
  }

  /** Resolves on the first CONFIG; rejects when the phone isn't sharing its screen. */
  start(): Promise<void> {
    this.startPromise ??= (async () => {
      const live = new Promise<void>((resolve, reject) => {
        this.resolveLive = resolve;
        this.rejectLive = reject;
      });
      void live.catch(() => undefined);
      this.timer = setTimeout(() => this.fail(new Error(APP_SHARE_SCREEN_MESSAGE)), this.deps.startTimeoutMs ?? APP_START_TIMEOUT_MS);
      const socket = await this.deps.open();
      if (this.ended) {
        socket.close();
        throw new Error('closed');
      }
      this.socket = socket;
      socket.onMessage((data, isBinary) => this.onMessage(data, isBinary));
      socket.onClose((_code, reason) => this.finish(reason || 'the iPhone stopped sharing its screen'));
      socket.onError((error) => this.deps.log?.(`[app-screen ${this.deps.deviceName}] socket error: ${error.message}`));
      await live;
    })().catch((error) => {
      this.finish(error instanceof Error ? error.message : String(error));
      throw error;
    });
    return this.startPromise;
  }

  private onMessage(data: Buffer | string, isBinary: boolean): void {
    if (this.ended) return;
    if (!isBinary) {
      let frame: { type?: unknown; reason?: unknown; message?: unknown } | null = null;
      try {
        frame = JSON.parse(String(data));
      } catch {
        return;
      }
      if (frame?.type === 'end') this.finish(typeof frame.reason === 'string' && frame.reason ? endReason(frame.reason) : 'the stream ended');
      else if (frame?.type === 'ping') this.socket?.send(JSON.stringify({ type: 'pong' }));
      return;
    }
    let packets;
    try {
      packets = this.parser.push(Buffer.isBuffer(data) ? data : Buffer.from(data));
    } catch (error) {
      this.finish(error instanceof Error ? error.message : String(error));
      return;
    }
    for (const packet of packets) {
      switch (packet.type) {
        case 'config': {
          const { width, height, rotation } = packet;
          const changed = !this.config || this.config.width !== width || this.config.height !== height || this.config.rotation !== rotation;
          this.config = packet;
          if (changed) this.emit('meta', { deviceName: this.deps.deviceName, width, height, codec: 'h264', ...(rotation ? { rotation } : {}) });
          if (this.resolveLive) {
            if (this.timer) clearTimeout(this.timer);
            this.timer = null;
            this.resolveLive();
            this.resolveLive = null;
            this.rejectLive = null;
          }
          break;
        }
        case 'frame':
          if (this.config) this.emit('videoChunk', packet.data);
          break;
        case 'audioConfig':
          this.emit('audioMeta', packet.meta);
          break;
        case 'audio':
          if (this.config) this.emit('audioChunk', packet.data);
          break;
        case 'ended':
          this.finish(packet.reason);
          break;
        default:
          break;
      }
    }
  }

  private fail(error: Error): void {
    this.rejectLive?.(error);
    this.resolveLive = null;
    this.rejectLive = null;
  }

  private finish(reason: string): void {
    if (this.ended) return;
    this.ended = true;
    if (this.timer) clearTimeout(this.timer);
    this.timer = null;
    this.fail(new Error(reason));
    const socket = this.socket;
    this.socket = null;
    try {
      socket?.close();
    } catch {
      // already closed
    }
    this.emit('ended', { reason });
  }

  sendControl(_events: PhoneInputEvent[]): { ok: boolean; message?: string } {
    return { ok: false, message: `${APP_TAPS_MESSAGE}. This iPhone shares its screen only.` };
  }

  setDisplayPower(_on: boolean): { ok: boolean; message?: string } {
    return { ok: false, message: 'Not available on iPhone.' };
  }

  close(reason = 'closed'): void {
    this.finish(reason);
  }
}

/** The relay's end reasons in plain words. */
function endReason(reason: string): string {
  if (reason === 'desktop_offline' || reason === 'phone_offline') return APP_SHARE_SCREEN_MESSAGE;
  if (reason === 'token_expired') return 'The stream token expired.';
  return reason;
}

// ── Stills for phone.screenshot ─────────────────────────────────────────────

/** First bytes of the NAL units in one Annex-B access unit. */
export function nalTypes(au: Buffer): number[] {
  const out: number[] = [];
  for (let i = 0; i + 3 < au.length; i += 1) {
    if (au[i] !== 0 || au[i + 1] !== 0) continue;
    if (au[i + 2] === 1) {
      out.push(au[i + 3] & 0x1f);
      i += 3;
    } else if (au[i + 2] === 0 && au[i + 3] === 1 && i + 4 < au.length) {
      out.push(au[i + 4] & 0x1f);
      i += 4;
    }
  }
  return out;
}

/** RFC 6381 `avc1.PPCCLL` from the SPS in an access unit, or null when it has none. */
export function codecFromAccessUnit(au: Buffer): string | null {
  for (let i = 0; i + 7 < au.length; i += 1) {
    const three = au[i] === 0 && au[i + 1] === 0 && au[i + 2] === 1;
    if (!three) continue;
    const nal = i + 3;
    if ((au[nal] & 0x1f) !== 7) continue;
    const hex = (n: number) => n.toString(16).padStart(2, '0');
    return `avc1.${hex(au[nal + 1])}${hex(au[nal + 2])}${hex(au[nal + 3])}`;
  }
  return null;
}

export interface StillInput {
  codec: string;
  /** Annex-B access units: a keyframe first, then the frames after it. */
  units: Buffer[];
  rotation: 0 | 90 | 180 | 270;
}

/** Decodes a GOP and returns its last picture, upright, as a JPEG. */
export type StillDecoder = (input: StillInput) => Promise<{ jpeg: Buffer; width: number; height: number }>;

/**
 * Keeps the newest keyframe and the frames after it, so a still is always
 * the latest picture. A GOP past the cap stops growing (its still is older),
 * until the next keyframe.
 */
export class AppFrameKeeper implements StreamConsumer {
  private units: Buffer[] = [];
  private bytes = 0;
  private codec: string | null = null;
  private rotation: 0 | 90 | 180 | 270 = 0;
  private at = 0;
  private waiters: Array<() => void> = [];

  constructor(private readonly now: () => number = Date.now) {}

  onMeta(_serial: string, meta: PhoneStreamMeta): void {
    this.rotation = meta.rotation ?? 0;
  }

  onChunk(_serial: string, chunk: Buffer): void {
    if (nalTypes(chunk).includes(5)) {
      this.units = [chunk];
      this.bytes = chunk.length;
      this.codec = codecFromAccessUnit(chunk) ?? this.codec;
    } else if (this.units.length > 0 && this.units.length < MAX_GOP_UNITS && this.bytes + chunk.length <= MAX_GOP_BYTES) {
      this.units.push(chunk);
      this.bytes += chunk.length;
    } else {
      return;
    }
    this.at = this.now();
    const waiters = this.waiters;
    this.waiters = [];
    for (const w of waiters) w();
  }

  onEnd(): void {
    this.units = [];
    this.bytes = 0;
    const waiters = this.waiters;
    this.waiters = [];
    for (const w of waiters) w();
  }

  /** The GOP to decode, or null before the first keyframe. */
  snapshot(): { input: StillInput; at: number } | null {
    if (this.units.length === 0 || !this.codec) return null;
    return { input: { codec: this.codec, units: [...this.units], rotation: this.rotation }, at: this.at };
  }

  /** Resolves when a frame arrives (or the stream ends), or after timeoutMs. */
  waitForFrame(timeoutMs: number): Promise<void> {
    if (this.snapshot()) return Promise.resolve();
    return new Promise((resolve) => {
      const timer = setTimeout(resolve, timeoutMs);
      this.waiters.push(() => {
        clearTimeout(timer);
        resolve();
      });
    });
  }
}

// ── The bot tool lane ───────────────────────────────────────────────────────

/** The stream manager's slice this lane uses (PhoneStreamManager). */
export interface AppStreams {
  start(serial: string): Promise<{ ok: true } | { ok: false; message: string }>;
  stop(serial: string): Promise<void>;
  status(serial: string): PhoneStreamStatus;
  subscribe(serial: string, consumer: StreamConsumer): () => void;
  consumerCount(serial: string): number;
}

export interface AppPhoneBackendDeps {
  node: AppPhoneNode;
  streams: AppStreams;
  decodeStill: StillDecoder;
  now?: () => number;
  log?: (message: string) => void;
  viewerIdleMs?: number;
  frameTimeoutMs?: number;
}

/** One app phone as phone.devices lists it. */
export interface AppPhoneDevice {
  serial: string;
  platform: 'ios';
  via: 'allternit-app';
  route: typeof APP_PHONE_ROUTE;
  name: string;
  /** The screen is shared right now. */
  screen: boolean;
  /** What a bot can do here, in plain words. */
  note: string;
}

const fail = (error: ToolErrorCode, message: string): ToolResult => ({ ok: false, error, message });

const WEB_METHODS: Record<string, string> = {
  'phone.web.open': 'web.open',
  'phone.web.snapshot': 'web.snapshot',
  'phone.web.click': 'web.click',
  'phone.web.type': 'web.type',
  'phone.web.scroll': 'web.scroll',
  'phone.web.back': 'web.back',
  'phone.web.wait': 'web.wait',
};

export const isWebTool = (name: string): boolean => name in WEB_METHODS;

/** Validated `{method, params}` for a phone.web.* call, or the error to return. */
export function webRpc(name: string, a: Record<string, unknown>): { method: string; params: Record<string, unknown> } | ToolResult {
  const method = WEB_METHODS[name];
  if (!method) return fail('unknown_tool', `No phone tool named ${name}.`);
  const ref = () => (typeof a.ref === 'string' && a.ref.trim() && a.ref.length <= 200 ? a.ref.trim() : null);
  switch (name) {
    case 'phone.web.open': {
      const url = typeof a.url === 'string' ? a.url.trim() : '';
      let parsed: URL | null = null;
      try {
        parsed = url.length <= 2048 ? new URL(url) : null;
      } catch {
        parsed = null;
      }
      if (!parsed || (parsed.protocol !== 'https:' && parsed.protocol !== 'http:')) return fail('invalid_args', 'url must be an http or https address.');
      return { method, params: { url: parsed.toString() } };
    }
    case 'phone.web.click': {
      const r = ref();
      return r ? { method, params: { ref: r } } : fail('invalid_args', 'ref is required: an element ref from the latest phone.web.snapshot.');
    }
    case 'phone.web.type': {
      const r = ref();
      if (!r) return fail('invalid_args', 'ref is required: an element ref from the latest phone.web.snapshot.');
      if (typeof a.text !== 'string' || a.text.length > 2000) return fail('invalid_args', 'text must be at most 2000 characters.');
      if (a.submit !== undefined && typeof a.submit !== 'boolean') return fail('invalid_args', 'submit must be true or false.');
      return { method, params: { ref: r, text: a.text, ...(a.submit === true ? { submit: true } : {}) } };
    }
    case 'phone.web.scroll': {
      const dy = a.dy;
      if (!Number.isInteger(dy) || Math.abs(dy as number) > 20_000) return fail('invalid_args', 'dy must be a whole number of points, -20000 to 20000 (positive scrolls down).');
      return { method, params: { dy } };
    }
    case 'phone.web.wait': {
      const ms = a.ms;
      if (!Number.isInteger(ms) || (ms as number) < 0 || (ms as number) > 10_000) return fail('invalid_args', 'ms must be 0–10000.');
      return { method, params: { ms } };
    }
    default:
      return { method, params: {} };
  }
}

function appError(res: { status: number; code: string; message: string }): ToolResult {
  const known = APP_ERRORS[res.code] ?? (res.status === 409 ? APP_ERRORS.app_not_open : undefined);
  if (known) return fail(known.error, res.code === 'bad_params' && res.message ? `${known.message} ${res.message}` : known.message);
  if (res.status === 404) return fail('device_offline', 'This iPhone is not paired with this computer any more. Pair it again in the Allternit app (Settings → Phones).');
  return fail('phone_failed', res.message || `The Allternit app answered ${res.code || res.status}.`);
}

/** Tools that need a finger on the screen, and what to say instead. */
function notOnAppPhone(name: string): string | null {
  switch (name) {
    case 'phone.tap':
    case 'phone.swipe':
    case 'phone.find_and_tap':
    case 'phone.home':
    case 'phone.back':
    case 'phone.key':
    case 'phone.type':
    case 'phone.open_app':
      return `${APP_TAPS_MESSAGE}. On this iPhone a bot can see the screen (phone.screenshot) and use the Allternit app's browser (phone.web.*).`;
    case 'phone.read_ui':
    case 'phone.screen_text':
      return 'Reading other apps as text needs the Bluetooth board. Use phone.screenshot to see the screen, or phone.web.snapshot for the Allternit app\'s browser.';
    case 'phone.task':
    case 'phone.run_skill':
      return `${APP_TAPS_MESSAGE}, so plain-English tasks and skills can't run on this iPhone yet. Use phone.web.* for web tasks in the Allternit app's browser.`;
    case 'phone.sms.send':
    case 'phone.sms.read':
    case 'phone.call.dial':
      return 'Texts and calls need an Android phone (adb or Phone Link). An iPhone doesn\'t let apps send or read texts.';
    default:
      return null;
  }
}

interface Viewer {
  keeper: AppFrameKeeper;
  unsubscribe: () => void;
  startedHere: boolean;
  idle: ReturnType<typeof setTimeout> | null;
}

export class AppPhoneBackend implements UsbPhoneBackend {
  private rows: AppPhoneRow[] = [];
  private listedAt = 0;
  private listing: Promise<void> | null = null;
  private readonly viewers = new Map<string, Viewer>();
  private readonly now: () => number;

  constructor(private readonly deps: AppPhoneBackendDeps) {
    this.now = deps.now ?? Date.now;
  }

  /** Re-reads the node's list when it is older than a few seconds. Never throws. */
  refresh(force = false): Promise<void> {
    if (!force && this.now() - this.listedAt < LIST_TTL_MS) return Promise.resolve();
    this.listing ??= this.deps.node
      .listPhones()
      .then((rows) => {
        this.rows = rows.filter((r) => isAppSerial(r.serial));
        this.listedAt = this.now();
      })
      .catch((error) => {
        this.deps.log?.(`[app-phones] list failed: ${error instanceof Error ? error.message : String(error)}`);
      })
      .finally(() => {
        this.listing = null;
      });
    return this.listing;
  }

  /** Every paired app phone, sharing or not: phone.web.* work while the app is open even without the screen. */
  has(serial: string): boolean {
    return this.rows.some((r) => r.serial === serial);
  }

  /** App phones sharing their screen right now (the runner's "online"). */
  serials(): string[] {
    return this.rows.filter((r) => r.online).map((r) => r.serial);
  }

  row(serial: string): AppPhoneRow | undefined {
    return this.rows.find((r) => r.serial === serial);
  }

  devices(): AppPhoneDevice[] {
    return this.rows.map((r) => ({
      serial: r.serial,
      platform: 'ios',
      via: 'allternit-app',
      route: APP_PHONE_ROUTE,
      name: r.name || 'iPhone',
      screen: r.online,
      note: r.online
        ? `Screen only: phone.screenshot and phone.web.* work; ${APP_TAPS_MESSAGE.toLowerCase()}.`
        : `${APP_SHARE_SCREEN_MESSAGE} phone.web.* work while the Allternit app is open.`,
    }));
  }

  decidePair(phoneId: string, approve: boolean): Promise<{ ok: boolean; message?: string }> {
    return this.deps.node.decidePair(phoneId, approve);
  }

  async call(name: string, serial: string, args: Record<string, unknown>): Promise<ToolResult> {
    if (name === 'phone.screenshot') return this.screenshot(serial);
    if (name === 'phone.screen.request') return this.screenRequest(serial);
    if (isWebTool(name)) return this.web(name, serial, args);
    const instead = notOnAppPhone(name);
    return fail('not_supported', instead ?? `${name} is not available on an iPhone connected with the Allternit app.`);
  }

  private async web(name: string, serial: string, args: Record<string, unknown>): Promise<ToolResult> {
    const rpc = webRpc(name, args);
    if ('ok' in rpc) return rpc;
    const timeout = rpc.method === 'web.wait' ? (rpc.params.ms as number) + RPC_TIMEOUT_MS : RPC_TIMEOUT_MS;
    let res: AppRpcResult;
    try {
      res = await this.deps.node.rpc(appPhoneId(serial), rpc.method, rpc.params, timeout);
    } catch (error) {
      return fail('device_offline', `Allternit Desktop's local API didn't answer (${error instanceof Error ? error.message : String(error)}).`);
    }
    if (!res.ok) return appError(res);
    const result = res.result;
    if (rpc.method === 'web.snapshot') {
      const { jpegBase64, ...rest } = result as { jpegBase64?: unknown };
      return {
        ok: true,
        ...rest,
        ...(typeof jpegBase64 === 'string' && jpegBase64 ? { mimeType: 'image/jpeg', dataBase64: jpegBase64 } : {}),
        note: 'Element refs are valid only until the next snapshot. Bounds are page points of this picture.',
      };
    }
    return { ...result, ok: true };
  }

  /** phone.screen.request: the app shows the Share screen sheet; only the person can start it. */
  private async screenRequest(serial: string): Promise<ToolResult> {
    let res: AppRpcResult;
    try {
      res = await this.deps.node.rpc(appPhoneId(serial), 'screen.prompt', {}, RPC_TIMEOUT_MS);
    } catch (error) {
      return fail('device_offline', `Allternit Desktop's local API didn't answer (${error instanceof Error ? error.message : String(error)}).`);
    }
    if (!res.ok) return appError(res);
    const shown = res.result.shown === true;
    const live = res.result.live === true;
    if (live) void this.refresh(true);
    return {
      ok: true,
      shown,
      live,
      note: live
        ? 'The screen is already shared: use phone.screenshot.'
        : shown
          ? 'The iPhone shows the Share screen sheet. Wait for the person to tap Start Broadcast, then call phone.screenshot.'
          : 'The Allternit app could not show the Share screen sheet. Ask the person to tap Share screen in the app.',
    };
  }

  /** The latest picture of the shared screen. Joins the stream when nobody watches yet (the node asks for a keyframe). */
  private async screenshot(serial: string): Promise<ToolResult> {
    if (!this.row(serial)?.online) return fail('no_screen', `${APP_SHARE_SCREEN_MESSAGE} phone.screen.request asks the person to start sharing.`);
    let viewer = this.viewers.get(serial);
    if (!viewer) {
      const keeper = new AppFrameKeeper(this.now);
      const wasLive = this.deps.streams.status(serial).state === 'live';
      const unsubscribe = this.deps.streams.subscribe(serial, keeper);
      viewer = { keeper, unsubscribe, startedHere: !wasLive, idle: null };
      this.viewers.set(serial, viewer);
      const started = await this.deps.streams.start(serial);
      if (!started.ok) {
        this.release(serial);
        return fail('no_screen', started.message || APP_SHARE_SCREEN_MESSAGE);
      }
    }
    this.armIdle(serial, viewer);
    await viewer.keeper.waitForFrame(this.deps.frameTimeoutMs ?? APP_FRAME_TIMEOUT_MS);
    const snap = viewer.keeper.snapshot();
    if (!snap) return fail('no_screen', `No picture from the iPhone yet. ${APP_SHARE_SCREEN_MESSAGE}`);
    try {
      const still = await this.deps.decodeStill(snap.input);
      return {
        ok: true,
        mimeType: 'image/jpeg',
        dataBase64: still.jpeg.toString('base64'),
        width: still.width,
        height: still.height,
        ageMs: Math.max(0, this.now() - snap.at),
        coordinate_space: 'none',
        note: `Screen only. ${APP_TAPS_MESSAGE}; use phone.web.* for the Allternit app's browser.`,
      };
    } catch (error) {
      return fail('no_screen', `Couldn't turn the iPhone's video into a picture (${error instanceof Error ? error.message : String(error)}).`);
    }
  }

  private armIdle(serial: string, viewer: Viewer): void {
    if (viewer.idle) clearTimeout(viewer.idle);
    viewer.idle = setTimeout(() => this.release(serial), this.deps.viewerIdleMs ?? APP_VIEWER_IDLE_MS);
    (viewer.idle as { unref?: () => void }).unref?.();
  }

  /** Detach; stop the stream only if this lane started it and nobody else watches (the extension pauses then). */
  private release(serial: string): void {
    const viewer = this.viewers.get(serial);
    if (!viewer) return;
    this.viewers.delete(serial);
    if (viewer.idle) clearTimeout(viewer.idle);
    viewer.unsubscribe();
    if (viewer.startedHere && this.deps.streams.consumerCount(serial) === 0) void this.deps.streams.stop(serial);
  }

  dispose(): void {
    for (const serial of [...this.viewers.keys()]) this.release(serial);
  }
}

// ── The real node client ────────────────────────────────────────────────────

export interface HttpAppPhoneNodeDeps {
  /** This Desktop's allternit-api, http base without a trailing slash. */
  apiUrl: () => string;
  /** The desktop credential plus the signed-in user's headers (as the phone-link socket sends). */
  headers: () => Record<string, string>;
  fetchImpl?: typeof fetch;
  createSocket?: (url: string, headers: Record<string, string>) => AppViewerSocket;
}

async function errorOf(res: Response): Promise<{ code: string; message: string }> {
  const body = (await res.json().catch(() => null)) as { error?: unknown; code?: unknown; message?: unknown } | null;
  const nested = body?.error && typeof body.error === 'object' ? (body.error as { code?: unknown; message?: unknown }) : null;
  const code = String(nested?.code ?? (typeof body?.error === 'string' ? body.error : body?.code) ?? '');
  const message = String(nested?.message ?? body?.message ?? '');
  return { code, message };
}

export function httpAppPhoneNode(deps: HttpAppPhoneNodeDeps): AppPhoneNode {
  const doFetch = deps.fetchImpl ?? fetch;
  const post = (path: string, body: unknown, timeoutMs = 15_000) =>
    doFetch(`${deps.apiUrl()}${path}`, {
      method: 'POST',
      headers: { ...deps.headers(), 'Content-Type': 'application/json' },
      body: JSON.stringify(body),
      signal: AbortSignal.timeout(timeoutMs),
    });
  return {
    async listPhones() {
      const res = await doFetch(`${deps.apiUrl()}/api/v1/phones`, { headers: deps.headers(), signal: AbortSignal.timeout(10_000) });
      if (!res.ok) throw new Error(`GET /api/v1/phones answered ${res.status}`);
      const json = (await res.json().catch(() => ({}))) as { phones?: unknown };
      const rows = Array.isArray(json.phones) ? json.phones : [];
      return rows.flatMap((row): AppPhoneRow[] => {
        const r = row as { serial?: unknown; name?: unknown; model?: unknown; online?: unknown; screen?: unknown; route?: unknown };
        if (!isAppSerial(r.serial)) return [];
        if (r.route !== undefined && r.route !== APP_PHONE_ROUTE) return [];
        return [{
          serial: r.serial as string,
          name: typeof r.name === 'string' && r.name ? r.name : 'iPhone',
          model: typeof r.model === 'string' ? r.model : 'iPhone',
          // `screen` is the ingest socket; older rows only carry `online`.
          online: r.screen === undefined ? r.online === true : r.screen === true && r.online !== false,
        }];
      });
    },
    async rpc(phoneId, method, params, timeoutMs) {
      const res = await post(`/api/v1/phones/app/${encodeURIComponent(phoneId)}/rpc`, { method, params }, timeoutMs);
      if (!res.ok) {
        const { code, message } = await errorOf(res);
        return { ok: false, status: res.status, code, message };
      }
      const body = (await res.json().catch(() => ({}))) as Record<string, unknown>;
      // The node returns the app's result; an `{id, error}` reply comes back as 200 with error on some builds.
      if (body.error && typeof body.error === 'object') {
        const e = body.error as { code?: unknown; message?: unknown };
        return { ok: false, status: 200, code: String(e.code ?? ''), message: String(e.message ?? '') };
      }
      const result = body.result && typeof body.result === 'object' && !Array.isArray(body.result) ? (body.result as Record<string, unknown>) : body;
      return { ok: true, result };
    },
    async openViewer(serial) {
      const res = await post(`/api/v1/phones/${encodeURIComponent(serial)}/ws-token`, { readOnly: true });
      if (!res.ok) {
        const { message } = await errorOf(res);
        throw new Error(res.status === 404 || res.status === 409 ? APP_SHARE_SCREEN_MESSAGE : message || `The phone stream token was refused (${res.status}).`);
      }
      const { token } = (await res.json()) as { token?: unknown };
      if (typeof token !== 'string' || !token) throw new Error('The phone stream token response was unreadable.');
      const ws = deps.apiUrl().replace(/^http/, 'ws');
      const url = `${ws}/ws/phones/${encodeURIComponent(serial)}/stream?token=${encodeURIComponent(token)}`;
      const create = deps.createSocket ?? defaultViewerSocket;
      return create(url, deps.headers());
    },
    async decidePair(phoneId, approve) {
      const res = await post(`/api/v1/phones/app/pair/${encodeURIComponent(phoneId)}/decision`, { approve });
      if (res.ok) return { ok: true };
      const { message } = await errorOf(res);
      return { ok: false, message: res.status === 404 ? 'That pairing request has ended. Start pairing again in the Allternit app.' : message || `The pairing answer was refused (${res.status}).` };
    },
  };
}

function defaultViewerSocket(url: string, headers: Record<string, string>): AppViewerSocket {
  const ws = new WebSocket(url, { headers });
  return {
    send: (data) => {
      if (ws.readyState === WebSocket.OPEN) ws.send(data);
    },
    close: () => ws.close(),
    onMessage: (cb) => ws.on('message', (data: Buffer, isBinary: boolean) => cb(data, isBinary)),
    onClose: (cb) => ws.on('close', (code: number, reason: Buffer) => cb(code, reason.toString())),
    onError: (cb) => ws.on('error', cb),
  };
}
