/**
 * Test fixture: an in-process iCTRL server speaking protocol v1, ported from
 * the Swift side (AuthManager.openEnvelope, SecureEnvelope.open/seal,
 * JSONRPCServer.handlePair). Same checks in the same order: parse → session
 * lookup → ±120 s timestamp → MAC → replay cache → decrypt.
 */

import { createServer, type IncomingMessage, type Server, type ServerResponse } from 'node:http';
import { randomBytes } from 'node:crypto';
import type { AddressInfo } from 'node:net';
import {
  be64,
  envelopeKeys,
  hmac,
  ICTRL_TIMESTAMP_WINDOW_S,
  keystreamXor,
  pairingPad,
  tokenIdFor,
  x25519KeyPair,
  x25519Shared,
  type SecretStore,
} from './ictrl-client.js';

export type RpcHandler = (params: Record<string, unknown>) => unknown;

export interface FakeIctrlOptions {
  deviceId?: string;
  name?: string;
  /** 'once' mirrors ICTRL_ALLOW_AUTH_ONCE: the first pairing is approved, later ones denied. */
  approve?: 'once' | 'always' | 'never';
  /** Server clock in ms (lets tests skew it). */
  now?: () => number;
  handlers?: Record<string, RpcHandler>;
  /** Corrupt reply tags, to exercise the client's response MAC check. */
  tamperResponses?: boolean;
}

export class FakeIctrlServer {
  readonly sessions = new Map<string, Buffer>();
  readonly seenNonces = new Map<string, Map<string, number>>();
  readonly calls: { method: string; params: Record<string, unknown> }[] = [];
  remoteAccess = false;
  private server: Server | null = null;
  private approvalsLeft: number;

  constructor(private readonly opts: FakeIctrlOptions = {}) {
    this.approvalsLeft = opts.approve === 'never' ? 0 : opts.approve === 'always' ? Infinity : 1;
  }

  async listen(): Promise<string> {
    this.server = createServer((req, res) => void this.handle(req, res));
    await new Promise<void>((resolve) => this.server!.listen(0, '127.0.0.1', resolve));
    return `127.0.0.1:${(this.server.address() as AddressInfo).port}`;
  }

  async close(): Promise<void> {
    await new Promise<void>((resolve) => (this.server ? this.server.close(() => resolve()) : resolve()));
  }

  private nowS(): number {
    return Math.floor((this.opts.now ?? Date.now)() / 1000);
  }

  private async handle(req: IncomingMessage, res: ServerResponse): Promise<void> {
    const chunks: Buffer[] = [];
    for await (const chunk of req) chunks.push(chunk as Buffer);
    const text = Buffer.concat(chunks).toString('utf8');
    const send = (status: number, body: unknown) => {
      res.writeHead(status, { 'Content-Type': 'application/json', 'X-ICTRL-Device': this.opts.deviceId ?? 'dev-1' });
      res.end(JSON.stringify(body));
    };
    const fail = (status: number, code: string, message = code) =>
      send(status, { error: { code, message, server_ts: this.nowS() } });

    if (req.method === 'GET' && req.url === '/whoami') {
      return send(200, { device_id: this.opts.deviceId ?? 'dev-1', name: this.opts.name ?? 'Test iPhone', protocol_version: 1 });
    }
    let obj: Record<string, unknown>;
    try {
      obj = JSON.parse(text);
    } catch {
      return fail(400, 'bad_request');
    }
    if (req.method === 'POST' && req.url === '/pair') return this.pair(obj, send, fail);
    if (req.method === 'POST' && req.url === '/rpc') return this.rpc(obj, send, fail);
    return fail(404, 'not_found');
  }

  private pair(obj: Record<string, unknown>, send: (s: number, b: unknown) => void, fail: (s: number, c: string) => void): void {
    if (typeof obj.client_name !== 'string' || !obj.client_name) return fail(400, 'bad_request');
    if (obj.plain) return fail(403, 'plain_forbidden');
    const clientPub = Buffer.from(String(obj.pub ?? ''), 'base64');
    if (clientPub.length !== 32) return fail(400, 'bad_request');
    if (this.approvalsLeft <= 0) return fail(403, 'pair_denied');
    this.approvalsLeft--;
    const key = randomBytes(32);
    const tok = tokenIdFor(key);
    this.sessions.set(tok, key);
    const device = x25519KeyPair();
    const pad = pairingPad(x25519Shared(device.privateKey, clientPub), clientPub, device.publicRaw);
    const ct = Buffer.alloc(32);
    for (let i = 0; i < 32; i++) ct[i] = key[i] ^ pad[i];
    send(200, { tok, client_name: obj.client_name, expires: this.nowS() + 30 * 86400, pub: device.publicRaw.toString('base64'), token_ct: ct.toString('hex') });
  }

  private rpc(obj: Record<string, unknown>, send: (s: number, b: unknown) => void, fail: (s: number, c: string) => void): void {
    if (obj.v !== 1 || typeof obj.tok !== 'string' || typeof obj.ct !== 'string') return fail(401, 'auth_required');
    const nonce = Buffer.from(String(obj.nonce ?? ''), 'hex');
    const tag = Buffer.from(String(obj.tag ?? ''), 'hex');
    const ts = obj.ts;
    if (nonce.length !== 16 || tag.length !== 32 || typeof ts !== 'number' || ts < 0) return fail(400, 'bad_envelope');
    const key = this.sessions.get(obj.tok);
    if (!key) return fail(401, 'auth_failed');
    if (Math.abs(this.nowS() - ts) > ICTRL_TIMESTAMP_WINDOW_S) return fail(401, 'ts_skew');
    const k = envelopeKeys(key, nonce);
    const ct = Buffer.from(obj.ct, 'base64');
    const signed = Buffer.concat([Buffer.from('v1'), Buffer.from(obj.tok, 'hex'), be64(ts), nonce, ct]);
    if (!hmac(k.mac, signed).equals(tag)) return fail(401, 'bad_tag');
    const seen = this.seenNonces.get(obj.tok) ?? new Map<string, number>();
    if (seen.has(nonce.toString('hex'))) return fail(401, 'nonce_replayed');
    seen.set(nonce.toString('hex'), ts);
    this.seenNonces.set(obj.tok, seen);

    const request = JSON.parse(keystreamXor(k.encReq, nonce, ct).toString('utf8')) as {
      id: number;
      method: string;
      params?: Record<string, unknown>;
    };
    const params = request.params ?? {};
    this.calls.push({ method: request.method, params });
    let reply: unknown;
    const handler = this.opts.handlers?.[request.method] ?? (request.method === 'config' ? (p: Record<string, unknown>) => this.config(p) : undefined);
    if (!handler) {
      reply = { jsonrpc: '2.0', id: request.id, error: { code: -32601, message: `Unknown method ${request.method}` } };
    } else {
      try {
        reply = { jsonrpc: '2.0', id: request.id, result: handler(params) };
      } catch (error) {
        reply = { jsonrpc: '2.0', id: request.id, error: { code: -32000, message: error instanceof Error ? error.message : String(error) } };
      }
    }
    const respCt = keystreamXor(k.encResp, nonce, Buffer.from(JSON.stringify(reply), 'utf8'));
    const respTag = hmac(k.mac, Buffer.concat([Buffer.from('resp'), nonce, respCt]));
    if (this.opts.tamperResponses) respTag[0] ^= 1;
    send(200, { v: 1, nonce: nonce.toString('hex'), ct: respCt.toString('base64'), tag: respTag.toString('hex') });
  }

  private config(params: Record<string, unknown>): unknown {
    if (typeof params.allow_remote_access === 'boolean') this.remoteAccess = params.allow_remote_access;
    return {
      allow_remote_access: this.remoteAccess,
      listen_port: 8200,
      address: this.remoteAccess ? '192.168.1.20:8200' : '127.0.0.1:8200',
      restarted: typeof params.allow_remote_access === 'boolean',
    };
  }
}

export function memorySecrets(): SecretStore & { map: Map<string, string> } {
  const map = new Map<string, string>();
  return {
    map,
    get: (k) => map.get(k) ?? null,
    set: (k, v) => void map.set(k, v),
    delete: (k) => void map.delete(k),
  };
}
