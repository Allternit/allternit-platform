/**
 * Phone Link cloud session (Phones 2.0, Phase 2): the Desktop-side persistent
 * WebSocket to allternit-api that lets any signed-in surface (ai.allternit.com
 * phone layout, m.allternit.com PWA) reach the linked Android through this
 * Desktop, the same way the browser ⇄ api ⇄ Desktop path works for computers.
 *
 * This module is the transport only: reconnect with backoff, auth on connect,
 * a `hello` carrying the linked-phone inventory, heartbeat, and a message
 * dispatcher. Stream-control frame shapes are marked RELAY-RECONCILE — they
 * must match cmd/allternit-api/src/phone_ws.rs exactly (see
 * docs/PHONES_RELAY_NOTES.md in the relay lane); the video path attaches
 * through PhoneStreamManager consumers in phone-service.ts, never here.
 *
 * Electron-free and Node-testable: the socket factory, clock and session
 * provider are injected.
 */

import type { PhoneStreamOptions } from './phone-scrcpy.js';

export interface LinkedPhoneSummary {
  serial: string;
  model: string;
  name: string;
}

export interface PhoneLinkSessionInfo {
  accessToken: string;
  userId: string;
  runtimeId: string;
}

/** What the service gives us: linked phones now, and auth when signed in. */
export interface PhoneLinkCloudDeps {
  apiUrl: () => string; // http(s) base, no trailing slash — 127.0.0.1:18013 in dev, api.allternit.com in prod
  getSession: () => Promise<PhoneLinkSessionInfo | null>;
  phones: () => LinkedPhoneSummary[];
  /** Called for each stream-control frame from the relay. Return value is sent back as an ack when the frame asks for one. */
  onStreamControl: (frame: StreamControlFrame) => Promise<StreamControlAck>;
  log?: (message: string) => void;
  /** Test hook: override the WebSocket implementation. */
  createSocket?: (url: string, headers: Record<string, string>) => PhoneLinkSocket;
  now?: () => number;
  /** Backoff: 1s, 2s, 4s … capped at 30s, jittered. */
  retryDelaysMs?: number[];
}

/**
 * The relay → Desktop stream-control vocabulary.
 * RELAY-RECONCILE: shapes must mirror phone_ws.rs; the serial must appear on
 * every frame because one session multiplexes every linked phone.
 */
export type StreamControlFrame =
  | { type: 'start-stream'; serial: string; opts?: PhoneStreamOptions; requestId?: string }
  | { type: 'stop-stream'; serial: string; requestId?: string };

export type StreamControlAck = { ok: boolean; message?: string };

/** Minimal socket surface so tests can fake it (ws-style). */
export interface PhoneLinkSocket {
  send(data: string | Uint8Array): void;
  close(code?: number, reason?: string): void;
  onOpen(cb: () => void): void;
  onMessage(cb: (data: string | Buffer | Uint8Array) => void): void;
  onClose(cb: (code: number, reason: string) => void): void;
  onError(cb: (error: Error) => void): void;
  readyState(): number;
}

export const SOCKET_OPEN = 1;

const DEFAULT_RETRY_DELAYS_MS = [1_000, 2_000, 4_000, 8_000, 16_000, 30_000];
const HEARTBEAT_INTERVAL_MS = 25_000; // server pings at 30s; we ping first
const HELLO_DEBOUNCE_MS = 1_000;

export class PhoneLinkCloudClient {
  private socket: PhoneLinkSocket | null = null;
  private stopped = false;
  private retryAttempt = 0;
  private retryTimer: ReturnType<typeof setTimeout> | null = null;
  private heartbeatTimer: ReturnType<typeof setInterval> | null = null;
  private helloTimer: ReturnType<typeof setTimeout> | null = null;
  private lastPhonesJson = '';
  private pendingAcks = new Map<string, (ack: StreamControlAck) => void>();

  constructor(private readonly deps: PhoneLinkCloudDeps) {}

  /** Connect (or schedule the first retry). Idempotent while running. */
  start(): void {
    if (this.stopped) return;
    if (this.socket) return;
    if (this.retryTimer) return;
    void this.connect();
  }

  /** Permanent shutdown: no reconnect after this. */
  stop(): void {
    this.stopped = true;
    this.clearTimers();
    this.socket?.close(1_000, 'desktop shutdown');
    this.socket = null;
    for (const resolve of this.pendingAcks.values()) resolve({ ok: false, message: 'disconnected' });
    this.pendingAcks.clear();
  }

  get running(): boolean {
    return !this.stopped;
  }

  get connected(): boolean {
    return this.socket?.readyState() === SOCKET_OPEN;
  }

  /** The service calls this when the linked-phone list changes. */
  notePhonesChanged(): void {
    if (this.helloTimer) return;
    this.helloTimer = setTimeout(() => {
      this.helloTimer = null;
      this.sendHello();
    }, HELLO_DEBOUNCE_MS);
  }

  /** RELAY-RECONCILE: how acks ride back (requestId echo vs envelope). */
  sendAck(requestId: string | undefined, ack: StreamControlAck): void {
    if (!requestId) return;
    this.sendJson({ type: 'ack', requestId, ...ack });
  }

  private clearTimers(): void {
    if (this.retryTimer) clearTimeout(this.retryTimer);
    if (this.heartbeatTimer) clearInterval(this.heartbeatTimer);
    if (this.helloTimer) clearTimeout(this.helloTimer);
    this.retryTimer = null;
    this.heartbeatTimer = null;
    this.helloTimer = null;
  }

  private async connect(): Promise<void> {
    if (this.stopped) return;
    const session = await this.deps.getSession().catch(() => null);
    if (!session) {
      // Signed out (or the auth read failed): retry slowly until signed in.
      this.scheduleRetry();
      return;
    }
    const base = this.deps.apiUrl().replace(/\/$/, '');
    // RELAY-RECONCILE: the auth header scheme must match phone_ws.rs's
    // desktop-session gate. The device token is this Desktop's credential
    // (runtime-device pairing, auth-manager.ts).
    const headers = {
      authorization: `Bearer ${session.accessToken}`,
      'x-allternit-runtime': session.runtimeId,
    };
    const url = `${base.replace(/^http/, 'ws')}/ws/phone-link/session`;
    let socket: PhoneLinkSocket;
    try {
      socket = this.deps.createSocket ? this.deps.createSocket(url, headers) : defaultSocket(url, headers);
    } catch (error) {
      this.deps.log?.(`[PhoneLinkCloud] socket create failed: ${error instanceof Error ? error.message : String(error)}`);
      this.scheduleRetry();
      return;
    }
    this.socket = socket;
    socket.onOpen(() => {
      this.retryAttempt = 0;
      this.lastPhonesJson = '';
      this.sendHello();
      this.heartbeatTimer = setInterval(() => this.sendJson({ type: 'ping' }), HEARTBEAT_INTERVAL_MS);
    });
    socket.onMessage((data) => this.handleMessage(data));
    socket.onClose((code, reason) => {
      if (this.socket !== socket) return;
      this.socket = null;
      this.clearTimers();
      for (const resolve of this.pendingAcks.values()) resolve({ ok: false, message: `relay closed (${code})` });
      this.pendingAcks.clear();
      this.deps.log?.(`[PhoneLinkCloud] closed ${code} ${reason}; reconnecting`);
      this.scheduleRetry();
    });
    socket.onError((error) => {
      this.deps.log?.(`[PhoneLinkCloud] socket error: ${error.message}`);
    });
  }

  private scheduleRetry(): void {
    if (this.stopped || this.retryTimer) return;
    const delays = this.deps.retryDelaysMs ?? DEFAULT_RETRY_DELAYS_MS;
    const base = delays[Math.min(this.retryAttempt, delays.length - 1)];
    this.retryAttempt += 1;
    const jitter = base * 0.2 * Math.random();
    this.retryTimer = setTimeout(() => {
      this.retryTimer = null;
      void this.connect();
    }, base + jitter);
  }

  /** One hello per connection, and again when the phone inventory changes. */
  private sendHello(): void {
    const phones = this.deps.phones();
    const json = JSON.stringify(phones);
    if (json === this.lastPhonesJson) return;
    this.lastPhonesJson = json;
    this.sendJson({ type: 'hello', phones });
  }

  private sendJson(frame: Record<string, unknown>): void {
    if (this.socket?.readyState() !== SOCKET_OPEN) return;
    try {
      this.socket.send(JSON.stringify(frame));
    } catch {
      // The close handler schedules the reconnect.
    }
  }

  private async handleMessage(data: string | Buffer | Uint8Array): Promise<void> {
    const text = typeof data === 'string' ? data : Buffer.from(data).toString('utf8');
    let frame: Record<string, unknown>;
    try {
      frame = JSON.parse(text) as Record<string, unknown>;
    } catch {
      return; // binary frames are not expected desktop-ward in v1
    }
    if (frame.type === 'pong' || frame.type === 'hello-ok') return;
    if (frame.type === 'ack') {
      const requestId = typeof frame.requestId === 'string' ? frame.requestId : undefined;
      const resolve = requestId ? this.pendingAcks.get(requestId) : undefined;
      if (resolve) {
        this.pendingAcks.delete(requestId!);
        resolve({ ok: frame.ok !== false, message: typeof frame.message === 'string' ? frame.message : undefined });
      }
      return;
    }
    // RELAY-RECONCILE: stream-control dispatch — extend here with the exact
    // start/stop (and later apps/comms passthrough) shapes from phone_ws.rs.
    if (frame.type === 'start-stream' || frame.type === 'stop-stream') {
      const control = frame as unknown as StreamControlFrame & { requestId?: string };
      const ack = await this.deps.onStreamControl(control);
      this.sendAck(control.requestId, ack);
    }
  }
}

/** The real ws client (lazy import so tests never touch it). */
function defaultSocket(url: string, headers: Record<string, string>): PhoneLinkSocket {
  // eslint-disable-next-line @typescript-eslint/no-require-imports
  const WebSocket = require('ws') as typeof import('ws').WebSocket;
  const ws = new WebSocket(url, { headers });
  return {
    send: (data) => ws.send(data),
    close: (code, reason) => ws.close(code, reason),
    onOpen: (cb) => ws.on('open', cb),
    onMessage: (cb) => ws.on('message', cb),
    onClose: (cb) => ws.on('close', (code: number, reason: Buffer) => cb(code, reason.toString())),
    onError: (cb) => ws.on('error', cb),
    readyState: () => ws.readyState,
  };
}
