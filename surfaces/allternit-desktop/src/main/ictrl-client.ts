/**
 * iCTRL client: pairing + sealed-envelope JSON-RPC (protocol v1) for the
 * runner Phone Link installs on an iPhone. A TypeScript port of
 * ictrl-fork/Scripts/ictrl_rpc.py; the server side is
 * iCTRLKit/SecureEnvelope.swift + AuthManager.swift + JSONRPCServer.swift.
 *
 *     K          = 32-byte key handed out once at pairing
 *     tok        = first 4 bytes of SHA-256(K), hex (lookup id only)
 *     K_enc_req  = HMAC(K, "ictrl-v1|enc-req"  || nonce)
 *     K_enc_resp = HMAC(K, "ictrl-v1|enc-resp" || nonce)
 *     K_mac      = HMAC(K, "ictrl-v1|mac"      || nonce)
 *     keystream  = HMAC(Kx, nonce || be32(0)) || HMAC(Kx, nonce || be32(1)) || …
 *     tag        = HMAC(K_mac, "v1" || tok_4B || be64(ts) || nonce || ct)
 *     reply tag  = HMAC(K_mac, "resp" || nonce || resp_ct)
 *
 * The server rejects |ts − now| > 120 s (ts_skew, with its own clock in
 * server_ts) and any nonce it has seen inside that window (nonce_replayed).
 * Pairing is X25519 only: the plain-token mode is never used, even though it
 * is allowed over loopback, so the key never crosses the wire in the clear.
 *
 * The key lives only in the injected SecretStore (Keychain-backed
 * secure-store.ts in the app), never in a plain file, and is never logged.
 */

import {
  createHash,
  createHmac,
  createPublicKey,
  diffieHellman,
  generateKeyPairSync,
  randomBytes as nodeRandomBytes,
  timingSafeEqual,
} from 'node:crypto';

/** Plain fetch signature (Node's `typeof fetch` also carries `preconnect`). */
export type FetchFn = (input: string | URL | Request, init?: RequestInit) => Promise<Response>;

export const ICTRL_PORT = 8200;
export const ICTRL_TIMESTAMP_WINDOW_S = 120;

const LABEL_MAC = Buffer.from('ictrl-v1|mac');
const LABEL_ENC_REQ = Buffer.from('ictrl-v1|enc-req');
const LABEL_ENC_RESP = Buffer.from('ictrl-v1|enc-resp');
const LABEL_PAIR = Buffer.from('ictrl-pair');
const LABEL_WRAP = Buffer.from('ictrl-wrap');

// ─── Primitives (shared with the test server) ────────────────────────────────

export function hmac(key: Buffer, data: Buffer): Buffer {
  return createHmac('sha256', key).update(data).digest();
}

/** PRF-CTR: keystream_i = HMAC(key, nonce || be32(i)), XORed onto data. */
export function keystreamXor(key: Buffer, nonce: Buffer, data: Buffer): Buffer {
  const out = Buffer.alloc(data.length);
  const counter = Buffer.alloc(4);
  for (let offset = 0, i = 0; offset < data.length; offset += 32, i++) {
    counter.writeUInt32BE(i);
    const block = hmac(key, Buffer.concat([nonce, counter]));
    for (let j = 0; j < 32 && offset + j < data.length; j++) out[offset + j] = data[offset + j] ^ block[j];
  }
  return out;
}

export function tokenIdFor(key: Buffer): string {
  return createHash('sha256').update(key).digest().subarray(0, 4).toString('hex');
}

export function be64(value: number): Buffer {
  const b = Buffer.alloc(8);
  b.writeBigUInt64BE(BigInt(value));
  return b;
}

export const envelopeKeys = (key: Buffer, nonce: Buffer) => ({
  encReq: hmac(key, Buffer.concat([LABEL_ENC_REQ, nonce])),
  encResp: hmac(key, Buffer.concat([LABEL_ENC_RESP, nonce])),
  mac: hmac(key, Buffer.concat([LABEL_MAC, nonce])),
});

export interface RequestEnvelope {
  v: 1;
  tok: string;
  ts: number;
  nonce: string;
  ct: string;
  tag: string;
}

export interface ResponseEnvelope {
  v: 1;
  nonce: string;
  ct: string;
  tag: string;
}

export function sealRequest(key: Buffer, body: string, ts: number, nonce: Buffer): RequestEnvelope {
  const tok = tokenIdFor(key);
  const k = envelopeKeys(key, nonce);
  const ct = keystreamXor(k.encReq, nonce, Buffer.from(body, 'utf8'));
  const signed = Buffer.concat([Buffer.from('v1'), Buffer.from(tok, 'hex'), be64(ts), nonce, ct]);
  return { v: 1, tok, ts, nonce: nonce.toString('hex'), ct: ct.toString('base64'), tag: hmac(k.mac, signed).toString('hex') };
}

/** Verify-then-decrypt a reply sealed under our request nonce. */
export function openResponse(key: Buffer, nonce: Buffer, envelope: { ct: string; tag: string }): string {
  const k = envelopeKeys(key, nonce);
  const ct = Buffer.from(envelope.ct, 'base64');
  const expected = hmac(k.mac, Buffer.concat([Buffer.from('resp'), nonce, ct]));
  const tag = Buffer.from(envelope.tag, 'hex');
  if (tag.length !== expected.length || !timingSafeEqual(tag, expected)) {
    throw new IctrlError('bad_response_mac', 'The phone\'s reply failed its integrity check; discarded.');
  }
  return keystreamXor(k.encResp, nonce, ct).toString('utf8');
}

/** The pad the device XORs K with, from the X25519 shared secret. */
export function pairingPad(shared: Buffer, clientPub: Buffer, devicePub: Buffer): Buffer {
  const kWrap = hmac(shared, Buffer.concat([LABEL_PAIR, clientPub, devicePub]));
  return hmac(kWrap, LABEL_WRAP);
}

export function x25519KeyPair(): { privateKey: import('node:crypto').KeyObject; publicRaw: Buffer } {
  const { publicKey, privateKey } = generateKeyPairSync('x25519');
  const jwk = publicKey.export({ format: 'jwk' }) as { x?: string };
  return { privateKey, publicRaw: Buffer.from(jwk.x ?? '', 'base64url') };
}

export function x25519Shared(privateKey: import('node:crypto').KeyObject, peerRaw: Buffer): Buffer {
  if (peerRaw.length !== 32) throw new IctrlError('bad_pair_response', 'The phone sent a malformed pairing key.');
  const publicKey = createPublicKey({ key: { kty: 'OKP', crv: 'X25519', x: peerRaw.toString('base64url') }, format: 'jwk' });
  return diffieHellman({ privateKey, publicKey });
}

// ─── Client ──────────────────────────────────────────────────────────────────

/** Keychain-backed in the app (secure-store.ts); a Map in tests. */
export interface SecretStore {
  get(key: string): string | null;
  set(key: string, value: string): void;
  delete(key: string): void;
}

export interface IctrlKeyRecord {
  tok: string;
  /** Hex of K. Only ever inside the SecretStore. */
  key: string;
  deviceId?: string;
  deviceName?: string;
  clientName: string;
  pairedAt: string;
  /** Last addresses that answered ("127.0.0.1:52011", "192.168.1.20:8200"), most recent first. */
  addresses: string[];
}

export interface IctrlWhoami {
  device_id: string;
  name?: string;
  protocol_version?: number;
  server_version?: string;
}

export class IctrlError extends Error {
  constructor(
    readonly code: string,
    message: string,
    readonly extra: { status?: number; retryAfter?: number; serverTs?: number; rpcCode?: number } = {},
  ) {
    super(message);
    this.name = 'IctrlError';
  }

  /** The key is gone on the phone (revoked, 30 days idle, app reinstalled): pair again. */
  get needsRepair(): boolean {
    return this.code === 'auth_failed' || this.code === 'no_key';
  }
}

export interface IctrlClientOptions {
  /** "127.0.0.1:52011" or "192.168.1.20:8200". */
  host: string;
  secrets: SecretStore;
  /** Secret-store entry holding this device's key record. */
  storeKey: string;
  fetchImpl?: FetchFn;
  /** Milliseconds since the epoch. */
  now?: () => number;
  random?: (n: number) => Buffer;
  timeoutMs?: number;
  logger?: (message: string) => void;
}

export function ictrlStoreKey(deviceKey: string): string {
  return `phone-link.ictrl.${deviceKey}`;
}

export class IctrlClient {
  private clockOffsetS = 0;
  private rpcId = 0;

  constructor(private readonly opts: IctrlClientOptions) {
    if (!/^[\w.-]+(:\d+)?$|^\[[0-9a-f:]+\](:\d+)?$/i.test(opts.host)) throw new Error(`Bad iCTRL host: ${opts.host}`);
  }

  get host(): string {
    return this.opts.host;
  }

  /** Same key, another address (USB forward → LAN after remote access is on). */
  withHost(host: string): IctrlClient {
    const next = new IctrlClient({ ...this.opts, host });
    next.clockOffsetS = this.clockOffsetS;
    return next;
  }

  record(): IctrlKeyRecord | null {
    const raw = this.opts.secrets.get(this.opts.storeKey);
    if (!raw) return null;
    try {
      const parsed = JSON.parse(raw) as IctrlKeyRecord;
      return typeof parsed.key === 'string' && parsed.key.length === 64 && typeof parsed.tok === 'string' ? parsed : null;
    } catch {
      return null;
    }
  }

  hasKey(): boolean {
    return this.record() !== null;
  }

  forget(): void {
    this.opts.secrets.delete(this.opts.storeKey);
  }

  async whoami(): Promise<IctrlWhoami | null> {
    const res = await this.request('GET', '/whoami');
    if (res.status !== 200) return null;
    const body = res.body as Partial<IctrlWhoami>;
    return typeof body.device_id === 'string' ? (body as IctrlWhoami) : null;
  }

  /**
   * POST /pair with an ephemeral X25519 key. Blocks until the phone approves:
   * instantly when Desktop's own runtest launch armed ICTRL_ALLOW_AUTH_ONCE,
   * otherwise up to the runner's 60 s prompt.
   */
  async pair(clientName: string, meta: { deviceId?: string; deviceName?: string } = {}): Promise<IctrlKeyRecord> {
    const { privateKey, publicRaw } = x25519KeyPair();
    const res = await this.request('POST', '/pair', { client_name: clientName, pub: publicRaw.toString('base64') }, 90_000);
    if (res.status !== 200) throw errorFromBody(res.status, res.body, 'pair_failed');
    const body = res.body as { pub?: string; token_ct?: string; tok?: string };
    if (typeof body.pub !== 'string' || typeof body.token_ct !== 'string') {
      throw new IctrlError('bad_pair_response', 'The phone answered pairing without a wrapped key.');
    }
    const devicePub = Buffer.from(body.pub, 'base64');
    const pad = pairingPad(x25519Shared(privateKey, devicePub), publicRaw, devicePub);
    const wrapped = Buffer.from(body.token_ct, 'hex');
    if (wrapped.length !== 32) throw new IctrlError('bad_pair_response', 'The phone sent a malformed wrapped key.');
    const key = Buffer.alloc(32);
    for (let i = 0; i < 32; i++) key[i] = wrapped[i] ^ pad[i];
    const tok = tokenIdFor(key);
    // A mismatch means the unwrap went wrong (or someone in the middle); never store it.
    if (body.tok && body.tok !== tok) throw new IctrlError('bad_pair_response', 'The unwrapped key does not match the phone\'s key id.');
    const record: IctrlKeyRecord = {
      tok,
      key: key.toString('hex'),
      deviceId: meta.deviceId,
      deviceName: meta.deviceName,
      clientName,
      pairedAt: new Date(this.nowMs()).toISOString(),
      addresses: [this.opts.host],
    };
    this.opts.secrets.set(this.opts.storeKey, JSON.stringify(record));
    this.opts.logger?.(`[ictrl] paired as ${clientName} (tok ${tok})`);
    return record;
  }

  /** One sealed JSON-RPC call; returns `result`. A skewed clock is corrected once from server_ts. */
  async call<T = Record<string, unknown>>(method: string, params: Record<string, unknown> = {}, timeoutMs?: number): Promise<T> {
    try {
      return await this.callOnce<T>(method, params, timeoutMs);
    } catch (error) {
      if (error instanceof IctrlError && error.code === 'ts_skew' && typeof error.extra.serverTs === 'number') {
        this.clockOffsetS = error.extra.serverTs - Math.floor(this.nowMs() / 1000);
        this.opts.logger?.(`[ictrl] phone clock differs by ${this.clockOffsetS}s; retrying with its time`);
        return this.callOnce<T>(method, params, timeoutMs);
      }
      throw error;
    }
  }

  private async callOnce<T>(method: string, params: Record<string, unknown>, timeoutMs?: number): Promise<T> {
    const record = this.record();
    if (!record) throw new IctrlError('no_key', 'Not paired with this phone yet.');
    const key = Buffer.from(record.key, 'hex');
    const nonce = (this.opts.random ?? nodeRandomBytes)(16);
    const ts = Math.floor(this.nowMs() / 1000) + this.clockOffsetS;
    const body = JSON.stringify({ jsonrpc: '2.0', method, params, id: ++this.rpcId });
    const res = await this.request('POST', '/rpc', sealRequest(key, body, ts, nonce), timeoutMs);

    const sealed = res.body as Partial<ResponseEnvelope>;
    if (typeof sealed.ct !== 'string' || typeof sealed.tag !== 'string') throw errorFromBody(res.status, res.body, 'rpc_failed');
    if (sealed.nonce !== undefined && sealed.nonce !== nonce.toString('hex')) {
      throw new IctrlError('bad_response_mac', 'The phone answered a different request.');
    }
    const inner = JSON.parse(openResponse(key, nonce, sealed as { ct: string; tag: string })) as {
      result?: T;
      error?: { code?: number; message?: string };
    };
    this.noteAddress(record);
    if (inner.error) {
      throw new IctrlError('rpc_error', inner.error.message ?? `${method} failed`, { status: res.status, rpcCode: inner.error.code });
    }
    return (inner.result ?? {}) as T;
  }

  private noteAddress(record: IctrlKeyRecord): void {
    if (record.addresses[0] === this.opts.host) return;
    record.addresses = [this.opts.host, ...record.addresses.filter((a) => a !== this.opts.host)].slice(0, 5);
    this.opts.secrets.set(this.opts.storeKey, JSON.stringify(record));
  }

  private nowMs(): number {
    return (this.opts.now ?? Date.now)();
  }

  private async request(method: 'GET' | 'POST', path: string, body?: unknown, timeoutMs?: number): Promise<{ status: number; body: unknown }> {
    const fetchImpl = this.opts.fetchImpl ?? fetch;
    let res: Response;
    try {
      res = await fetchImpl(`http://${this.opts.host}${path}`, {
        method,
        headers: body === undefined ? undefined : { 'Content-Type': 'application/json' },
        body: body === undefined ? undefined : JSON.stringify(body),
        signal: AbortSignal.timeout(timeoutMs ?? this.opts.timeoutMs ?? 20_000),
      });
    } catch (error) {
      throw new IctrlError('unreachable', `Can't reach the phone at ${this.opts.host}: ${error instanceof Error ? error.message : String(error)}`);
    }
    const text = await res.text();
    let parsed: unknown = {};
    try {
      parsed = text ? JSON.parse(text) : {};
    } catch {
      parsed = { error: { code: 'bad_body', message: text.slice(0, 200) } };
    }
    return { status: res.status, body: parsed };
  }
}

function errorFromBody(status: number, body: unknown, fallback: string): IctrlError {
  const error = (body as { error?: { code?: unknown; message?: unknown; retry_after?: unknown; server_ts?: unknown } })?.error;
  const code = typeof error?.code === 'string' ? error.code : fallback;
  const message = typeof error?.message === 'string' ? error.message : `HTTP ${status}`;
  return new IctrlError(code, message, {
    status,
    retryAfter: typeof error?.retry_after === 'number' ? error.retry_after : undefined,
    serverTs: typeof error?.server_ts === 'number' ? error.server_ts : undefined,
  });
}
