/**
 * Subscription Gateway client — UDS transport + token resolution.
 *
 * The gateway listens on a unix domain socket (default
 * ~/.allternit/subscriptions/gateway.sock, mode 0600) and always requires a
 * scoped bearer token. The CLI token is issued by the gateway at boot and
 * stored in the macOS keychain (service com.allternit.subscription-gateway,
 * account cli-token).
 */
import { execFileSync } from 'node:child_process';
import http from 'node:http';
import { homedir } from 'node:os';
import { join } from 'node:path';

export function gatewayStateDir(env: NodeJS.ProcessEnv = process.env): string {
  return env.SUBS_GATEWAY_STATE_DIR ?? join(homedir(), '.allternit', 'subscriptions');
}

export function gatewaySocketPath(env: NodeJS.ProcessEnv = process.env): string {
  return join(gatewayStateDir(env), 'gateway.sock');
}

export function resolveGatewayToken(env: NodeJS.ProcessEnv = process.env): string {
  if (env.SUBS_GATEWAY_TOKEN) return env.SUBS_GATEWAY_TOKEN;
  try {
    return execFileSync(
      '/usr/bin/security',
      ['find-generic-password', '-s', 'com.allternit.subscription-gateway', '-a', 'cli-token', '-w'],
      { stdio: ['ignore', 'pipe', 'ignore'] },
    ).toString('utf8').trim();
  } catch {
    throw new Error(
      'no subscription-gateway token: set SUBS_GATEWAY_TOKEN, or start the gateway once ' +
        '(it stores a cli-token in the macOS keychain)',
    );
  }
}

export interface SubsClientOptions {
  socketPath?: string;
  token?: string;
}

export interface GatewayResponse<T = unknown> {
  status: number;
  body: T;
}

export class SubsClient {
  readonly socketPath: string;
  readonly token: string;

  constructor(options: SubsClientOptions = {}) {
    this.socketPath = options.socketPath ?? gatewaySocketPath();
    this.token = options.token ?? resolveGatewayToken();
  }

  request<T = unknown>(method: string, path: string, body?: unknown): Promise<GatewayResponse<T>> {
    return new Promise((resolve, reject) => {
      const req = http.request(
        {
          socketPath: this.socketPath,
          path,
          method,
          headers: {
            authorization: `Bearer ${this.token}`,
            accept: 'application/json',
            ...(body === undefined ? {} : { 'content-type': 'application/json' }),
          },
        },
        (res) => {
          let data = '';
          res.on('data', (chunk) => (data += chunk));
          res.on('end', () => {
            let parsed: unknown = data;
            try {
              parsed = JSON.parse(data);
            } catch {
              // plain-text body
            }
            resolve({ status: res.statusCode ?? 0, body: parsed as T });
          });
        },
      );
      req.on('error', (err) => {
        reject(
          new Error(
            `subscription-gateway unreachable at ${this.socketPath} (${err.message}) — is the daemon running?`,
          ),
        );
      });
      if (body !== undefined) req.write(JSON.stringify(body));
      req.end();
    });
  }

  async requestOk<T = unknown>(method: string, path: string, body?: unknown): Promise<T> {
    const res = await this.request<T>(method, path, body);
    if (res.status >= 400) {
      const payload = res.body as { error?: string; detail?: unknown };
      throw new Error(
        `${res.status}: ${payload?.error ?? 'request failed'}${payload?.detail ? ` — ${JSON.stringify(payload.detail)}` : ''}`,
      );
    }
    return res.body;
  }
}

export interface SseEvent {
  id: string | null;
  event: string;
  data: string;
}

/**
 * Follow a task's SSE stream until a terminal event kind arrives; acks all
 * received event ids in one POST /v1/events/ack at the end (D12).
 */
export function followTaskEvents(
  client: SubsClient,
  taskId: string,
  onEvent: (event: SseEvent, parsed: unknown) => void,
  isTerminal: (event: SseEvent, parsed: unknown) => boolean,
): Promise<{ acked: number }> {
  return new Promise((resolve, reject) => {
    const ids: string[] = [];
    let buffer = '';
    const req = http.get(
      {
        socketPath: client.socketPath,
        path: `/v1/tasks/${taskId}/events`,
        headers: { authorization: `Bearer ${client.token}` },
      },
      (res) => {
        res.on('data', (chunk) => {
          buffer += chunk.toString();
          const parts = buffer.split('\n\n');
          buffer = parts.pop() ?? '';
          for (const part of parts) {
            const lines = part.split('\n').filter((l) => l.length > 0);
            if (lines.every((l) => l.startsWith(':'))) continue; // heartbeat comment
            const ev: SseEvent = { id: null, event: 'message', data: '' };
            for (const line of lines) {
              if (line.startsWith('id: ')) ev.id = line.slice(4);
              else if (line.startsWith('event: ')) ev.event = line.slice(7);
              else if (line.startsWith('data: ')) ev.data = line.slice(6);
            }
            if (ev.data === '' && ev.id === null) continue;
            if (ev.id) ids.push(ev.id);
            let parsed: unknown = null;
            try {
              parsed = JSON.parse(ev.data);
            } catch {
              parsed = ev.data;
            }
            onEvent(ev, parsed);
            if (isTerminal(ev, parsed)) {
              req.destroy();
              void client
                .requestOk('POST', '/v1/events/ack', { event_ids: ids })
                .then(() => resolve({ acked: ids.length }))
                .catch(reject);
              return;
            }
          }
        });
        res.on('end', () => reject(new Error('event stream ended before a terminal event')));
      },
    );
    req.on('error', reject);
  });
}
