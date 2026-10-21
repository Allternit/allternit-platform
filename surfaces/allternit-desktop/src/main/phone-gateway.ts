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
 */

import * as http from 'node:http';
import { randomBytes, timingSafeEqual } from 'node:crypto';
import { PHONE_TOOLS, type PhoneToolRunner } from './phone-tools.js';

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
  deps: { token: string; runner: PhoneToolRunner },
  req: http.IncomingMessage,
  res: http.ServerResponse,
): void {
  const reply = (status: number, body: unknown) => {
    res.writeHead(status, { 'content-type': 'application/json' });
    res.end(JSON.stringify(body));
  };
  if (!tokenOk(req.headers[PHONE_GATEWAY_TOKEN_HEADER], deps.token)) return reply(403, { error: 'forbidden' });
  if (req.method === 'GET' && req.url === '/phone/tools') return reply(200, { tools: PHONE_TOOLS });
  if (req.method !== 'POST' || req.url !== '/phone/call') return reply(404, { error: 'not_found' });

  let raw = '';
  req.on('data', (chunk: Buffer) => {
    raw += chunk.toString();
    if (raw.length > 64 * 1024) req.destroy();
  });
  req.on('end', () => {
    let body: { name?: unknown; arguments?: unknown };
    try {
      body = JSON.parse(raw) as typeof body;
    } catch {
      return reply(400, { error: 'bad_json' });
    }
    if (typeof body.name !== 'string') return reply(400, { error: 'name_required' });
    deps.runner.call(body.name, body.arguments ?? {}).then(
      (result) => reply(200, result),
      (error: unknown) => reply(500, { ok: false, error: 'adb_failed', message: error instanceof Error ? error.message : String(error) }),
    );
  });
}

export function startPhoneGateway(deps: { token: string; runner: PhoneToolRunner }): Promise<{ url: string; close: () => void }> {
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
