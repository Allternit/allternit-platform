// driver-rpc.ts — a small client for the bundled Allternit Driver's local API.
//
// The driver (domains/computer-use/driver/allternit_driver/server.py) speaks
// JSON-RPC 2.0, one JSON object per line: a private unix socket on macOS and
// Linux, loopback TCP on Windows where every request carries the launch token
// in `auth`. computer-use-driver-manager.ts starts it and hands out the
// endpoint. One connection per call keeps this stateless: the pet makes at most
// two calls per roam tick.
//
// Privacy: answers can hold screen text. Nothing here logs a request or an answer.

import * as net from 'node:net';

/** Where the driver listens: `unix:<path>` or `tcp:127.0.0.1:<port>`, plus the Windows token. */
export interface DriverEndpoint {
  endpoint: string;
  token: string | null;
}

export type ParsedDriverEndpoint = { kind: 'unix'; path: string } | { kind: 'tcp'; host: string; port: number };

/** The driver's own error, with its machine code (`data.code`, e.g. `app_not_running`). */
export class DriverRpcError extends Error {
  constructor(public readonly code: string, message: string) {
    super(message);
    this.name = 'DriverRpcError';
  }
}

export const DRIVER_RPC_TIMEOUT_MS = 1500;
/** A read_ui answer with 120 elements is well under this; anything bigger is cut off as an error. */
const MAX_ANSWER_BYTES = 4 * 1024 * 1024;

export function parseDriverEndpoint(endpoint: string): ParsedDriverEndpoint | null {
  if (endpoint.startsWith('unix:') && endpoint.length > 5) return { kind: 'unix', path: endpoint.slice(5) };
  if (endpoint.startsWith('tcp:')) {
    const rest = endpoint.slice(4);
    const colon = rest.lastIndexOf(':');
    const host = rest.slice(0, colon);
    const port = Number(rest.slice(colon + 1));
    // The driver only ever listens on loopback; refuse anything else.
    if (colon > 0 && (host === '127.0.0.1' || host === 'localhost') && Number.isInteger(port) && port > 0 && port < 65536) {
      return { kind: 'tcp', host, port };
    }
  }
  return null;
}

let nextId = 1;

/**
 * Call one driver method. Resolves with `result`, rejects with a DriverRpcError
 * (`code` from the driver, or `timeout`, `bad_endpoint`, `connect`, `bad_answer`).
 */
export function driverCall<T = unknown>(
  where: DriverEndpoint,
  method: string,
  params: Record<string, unknown> = {},
  timeoutMs = DRIVER_RPC_TIMEOUT_MS,
): Promise<T> {
  const parsed = parseDriverEndpoint(where.endpoint);
  if (!parsed) return Promise.reject(new DriverRpcError('bad_endpoint', 'The driver endpoint is not a unix socket or loopback port.'));
  const id = nextId++;
  const request: Record<string, unknown> = { jsonrpc: '2.0', id, method, params };
  if (parsed.kind === 'tcp') request.auth = where.token ?? '';
  return new Promise<T>((resolve, reject) => {
    const socket = parsed.kind === 'unix'
      ? net.createConnection(parsed.path)
      : net.createConnection({ host: parsed.host, port: parsed.port });
    let buffer = '';
    let settled = false;
    const finish = (error: Error | null, value?: T) => {
      if (settled) return;
      settled = true;
      clearTimeout(timer);
      socket.removeAllListeners('data');
      socket.destroy();
      if (error) reject(error);
      else resolve(value as T);
    };
    const timer = setTimeout(() => finish(new DriverRpcError('timeout', `The driver did not answer ${method} within ${timeoutMs} ms.`)), timeoutMs);
    socket.setEncoding('utf8');
    socket.once('connect', () => socket.write(`${JSON.stringify(request)}\n`));
    socket.on('error', (error: NodeJS.ErrnoException) => finish(new DriverRpcError('connect', `The driver is not reachable (${error.code ?? 'error'}).`)));
    socket.on('close', () => finish(new DriverRpcError('connect', 'The driver closed the connection.')));
    socket.on('data', (chunk: string) => {
      buffer += chunk;
      if (buffer.length > MAX_ANSWER_BYTES) { finish(new DriverRpcError('bad_answer', 'The driver answer is too large.')); return; }
      let newline = buffer.indexOf('\n');
      while (newline !== -1) {
        const line = buffer.slice(0, newline).trim();
        buffer = buffer.slice(newline + 1);
        newline = buffer.indexOf('\n');
        if (!line) continue;
        let message: { id?: unknown; result?: unknown; error?: { code?: unknown; message?: unknown; data?: { code?: unknown } } };
        try {
          message = JSON.parse(line);
        } catch {
          finish(new DriverRpcError('bad_answer', 'The driver sent a line that is not JSON.'));
          return;
        }
        if (message.id !== id) continue;
        if (message.error) {
          const code = typeof message.error.data?.code === 'string' ? message.error.data.code
            : message.error.code === -32001 ? 'unauthorized' : 'error';
          finish(new DriverRpcError(code, typeof message.error.message === 'string' ? message.error.message : 'driver error'));
        } else {
          finish(null, message.result as T);
        }
        return;
      }
    });
  });
}
