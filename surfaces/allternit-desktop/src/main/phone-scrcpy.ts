/**
 * scrcpy wire protocol, pinned to server v5.0.1 — the video + input half of
 * the in-app linked-Android live view (the external scrcpy window in
 * phone-device-manager.ts stays a separate feature).
 *
 * Everything here is transport-agnostic and Electron-free: the adb and socket
 * adapters are injected, so the protocol is unit-tested against fakes with no
 * phone and no adb.
 *
 * Protocol sources followed (all at tag v5.0.1):
 *  - doc/develop.md — socket order (video, audio, control; audio is
 *    skipped when disabled), the dummy byte on the first forward-tunnel
 *    socket, the 64-byte device name, the u32 codec id, the 12-byte session
 *    packet (rotation/size changes) and the 12-byte media frame header.
 *  - app/src/control_msg.c + control_msg.h — control message serialization
 *    (byte layout for keycode/text/touch/scroll/back-or-screen-on/
 *    set-display-power/rotate-device; type ids from the enum).
 *  - app/src/device_msg.c + device_msg.h — device message deserialization
 *    (clipboard, ack-clipboard, uhid-output).
 *  - app/src/mouse_sdk.c — the touch/scroll semantics a mouse-driven client
 *    uses (pressure 1.0/0.0, action_button on down, buttons while pressed).
 *  - app/src/util/binary.h — float→fixed-point (u16fp = f * 2^16 clamped,
 *    i16fp = f * 2^15 clamped).
 *  - server/.../Server.java, Options.java, device/DesktopConnection.java,
 *    device/Streamer.java — launch args (version must equal the server's
 *    exactly), option defaults (audio defaults to on and must be disabled
 *    explicitly), the accept/meta order, the packet flag bits
 *    (PACKET_FLAG_SESSION = 1<<63, PACKET_FLAG_CONFIG = 1<<62,
 *    PACKET_FLAG_KEY_FRAME = 1<<61), writeDisableStream (audio codec id 0 =
 *    "could not capture, carry on with video", 1 = "configuration error")
 *    and fixOpusConfigPacket (the opus config packet is the bare OpusHead).
 *
 * Wire layout this implements (video socket, forward tunnel, audio=false):
 *   dummy byte (1, 0x00) → device name (64, UTF-8 NUL-padded) → codec id
 *   (u32, "h264" = 0x68323634) → [session packet (12) | media packet
 *   (12-byte header + Annex-B payload)]…
 * send_frame_meta stays enabled (it is the server's default) so media and
 * session packets can be demuxed even when TCP coalesces them into one read;
 * main strips the 12-byte frame headers (PTS/config/keyframe flags) and
 * forwards only Annex-B payloads to the renderer. Rotation/size changes
 * arrive as session packets on this same socket. The control socket carries
 * our control messages and, rarely, device messages from the server.
 *
 * Audio socket (opened between video and control when audio is on — the
 * default; scrcpy's own default keeps playback off the phone while it is
 * captured):
 *   codec id (u32: "opus" = 0x6f707573; 0 = disabled, e.g. Android < 11;
 *   1 = error) → [media packet (12-byte header + payload)]… — no session
 *   packets. The first packet carries the config flag (bit 62) and is the
 *   OpusHead → `audioMeta` (base64 description for WebCodecs); every other
 *   packet is one opus frame → `audioChunk`. Audio is strictly best-effort:
 *   a disabled/unknown codec, a corrupt packet, or an audio socket error or
 *   close only stops audio (logged); the video stream never ends because of it.
 */

import { execFile, spawn, type ChildProcess } from 'node:child_process';
import { createHash, randomBytes } from 'node:crypto';
import * as fs from 'node:fs';
import * as net from 'node:net';
import * as path from 'node:path';

// ── Pinned server ────────────────────────────────────────────────────────────

export const SCRCPY_VERSION = '5.0.1';
/** The first server arg is checked verbatim against BuildConfig.VERSION_NAME. */
export const SCRCPY_SERVER_VERSION_ARG = '5.0.1';
export const SCRCPY_SERVER_URL = 'https://github.com/Genymobile/scrcpy/releases/download/v5.0.1/scrcpy-server-v5.0.1';
export const SCRCPY_SERVER_SHA256 = '764eb6f79811d5211fe9df341120882ba9994c7a61b897d7bf3fb662e53bc536';
export const SCRCPY_SERVER_SIZE = 733_930;

/** Where the server jar lives on the phone (scrcpy's own "-manual" convention). */
export const DEVICE_SERVER_PATH = '/data/local/tmp/scrcpy-server-manual.jar';
/** scid=-1 → the server listens on localabstract:scrcpy. */
export const DEVICE_SOCKET_NAME = 'scrcpy';

const VIDEO_CODEC_H264 = 0x68323634; // "h264", u32 big-endian (Streamer.java)
export const AUDIO_CODEC_OPUS = 0x6f707573; // "opus" (AudioCodec.java)
const AUDIO_CODEC_DISABLED = 0; // writeDisableStream(false): no capture, video only
const AUDIO_CODEC_ERROR = 1; // writeDisableStream(true): configuration error
const OPUS_SAMPLE_RATE = 48_000; // opus always decodes at 48 kHz
const DEVICE_NAME_FIELD_LENGTH = 64;

const DUMMY_BYTE_LENGTH = 1;
const CODEC_ID_LENGTH = 4;
const SESSION_PACKET_LENGTH = 12;
const SESSION_PACKET_FLAG = 0x80000000; // bit 63 of the u64 header, as seen in its first u32
const CONFIG_PACKET_FLAG = 0x40000000; // bit 62 (PACKET_FLAG_CONFIG)
const FRAME_HEADER_LENGTH = 12;
const MEDIA_PACKET_MAX_SIZE = 8 * 1024 * 1024; // way above anything 5 Mbit/s 30 fps produces

// ── Renderer-facing contract (shared shapes; a React client builds against
//    the same names via window.allternit.phones) ────────────────────────────

export interface PhoneStreamOptions {
  maxSize?: number;
  videoBitrate?: number;
  maxFps?: number;
  /** Device audio (opus) alongside the video; default on. Android 11+ only — older phones stream video only. */
  audio?: boolean;
}

/**
 * Audio stream format, shared by both lanes and every surface (Desktop main ↔
 * renderer ↔ web relay). `description` is base64: the OpusHead for opus, the
 * AudioSpecificConfig for AAC — exactly WebCodecs AudioDecoderConfig.description.
 */
export interface PhoneAudioMeta {
  codec: 'opus' | 'aac';
  sampleRate: number;
  channels: number;
  description: string;
}

export interface PhoneStreamMeta {
  deviceName: string;
  width: number;
  height: number;
  codec: 'h264';
  /**
   * iPhone only: the video is the panel in portrait and the screen is turned
   * this far (90 = view it turned 90° counter-clockwise). Viewers draw it
   * upright and still send input in the video's own pixels. Absent = 0.
   */
  rotation?: 0 | 90 | 180 | 270;
}

export type PhoneStreamStatus =
  | { state: 'idle' }
  | { state: 'starting' }
  | { state: 'live'; meta: PhoneStreamMeta }
  | { state: 'ended'; reason: string }
  | { state: 'error'; message: string };

export type PhoneInputEvent =
  | { type: 'touch'; action: 'down' | 'move' | 'up'; pointerId: number; x: number; y: number; pressure?: number }
  | { type: 'key'; action: 'down' | 'up'; keyCode: number; metaState?: number }
  | { type: 'text'; text: string }
  | { type: 'scroll'; x: number; y: number; hScroll?: number; vScroll?: number }
  | { type: 'back' } | { type: 'home' } | { type: 'app_switch' } | { type: 'power' };

/** Server→client messages we surface; clipboard is parsed for stream resync but bots don't need it. */
export type ScrcpyDeviceMessage =
  | { type: 'size'; width: number; height: number; clientResized: boolean }
  | { type: 'clipboard'; text: string }
  | { type: 'ackClipboard'; sequence: bigint }
  | { type: 'uhidOutput'; id: number; data: Buffer };

// ── scrcpy-server binary (pinned, sha256-verified, cached like platform-tools) ──

export interface ScrcpyServerArchive {
  url: string;
  sha256: string;
  size: number;
}

export const SCRCPY_SERVER: ScrcpyServerArchive = {
  url: SCRCPY_SERVER_URL,
  sha256: SCRCPY_SERVER_SHA256,
  size: SCRCPY_SERVER_SIZE,
};

export function sha256Hex(data: Buffer): string {
  return createHash('sha256').update(data).digest('hex');
}

export function downloadedScrcpyServerPath(dataDir: string): string {
  return path.join(dataDir, 'scrcpy-server', SCRCPY_VERSION, 'scrcpy-server.jar');
}

export function verifyServerBinary(data: Buffer, archive: ScrcpyServerArchive = SCRCPY_SERVER): boolean {
  return data.length === archive.size && sha256Hex(data) === archive.sha256;
}

export interface EnsureScrcpyServerDeps {
  fetchImpl?: typeof fetch;
}

/** The local scrcpy-server binary, downloading and verifying it on first use. */
export async function ensureScrcpyServer(dataDir: string, deps: EnsureScrcpyServerDeps = {}): Promise<string> {
  const cached = downloadedScrcpyServerPath(dataDir);
  if (fs.existsSync(cached)) return cached;

  const response = await (deps.fetchImpl ?? fetch)(SCRCPY_SERVER.url);
  if (!response.ok) throw new Error(`Downloading scrcpy-server failed: HTTP ${response.status}`);
  const data = Buffer.from(await response.arrayBuffer());
  if (!verifyServerBinary(data)) {
    throw new Error('Downloaded scrcpy-server did not match the pinned checksum; refusing to install it.');
  }
  fs.mkdirSync(path.dirname(cached), { recursive: true });
  fs.writeFileSync(cached, data, { mode: 0o755 });
  return cached;
}

// ── Injected transport adapters ─────────────────────────────────────────────

/** A duplex byte stream — structurally satisfied by node:net.Socket and by test fakes. */
export interface ScrcpySocket {
  write(data: Buffer): unknown;
  destroy(): void;
  on(event: 'data', cb: (chunk: Buffer) => void): void;
  on(event: 'close', cb: () => void): void;
  on(event: 'error', cb: (err: Error) => void): void;
}

export interface ScrcpyShell {
  /** Resolves with the exit code when the remote process ends. */
  done: Promise<number | null>;
  /** Stops the remote app_process (killing the adb shell child, like the scrcpy client does). */
  kill(): void;
}

/** The narrow adb surface the session needs. */
export interface ScrcpyAdb {
  push(localPath: string, remotePath: string): Promise<void>;
  /** Start a long-lived `adb shell` running `command`; kill() stops the server on the phone. */
  shell(command: string): ScrcpyShell;
  forward(localTcpPort: number, remoteAbstract: string): Promise<void>;
  removeForward(localTcpPort: number): Promise<void>;
  remove(remotePath: string): Promise<void>;
}

export interface ScrcpyDeps {
  adb: ScrcpyAdb;
  serial: string;
  /** Local scrcpy-server binary; a promise is awaited on first use (the pinned download). */
  serverPath: string | Promise<string>;
  /** Opens a TCP connection to the forwarded local port. */
  connect: (port: number) => Promise<ScrcpySocket>;
  /** Fixed local port (tests); otherwise a free port is picked per session. */
  port?: number;
  allocatePort?: () => Promise<number>;
  log?: (message: string) => void;
  /** Bounds the initial header phase (push/forward/connect/meta). */
  startTimeoutMs?: number;
}

/** One raw scrcpy socket from the Phone Link Android helper (phone-link-stream.ts). */
export interface ScrcpyHelperStreamRequest {
  /** 'video' starts scrcpy-server with `args`; 'control' is the next socket of the same session. */
  kind: 'video' | 'control';
  scid: string;
  args?: string[];
}

/**
 * Helper mode: the phone's Phone Link app starts scrcpy-server itself (its
 * shell server, over the mesh) and hands back each socket already connected,
 * so Desktop does no adb push/forward/shell. Sockets are opened in scrcpy's
 * accept order: video, then audio (when on), then control.
 */
export interface ScrcpyHelperDeps {
  serial: string;
  openStream: (req: ScrcpyHelperStreamRequest) => Promise<ScrcpySocket>;
  /** Session id (8 hex, 31-bit); random by default. */
  newScid?: () => string;
  log?: (message: string) => void;
  /** Bounds the start (the phone may have to start its screen service first). */
  startTimeoutMs?: number;
}

export type ScrcpySessionDeps = ScrcpyDeps | ScrcpyHelperDeps;

function isHelperDeps(deps: ScrcpySessionDeps): deps is ScrcpyHelperDeps {
  return 'openStream' in deps;
}

/** scrcpy parses scid as a non-negative 31-bit hex int (Options.java), so the top bit stays clear. */
export function randomScid(): string {
  return (randomBytes(4).readUInt32BE(0) & 0x7fffffff).toString(16).padStart(8, '0');
}

// ── Control message serialization (control_msg.c, v5.0.1) ───────────────────

const CONTROL_MSG_TYPE_INJECT_KEYCODE = 0;
const CONTROL_MSG_TYPE_INJECT_TEXT = 1;
const CONTROL_MSG_TYPE_INJECT_TOUCH_EVENT = 2;
const CONTROL_MSG_TYPE_INJECT_SCROLL_EVENT = 3;
const CONTROL_MSG_TYPE_BACK_OR_SCREEN_ON = 4;
const CONTROL_MSG_TYPE_SET_DISPLAY_POWER = 10;

const KEYEVENT_ACTION_DOWN = 0;
const KEYEVENT_ACTION_UP = 1;
const MOTIONEVENT_ACTION_DOWN = 0;
const MOTIONEVENT_ACTION_UP = 1;
const MOTIONEVENT_ACTION_MOVE = 2;
const MOTION_EVENT_BUTTON_PRIMARY = 1;

const INJECT_TEXT_MAX_BYTES = 300; // SC_CONTROL_MSG_INJECT_TEXT_MAX_LENGTH
const DEVICE_MSG_MAX_SIZE = 1 << 18; // 256k, mirrors DEVICE_MSG_MAX_SIZE

// Android KeyEvent keycodes used by the named PhoneInputEvent shortcuts.
const KEYCODE_HOME = 3;
const KEYCODE_POWER = 26;
const KEYCODE_APP_SWITCH = 187;

function writeU16(out: Buffer, offset: number, value: number): void {
  out.writeUInt16BE(value & 0xffff, offset);
}

function writeU32(out: Buffer, offset: number, value: number): void {
  out.writeUInt32BE(value >>> 0, offset);
}

function writeU64(out: Buffer, offset: number, value: bigint): void {
  out.writeBigUInt64BE(value & 0xffffffffffffffffn, offset);
}

/** float in [0,1] → u16 fixed-point (sc_float_to_u16fp). */
export function floatToU16fp(value: number): number {
  const u = value * 0x10000;
  return Math.min(0xffff, Math.max(0, Math.trunc(u)));
}

/** float in [-1,1] → i16 fixed-point (sc_float_to_i16fp). */
export function floatToI16fp(value: number): number {
  const i = Math.trunc(value * 0x8000);
  return Math.min(0x7fff, Math.max(-0x8000, i));
}

/** Truncate to a byte boundary not splitting a UTF-8 sequence (sc_str_utf8_truncation_index). */
export function utf8TruncationIndex(bytes: Buffer, maxLen: number): number {
  if (bytes.length <= maxLen) return bytes.length;
  let len = maxLen;
  // A UTF-8 continuation byte has the top bits 10; back off to the lead byte.
  while (len > 0 && (bytes[len] & 0xc0) === 0x80) len -= 1;
  return len;
}

const assertInt = (name: string, value: number, min: number, max: number): void => {
  if (!Number.isFinite(value) || !Number.isInteger(value) || value < min || value > max) {
    throw new Error(`${name} must be an integer in [${min}, ${max}].`);
  }
};

/**
 * Encode one renderer input event into its scrcpy control message(s).
 * `videoSize` is the streamed video size (the touch/scroll coordinate space).
 * Throws on invalid input; the caller turns that into `{ ok: false, message }`.
 */
export function encodeControlMessage(event: PhoneInputEvent, videoSize: { width: number; height: number }): Buffer[] {
  switch (event.type) {
    case 'touch': {
      assertInt('x', event.x, 0, 0x7fffffff);
      assertInt('y', event.y, 0, 0x7fffffff);
      assertInt('pointerId', event.pointerId, 0, Number.MAX_SAFE_INTEGER);
      const action = event.action === 'down' ? MOTIONEVENT_ACTION_DOWN : event.action === 'up' ? MOTIONEVENT_ACTION_UP : MOTIONEVENT_ACTION_MOVE;
      const held = event.action !== 'up';
      const pressure = event.pressure === undefined ? (held ? 1 : 0) : event.pressure;
      if (!(pressure >= 0 && pressure <= 1)) throw new Error('pressure must be between 0 and 1.');
      // Mouse-style semantics (mouse_sdk.c): action_button and buttons are the
      // primary button while the pointer is down.
      const out = Buffer.alloc(32);
      out[0] = CONTROL_MSG_TYPE_INJECT_TOUCH_EVENT;
      out[1] = action;
      writeU64(out, 2, BigInt(event.pointerId));
      writeU32(out, 10, event.x);
      writeU32(out, 14, event.y);
      writeU16(out, 18, videoSize.width);
      writeU16(out, 20, videoSize.height);
      writeU16(out, 22, floatToU16fp(pressure));
      writeU32(out, 24, event.action === 'down' ? MOTION_EVENT_BUTTON_PRIMARY : 0);
      writeU32(out, 28, held ? MOTION_EVENT_BUTTON_PRIMARY : 0);
      return [out];
    }
    case 'key': {
      assertInt('keyCode', event.keyCode, 0, 0x7fffffff);
      const out = Buffer.alloc(14);
      out[0] = CONTROL_MSG_TYPE_INJECT_KEYCODE;
      out[1] = event.action === 'down' ? KEYEVENT_ACTION_DOWN : KEYEVENT_ACTION_UP;
      writeU32(out, 2, event.keyCode);
      writeU32(out, 6, 0); // repeat
      writeU32(out, 10, event.metaState ?? 0);
      return [out];
    }
    case 'text': {
      const bytes = Buffer.from(String(event.text ?? ''), 'utf8');
      const len = utf8TruncationIndex(bytes, INJECT_TEXT_MAX_BYTES);
      const out = Buffer.alloc(5 + len);
      out[0] = CONTROL_MSG_TYPE_INJECT_TEXT;
      writeU32(out, 1, len);
      bytes.copy(out, 5, 0, len);
      return [out];
    }
    case 'scroll': {
      assertInt('x', event.x, 0, 0x7fffffff);
      assertInt('y', event.y, 0, 0x7fffffff);
      const h = event.hScroll ?? 0;
      const v = event.vScroll ?? 0;
      if (!Number.isFinite(h) || !Number.isFinite(v)) throw new Error('scroll amounts must be finite numbers.');
      const out = Buffer.alloc(21);
      out[0] = CONTROL_MSG_TYPE_INJECT_SCROLL_EVENT;
      writeU32(out, 1, event.x);
      writeU32(out, 5, event.y);
      writeU16(out, 9, videoSize.width);
      writeU16(out, 11, videoSize.height);
      // The wire takes normalized values in [-1,1]; the client divides by 16.
      writeU16(out, 13, floatToI16fp(Math.max(-1, Math.min(1, h / 16))) & 0xffff);
      writeU16(out, 15, floatToI16fp(Math.max(-1, Math.min(1, v / 16))) & 0xffff);
      writeU32(out, 17, 0); // buttons: plain hover scroll
      return [out];
    }
    case 'back':
      // Server acts on ACTION_DOWN only; send the pair like a real key tap.
      return [Buffer.from([CONTROL_MSG_TYPE_BACK_OR_SCREEN_ON, KEYEVENT_ACTION_DOWN]), Buffer.from([CONTROL_MSG_TYPE_BACK_OR_SCREEN_ON, KEYEVENT_ACTION_UP])];
    case 'home':
      return keyTap(KEYCODE_HOME);
    case 'app_switch':
      return keyTap(KEYCODE_APP_SWITCH);
    case 'power':
      return keyTap(KEYCODE_POWER);
    default:
      throw new Error(`Unknown input event type: ${(event as { type: string }).type}`);
  }
}

function keyTap(keyCode: number): Buffer[] {
  const down = Buffer.alloc(14);
  down[0] = CONTROL_MSG_TYPE_INJECT_KEYCODE;
  down[1] = KEYEVENT_ACTION_DOWN;
  writeU32(down, 2, keyCode);
  const up = Buffer.from(down);
  up[1] = KEYEVENT_ACTION_UP;
  return [down, up];
}

/** SET_DISPLAY_POWER (control_msg.c: 2 bytes, payload 1 = on). */
export function encodeSetDisplayPower(on: boolean): Buffer {
  return Buffer.from([CONTROL_MSG_TYPE_SET_DISPLAY_POWER, on ? 1 : 0]);
}

// ── Device message + stream header parsing (device_msg.c / Streamer.java) ───

/** Parse the fixed 64-byte device name field (NUL-padded UTF-8). */
export function parseDeviceName(field: Buffer): string {
  if (field.length !== DEVICE_NAME_FIELD_LENGTH) throw new Error(`Device name field must be 64 bytes, got ${field.length}.`);
  let end = 0;
  while (end < field.length && field[end] !== 0) end += 1;
  return field.subarray(0, end).toString('utf8');
}

export interface ParsedSessionPacket {
  width: number;
  height: number;
  clientResized: boolean;
}

/** Parse a 12-byte session packet (MSB set on the first u32). */
export function parseSessionPacket(packet: Buffer): ParsedSessionPacket {
  if (packet.length !== SESSION_PACKET_LENGTH) throw new Error(`Session packet must be 12 bytes, got ${packet.length}.`);
  const flags = packet.readUInt32BE(0);
  if ((flags & SESSION_PACKET_FLAG) === 0) throw new Error('Not a session packet.');
  return {
    width: packet.readUInt32BE(4),
    height: packet.readUInt32BE(8),
    clientResized: (flags & 1) !== 0,
  };
}

export type DeviceMessageParse =
  | { message: ScrcpyDeviceMessage; consumed: number }
  | { needMore: true }
  | { error: string };

/**
 * Incremental device message decode (control socket, server→client).
 * Mirrors sc_device_msg_deserialize(): 0 means "need more data".
 */
export function parseDeviceMessage(buf: Buffer): DeviceMessageParse {
  if (buf.length < 1) return { needMore: true };
  const type = buf[0];
  switch (type) {
    case 0: { // DEVICE_MSG_TYPE_CLIPBOARD
      if (buf.length < 5) return { needMore: true };
      const len = buf.readUInt32BE(1);
      if (len > DEVICE_MSG_MAX_SIZE) return { error: `Clipboard message too large (${len} bytes).` };
      if (buf.length < 5 + len) return { needMore: true };
      return { message: { type: 'clipboard', text: buf.subarray(5, 5 + len).toString('utf8') }, consumed: 5 + len };
    }
    case 1: { // DEVICE_MSG_TYPE_ACK_CLIPBOARD
      if (buf.length < 9) return { needMore: true };
      return { message: { type: 'ackClipboard', sequence: buf.readBigUInt64BE(1) }, consumed: 9 };
    }
    case 2: { // DEVICE_MSG_TYPE_UHID_OUTPUT
      if (buf.length < 5) return { needMore: true };
      const size = buf.readUInt16BE(3);
      if (size > DEVICE_MSG_MAX_SIZE) return { error: `UHID output too large (${size} bytes).` };
      if (buf.length < 5 + size) return { needMore: true };
      return { message: { type: 'uhidOutput', id: buf.readUInt16BE(1), data: Buffer.from(buf.subarray(5, 5 + size)) }, consumed: 5 + size };
    }
    default:
      return { error: `Unknown device message type: ${type}.` };
  }
}

// ── Audio stream demux (audio socket, Streamer.java v5.0.1) ─────────────────

/** Channels from an OpusHead (RFC 7845 §5.1: "OpusHead", version, channel count, …); stereo when unreadable. */
export function opusHeadChannels(head: Buffer): number {
  if (head.length >= 19 && head.subarray(0, 8).toString('latin1') === 'OpusHead' && head[9] > 0) return head[9];
  return 2;
}

export type ScrcpyAudioEvent =
  | { type: 'disabled'; reason: string }
  | { type: 'meta'; meta: PhoneAudioMeta }
  | { type: 'chunk'; data: Buffer };

/**
 * Incremental audio socket parser: u32 codec id, then 12-byte-header packets.
 * Once disabled (codec 0/1/unsupported, or a corrupt packet) it swallows
 * everything after — the server keeps writing until it is told to stop, and
 * we keep draining so it never blocks — and never throws.
 */
export class ScrcpyAudioDemuxer {
  private buf: Buffer = Buffer.alloc(0);
  private phase: 'codec' | 'stream' | 'disabled' = 'codec';

  get disabled(): boolean {
    return this.phase === 'disabled';
  }

  push(chunk: Buffer): ScrcpyAudioEvent[] {
    if (this.phase === 'disabled') return [];
    this.buf = this.buf.length ? Buffer.concat([this.buf, chunk]) : chunk;
    const out: ScrcpyAudioEvent[] = [];
    if (this.phase === 'codec') {
      if (this.buf.length < CODEC_ID_LENGTH) return out;
      const codecId = this.buf.readUInt32BE(0);
      this.buf = this.buf.subarray(CODEC_ID_LENGTH);
      if (codecId !== AUDIO_CODEC_OPUS) {
        const reason =
          codecId === AUDIO_CODEC_DISABLED
            ? 'the phone cannot capture audio (Android 11+ is needed)'
            : codecId === AUDIO_CODEC_ERROR
              ? 'the phone reported an audio configuration error'
              : `unsupported audio codec 0x${codecId.toString(16)}`;
        return this.disable(out, reason);
      }
      this.phase = 'stream';
    }
    for (;;) {
      if (this.buf.length < FRAME_HEADER_LENGTH) return out;
      const flags = this.buf.readUInt32BE(0);
      const size = this.buf.readUInt32BE(8);
      if ((flags & SESSION_PACKET_FLAG) !== 0 || size > MEDIA_PACKET_MAX_SIZE) {
        return this.disable(out, 'corrupt audio packet');
      }
      if (this.buf.length < FRAME_HEADER_LENGTH + size) return out;
      const payload = Buffer.from(this.buf.subarray(FRAME_HEADER_LENGTH, FRAME_HEADER_LENGTH + size));
      this.buf = this.buf.subarray(FRAME_HEADER_LENGTH + size);
      if ((flags & CONFIG_PACKET_FLAG) !== 0) {
        out.push({
          type: 'meta',
          meta: { codec: 'opus', sampleRate: OPUS_SAMPLE_RATE, channels: opusHeadChannels(payload), description: payload.toString('base64') },
        });
      } else if (size > 0) {
        out.push({ type: 'chunk', data: payload });
      }
    }
  }

  private disable(out: ScrcpyAudioEvent[], reason: string): ScrcpyAudioEvent[] {
    this.phase = 'disabled';
    this.buf = Buffer.alloc(0);
    out.push({ type: 'disabled', reason });
    return out;
  }
}

// ── The session ─────────────────────────────────────────────────────────────

export interface ScrcpySessionEvents {
  meta(meta: PhoneStreamMeta): void;
  videoChunk(chunk: Buffer): void;
  /** Opus stream format; may arrive before or after `meta`, and again if the encoder restarts. */
  audioMeta(meta: PhoneAudioMeta): void;
  /** One opus packet (no header). */
  audioChunk(chunk: Buffer): void;
  deviceMessage(message: ScrcpyDeviceMessage): void;
  ended(info: { reason: string }): void;
}

const DEFAULT_START_TIMEOUT_MS = 20_000;
/** Helper mode may first start the phone's screen service (up to ~30 s, phone-link-stream.ts). */
const HELPER_START_TIMEOUT_MS = 45_000;
const CONNECT_RETRY_MS = 250;
const CONNECT_RETRIES = 24; // ~6s: the server needs a moment after the shell spawns

function defaultAllocatePort(): Promise<number> {
  return new Promise((resolve, reject) => {
    const srv = net.createServer();
    srv.once('error', reject);
    srv.listen(0, '127.0.0.1', () => {
      const address = srv.address();
      const port = typeof address === 'object' && address ? address.port : 0;
      srv.close(() => (port ? resolve(port) : reject(new Error('Could not allocate a local port.'))));
    });
  });
}

export function buildServerCommand(options: PhoneStreamOptions): string {
  return [`CLASSPATH=${DEVICE_SERVER_PATH}`, 'app_process', '/', 'com.genymobile.scrcpy.Server', SCRCPY_SERVER_VERSION_ARG, ...serverOptions(options)].join(' ');
}

/**
 * Options the Phone Link helper sets itself and refuses from callers
 * (ShellServerProtocol.RESERVED: it owns the session id and tunnel mode).
 */
export const HELPER_RESERVED_OPTIONS: readonly string[] = ['scid', 'tunnel_forward', 'log_level'];

/**
 * The same server options the adb lane uses, minus the ones the helper
 * reserves, and without scrcpy's cleanup: cleanup deletes the server jar from
 * the phone at start (checked on a moto 2026-10-19), and on this lane nothing
 * pushes it again before the next stream, so only the first stream after
 * setup worked. With no cleanup to undo it, stay_awake stays off too; a
 * screen turned off from the frame is turned back on by close().
 */
export function helperServerArgs(options: PhoneStreamOptions): string[] {
  return serverOptions(options)
    .filter((arg) => !HELPER_RESERVED_OPTIONS.includes(arg.slice(0, arg.indexOf('='))))
    .map((arg) => (arg === 'cleanup=true' ? 'cleanup=false' : arg === 'stay_awake=true' ? 'stay_awake=false' : arg));
}

/** scrcpy-server key=value options after the version argument. */
function serverOptions(options: PhoneStreamOptions): string[] {
  const parts = [
    'tunnel_forward=true',
    // audio_dup stays at scrcpy's default (off): while the live view is open
    // the phone's sound plays here, not on the phone (Android 11+).
    ...(options.audio === false ? ['audio=false'] : ['audio=true', 'audio_codec=opus']),
    'video_codec=h264',
    // send_frame_meta stays on (default): the 12-byte headers let main
    // demux media from session packets even when TCP coalesces them, and
    // they are stripped before chunks reach the renderer.
    'log_level=info',
    'stay_awake=true',
    'cleanup=true',
    'clipboard_autosync=false',
  ];
  const maxSize = options.maxSize ?? 1080;
  if (maxSize > 0) parts.push(`max_size=${Math.trunc(maxSize)}`);
  const videoBitrate = options.videoBitrate ?? 5_000_000;
  if (videoBitrate > 0) parts.push(`video_bit_rate=${Math.trunc(videoBitrate)}`);
  const maxFps = options.maxFps ?? 30;
  if (maxFps > 0) parts.push(`max_fps=${maxFps}`);
  return parts;
}

/**
 * One scrcpy session: pushes the pinned server, starts it on the phone,
 * reads the video stream and writes input. Everything unrecoverable — phone
 * unplugged, version mismatch, adb failure, socket close — surfaces as a
 * single `ended` event with a reason; nothing throws asynchronously.
 */
export class ScrcpySession {
  private listeners: { [K in keyof ScrcpySessionEvents]?: Array<ScrcpySessionEvents[K]> } = {};
  private startPromise: Promise<void> | null = null;
  private metaResolve: (() => void) | null = null;
  private metaReject: ((error: Error) => void) | null = null;
  private readonly metaPromise: Promise<void>;
  private videoSocket: ScrcpySocket | null = null;
  private controlSocket: ScrcpySocket | null = null;
  private audioSocket: ScrcpySocket | null = null;
  private readonly audio = new ScrcpyAudioDemuxer();
  /** Set once audio stopped for this session (logged once); the socket keeps draining. */
  private audioOff = false;
  /** The first session packet, held until the control socket is attached. */
  private pendingLive: ParsedSessionPacket | null = null;
  private live = false;
  private endReason: string | null = null;
  private shell: ScrcpyShell | null = null;
  private ended = false;
  private intentionalClose = false;
  /** The screen was turned off from the frame; close() turns it back on. */
  private displayOff = false;
  private port = 0;
  private videoSize: { width: number; height: number } | null = null;
  private deviceName = '';
  private headerPhase: 'dummy' | 'deviceName' | 'codec' | 'stream' = 'dummy';
  private videoBuf: Buffer = Buffer.alloc(0);
  private controlBuf: Buffer = Buffer.alloc(0);
  private startTimer: ReturnType<typeof setTimeout> | null = null;

  constructor(
    private readonly deps: ScrcpySessionDeps,
    private readonly options: PhoneStreamOptions = {},
  ) {
    this.metaPromise = new Promise<void>((resolve, reject) => {
      this.metaResolve = resolve;
      this.metaReject = reject;
    });
    // An early dispose (start timeout) rejects this before run() awaits it;
    // the rejection also reaches the start() caller through run(), and this
    // branch keeps it from ever being an unhandled rejection.
    void this.metaPromise.catch(() => undefined);
  }

  on<K extends keyof ScrcpySessionEvents>(event: K, cb: ScrcpySessionEvents[K]): () => void {
    const list = (this.listeners[event] ??= []) as Array<ScrcpySessionEvents[K]>;
    list.push(cb);
    return () => {
      const i = list.indexOf(cb);
      if (i >= 0) list.splice(i, 1);
    };
  }

  private emit<K extends keyof ScrcpySessionEvents>(event: K, ...args: Parameters<ScrcpySessionEvents[K]>): void {
    for (const cb of this.listeners[event] ?? []) (cb as (...a: Parameters<ScrcpySessionEvents[K]>) => void)(...args);
  }

  private log(message: string): void {
    this.deps.log?.(`[scrcpy ${this.deps.serial}] ${message}`);
  }

  /** Starts the session; resolves once the stream is live (meta emitted), rejects on early failure. */
  start(): Promise<void> {
    this.startPromise ??= this.run().catch((error) => {
      this.dispose(`start failed: ${error instanceof Error ? error.message : String(error)}`);
      throw error;
    });
    return this.startPromise;
  }

  private async run(): Promise<void> {
    if (isHelperDeps(this.deps)) return this.runHelper(this.deps);
    const deps = this.deps;
    const { adb } = deps;
    this.armStartTimeout();

    const serverPath = await deps.serverPath;
    await adb.push(serverPath, DEVICE_SERVER_PATH);
    this.port = deps.port ?? (await (deps.allocatePort ?? defaultAllocatePort)());
    await adb.forward(this.port, `localabstract:${DEVICE_SOCKET_NAME}`);

    this.shell = adb.shell(buildServerCommand(this.options));
    this.shell.done.then((code) => {
      if (!this.intentionalClose && !this.ended) this.dispose(`the scrcpy server on the phone stopped (exit ${code ?? 'signal'})`);
    });

    await this.connectVideo(deps);
    this.log(`video socket connected on 127.0.0.1:${this.port}`);
    // The server sends the device name, codec and first session packet only
    // after it has accepted every socket (DesktopConnection.open, then
    // Server.java sendDeviceMeta), so the audio and control sockets connect
    // now, right after the dummy byte, in the server's accept order.
    if (this.options.audio !== false) {
      const audio = await this.connectWithRetry(deps);
      if (this.ended) {
        audio.destroy();
        throw new Error(this.endReason ?? 'The session was closed while connecting.');
      }
      this.audioSocket = audio;
      this.attachAudioSocket(audio);
    }
    const control = await this.connectWithRetry(deps);
    if (this.ended) {
      control.destroy();
      throw new Error(this.endReason ?? 'The session was closed while connecting.');
    }
    this.controlSocket = control;
    this.attachSocket(control, 'control');
    this.maybeGoLive();
    // Resolve start() only once the stream is live (meta emitted) — or reject
    // with the reason it failed, so callers get a real verdict.
    await this.metaPromise;
  }

  /**
   * Helper mode: the same socket order and parsing as the adb lane, but each
   * socket comes from the phone's Phone Link app, already connected to
   * scrcpy-server (no push, forward or shell here). The video socket still
   * starts with the forward-tunnel dummy byte.
   */
  private async runHelper(deps: ScrcpyHelperDeps): Promise<void> {
    this.armStartTimeout();
    const scid = (deps.newScid ?? randomScid)();
    const open = async (req: ScrcpyHelperStreamRequest): Promise<ScrcpySocket> => {
      if (this.ended) throw new Error(this.endReason ?? 'The session was closed while connecting.');
      const socket = await deps.openStream(req);
      if (this.ended) {
        socket.destroy();
        throw new Error(this.endReason ?? 'The session was closed while connecting.');
      }
      return socket;
    };
    const video = await open({ kind: 'video', scid, args: helperServerArgs(this.options) });
    this.videoSocket = video;
    const probe = await this.probeVideo(video);
    if (!probe.accepted) {
      this.videoSocket = null;
      video.destroy();
      throw new Error(this.ended ? (this.endReason ?? 'The session was closed while connecting.') : `The phone's scrcpy server didn't answer: ${probe.error}`);
    }
    this.log(`video socket open through Phone Link (scid ${scid})`);
    if (this.options.audio !== false) {
      const audio = await open({ kind: 'control', scid });
      this.audioSocket = audio;
      this.attachAudioSocket(audio);
    }
    const control = await open({ kind: 'control', scid });
    this.controlSocket = control;
    this.attachSocket(control, 'control');
    this.maybeGoLive();
    await this.metaPromise;
  }

  /**
   * Wait for the video socket's first byte (the dummy byte). Resolves
   * accepted=false when it errors or closes first; once accepted, the socket
   * feeds the video parser and its close/error end the session.
   */
  private probeVideo(socket: ScrcpySocket): Promise<{ accepted: boolean; error: string }> {
    return new Promise((resolve) => {
      let probing = true;
      socket.on('data', (chunk) => {
        if (probing) {
          probing = false;
          resolve({ accepted: true, error: '' });
        }
        this.onVideoData(chunk);
      });
      socket.on('error', (error) => {
        if (probing) {
          probing = false;
          resolve({ accepted: false, error: error.message });
          return;
        }
        this.dispose(`video socket error: ${error.message}`);
      });
      socket.on('close', () => {
        if (probing) {
          probing = false;
          resolve({ accepted: false, error: 'the connection closed before the server answered' });
          return;
        }
        if (!this.intentionalClose && !this.ended) this.dispose('the video stream closed (phone unplugged or server stopped)');
      });
    });
  }

  /**
   * Connect the video socket the way scrcpy's client does
   * (server.c connect_and_read_byte): through an adb forward the TCP connect
   * succeeds even while nothing listens on the phone yet, and adb then closes
   * it. Only the server's dummy byte proves the server accepted, so a close
   * before that byte is a retry, not the end of the stream.
   */
  private async connectVideo(deps: ScrcpyDeps): Promise<void> {
    let lastError = 'no attempt was made';
    for (let attempt = 0; attempt < CONNECT_RETRIES; attempt += 1) {
      if (this.ended) throw new Error(this.endReason ?? 'The session was closed while connecting.');
      let socket: ScrcpySocket;
      try {
        socket = await deps.connect(this.port);
      } catch (error) {
        lastError = error instanceof Error ? error.message : String(error);
        await new Promise((r) => setTimeout(r, CONNECT_RETRY_MS));
        continue;
      }
      // Held here so dispose() (start timeout) destroys a socket still waiting for its byte.
      this.videoSocket = socket;
      const probe = await this.probeVideo(socket);
      const accepted = probe.accepted;
      if (!accepted) lastError = probe.error;
      if (accepted) return;
      this.videoSocket = null;
      socket.destroy();
      if (this.ended) break;
      await new Promise((r) => setTimeout(r, CONNECT_RETRY_MS));
    }
    if (this.ended) throw new Error(this.endReason ?? 'The session was closed while connecting.');
    throw new Error(`Could not connect to the scrcpy server (127.0.0.1:${this.port}): ${lastError}`);
  }

  /** Connect + read the video header; called by the socket reader as bytes arrive. */
  private onVideoData(chunk: Buffer): void {
    if (this.ended) return;
    this.videoBuf = this.videoBuf.length ? Buffer.concat([this.videoBuf, chunk]) : chunk;
    try {
      this.consumeVideoBuffer();
    } catch (error) {
      this.dispose(error instanceof Error ? error.message : String(error));
    }
  }

  private consumeVideoBuffer(): void {
    // Fixed-size header fields first: dummy byte → device name → codec id.
    for (;;) {
      if (this.headerPhase === 'dummy') {
        if (this.videoBuf.length < DUMMY_BYTE_LENGTH) return;
        const dummy = this.videoBuf[0];
        this.videoBuf = this.videoBuf.subarray(DUMMY_BYTE_LENGTH);
        if (dummy !== 0) throw new Error(`Unexpected dummy byte 0x${dummy.toString(16)} from the scrcpy server.`);
        this.headerPhase = 'deviceName';
      } else if (this.headerPhase === 'deviceName') {
        if (this.videoBuf.length < DEVICE_NAME_FIELD_LENGTH) return;
        this.deviceName = parseDeviceName(this.videoBuf.subarray(0, DEVICE_NAME_FIELD_LENGTH));
        this.videoBuf = this.videoBuf.subarray(DEVICE_NAME_FIELD_LENGTH);
        this.headerPhase = 'codec';
      } else if (this.headerPhase === 'codec') {
        if (this.videoBuf.length < CODEC_ID_LENGTH) return;
        const codecId = this.videoBuf.readUInt32BE(0);
        this.videoBuf = this.videoBuf.subarray(CODEC_ID_LENGTH);
        if (codecId !== VIDEO_CODEC_H264) throw new Error(`The phone streams an unsupported video codec (0x${codecId.toString(16)}); only h264 is supported.`);
        this.headerPhase = 'stream';
        this.log(`device "${this.deviceName}", codec h264`);
      } else {
        break;
      }
    }

    // Streaming phase: 12-byte frame headers (MSB clear) with an explicit
    // payload size, and 12-byte session packets (MSB set) for rotation/size.
    for (;;) {
      if (this.videoBuf.length < FRAME_HEADER_LENGTH) return;
      const firstWord = this.videoBuf.readUInt32BE(0);
      if ((firstWord & SESSION_PACKET_FLAG) !== 0) {
        const packet = parseSessionPacket(this.videoBuf.subarray(0, SESSION_PACKET_LENGTH));
        this.videoBuf = this.videoBuf.subarray(SESSION_PACKET_LENGTH);
        this.onSessionPacket(packet);
        continue;
      }
      const size = this.videoBuf.readUInt32BE(8);
      if (size > MEDIA_PACKET_MAX_SIZE) throw new Error(`Media packet too large (${size} bytes).`);
      if (this.videoBuf.length < FRAME_HEADER_LENGTH + size) return;
      const payload = this.videoBuf.subarray(FRAME_HEADER_LENGTH, FRAME_HEADER_LENGTH + size);
      this.videoBuf = this.videoBuf.subarray(FRAME_HEADER_LENGTH + size);
      if (size > 0) this.emit('videoChunk', Buffer.from(payload));
    }
  }

  private onSessionPacket(packet: ParsedSessionPacket): void {
    this.videoSize = { width: packet.width, height: packet.height };
    if (!this.live && !this.pendingLive) {
      // First session packet: go live once the control socket is attached too.
      this.pendingLive = packet;
      this.maybeGoLive();
      return;
    }
    this.emit('deviceMessage', { type: 'size', width: packet.width, height: packet.height, clientResized: packet.clientResized });
  }

  private maybeGoLive(): void {
    const packet = this.pendingLive;
    if (this.ended || this.live || !packet || !this.controlSocket) return;
    this.pendingLive = null;
    this.live = true;
    this.clearStartTimeout();
    this.emit('meta', {
      deviceName: this.deviceName,
      width: packet.width,
      height: packet.height,
      codec: 'h264',
    });
    this.log(`live at ${packet.width}x${packet.height}`);
    this.metaResolve?.();
    this.metaResolve = null;
    this.metaReject = null;
  }

  private onControlData(chunk: Buffer): void {
    if (this.ended) return;
    this.controlBuf = this.controlBuf.length ? Buffer.concat([this.controlBuf, chunk]) : chunk;
    for (;;) {
      const parsed = parseDeviceMessage(this.controlBuf);
      if ('needMore' in parsed) return;
      if ('error' in parsed) {
        this.dispose(parsed.error);
        return;
      }
      this.controlBuf = this.controlBuf.subarray(parsed.consumed);
      this.emit('deviceMessage', parsed.message);
      if (this.controlBuf.length === 0) return;
    }
  }

  private attachSocket(socket: ScrcpySocket, kind: 'video' | 'control'): void {
    socket.on('data', (chunk) => (kind === 'video' ? this.onVideoData(chunk) : this.onControlData(chunk)));
    socket.on('error', (error) => this.dispose(`${kind} socket error: ${error.message}`));
    socket.on('close', () => {
      if (!this.intentionalClose && !this.ended) this.dispose(`the ${kind} stream closed (phone unplugged or server stopped)`);
    });
  }

  /** Audio is best-effort: nothing on this socket may end the session. */
  private attachAudioSocket(socket: ScrcpySocket): void {
    socket.on('data', (chunk) => this.onAudioData(chunk));
    socket.on('error', (error) => {
      if (!this.ended) this.stopAudio(`audio socket error: ${error.message}`);
    });
    socket.on('close', () => {
      if (!this.ended && !this.intentionalClose) this.stopAudio('the audio stream closed');
    });
  }

  private onAudioData(chunk: Buffer): void {
    if (this.ended || this.audioOff) return;
    try {
      for (const event of this.audio.push(chunk)) {
        if (event.type === 'disabled') this.stopAudio(event.reason);
        else if (event.type === 'meta') {
          this.log(`audio: opus ${event.meta.channels} ch`);
          this.emit('audioMeta', event.meta);
        } else this.emit('audioChunk', event.data);
      }
    } catch (error) {
      // A throwing audio listener must not reach the socket reader.
      this.stopAudio(`audio listener failed: ${error instanceof Error ? error.message : String(error)}`);
    }
  }

  /**
   * Stop producing audio for this session. The socket stays open and keeps
   * being drained (a full buffer would block the server's audio thread);
   * dispose() closes it with the others.
   */
  private stopAudio(reason: string): void {
    if (this.audioOff) return;
    this.audioOff = true;
    this.log(`audio off: ${reason}; video continues`);
  }

  private async connectWithRetry(deps: ScrcpyDeps): Promise<ScrcpySocket> {
    let lastError: unknown = null;
    for (let attempt = 0; attempt < CONNECT_RETRIES; attempt += 1) {
      if (this.ended) throw new Error(this.endReason ?? 'The session was closed while connecting.');
      try {
        return await deps.connect(this.port);
      } catch (error) {
        lastError = error;
        await new Promise((r) => setTimeout(r, CONNECT_RETRY_MS));
      }
    }
    throw new Error(`Could not connect to the scrcpy server (127.0.0.1:${this.port}): ${lastError instanceof Error ? lastError.message : String(lastError)}`);
  }

  private armStartTimeout(): void {
    this.clearStartTimeout();
    this.startTimer = setTimeout(() => {
      this.dispose('Timed out waiting for the phone to start streaming.');
    }, this.deps.startTimeoutMs ?? (isHelperDeps(this.deps) ? HELPER_START_TIMEOUT_MS : DEFAULT_START_TIMEOUT_MS));
    this.startTimer.unref?.();
  }

  private clearStartTimeout(): void {
    if (this.startTimer) clearTimeout(this.startTimer);
    this.startTimer = null;
  }

  /** Send input events. Invalid events are rejected before anything is written. */
  sendControl(events: PhoneInputEvent[]): { ok: boolean; message?: string } {
    if (this.ended || !this.controlSocket || !this.videoSize) return { ok: false, message: 'The stream is not live.' };
    try {
      const frames = events.map((event) => encodeControlMessage(event, this.videoSize!));
      for (const messages of frames) for (const message of messages) this.controlSocket.write(message);
      return { ok: true };
    } catch (error) {
      return { ok: false, message: error instanceof Error ? error.message : String(error) };
    }
  }

  setDisplayPower(on: boolean): { ok: boolean; message?: string } {
    if (this.ended || !this.controlSocket) return { ok: false, message: 'The stream is not live.' };
    this.controlSocket.write(encodeSetDisplayPower(on));
    this.displayOff = !on;
    return { ok: true };
  }

  /** Idempotent; emits `ended` exactly once with the reason. */
  close(reason = 'closed'): void {
    this.intentionalClose = true;
    // The helper lane runs without scrcpy's cleanup, which would otherwise
    // turn the phone's screen back on when the stream ends.
    if (this.displayOff && !this.ended && this.controlSocket) {
      try {
        this.controlSocket.write(encodeSetDisplayPower(true));
      } catch {
        /* the socket is going away anyway */
      }
    }
    this.dispose(reason);
  }

  private dispose(reason: string): void {
    if (this.ended) return;
    this.ended = true;
    this.endReason = reason;
    this.log(`ended: ${reason}`);
    this.clearStartTimeout();
    this.metaReject?.(new Error(reason));
    this.metaResolve = null;
    this.metaReject = null;
    try {
      this.videoSocket?.destroy();
    } catch {
      /* already gone */
    }
    try {
      this.controlSocket?.destroy();
    } catch {
      /* already gone */
    }
    try {
      this.audioSocket?.destroy();
    } catch {
      /* already gone */
    }
    try {
      this.shell?.kill();
    } catch {
      /* already gone */
    }
    if (!isHelperDeps(this.deps)) {
      void this.deps.adb.removeForward(this.port).catch(() => undefined);
      void this.deps.adb.remove(DEVICE_SERVER_PATH).catch(() => undefined);
    }
    this.emit('ended', { reason });
  }
}

// ── Real adb/socket adapter (node, still Electron-free) ─────────────────────

function runFile(command: string, args: string[], timeoutMs = 20_000): Promise<void> {
  return new Promise((resolve, reject) => {
    execFile(command, args, { timeout: timeoutMs, windowsHide: true, maxBuffer: 4 * 1024 * 1024 }, (error, _stdout, stderr) => {
      if (error) reject(new Error(`${args[0]} failed: ${(stderr || error.message).slice(0, 300)}`));
      else resolve();
    });
  });
}

/** The production ScrcpyAdb over an adb binary path (mirrors how scrcpy's client drives adb). */
export function nodeScrcpyAdb(adbPath: string, serial: string): ScrcpyAdb {
  const forSerial = (args: string[]) => ['-s', serial, ...args];
  return {
    push: (local, remote) => runFile(adbPath, forSerial(['push', local, remote]), 60_000),
    shell: (command) => {
      const child: ChildProcess = spawn(adbPath, forSerial(['shell']), { stdio: ['pipe', 'pipe', 'pipe'], windowsHide: true });
      child.stdin?.write(`${command}\n`);
      child.stderr?.resume();
      child.stdout?.resume();
      const done = new Promise<number | null>((resolve) => {
        child.once('exit', (code) => resolve(code));
        child.once('error', () => resolve(null));
      });
      return {
        done,
        kill: () => {
          // Killing the adb shell child tears down the remote app_process
          // (same mechanism the scrcpy client uses to stop its server).
          try {
            child.kill('SIGKILL');
          } catch {
            /* already exited */
          }
        },
      };
    },
    forward: async (port, remoteAbstract) => {
      await runFile(adbPath, forSerial(['forward', `tcp:${port}`, remoteAbstract]));
    },
    removeForward: async (port) => {
      await runFile(adbPath, forSerial(['forward', '--remove', `tcp:${port}`])).catch(() => undefined);
    },
    remove: async (remote) => {
      await runFile(adbPath, forSerial(['shell', 'rm', '-f', remote]), 10_000).catch(() => undefined);
    },
  };
}

/** The production socket factory: TCP to the forwarded adb port. */
export function nodeScrcpyConnect(port: number): Promise<ScrcpySocket> {
  return new Promise((resolve, reject) => {
    const socket = net.connect({ host: '127.0.0.1', port });
    const onError = (error: Error) => reject(error);
    socket.once('error', onError);
    socket.setNoDelay(true);
    socket.once('connect', () => {
      socket.removeListener('error', onError);
      resolve(socket);
    });
  });
}
