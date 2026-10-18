/**
 * Phone Link cloud session (Phones 2.0, Phase 2): the Desktop-side WebSocket
 * plane that lets any signed-in surface (ai.allternit.com phone layout,
 * m.allternit.com PWA) view and control a linked Android through this Desktop,
 * the same shape as the computer lane minus VNC.
 *
 * One socket per linked phone (`/ws/phone-link/session`, authed with the
 * desktop-access-token credential + the signed-in user's headers — see
 * cmd/allternit-api/src/phone_ws.rs). The relay tells us when a viewer
 * attaches/detaches; scrcpy starts on attach and idles off on detach. The
 * data plane is opaque to the relay:
 *
 *   Desktop → browser: JSON {type:'meta'|'end'} first/last, binary H.264
 *     Annex-B access units between (the WebCodecs client in allternit-ai
 *     decodes; see src/lib/phone-stream).
 *   Browser → Desktop: JSON {type:'input', events}, {type:'displayPower', on},
 *     {type:'ping'} (we answer pong).
 *
 * Electron-free and Node-testable: socket factory, clock and every service
 * hook are injected. phone-service.ts owns the lifecycle and the scrcpy
 * stream wiring.
 */

import WebSocket from 'ws';
import type { PhoneInputEvent, PhoneStreamMeta } from './phone-scrcpy.js';

export interface LinkedPhoneSummary {
  serial: string;
  model: string;
  name: string;
  /** Call audio on this phone (phone-call-bridge.ts); never device ids or labels. Absent = never set up. */
  callBridge?: { enabled: boolean; botName: string | null };
}

/** The relay's per-socket relay-level notifications (not part of the data plane). */
interface ViewerAttachedFrame {
  type: 'viewer_attached';
  serial: string;
}
interface ViewerDetachedFrame {
  type: 'viewer_detached';
  serial: string;
}

export interface PhoneLinkSocketDeps {
  apiUrl: () => string; // http(s) base, no trailing slash — dev points at 18013, prod at the cloud api
  /** Full auth header set for the connect request (desktop token + signed-in user headers). */
  headers: () => Record<string, string>;
  /** This socket's one phone; called at connect and on re-hello. */
  phone: () => LinkedPhoneSummary;
  onViewerAttached: (serial: string) => Promise<{ ok: boolean; message?: string }>;
  onViewerDetached: (serial: string) => void;
  onInput: (serial: string, events: PhoneInputEvent[]) => { ok: boolean; message?: string };
  onDisplayPower: (serial: string, on: boolean) => { ok: boolean; message?: string };
  createSocket?: (url: string, headers: Record<string, string>) => PhoneLinkSocket;
  log?: (message: string) => void;
  now?: () => number;
  retryDelaysMs?: number[];
}

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
const HEARTBEAT_INTERVAL_MS = 25_000; // relay pings at 30s; we ping first
const HELLO_DEBOUNCE_MS = 1_000;

/** What the service pushes onto the socket for the viewer (opaque data plane). */
export type PhoneLinkOutgoing =
  | { kind: 'meta'; meta: PhoneStreamMeta }
  | { kind: 'chunk'; chunk: Uint8Array }
  | { kind: 'end'; reason: string };

export class PhoneLinkPhoneSocket {
  private socket: PhoneLinkSocket | null = null;
  private stopped = false;
  private retryAttempt = 0;
  private retryTimer: ReturnType<typeof setTimeout> | null = null;
  private heartbeatTimer: ReturnType<typeof setInterval> | null = null;
  private helloTimer: ReturnType<typeof setTimeout> | null = null;
  private lastHelloJson = '';
  private viewers = 0;

  constructor(private readonly deps: PhoneLinkSocketDeps) {}

  /** Connect (or schedule the first retry). Idempotent while running. */
  start(): void {
    if (this.stopped) return;
    if (this.socket) return;
    if (this.retryTimer) return;
    void this.connect();
  }

  /** Permanent shutdown: no reconnect, viewer grace cancelled. */
  stop(): void {
    this.stopped = true;
    this.clearTimers();
    this.socket?.close(1_000, 'desktop shutdown');
    this.socket = null;
  }

  get running(): boolean {
    return !this.stopped;
  }

  get connected(): boolean {
    return this.socket?.readyState() === SOCKET_OPEN;
  }

  get viewerCount(): number {
    return this.viewers;
  }

  /** The service re-helloes when the phone's model/name or Call audio changes (identical hellos are skipped). */
  notePhoneChanged(): void {
    if (this.helloTimer) return;
    this.helloTimer = setTimeout(() => {
      this.helloTimer = null;
      this.sendHello();
    }, HELLO_DEBOUNCE_MS);
  }

  /** Data plane, service → viewer. */
  push(out: PhoneLinkOutgoing): void {
    if (this.socket?.readyState() !== SOCKET_OPEN) return;
    try {
      if (out.kind === 'chunk') {
        this.socket.send(out.chunk);
      } else if (out.kind === 'meta') {
        this.socket.send(JSON.stringify({ type: 'meta', ...out.meta }));
      } else {
        this.socket.send(JSON.stringify({ type: 'end', reason: out.reason }));
      }
    } catch {
      // The close handler schedules the reconnect.
    }
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
    const base = this.deps.apiUrl().replace(/\/$/, '');
    const url = `${base.replace(/^http/, 'ws')}/ws/phone-link/session`;
    const headers = this.deps.headers();
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
      this.lastHelloJson = '';
      this.sendHello();
      this.heartbeatTimer = setInterval(() => this.sendJson({ type: 'ping' }), HEARTBEAT_INTERVAL_MS);
    });
    socket.onMessage((data) => void this.handleMessage(data));
    socket.onClose((code, reason) => {
      if (this.socket !== socket) return;
      this.socket = null;
      this.clearTimers();
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

  private sendHello(): void {
    const phone = this.deps.phone();
    const json = JSON.stringify([phone]);
    if (json === this.lastHelloJson) return;
    this.lastHelloJson = json;
    this.sendJson({ type: 'hello', phones: [phone] });
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
      return; // binary frames desktop-ward are not expected
    }
    const serial = typeof frame.serial === 'string' ? frame.serial : this.deps.phone().serial;
    switch (frame.type) {
      case 'pong':
      case 'hello-ok':
        return;
      case 'ping':
        this.sendJson({ type: 'pong' });
        return;
      case 'viewer_attached': {
        this.viewers += 1;
        const result = await this.deps.onViewerAttached(serial);
        if (!result.ok) {
          // Tell the viewer why nothing is coming (opaque pass-through).
          this.push({ kind: 'end', reason: result.message ?? 'stream failed to start' });
        }
        return;
      }
      case 'viewer_detached': {
        this.viewers = Math.max(0, this.viewers - 1);
        // The service owns the scrcpy stop (it tracks viewer counts per
        // serial and grace-timers); we only report that this socket's last
        // viewer for the serial is gone.
        if (this.viewers === 0) this.deps.onViewerDetached(serial);
        return;
      }
      case 'input': {
        const events = Array.isArray(frame.events) ? (frame.events as PhoneInputEvent[]) : [];
        this.deps.onInput(serial, events);
        return;
      }
      case 'displayPower': {
        this.deps.onDisplayPower(serial, frame.on !== false);
        return;
      }
      default:
        return;
    }
  }
}

/** The real ws client (tests inject createSocket instead). Main is ESM, so no require(). */
function defaultSocket(url: string, headers: Record<string, string>): PhoneLinkSocket {
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
