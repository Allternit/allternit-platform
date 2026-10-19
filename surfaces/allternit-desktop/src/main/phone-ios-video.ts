/**
 * iPhone live screen: real H.264 video from a linked iPhone, fed into the
 * same PhoneStreamManager → WebCodecs path the Android/scrcpy stream uses.
 *
 * The phone side (ictrl fork): a ReplayKit Broadcast Upload Extension
 * encodes the screen and hands it to the iCTRL runner over loopback; the
 * runner serves it on `GET /video?ticket=T` (port 8200, the ticket minted by
 * the sealed `stream_ticket` RPC). Both hops use one framing:
 *
 *   u8 type | u32 big-endian length | payload[length]
 *
 *   0 STATUS  JSON {"broadcasting":bool}          (also a 2 s keepalive)
 *   1 CONFIG  JSON {"width","height","scale","rotation"}
 *   2 FRAME   one H.264 access unit, Annex-B, SPS/PPS in-band on every IDR
 *   3 ENDED   JSON {"reason"} — the broadcast stopped; the socket stays open
 *   4 AUDIO_CONFIG JSON {"codec":"aac","sampleRate","channels","asc":"<base64>"}
 *             — the app-audio format (AAC-LC AudioSpecificConfig); resent
 *             after CONFIG for each subscriber and whenever it changes
 *   5 AUDIO   one raw AAC access unit (no ADTS)
 *
 * Audio is best-effort: a malformed AUDIO_CONFIG is skipped, AUDIO before a
 * valid AUDIO_CONFIG or before live is dropped, and nothing audio-related
 * ends the session. Older helpers never send 4/5 → video only.
 *
 * iOS needs a person to start a broadcast once each time. On a fresh
 * connection that reports "not broadcasting" this asks the runner to try
 * (`start_live_screen`), then waits up to a minute for the first CONFIG —
 * the user may have to tap "Start live screen" on the phone.
 *
 * Input: renderer touch events (video pixels) become runner gestures in
 * UIKit points (pixels / scale): tap, long_press, drag; scroll → a short
 * swipe; back → a left-edge swipe. One RPC at a time, in order.
 *
 * Everything phone-facing is injected (deps), so the parser, the gesture
 * mapper and the session are unit-tested without an iPhone.
 */

import * as net from 'node:net';
import type { Duplex } from 'node:stream';
import { ICTRL_PORT } from './ictrl-client.js';
import type { PhoneAudioMeta, PhoneInputEvent, PhoneStreamMeta, ScrcpyDeviceMessage } from './phone-scrcpy.js';
import type { StreamSessionHandle } from './phone-stream.js';

// ── Packet framing ──────────────────────────────────────────────────────────

export const IOS_PACKET_STATUS = 0;
export const IOS_PACKET_CONFIG = 1;
export const IOS_PACKET_FRAME = 2;
export const IOS_PACKET_ENDED = 3;
export const IOS_PACKET_AUDIO_CONFIG = 4;
export const IOS_PACKET_AUDIO = 5;

const PACKET_HEADER_LENGTH = 5;
const PACKET_MAX_SIZE = 8 * 1024 * 1024; // far above a 4 Mbit/s IDR
const HTTP_HEAD_MAX = 16 * 1024;

export interface IosVideoConfig {
  width: number;
  height: number;
  /** Pixels per UIKit point. */
  scale: number;
  rotation: 0 | 90 | 180 | 270;
}

export type IosVideoPacket =
  | { type: 'status'; broadcasting: boolean }
  | ({ type: 'config' } & IosVideoConfig)
  | { type: 'frame'; data: Buffer }
  | { type: 'ended'; reason: string }
  | { type: 'audioConfig'; meta: PhoneAudioMeta }
  | { type: 'audio'; data: Buffer };

/** Build one packet (tests, and anything that ever needs to speak the format). */
export function encodeIosVideoPacket(type: number, payload: Buffer | string | object): Buffer {
  const body = Buffer.isBuffer(payload) ? payload : Buffer.from(typeof payload === 'string' ? payload : JSON.stringify(payload), 'utf8');
  const head = Buffer.alloc(PACKET_HEADER_LENGTH);
  head[0] = type;
  head.writeUInt32BE(body.length, 1);
  return Buffer.concat([head, body]);
}

function jsonOf(payload: Buffer): Record<string, unknown> | null {
  try {
    const value = JSON.parse(payload.toString('utf8'));
    return value && typeof value === 'object' && !Array.isArray(value) ? (value as Record<string, unknown>) : null;
  } catch {
    return null;
  }
}

/** One decoded packet, or null for a malformed/unknown one (skipped, never fatal). */
export function decodeIosVideoPacket(type: number, payload: Buffer): IosVideoPacket | null {
  switch (type) {
    case IOS_PACKET_STATUS: {
      const json = jsonOf(payload);
      return json ? { type: 'status', broadcasting: json.broadcasting === true } : null;
    }
    case IOS_PACKET_CONFIG: {
      const json = jsonOf(payload);
      const width = json?.width;
      const height = json?.height;
      if (!Number.isInteger(width) || !Number.isInteger(height) || (width as number) <= 0 || (height as number) <= 0) return null;
      const scale = typeof json?.scale === 'number' && json.scale > 0 ? json.scale : 1;
      const rotation = json?.rotation === 90 || json?.rotation === 180 || json?.rotation === 270 ? json.rotation : 0;
      return { type: 'config', width: width as number, height: height as number, scale, rotation };
    }
    case IOS_PACKET_FRAME:
      return payload.length > 0 ? { type: 'frame', data: payload } : null;
    case IOS_PACKET_ENDED: {
      const json = jsonOf(payload);
      const reason = typeof json?.reason === 'string' && json.reason.trim() ? json.reason.trim() : 'the iPhone stopped the live screen';
      return { type: 'ended', reason };
    }
    case IOS_PACKET_AUDIO_CONFIG: {
      const json = jsonOf(payload);
      const sampleRate = json?.sampleRate;
      const channels = json?.channels;
      const asc = typeof json?.asc === 'string' ? json.asc : '';
      if (json?.codec !== 'aac' || !Number.isInteger(sampleRate) || (sampleRate as number) <= 0) return null;
      if (!Number.isInteger(channels) || (channels as number) <= 0 || (channels as number) > 8) return null;
      // The AudioSpecificConfig is 2+ bytes; re-encode so the description is canonical base64.
      const ascBytes = Buffer.from(asc, 'base64');
      if (ascBytes.length < 2) return null;
      return {
        type: 'audioConfig',
        meta: { codec: 'aac', sampleRate: sampleRate as number, channels: channels as number, description: ascBytes.toString('base64') },
      };
    }
    case IOS_PACKET_AUDIO:
      return payload.length > 0 ? { type: 'audio', data: payload } : null;
    default:
      return null;
  }
}

/** Incremental parser: feed socket chunks in any split, get whole packets out. */
export class IosVideoPacketParser {
  private buf: Buffer = Buffer.alloc(0);

  /** Drop a partial packet (a new connection starts on a packet boundary). */
  reset(): void {
    this.buf = Buffer.alloc(0);
  }

  /** Throws on a packet larger than the cap (a corrupt stream; the caller ends the session). */
  push(chunk: Buffer): IosVideoPacket[] {
    this.buf = this.buf.length ? Buffer.concat([this.buf, chunk]) : chunk;
    const out: IosVideoPacket[] = [];
    for (;;) {
      if (this.buf.length < PACKET_HEADER_LENGTH) break;
      const type = this.buf[0];
      const size = this.buf.readUInt32BE(1);
      if (size > PACKET_MAX_SIZE) throw new Error(`Video packet too large (${size} bytes).`);
      if (this.buf.length < PACKET_HEADER_LENGTH + size) break;
      // Copy out: the frame outlives this buffer (it is fanned to consumers).
      const payload = Buffer.from(this.buf.subarray(PACKET_HEADER_LENGTH, PACKET_HEADER_LENGTH + size));
      this.buf = this.buf.subarray(PACKET_HEADER_LENGTH + size);
      const packet = decodeIosVideoPacket(type, payload);
      if (packet) out.push(packet);
    }
    return out;
  }
}

/** The response head of `GET /video`, or null while it is still incomplete. */
export function parseHttpHead(buf: Buffer): { status: number; headers: Record<string, string>; rest: Buffer } | null {
  const end = buf.indexOf('\r\n\r\n');
  if (end < 0) {
    if (buf.length > HTTP_HEAD_MAX) throw new Error('The iPhone sent a malformed response.');
    return null;
  }
  const lines = buf.subarray(0, end).toString('latin1').split('\r\n');
  const match = /^HTTP\/1\.[01] (\d{3})/.exec(lines[0] ?? '');
  if (!match) throw new Error('The iPhone sent a malformed response.');
  const headers: Record<string, string> = {};
  for (const line of lines.slice(1)) {
    const i = line.indexOf(':');
    if (i > 0) headers[line.slice(0, i).trim().toLowerCase()] = line.slice(i + 1).trim();
  }
  return { status: Number(match[1]), headers, rest: buf.subarray(end + 4) };
}

// ── Gestures ────────────────────────────────────────────────────────────────

/** One runner RPC (ictrl-fork iCTRLUITests/ActionHandler.swift param names). */
export interface IosRpc {
  method: string;
  params: Record<string, unknown>;
}

export const TAP_SLOP_PX = 10;
export const LONG_PRESS_MS = 450;
const SCROLL_DISTANCE_PT = 120;
const NOT_ON_IPHONE = 'Not available on iPhone.';

const pt = (px: number, scale: number) => Math.round((px / scale) * 10) / 10;

/**
 * Turns the renderer's input events (the scrcpy-shaped PhoneInputEvent, in
 * video pixels) into runner gestures in points. Single-finger only: a second
 * pointer while one is down is ignored. Stateful across sendControl batches
 * (down and up usually arrive in different IPC calls).
 */
export class IosGestureMapper {
  private down: { pointerId: number; x: number; y: number; at: number; lastX: number; lastY: number } | null = null;

  map(event: PhoneInputEvent, screen: IosVideoConfig, now: number): { ok: true; rpcs: IosRpc[] } | { ok: false; message: string } {
    const s = screen.scale;
    switch (event.type) {
      case 'touch': {
        if (!Number.isFinite(event.x) || !Number.isFinite(event.y)) return { ok: false, message: 'x and y must be numbers.' };
        if (event.action === 'down') {
          if (!this.down) this.down = { pointerId: event.pointerId, x: event.x, y: event.y, at: now, lastX: event.x, lastY: event.y };
          return { ok: true, rpcs: [] };
        }
        const d = this.down;
        if (!d || d.pointerId !== event.pointerId) return { ok: true, rpcs: [] };
        d.lastX = event.x;
        d.lastY = event.y;
        if (event.action === 'move') return { ok: true, rpcs: [] };
        this.down = null;
        const moved = Math.hypot(event.x - d.x, event.y - d.y);
        const held = Math.max(0, now - d.at);
        if (moved < TAP_SLOP_PX && held < LONG_PRESS_MS) {
          return { ok: true, rpcs: [{ method: 'tap', params: { x: pt(d.x, s), y: pt(d.y, s) } }] };
        }
        if (moved < TAP_SLOP_PX) {
          return { ok: true, rpcs: [{ method: 'long_press', params: { x: pt(d.x, s), y: pt(d.y, s), duration: Math.round(held / 10) / 100 } }] };
        }
        // drag replays the finger's path over (about) the time it took, so a
        // quick flick keeps its momentum and a slow drag moves precisely.
        const duration = Math.min(5, Math.max(0.05, Math.round(held / 10) / 100));
        return {
          ok: true,
          rpcs: [{ method: 'drag', params: { from_x: pt(d.x, s), from_y: pt(d.y, s), to_x: pt(event.x, s), to_y: pt(event.y, s), duration } }],
        };
      }
      case 'scroll': {
        const v = event.vScroll ?? 0;
        const h = event.hScroll ?? 0;
        if (!v && !h) return { ok: true, rpcs: [] };
        // Wheel up (v > 0) shows earlier content: the finger moves down.
        const direction = Math.abs(v) >= Math.abs(h) ? (v > 0 ? 'down' : 'up') : h > 0 ? 'left' : 'right';
        return {
          ok: true,
          rpcs: [{ method: 'swipe', params: { x: pt(event.x, s), y: pt(event.y, s), direction, distance: SCROLL_DISTANCE_PT } }],
        };
      }
      case 'back': {
        // iOS has no back key: the edge swipe most apps honour.
        const w = screen.width / s;
        const y = Math.round(screen.height / s / 2);
        return { ok: true, rpcs: [{ method: 'drag', params: { from_x: 2, from_y: y, to_x: Math.round(w * 0.6), to_y: y, duration: 0.3 } }] };
      }
      case 'text':
        if (typeof event.text !== 'string' || !event.text) return { ok: false, message: 'text must be a non-empty string.' };
        return { ok: true, rpcs: [{ method: 'type', params: { text: event.text } }] };
      case 'home':
        return { ok: true, rpcs: [{ method: 'home', params: {} }] };
      case 'key':
      case 'app_switch':
      case 'power':
        return { ok: false, message: NOT_ON_IPHONE };
      default:
        return { ok: false, message: 'Unknown input event.' };
    }
  }
}

// ── The session ─────────────────────────────────────────────────────────────

export interface IosVideoDeps {
  /** One sealed RPC to the runner (PhoneLinkBackend.helperCall bound to the serial). */
  call(method: string, params: Record<string, unknown>, timeoutMs: number): Promise<unknown>;
  /** Where the runner is reachable right now: "ip:port" (LAN or mesh-proxy loopback). */
  host(): Promise<string | null>;
  /** Open the HTTP request; the default dials TCP and writes a plain GET. */
  connect?(host: string, path: string): Duplex;
  deviceName: string;
  log?(message: string): void;
  now?(): number;
  /** How long to wait for the first CONFIG (default 3 min: the user may tap Start Broadcast late). */
  startTimeoutMs?: number;
  /** Silence on a live connection that counts as lost (default 7 s; the runner sends STATUS every 2 s). */
  stallMs?: number;
  /** How long to keep reconnecting a lost live connection (default 45 s; a cable relaunch takes ~6 s). */
  reconnectForMs?: number;
}

interface IosVideoEvents {
  meta(meta: PhoneStreamMeta): void;
  videoChunk(chunk: Buffer): void;
  audioMeta(meta: PhoneAudioMeta): void;
  audioChunk(chunk: Buffer): void;
  deviceMessage(message: ScrcpyDeviceMessage): void;
  ended(info: { reason: string }): void;
}

export const IOS_START_TIMEOUT_MS = 180_000;
export const IOS_START_TIMEOUT_MESSAGE = 'Tap Start live screen on the iPhone, then try again.';
export const IOS_STALL_MS = 7_000;
export const IOS_RECONNECT_FOR_MS = 45_000;
const RECONNECT_EVERY_MS = 2_000;
export const IOS_HELPER_LOST_MESSAGE = 'The helper on your iPhone stopped. Plug the iPhone into this Mac to restart it, or open iCTRL › Start Server, then try again.';
const TICKET_TIMEOUT_MS = 5_000;
const START_LIVE_SCREEN_TIMEOUT_MS = 20_000;
const GESTURE_TIMEOUT_MS = 10_000;
const MAX_QUEUED_RPCS = 32;

/** Dial "ip:port" (or "[v6]:port") and send `GET path`; the response head is parsed by the session. */
export function nodeIosVideoConnect(host: string, path: string): Duplex {
  const m = /^\[([^\]]+)\](?::(\d+))?$/.exec(host) ?? /^([^:]+)(?::(\d+))?$/.exec(host);
  if (!m) throw new Error(`Bad iPhone address: ${host}`);
  const socket = net.connect({ host: m[1], port: m[2] ? Number(m[2]) : ICTRL_PORT });
  socket.setNoDelay(true);
  socket.write(`GET ${path} HTTP/1.1\r\nHost: ${host}\r\nAccept: application/x-ictrl-h264\r\nConnection: close\r\n\r\n`);
  return socket;
}

export class IosVideoSession implements StreamSessionHandle {
  private listeners: { [K in keyof IosVideoEvents]?: Array<IosVideoEvents[K]> } = {};
  private readonly parser = new IosVideoPacketParser();
  private readonly gestures = new IosGestureMapper();
  private startPromise: Promise<void> | null = null;
  private liveResolve: (() => void) | null = null;
  private liveReject: ((error: Error) => void) | null = null;
  private socket: Duplex | null = null;
  private head: Buffer | null = Buffer.alloc(0); // null once the HTTP head is parsed
  private errorBody: { status: number; body: Buffer } | null = null;
  private config: IosVideoConfig | null = null;
  private audioMeta: PhoneAudioMeta | null = null;
  private live = false;
  private ended = false;
  private askedToStart = false;
  private startTimer: ReturnType<typeof setTimeout> | null = null;
  /** Live: fires when the connection goes quiet (the runner's 2 s STATUS stopped). */
  private stallTimer: ReturnType<typeof setTimeout> | null = null;
  /** Live and reconnecting since this time (ms); null while connected. */
  private lostAt: number | null = null;
  private reconnectTimer: ReturnType<typeof setTimeout> | null = null;
  private queue: Promise<void> = Promise.resolve();
  private queued = 0;

  constructor(private readonly deps: IosVideoDeps) {}

  on<K extends keyof IosVideoEvents>(event: K, cb: IosVideoEvents[K]): () => void {
    const list = (this.listeners[event] ??= []) as Array<IosVideoEvents[K]>;
    list.push(cb);
    return () => {
      const i = list.indexOf(cb);
      if (i >= 0) list.splice(i, 1);
    };
  }

  private emit<K extends keyof IosVideoEvents>(event: K, ...args: Parameters<IosVideoEvents[K]>): void {
    for (const cb of this.listeners[event] ?? []) (cb as (...a: Parameters<IosVideoEvents[K]>) => void)(...args);
  }

  private log(message: string): void {
    this.deps.log?.(`[ios-video ${this.deps.deviceName}] ${message}`);
  }

  /** Resolves on the first CONFIG (meta emitted); rejects on failure or after the start timeout. */
  start(): Promise<void> {
    this.startPromise ??= this.run().catch((error) => {
      this.dispose(`start failed: ${error instanceof Error ? error.message : String(error)}`);
      throw error;
    });
    return this.startPromise;
  }

  private async run(): Promise<void> {
    const livePromise = new Promise<void>((resolve, reject) => {
      this.liveResolve = resolve;
      this.liveReject = reject;
    });
    // dispose() may reject this before the await below; never unhandled.
    void livePromise.catch(() => undefined);
    this.startTimer = setTimeout(() => this.failStart(new Error(IOS_START_TIMEOUT_MESSAGE)), this.deps.startTimeoutMs ?? IOS_START_TIMEOUT_MS);

    await this.openVideo();
    await livePromise;
  }

  /** Mint a ticket and open `/video`; every socket event is ignored once a newer socket replaced it. */
  private async openVideo(): Promise<void> {
    const minted = (await this.deps.call('stream_ticket', {}, TICKET_TIMEOUT_MS)) as { ticket?: unknown } | null;
    const ticket = typeof minted?.ticket === 'string' ? minted.ticket : '';
    if (!ticket) throw new Error('The iPhone did not hand out a video ticket.');
    if (this.ended) throw new Error('closed');
    const host = await this.deps.host();
    if (!host) throw new Error(`No address is known for ${this.deps.deviceName} yet.`);
    if (this.ended) throw new Error('closed');

    const socket = (this.deps.connect ?? nodeIosVideoConnect)(host, `/video?ticket=${encodeURIComponent(ticket)}`);
    this.socket = socket;
    this.head = Buffer.alloc(0);
    this.errorBody = null;
    this.parser.reset();
    const current = () => this.socket === socket;
    socket.on('data', (chunk: Buffer) => current() && this.onData(chunk));
    socket.on('error', (error: Error) => current() && this.onLost(`video socket error: ${error.message}`));
    socket.on('close', () => current() && this.onClose());
    socket.on('end', () => current() && this.onClose());
    this.log(`requesting /video from ${host}`);
    this.armStall();
  }

  /** Live only: no bytes for `stallMs` means the runner is gone (a killed process leaves the TCP socket open). */
  private armStall(): void {
    if (this.stallTimer) clearTimeout(this.stallTimer);
    this.stallTimer = null;
    if (!this.live || this.ended) return;
    this.stallTimer = setTimeout(() => this.onLost('no data from the iPhone'), this.deps.stallMs ?? IOS_STALL_MS);
  }

  /**
   * A live connection dropped or went quiet. Keep the stream live and reopen
   * `/video` every 2 s: the runner restarts (Desktop's recovery relaunches it
   * over the cable in ~6 s) while the broadcast extension keeps running and
   * reconnects to it, and the runner starts each new viewer at a keyframe.
   */
  private onLost(reason: string): void {
    if (this.ended) return;
    if (!this.live) return this.dispose(reason);
    if (this.stallTimer) clearTimeout(this.stallTimer);
    this.stallTimer = null;
    const old = this.socket;
    this.socket = null;
    try {
      old?.destroy();
    } catch {
      /* already gone */
    }
    if (this.lostAt === null) {
      this.lostAt = this.now();
      this.log(`connection lost (${reason}); reconnecting`);
    }
    if (this.now() - this.lostAt >= (this.deps.reconnectForMs ?? IOS_RECONNECT_FOR_MS)) return this.dispose(IOS_HELPER_LOST_MESSAGE);
    if (this.reconnectTimer) return;
    this.reconnectTimer = setTimeout(() => {
      this.reconnectTimer = null;
      if (this.ended) return;
      this.openVideo().catch((error) => this.onLost(error instanceof Error ? error.message : String(error)));
    }, RECONNECT_EVERY_MS);
  }

  private now(): number {
    return this.deps.now?.() ?? Date.now();
  }

  private failStart(error: Error): void {
    if (this.live || this.ended) return;
    this.liveReject?.(error);
    this.liveResolve = null;
    this.liveReject = null;
    this.dispose(error.message);
  }

  private onData(chunk: Buffer): void {
    if (this.ended) return;
    this.armStall();
    try {
      if (this.errorBody) {
        this.errorBody.body = Buffer.concat([this.errorBody.body, chunk]).subarray(0, 4096);
        return;
      }
      if (this.head) {
        this.head = Buffer.concat([this.head, chunk]);
        const parsed = parseHttpHead(this.head);
        if (!parsed) return;
        this.head = null;
        if (parsed.status !== 200) {
          // Connection: close — the body (the runner's reason) ends with the socket.
          this.errorBody = { status: parsed.status, body: parsed.rest };
          return;
        }
        if (parsed.rest.length === 0) return;
        chunk = parsed.rest;
      }
      for (const packet of this.parser.push(chunk)) {
        this.onPacket(packet);
        if (this.ended) return;
      }
    } catch (error) {
      this.onLost(error instanceof Error ? error.message : String(error));
    }
  }

  private onClose(): void {
    if (this.ended) return;
    if (this.errorBody) {
      const text = this.errorBody.body.toString('utf8').trim();
      const error = new Error(text || `The iPhone refused the video request (HTTP ${this.errorBody.status}).`);
      if (!this.live) return this.failStart(error);
      return this.onLost(error.message);
    }
    const reason = this.live ? 'the iPhone closed the video connection' : 'the iPhone closed the video connection before the live screen started';
    if (!this.live) return this.failStart(new Error(reason));
    this.onLost(reason);
  }

  private onPacket(packet: IosVideoPacket): void {
    switch (packet.type) {
      case 'status':
        if (!packet.broadcasting && !this.live && !this.askedToStart) {
          this.askedToStart = true;
          this.log('not broadcasting: asking the runner to start the live screen');
          // Best effort: if automation can't finish, the iCTRL screen asks the user to tap.
          this.deps.call('start_live_screen', {}, START_LIVE_SCREEN_TIMEOUT_MS).then(
            (res) => {
              this.log(`start_live_screen: ${JSON.stringify(res)}`);
              // Another recording holds the screen: waiting can't help, so say what to do.
              const r = res as { otherCapture?: unknown; message?: unknown } | null;
              if (r?.otherCapture === true) this.failStart(new Error(typeof r.message === 'string' ? r.message : 'Another screen recording is running on the iPhone.'));
            },
            (error) => this.log(`start_live_screen failed: ${error instanceof Error ? error.message : String(error)}`),
          );
        }
        return;
      case 'config': {
        const { type: _type, ...config } = packet;
        const previous = this.config;
        this.config = config;
        if (this.lostAt !== null) {
          this.log(`reconnected after ${this.now() - this.lostAt} ms`);
          this.lostAt = null;
        }
        if (!this.live) {
          this.live = true;
          this.armStall();
          if (this.startTimer) clearTimeout(this.startTimer);
          this.startTimer = null;
          this.emit('meta', { deviceName: this.deps.deviceName, width: config.width, height: config.height, codec: 'h264' });
          this.log(`live at ${config.width}x${config.height} (scale ${config.scale})`);
          this.liveResolve?.();
          this.liveResolve = null;
          this.liveReject = null;
          return;
        }
        if (previous && (previous.width !== config.width || previous.height !== config.height)) {
          this.emit('deviceMessage', { type: 'size', width: config.width, height: config.height, clientResized: false });
        }
        return;
      }
      case 'frame':
        if (this.live) this.emit('videoChunk', packet.data);
        return;
      case 'ended':
        // Before live this is a stale broadcast ending; keep waiting for a CONFIG.
        if (this.live) this.dispose(packet.reason);
        return;
      case 'audioConfig':
        this.audioMeta = packet.meta;
        this.emitAudio(() => this.emit('audioMeta', packet.meta));
        return;
      case 'audio':
        if (this.live && this.audioMeta) this.emitAudio(() => this.emit('audioChunk', packet.data));
        return;
    }
  }

  /** Audio listeners can never break the video reader. */
  private emitAudio(fn: () => void): void {
    try {
      fn();
    } catch (error) {
      this.log(`audio listener failed: ${error instanceof Error ? error.message : String(error)}`);
    }
  }

  /** Maps the events to runner gestures and queues them; answers before they run. */
  sendControl(events: PhoneInputEvent[]): { ok: boolean; message?: string } {
    const screen = this.config;
    if (this.ended || !this.live || !screen) return { ok: false, message: 'The stream is not live.' };
    const now = (this.deps.now ?? Date.now)();
    let failure: string | null = null;
    for (const event of events) {
      const mapped = this.gestures.map(event, screen, now);
      if (!mapped.ok) {
        failure ??= mapped.message;
        continue;
      }
      for (const rpc of mapped.rpcs) this.enqueue(rpc);
    }
    return failure ? { ok: false, message: failure } : { ok: true };
  }

  private enqueue(rpc: IosRpc): void {
    if (this.queued >= MAX_QUEUED_RPCS) {
      this.log(`input queue full; dropped ${rpc.method}`);
      return;
    }
    this.queued += 1;
    this.queue = this.queue.then(async () => {
      try {
        if (!this.ended) await this.deps.call(rpc.method, rpc.params, GESTURE_TIMEOUT_MS);
      } catch (error) {
        this.log(`${rpc.method} failed: ${error instanceof Error ? error.message : String(error)}`);
      } finally {
        this.queued -= 1;
      }
    });
  }

  setDisplayPower(_on: boolean): { ok: boolean; message?: string } {
    return { ok: false, message: NOT_ON_IPHONE };
  }

  /** Idempotent; emits `ended` exactly once with the reason. The broadcast keeps running on the phone. */
  close(reason = 'closed'): void {
    this.dispose(reason);
  }

  private dispose(reason: string): void {
    if (this.ended) return;
    this.ended = true;
    this.log(`ended: ${reason}`);
    if (this.startTimer) clearTimeout(this.startTimer);
    this.startTimer = null;
    if (this.stallTimer) clearTimeout(this.stallTimer);
    this.stallTimer = null;
    if (this.reconnectTimer) clearTimeout(this.reconnectTimer);
    this.reconnectTimer = null;
    this.liveReject?.(new Error(reason));
    this.liveResolve = null;
    this.liveReject = null;
    try {
      this.socket?.destroy();
    } catch {
      /* already gone */
    }
    this.emit('ended', { reason });
  }
}
