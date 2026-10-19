/**
 * The Phone Link Android helper's raw screen sockets: `POST /stream`.
 *
 * The helper (surfaces/allternit-mobile/android, core/PhoneServer.kt
 * handleStream) takes a sealed v1 envelope — the same one /rpc uses — whose
 * plaintext is
 *
 *   {"kind":"video","scid":"<8 hex>","args":["key=value",…]}   starts scrcpy-server
 *   {"kind":"control","scid":"<same>"}                         the next socket of it
 *
 * and answers `101` followed by the scrcpy bytes both ways (its shell server
 * pipes the scrcpy abstract socket through). Every other status carries
 * `{"error":{"code","message"}}`. scrcpy accepts its sockets in order (video,
 * then audio when on, then control), and a "control" request is simply the
 * next socket, so the audio socket is a "control" request too.
 *
 * The helper serves this route over the Allternit mesh or loopback only
 * (403 on a plain LAN socket), so Desktop always dials the mesh proxy's
 * loopback address (`127.0.0.1:<port>`), at home too.
 */

import * as net from 'node:net';
import type { Duplex } from 'node:stream';
import type { RequestEnvelope } from './ictrl-client.js';
import type { LinkedPhone } from './phone-link-health.js';
import type { ScrcpySocket } from './phone-scrcpy.js';
import type { MeshHealFn } from './phone-mesh-heal.js';

export type HelperStreamKind = 'video' | 'control';

export interface HelperStreamRequest {
  kind: HelperStreamKind;
  scid: string;
  /** scrcpy-server key=value options (video only). */
  args?: string[];
}

export interface HelperStreamDeps {
  /** `127.0.0.1:<port>` of the mesh proxy to this phone (or any host:port that reaches the helper over the mesh). */
  host: string;
  /** Seals the JSON body with this phone's Phone Link key (IctrlClient.sealBody). */
  seal: (body: string) => RequestEnvelope;
  /** Opens the TCP connection; tests pass a fake duplex. */
  dial?: (host: string) => Duplex;
  /** Bounds the wait for the response head (the phone may start its screen service first). */
  timeoutMs?: number;
}

/** Starting the screen service on the phone can take ~15 s (wireless debugging toggles, adbd TLS). */
export const HELPER_STREAM_TIMEOUT_MS = 30_000;
const MAX_HEAD_BYTES = 16 * 1024;
const MAX_ERROR_BODY_BYTES = 64 * 1024;

export class HelperStreamError extends Error {
  constructor(
    readonly status: number,
    readonly code: string,
    message: string,
  ) {
    super(message);
    this.name = 'HelperStreamError';
  }
}

export function buildHelperStreamRequest(host: string, envelope: RequestEnvelope): Buffer {
  const body = Buffer.from(JSON.stringify(envelope), 'utf8');
  const head =
    `POST /stream HTTP/1.1\r\nHost: ${host}\r\nContent-Type: application/json\r\n` +
    `Content-Length: ${body.length}\r\nConnection: Upgrade\r\nUpgrade: allternit-stream\r\n\r\n`;
  return Buffer.concat([Buffer.from(head, 'utf8'), body]);
}

/** The user-facing sentence for a refused /stream (status + the helper's own error). */
export function helperStreamErrorMessage(status: number, code: string | undefined, message: string | undefined): string {
  if (status === 404 && code !== 'stream_unavailable') {
    return 'Phone Link on this phone is too old for the live screen. Update it from Phone Link setup (cable), then try again.';
  }
  if (status === 403) return message || 'The phone only streams its screen over the Allternit mesh.';
  if (status === 503) return message ? `The phone's screen service isn't running: ${message}` : "The phone's screen service isn't running.";
  if (status === 401) return message || 'This computer is no longer paired with Phone Link on the phone. Set the phone up again.';
  return message || `Phone Link answered HTTP ${status}.`;
}

export function nodeHelperDial(host: string): Duplex {
  const m = /^\[([^\]]+)\]:(\d+)$/.exec(host) ?? /^([^:]+):(\d+)$/.exec(host);
  if (!m) throw new Error(`Bad Phone Link address: ${host}`);
  const socket = net.connect({ host: m[1], port: Number(m[2]) });
  socket.setNoDelay(true);
  return socket;
}

/**
 * POST /stream and wait for the head. 101 → the socket, with any bytes that
 * arrived after the head delivered first; anything else → HelperStreamError.
 */
export function openHelperStream(deps: HelperStreamDeps, req: HelperStreamRequest): Promise<ScrcpySocket> {
  const plaintext =
    req.kind === 'video' ? { kind: 'video', scid: req.scid, args: req.args ?? [] } : { kind: 'control', scid: req.scid };
  let request: Buffer;
  try {
    request = buildHelperStreamRequest(deps.host, deps.seal(JSON.stringify(plaintext)));
  } catch (error) {
    return Promise.reject(error);
  }
  return new Promise<ScrcpySocket>((resolve, reject) => {
    let socket: Duplex;
    try {
      socket = (deps.dial ?? nodeHelperDial)(deps.host);
    } catch (error) {
      reject(error);
      return;
    }
    let buf: Buffer = Buffer.alloc(0);
    let status = 0;
    let headEnd = -1;
    let contentLength = -1;
    let settled = false;

    const cleanup = () => {
      clearTimeout(timer);
      socket.removeListener('data', onData);
      socket.removeListener('error', onError);
      socket.removeListener('close', onClose);
    };
    const fail = (error: Error) => {
      if (settled) return;
      settled = true;
      cleanup();
      socket.destroy();
      reject(error);
    };
    const failWithBody = () => {
      const body = buf.subarray(headEnd + 4, contentLength >= 0 ? headEnd + 4 + contentLength : undefined);
      let code: string | undefined;
      let message: string | undefined;
      try {
        const parsed = JSON.parse(body.toString('utf8')) as { error?: { code?: unknown; message?: unknown } };
        if (typeof parsed.error?.code === 'string') code = parsed.error.code;
        if (typeof parsed.error?.message === 'string') message = parsed.error.message;
      } catch {
        /* not JSON: the generic sentence */
      }
      fail(new HelperStreamError(status, code ?? `http_${status}`, helperStreamErrorMessage(status, code, message)));
    };
    const onData = (chunk: Buffer) => {
      buf = buf.length ? Buffer.concat([buf, chunk]) : chunk;
      if (headEnd < 0) {
        headEnd = buf.indexOf('\r\n\r\n');
        if (headEnd < 0) {
          if (buf.length > MAX_HEAD_BYTES) fail(new Error('Phone Link sent an oversized response head.'));
          return;
        }
        const lines = buf.subarray(0, headEnd).toString('latin1').split('\r\n');
        const m = /^HTTP\/1\.[01] (\d{3})/.exec(lines[0] ?? '');
        if (!m) {
          fail(new Error('Phone Link sent a response that is not HTTP.'));
          return;
        }
        status = Number(m[1]);
        for (const line of lines.slice(1)) {
          const cl = /^content-length:\s*(\d+)\s*$/i.exec(line);
          if (cl) contentLength = Number(cl[1]);
        }
        if (status === 101) {
          settled = true;
          cleanup();
          resolve(new HandedOverSocket(socket, buf.subarray(headEnd + 4)));
          return;
        }
      }
      // Error reply: wait for its body (or the close).
      const have = buf.length - (headEnd + 4);
      if (have >= MAX_ERROR_BODY_BYTES || (contentLength >= 0 && have >= contentLength)) failWithBody();
    };
    const onError = (error: Error) => fail(new Error(`Couldn't reach Phone Link over the mesh: ${error.message}`));
    const onClose = () => {
      if (headEnd >= 0 && status !== 0) failWithBody();
      else fail(new Error('Phone Link closed the connection before answering.'));
    };
    const timer = setTimeout(
      () => fail(new Error("Timed out waiting for the phone's screen service.")),
      deps.timeoutMs ?? HELPER_STREAM_TIMEOUT_MS,
    );
    timer.unref?.();
    socket.on('data', onData);
    socket.on('error', onError);
    socket.on('close', onClose);
    socket.write(request);
  });
}

type DataCb = (chunk: Buffer) => void;
type CloseCb = () => void;
type ErrorCb = (err: Error) => void;

/**
 * The upgraded socket as a ScrcpySocket. Bytes that arrived with the head,
 * or before the session attached its reader, are kept and delivered first,
 * in order; a close or error that happened meanwhile is replayed too.
 */
export class HandedOverSocket implements ScrcpySocket {
  private readonly dataCbs: DataCb[] = [];
  private readonly closeCbs: CloseCb[] = [];
  private readonly errorCbs: ErrorCb[] = [];
  private pending: Buffer[] = [];
  private closed = false;
  private error: Error | null = null;

  constructor(
    private readonly inner: Duplex,
    leftover: Buffer,
  ) {
    if (leftover.length) this.pending.push(Buffer.from(leftover));
    inner.on('data', (chunk: Buffer) => {
      if (this.dataCbs.length === 0 || this.pending.length) this.pending.push(chunk);
      else for (const cb of this.dataCbs) cb(chunk);
    });
    inner.on('error', (err: Error) => {
      this.error = err;
      for (const cb of this.errorCbs) cb(err);
    });
    inner.on('close', () => {
      this.closed = true;
      // Data first: a close never overtakes bytes still queued for the reader.
      if (this.pending.length && this.dataCbs.length) this.flush();
      for (const cb of this.closeCbs) cb();
    });
  }

  write(data: Buffer): unknown {
    return this.inner.write(data);
  }

  destroy(): void {
    this.inner.destroy();
  }

  on(event: 'data', cb: DataCb): void;
  on(event: 'close', cb: CloseCb): void;
  on(event: 'error', cb: ErrorCb): void;
  on(event: 'data' | 'close' | 'error', cb: DataCb | CloseCb | ErrorCb): void {
    if (event === 'data') {
      this.dataCbs.push(cb as DataCb);
      if (this.pending.length) queueMicrotask(() => this.flush());
    } else if (event === 'close') {
      this.closeCbs.push(cb as CloseCb);
      if (this.closed) queueMicrotask(() => (cb as CloseCb)());
    } else {
      this.errorCbs.push(cb as ErrorCb);
      if (this.error) {
        const err = this.error;
        queueMicrotask(() => (cb as ErrorCb)(err));
      }
    }
  }

  private flush(): void {
    while (this.pending.length && this.dataCbs.length) {
      const chunk = this.pending.shift()!;
      for (const cb of this.dataCbs) cb(chunk);
    }
  }
}

// ── Routing (phone-service glue, kept pure for tests) ───────────────────────

export const LINK_SERIAL = 'link:';

export type StreamLane = 'adb' | 'ios-video' | 'android-helper' | 'unlinked';

/**
 * Which live-screen lane serves a serial: adb serials stream over adb
 * (scrcpy pushed by Desktop); `link:<deviceId>` streams through the linked
 * phone's own helper — the iPhone runner's /video, or the Android Phone Link
 * app's /stream — and never needs adb. `phone` is the linked phone the
 * serial names (null when none is linked under that id).
 */
export function streamLaneFor(serial: string, phone: LinkedPhone | null): StreamLane {
  if (!serial.startsWith(LINK_SERIAL)) return 'adb';
  if (!phone || `${LINK_SERIAL}${phone.deviceId}` !== serial) return 'unlinked';
  return phone.platform === 'ios' ? 'ios-video' : 'android-helper';
}

export interface HelperHostDeps {
  /** PHONE_LINK_MESH_ENABLED. */
  meshEnabled: boolean;
  /** `<100.x>:8200` → `127.0.0.1:<port>` via the mesh-node sidecar (MeshManager.proxyFor). */
  meshProxy: (meshHost: string) => Promise<string>;
  fetchWhoami: (host: string, timeoutMs: number) => Promise<{ device_id?: string } | null>;
  /**
   * The phone's mesh route failed: restart its mesh node over LAN/cable
   * (phone-mesh-heal.ts). Non-null = restarted; the route is retried once.
   */
  healMesh?: MeshHealFn;
}

export const NO_MESH_MESSAGE =
  "This phone hasn't joined the Allternit mesh yet, so its live screen can't go through Phone Link. " +
  'Plug it in and run Phone Link setup again while signed in to Allternit.';

/**
 * Where to POST /stream for a linked Android phone: always the mesh proxy's
 * loopback address, at home too — the helper refuses plain LAN for the screen.
 * The proxy must answer /whoami as this phone (a shared sidecar could be
 * bridging another one).
 */
export async function resolveHelperStreamHost(phone: LinkedPhone, deps: HelperHostDeps): Promise<string> {
  if (!deps.meshEnabled) throw new Error('The Allternit mesh is turned off on this computer (ALLTERNIT_PHONE_LINK_MESH=0), so the live screen needs Wireless debugging.');
  if (!phone.meshHost) throw new Error(NO_MESH_MESSAGE);
  const viaMesh = async (meshHost: string, timeoutMs: number) => {
    let proxy: string;
    try {
      proxy = await deps.meshProxy(meshHost);
    } catch (error) {
      // This computer's own mesh is down: nothing a phone-side restart fixes.
      throw new Error(`Couldn't open the Allternit mesh: ${error instanceof Error ? error.message : String(error)}`);
    }
    return { proxy, who: await deps.fetchWhoami(proxy, timeoutMs).catch(() => null) };
  };
  let { proxy, who } = await viaMesh(phone.meshHost, 5000);
  if (!who && deps.healMesh) {
    // The phone's node can look online yet reset every connection (seen after
    // a Headscale outage); if it answers off the mesh, restart its node and retry once.
    const healed = await deps.healMesh(phone, 'live-screen mesh route failed').catch(() => null);
    if (healed) ({ proxy, who } = await viaMesh(healed.meshHost ?? phone.meshHost, 15_000));
  }
  if (!who) throw new Error(`Can't reach ${phone.name || 'the phone'} over the Allternit mesh. Check it is on and online.`);
  if (who.device_id !== phone.deviceId) throw new Error('The mesh address answered as a different phone. Run Phone Link setup again.');
  return proxy;
}

export interface RelayAdbPhone {
  serial: string;
  id: string;
  state: string;
}

export interface RelayLinkedAndroid {
  deviceId: string;
  /** The adb serial seen at cable setup (phone-link-android.json `serial`). */
  serial: string;
}

/**
 * The serials the cloud relay keeps a socket for: every adb phone, every
 * linked iPhone, and every linked Android whose Phone Link answers. A linked
 * Android stands in for its adb twin while that twin is not online (left the
 * Wi-Fi, wireless debugging off), so the web shows one live entry per phone.
 */
export function relaySerials(input: {
  adb: readonly RelayAdbPhone[];
  iosDeviceIds: readonly string[];
  androidLinks: readonly RelayLinkedAndroid[];
  linkReachable: (deviceId: string) => boolean;
}): string[] {
  const out: string[] = [];
  const reachable = input.androidLinks.filter((l) => input.linkReachable(l.deviceId));
  const twinOf = (phone: RelayAdbPhone) => reachable.find((l) => l.serial === phone.id || l.serial === phone.serial);
  /** Linked Androids whose adb lane is online: that serial is already relayed. */
  const covered = new Set<string>();
  for (const phone of input.adb) {
    if (!phone.serial) continue;
    const twin = twinOf(phone);
    if (twin && phone.state !== 'online') continue; // the link: serial below stands in
    if (twin) covered.add(twin.deviceId);
    out.push(phone.serial);
  }
  for (const id of input.iosDeviceIds) out.push(`${LINK_SERIAL}${id}`);
  for (const link of reachable) if (!covered.has(link.deviceId)) out.push(`${LINK_SERIAL}${link.deviceId}`);
  return [...new Set(out)];
}
