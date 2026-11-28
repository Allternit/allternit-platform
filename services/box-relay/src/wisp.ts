// Wisp v1 server side (the protocol unkernel's `js/core/net-wisp.ts` speaks).
//
// Packet: type u8 | stream id u32 LE | payload.
//   CONNECT  0x01  stream type u8 | port u16 LE | hostname (UTF-8, rest)
//   DATA     0x02  bytes
//   CONTINUE 0x03  buffer remaining u32 LE (server -> client flow control)
//   CLOSE    0x04  reason u8
//
// Wisp v1 has no half-close: when the remote end finishes, the whole stream
// closes (CLOSE 0x02), and a client CLOSE ends both directions after the
// writes already queued are flushed. No UDP, no ICMP.
//
// Everything here is plain TypeScript with the edges injected (TCP connect,
// DNS, usage storage, clock, log), so it runs under vitest without workerd.

import { checkDestination, isBlockedAddress, loggableHost } from "./allowlist";
import type { Resolver } from "./dns";

export const PacketType = { CONNECT: 0x01, DATA: 0x02, CONTINUE: 0x03, CLOSE: 0x04 } as const;
export const StreamType = { TCP: 0x01, UDP: 0x02 } as const;
export const CloseReason = {
  UNKNOWN: 0x01,
  VOLUNTARY: 0x02,
  NETWORK_ERROR: 0x03,
  INVALID_INFO: 0x41,
  UNREACHABLE: 0x42,
  CONNECT_TIMEOUT: 0x43,
  REFUSED: 0x44,
  BLOCKED: 0x48,
  THROTTLED: 0x49,
  CLIENT_ERROR: 0x81,
} as const;

/** Packets a client may send on one stream before it waits for CONTINUE. */
export const BUFFER_PACKETS = 128;

/** WebSocket close code when a newer socket for the same user replaces this one. */
export const WS_CLOSE_REPLACED = 4001;

/** Starting caps from contract section 9 (tuned at deploy time). */
export interface Limits {
  maxOpenStreams: number;
  maxNewStreamsPerMinute: number;
  maxBytesPerDay: number;
  maxSocketMs: number;
}

export const DEFAULT_LIMITS: Limits = {
  maxOpenStreams: 32,
  maxNewStreamsPerMinute: 100,
  maxBytesPerDay: 512 * 1024 * 1024,
  maxSocketMs: 2 * 60 * 60 * 1000,
};

// ─── Edges ──────────────────────────────────────────────────────────────────

export interface WsLike {
  send(data: Uint8Array): void;
  close(code?: number, reason?: string): void;
}

export interface SocketLike {
  readable: ReadableStream<Uint8Array>;
  writable: WritableStream<Uint8Array>;
  opened: Promise<unknown>;
  close(): Promise<void>;
}

export type ConnectFn = (
  address: { hostname: string; port: number },
  options: { secureTransport: "off"; allowHalfOpen: boolean },
) => SocketLike;

export interface Usage {
  day: string;
  bytes: number;
}

export interface UsageStore {
  load(): Promise<Usage | undefined>;
  save(usage: Usage): Promise<void>;
}

/** Counts and hostnames only. Never payload, never URLs, never tokens. */
export type LogFn = (event: Record<string, string | number>) => void;

export interface RelayDeps {
  connect: ConnectFn;
  resolve: Resolver;
  store: UsageStore;
  log: LogFn;
  now?: () => number;
  limits?: Partial<Limits>;
}

// ─── Packets ────────────────────────────────────────────────────────────────

export interface Packet {
  type: number;
  streamId: number;
  payload: Uint8Array;
}

export function encodePacket(type: number, streamId: number, payload: Uint8Array): Uint8Array {
  const out = new Uint8Array(5 + payload.byteLength);
  const view = new DataView(out.buffer);
  view.setUint8(0, type);
  view.setUint32(1, streamId, true);
  out.set(payload, 5);
  return out;
}

export function decodePacket(bytes: Uint8Array): Packet | null {
  if (bytes.byteLength < 5) return null;
  const view = new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength);
  return { type: view.getUint8(0), streamId: view.getUint32(1, true), payload: bytes.subarray(5) };
}

export function encodeConnect(streamId: number, streamType: number, port: number, host: string): Uint8Array {
  const name = new TextEncoder().encode(host);
  const payload = new Uint8Array(3 + name.byteLength);
  payload[0] = streamType;
  new DataView(payload.buffer).setUint16(1, port, true);
  payload.set(name, 3);
  return encodePacket(PacketType.CONNECT, streamId, payload);
}

export function encodeData(streamId: number, data: Uint8Array): Uint8Array {
  return encodePacket(PacketType.DATA, streamId, data);
}

export function encodeContinue(streamId: number, remaining: number): Uint8Array {
  const payload = new Uint8Array(4);
  new DataView(payload.buffer).setUint32(0, remaining, true);
  return encodePacket(PacketType.CONTINUE, streamId, payload);
}

export function encodeClose(streamId: number, reason: number): Uint8Array {
  return encodePacket(PacketType.CLOSE, streamId, Uint8Array.of(reason));
}

function toBytes(data: unknown): Uint8Array | null {
  if (data instanceof Uint8Array) return data;
  if (data instanceof ArrayBuffer) return new Uint8Array(data);
  if (ArrayBuffer.isView(data)) return new Uint8Array(data.buffer, data.byteOffset, data.byteLength);
  return null; // text frames are not Wisp
}

// ─── Per-user relay (one per Durable Object) ────────────────────────────────

const PERSIST_EVERY_BYTES = 8 * 1024 * 1024;

function utcDay(ms: number): string {
  return new Date(ms).toISOString().slice(0, 10);
}

export class UserRelay {
  readonly limits: Limits;
  readonly deps: RelayDeps;
  readonly now: () => number;
  private current: WispSession | undefined;
  private timer: ReturnType<typeof setTimeout> | undefined;
  private newStreamTimes: number[] = [];
  private usage: Usage;
  private savedBytes = 0;

  constructor(deps: RelayDeps) {
    this.deps = deps;
    this.now = deps.now ?? Date.now;
    this.limits = { ...DEFAULT_LIMITS, ...deps.limits };
    this.usage = { day: utcDay(this.now()), bytes: 0 };
  }

  async loadUsage(): Promise<void> {
    const stored = await this.deps.store.load();
    if (stored && stored.day === utcDay(this.now())) {
      this.usage = { day: stored.day, bytes: stored.bytes };
      this.savedBytes = stored.bytes;
    }
  }

  get bytesToday(): number {
    this.rollDay();
    return this.usage.bytes;
  }

  get session(): WispSession | undefined {
    return this.current;
  }

  /** One socket per user: a new one closes the old one with 4001. */
  open(ws: WsLike): WispSession {
    if (this.current) this.current.end(WS_CLOSE_REPLACED, "replaced by a newer connection");
    const session = new WispSession(ws, this);
    this.current = session;
    if (this.timer) clearTimeout(this.timer);
    this.timer = setTimeout(() => session.end(1000, "socket time limit"), this.limits.maxSocketMs);
    session.start();
    return session;
  }

  /** Stream caps: open at once, new per minute, and the daily byte budget. */
  admitStream(openNow: number): boolean {
    if (openNow >= this.limits.maxOpenStreams) return false;
    if (this.bytesToday >= this.limits.maxBytesPerDay) return false;
    const since = this.now() - 60_000;
    this.newStreamTimes = this.newStreamTimes.filter((t) => t > since);
    if (this.newStreamTimes.length >= this.limits.maxNewStreamsPerMinute) return false;
    this.newStreamTimes.push(this.now());
    return true;
  }

  /** Counts bytes in either direction. False once the day's budget is spent. */
  addBytes(n: number): boolean {
    this.rollDay();
    this.usage.bytes += n;
    if (this.usage.bytes - this.savedBytes >= PERSIST_EVERY_BYTES) this.persist();
    return this.usage.bytes <= this.limits.maxBytesPerDay;
  }

  sessionEnded(session: WispSession): void {
    if (this.current === session) {
      this.current = undefined;
      if (this.timer) clearTimeout(this.timer);
      this.timer = undefined;
    }
    this.persist();
  }

  persist(): void {
    const snapshot = { ...this.usage };
    this.savedBytes = snapshot.bytes;
    this.deps.store.save(snapshot).catch(() => {
      this.deps.log({ event: "usage_save_failed" });
    });
  }

  private rollDay(): void {
    const day = utcDay(this.now());
    if (day !== this.usage.day) {
      this.usage = { day, bytes: 0 };
      this.savedBytes = 0;
    }
  }
}

// ─── One WebSocket ──────────────────────────────────────────────────────────

interface Stream {
  id: number;
  host: string;
  socket?: SocketLike;
  writer?: WritableStreamDefaultWriter<Uint8Array>;
  /** Connect, then every client write, in order. Never rejects. */
  chain: Promise<void>;
  pending: number;
  writtenSinceContinue: number;
  /** No more client data accepted. */
  closed: boolean;
  /** Drop queued writes and close the socket now. */
  aborted: boolean;
  up: number;
  down: number;
}

export class WispSession {
  private readonly streams = new Map<number, Stream>();
  private readonly openedAt: number;
  private ended = false;

  constructor(
    private readonly ws: WsLike,
    private readonly relay: UserRelay,
  ) {
    this.openedAt = relay.now();
  }

  get openStreams(): number {
    return this.streams.size;
  }

  start(): void {
    // Stream 0 CONTINUE: the buffer size every new stream starts with.
    this.send(encodeContinue(0, BUFFER_PACKETS));
  }

  onMessage(data: unknown): void {
    if (this.ended) return;
    if (this.relay.now() - this.openedAt > this.relay.limits.maxSocketMs) {
      this.end(1000, "socket time limit");
      return;
    }
    const bytes = toBytes(data);
    if (!bytes) return;
    const packet = decodePacket(bytes);
    if (!packet) return;
    switch (packet.type) {
      case PacketType.CONNECT:
        this.handleConnect(packet.streamId, packet.payload);
        break;
      case PacketType.DATA:
        this.handleData(packet.streamId, packet.payload);
        break;
      case PacketType.CLOSE:
        this.handleClientClose(packet.streamId);
        break;
      default:
        // CONTINUE is server-to-client only; unknown types are ignored.
        break;
    }
  }

  /** Our side ends the socket (replaced, time limit). */
  end(code: number, reason: string): void {
    if (this.ended) return;
    this.teardown();
    try {
      this.ws.close(code, reason);
    } catch {
      // already closing
    }
  }

  /** The WebSocket closed or errored. */
  onSocketClosed(): void {
    if (this.ended) return;
    this.teardown();
  }

  private teardown(): void {
    this.ended = true;
    let up = 0;
    let down = 0;
    for (const s of [...this.streams.values()]) {
      up += s.up;
      down += s.down;
      this.closeStream(s.id, CloseReason.NETWORK_ERROR, { notify: false, abort: true });
    }
    this.relay.deps.log({ event: "socket_end", up, down });
    this.relay.sessionEnded(this);
  }

  private handleConnect(id: number, payload: Uint8Array): void {
    if (payload.byteLength < 3 || id === 0) {
      this.send(encodeClose(id, CloseReason.INVALID_INFO));
      return;
    }
    if (this.streams.has(id)) {
      this.closeStream(id, CloseReason.INVALID_INFO, { notify: true, abort: true });
      return;
    }
    const streamType = payload[0];
    const port = new DataView(payload.buffer, payload.byteOffset, payload.byteLength).getUint16(1, true);
    let host: string;
    try {
      host = new TextDecoder("utf-8", { fatal: true, ignoreBOM: true }).decode(payload.subarray(3));
    } catch {
      this.refuse(id, "(invalid)", CloseReason.INVALID_INFO, "bad_name");
      return;
    }
    if (streamType !== StreamType.TCP) {
      this.refuse(id, host, CloseReason.INVALID_INFO, "not_tcp");
      return;
    }
    const refusal = checkDestination(host, port);
    if (refusal) {
      this.refuse(id, host, CloseReason.BLOCKED, refusal);
      return;
    }
    if (!this.relay.admitStream(this.streams.size)) {
      this.refuse(id, host, CloseReason.THROTTLED, "cap");
      return;
    }

    const stream: Stream = {
      id,
      host: host.toLowerCase(),
      chain: Promise.resolve(),
      pending: 0,
      writtenSinceContinue: 0,
      closed: false,
      aborted: false,
      up: 0,
      down: 0,
    };
    this.streams.set(id, stream);
    this.relay.deps.log({ event: "stream_open", host: stream.host });
    stream.chain = this.openStream(stream, port);
  }

  private async openStream(stream: Stream, port: number): Promise<void> {
    try {
      const addresses = await this.relay.deps.resolve(stream.host);
      // A cap or error closed it during the lookup. A client CLOSE (closed, not
      // aborted) still connects so the writes it already queued are flushed.
      if (stream.aborted) return;
      if (addresses.length === 0) {
        this.closeStream(stream.id, CloseReason.UNREACHABLE);
        return;
      }
      if (addresses.some(isBlockedAddress)) {
        this.relay.deps.log({ event: "stream_refused", host: stream.host, why: "private_address" });
        this.closeStream(stream.id, CloseReason.BLOCKED);
        return;
      }
      // Connect to the checked address, not the name, so a second lookup
      // cannot hand back a different (private) address.
      const socket = this.relay.deps.connect(
        { hostname: addresses[0] as string, port },
        { secureTransport: "off", allowHalfOpen: false },
      );
      stream.socket = socket;
      if (stream.aborted) {
        socket.close().catch(() => {});
        return;
      }
      stream.writer = socket.writable.getWriter();
      await socket.opened;
      if (!stream.closed) void this.pump(stream, socket);
    } catch {
      this.closeStream(stream.id, CloseReason.UNREACHABLE);
    }
  }

  private async pump(stream: Stream, socket: SocketLike): Promise<void> {
    const reader = socket.readable.getReader();
    try {
      for (;;) {
        const { done, value } = await reader.read();
        if (done) break;
        if (stream.closed) return; // closed by the client or a cap; drop late bytes
        if (!value || value.byteLength === 0) continue;
        if (!this.relay.addBytes(value.byteLength)) {
          this.closeStream(stream.id, CloseReason.THROTTLED);
          return;
        }
        stream.down += value.byteLength;
        this.send(encodeData(stream.id, value));
      }
      // Remote finished. Wisp v1 has no half-close, so the stream ends here.
      this.closeStream(stream.id, CloseReason.VOLUNTARY);
    } catch {
      this.closeStream(stream.id, CloseReason.NETWORK_ERROR);
    } finally {
      try {
        reader.releaseLock();
      } catch {
        // stream already gone
      }
    }
  }

  private handleData(id: number, payload: Uint8Array): void {
    const stream = this.streams.get(id);
    if (!stream || stream.closed) return;
    if (!this.relay.addBytes(payload.byteLength)) {
      this.closeStream(id, CloseReason.THROTTLED);
      return;
    }
    stream.up += payload.byteLength;
    stream.pending += 1;
    // A client that honours CONTINUE stays under 2x the buffer even with
    // packets in flight; past that it is ignoring flow control.
    if (stream.pending > 2 * BUFFER_PACKETS) {
      this.closeStream(id, CloseReason.CLIENT_ERROR);
      return;
    }
    const chunk = payload.slice();
    stream.chain = stream.chain
      .then(async () => {
        if (stream.aborted || !stream.writer) return;
        await stream.writer.write(chunk);
        stream.pending -= 1;
        stream.writtenSinceContinue += 1;
        if (!stream.closed && stream.writtenSinceContinue >= BUFFER_PACKETS / 4) {
          stream.writtenSinceContinue = 0;
          this.send(encodeContinue(id, Math.max(0, BUFFER_PACKETS - stream.pending)));
        }
      })
      .catch(() => this.closeStream(id, CloseReason.NETWORK_ERROR));
  }

  private handleClientClose(id: number): void {
    // Flush what the client already sent, then close both directions.
    this.closeStream(id, CloseReason.VOLUNTARY, { notify: false, abort: false });
  }

  private refuse(id: number, host: string, reason: number, why: string): void {
    this.relay.deps.log({ event: "stream_refused", host: loggableHost(host), why });
    this.send(encodeClose(id, reason));
  }

  private closeStream(
    id: number,
    reason: number,
    opts: { notify: boolean; abort: boolean } = { notify: true, abort: true },
  ): void {
    const stream = this.streams.get(id);
    if (!stream || stream.closed) return;
    stream.closed = true;
    this.streams.delete(id);
    if (opts.notify) this.send(encodeClose(id, reason));
    const closeSocket = () => {
      stream.aborted = true;
      stream.socket?.close().catch(() => {});
    };
    if (opts.abort) closeSocket();
    else stream.chain = stream.chain.finally(closeSocket);
    this.relay.deps.log({
      event: "stream_close",
      host: stream.host,
      reason,
      up: stream.up,
      down: stream.down,
    });
  }

  private send(packet: Uint8Array): void {
    if (this.ended) return;
    try {
      this.ws.send(packet);
    } catch {
      this.onSocketClosed();
    }
  }
}
