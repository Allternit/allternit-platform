/**
 * Loopback HTTP gateway that exposes the phone tools to bots.
 *
 * The MCP shim (resources/phone/phone-mcp-shim.cjs) is the bots' door: Desktop
 * registers it with the native MCP host, so every bot that already reaches MCP
 * tools reaches the phone, with no new transport. The shim forwards to this
 * gateway. Loopback-only, every request carries a per-launch random token that
 * is also written (0600) to ~/.allternit/phone-gateway.json for the shim.
 *
 *   GET  /phone/tools                      → { tools: [...] }
 *   POST /phone/call {name, arguments}     → ToolResult
 *   GET  /phone/handover/key               → HandoverKey (this Desktop's public key)
 *   POST /phone/handover {sealed}          → { ok, deviceId, platform, name }
 *
 * The handover pair receives a phone's pairing from the person's Mac
 * (phone-link-handover.ts). The subscription gateway on this computer is the
 * only caller (services/subscription-gateway src/http/routes_phone_handover.ts);
 * it reads the token from the same file.
 */

import * as http from 'node:http';
import { randomBytes, timingSafeEqual } from 'node:crypto';
import { PHONE_TOOLS, type PhoneToolRunner } from './phone-tools.js';
import type { HandoverKey, SealedHandover } from './phone-link-handover.js';

/** The receiving half of a pairing handover (phone-service.ts wires phone-link-handover.ts). */
export interface PhoneGatewayHandover {
  key(): HandoverKey;
  receive(sealed: SealedHandover): Promise<{ deviceId: string; platform: 'ios' | 'android'; name?: string }>;
}

export interface PhoneGatewayDeps {
  token: string;
  runner: PhoneToolRunner;
  handover?: PhoneGatewayHandover;
}

export const PHONE_GATEWAY_TOKEN_HEADER = 'x-allternit-phone-token';

export function newGatewayToken(): string {
  return randomBytes(24).toString('hex');
}

function tokenOk(provided: unknown, expected: string): boolean {
  if (typeof provided !== 'string') return false;
  const a = Buffer.from(provided);
  const b = Buffer.from(expected);
  return a.length === b.length && timingSafeEqual(a, b);
}

export function handlePhoneGatewayRequest(
  deps: PhoneGatewayDeps,
  req: http.IncomingMessage,
  res: http.ServerResponse,
): void {
  const reply = (status: number, body: unknown) => {
    res.writeHead(status, { 'content-type': 'application/json' });
    res.end(JSON.stringify(body));
  };
  if (!tokenOk(req.headers[PHONE_GATEWAY_TOKEN_HEADER], deps.token)) return reply(403, { error: 'forbidden' });
  if (req.method === 'GET' && req.url === '/phone/tools') return reply(200, { tools: PHONE_TOOLS });
  if (req.url === '/phone/handover/key' || req.url === '/phone/handover') {
    const handover = deps.handover;
    if (!handover) return reply(404, { error: 'not_found' });
    if (req.method === 'GET' && req.url === '/phone/handover/key') return reply(200, handover.key());
    if (req.method !== 'POST' || req.url !== '/phone/handover') return reply(404, { error: 'not_found' });
    return readJson(req, (body) => {
      const sealed = (body as { sealed?: unknown } | null)?.sealed;
      if (!sealed || typeof sealed !== 'object') return reply(400, { error: 'sealed_required' });
      handover.receive(sealed as SealedHandover).then(
        (r) => reply(200, { ok: true, ...r }),
        (error: unknown) => {
          const code = (error as { code?: unknown })?.code;
          reply(400, { ok: false, error: typeof code === 'string' ? code : 'handover_failed', message: error instanceof Error ? error.message : String(error) });
        },
      );
    }, () => reply(400, { error: 'bad_json' }));
  }
  if (req.method !== 'POST' || req.url !== '/phone/call') return reply(404, { error: 'not_found' });

  readJson(req, (parsed) => {
    const body = (parsed ?? {}) as { name?: unknown; arguments?: unknown };
    if (typeof body.name !== 'string') return reply(400, { error: 'name_required' });
    deps.runner.call(body.name, body.arguments ?? {}).then(
      (result) => reply(200, result),
      (error: unknown) => reply(500, { ok: false, error: 'adb_failed', message: error instanceof Error ? error.message : String(error) }),
    );
  }, () => reply(400, { error: 'bad_json' }));
}

function readJson(req: http.IncomingMessage, ok: (body: unknown) => void, bad: () => void): void {
  let raw = '';
  req.on('data', (chunk: Buffer) => {
    raw += chunk.toString();
    if (raw.length > 64 * 1024) req.destroy();
  });
  req.on('end', () => {
    let body: unknown;
    try {
      body = JSON.parse(raw);
    } catch {
      return bad();
    }
    ok(body);
  });
}

export function startPhoneGateway(deps: PhoneGatewayDeps): Promise<{ url: string; close: () => void }> {
  const server = http.createServer((req, res) => handlePhoneGatewayRequest(deps, req, res));
  return new Promise((resolve, reject) => {
    server.once('error', reject);
    server.listen(0, '127.0.0.1', () => {
      const address = server.address();
      const port = typeof address === 'object' && address ? address.port : 0;
      resolve({ url: `http://127.0.0.1:${port}`, close: () => server.close() });
    });
  });
}
