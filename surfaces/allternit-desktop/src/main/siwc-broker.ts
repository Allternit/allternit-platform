/**
 * Loopback token broker for gizzi-code (Sign in with ChatGPT).
 *
 * gizzi-code makes the model calls, but the refresh token and its rotation
 * stay in Desktop main: gizzi asks this broker for a short-lived access token
 * per request. The broker binds 127.0.0.1 on an ephemeral port, requires a
 * per-launch bearer secret (handed to gizzi through its environment), and
 * answers only while the `feature.siwc` flag is on and the user is signed in
 * with ChatGPT plan usage. It never returns the refresh or ID token.
 */

import * as http from 'node:http';
import type { AddressInfo } from 'node:net';
import { randomBytes } from 'node:crypto';
import { constantTimeEqual } from './mini-app-oauth-broker.js';
import type { SiwcManager } from './siwc.js';

export interface SiwcBroker {
  /** Environment for gizzi-code: broker URL + secret. */
  env(): Record<string, string>;
  close(): void;
}

export const SIWC_BROKER_URL_ENV = 'ALLTERNIT_SIWC_BROKER_URL';
export const SIWC_BROKER_TOKEN_ENV = 'ALLTERNIT_SIWC_BROKER_TOKEN';

export async function startSiwcBroker(manager: SiwcManager): Promise<SiwcBroker> {
  const secret = randomBytes(32).toString('base64url');
  const server = http.createServer(async (req, res) => {
    const send = (code: number, body: unknown) => {
      res.writeHead(code, { 'Content-Type': 'application/json', 'Cache-Control': 'no-store' });
      res.end(JSON.stringify(body));
    };
    const auth = /^Bearer (.+)$/.exec(String(req.headers.authorization ?? ''));
    if (!auth || !constantTimeEqual(auth[1], secret)) return send(401, { error: 'unauthorized' });
    if (req.method !== 'GET') return send(405, { error: 'method_not_allowed' });
    const path = (req.url ?? '').split('?')[0];
    try {
      if (path === '/v1/token') {
        const t = await manager.accessToken();
        if (!t) return send(409, { error: 'not_available', state: manager.status().state });
        return send(200, { access_token: t.token, expires_at: t.expiresAt, email: t.email });
      }
      if (path === '/v1/models') {
        return send(200, { models: await manager.models() });
      }
      return send(404, { error: 'not_found' });
    } catch (err) {
      return send(409, { error: 'not_available', state: manager.status().state, detail: err instanceof Error ? err.message : String(err) });
    }
  });
  await new Promise<void>((resolve, reject) => {
    server.once('error', reject);
    server.listen(0, '127.0.0.1', () => resolve());
  });
  const port = (server.address() as AddressInfo).port;
  return {
    env: () => ({
      [SIWC_BROKER_URL_ENV]: `http://127.0.0.1:${port}`,
      [SIWC_BROKER_TOKEN_ENV]: secret,
    }),
    close: () => {
      server.closeAllConnections?.();
      server.close();
    },
  };
}
