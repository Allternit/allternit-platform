import * as fs from 'node:fs';
import * as net from 'node:net';
import * as os from 'node:os';
import * as path from 'node:path';
import { afterEach, describe, expect, it } from 'vitest';

import { DriverRpcError, driverCall, parseDriverEndpoint } from './driver-rpc.js';

// A stand-in for the driver process (the edge): line JSON-RPC exactly as
// allternit_driver/server.py answers, including the Windows token check.
function fakeDriver(token: string | null, answer: (req: any) => unknown | 'hang' | 'split') {
  return net.createServer((socket) => {
    let buffer = '';
    socket.setEncoding('utf8');
    socket.on('data', (chunk: string) => {
      buffer += chunk;
      let i = buffer.indexOf('\n');
      while (i !== -1) {
        const req = JSON.parse(buffer.slice(0, i));
        buffer = buffer.slice(i + 1);
        i = buffer.indexOf('\n');
        if (token !== null && req.auth !== token) {
          socket.write(`${JSON.stringify({ jsonrpc: '2.0', id: req.id, error: { code: -32001, message: 'unauthorized' } })}\n`);
          continue;
        }
        const out = answer(req);
        if (out === 'hang') continue;
        const line = `${JSON.stringify(out instanceof Error
          ? { jsonrpc: '2.0', id: req.id, error: { code: -32000, message: out.message, data: { code: out.name } } }
          : { jsonrpc: '2.0', id: req.id, result: out })}\n`;
        if (out && typeof out === 'object' && (out as any).split) {
          // An answer that arrives in two packets, after an unrelated line.
          socket.write(`${JSON.stringify({ jsonrpc: '2.0', id: -1, result: {} })}\n${line.slice(0, 7)}`);
          setTimeout(() => socket.write(line.slice(7)), 20);
        } else socket.write(line);
      }
    });
  });
}

const servers: net.Server[] = [];
const dirs: string[] = [];
afterEach(async () => {
  for (const s of servers.splice(0)) await new Promise<void>((r) => s.close(() => r()));
  for (const d of dirs.splice(0)) fs.rmSync(d, { recursive: true, force: true });
});

async function listenUnix(server: net.Server): Promise<string> {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'drv-'));
  dirs.push(dir);
  const sock = path.join(dir, 'd.sock');
  servers.push(server);
  await new Promise<void>((r) => server.listen(sock, r));
  return `unix:${sock}`;
}
async function listenTcp(server: net.Server): Promise<string> {
  servers.push(server);
  await new Promise<void>((r) => server.listen(0, '127.0.0.1', r));
  return `tcp:127.0.0.1:${(server.address() as net.AddressInfo).port}`;
}

describe('driverCall', () => {
  it.skipIf(process.platform === 'win32')('returns the result over the unix socket', async () => {
    const endpoint = await listenUnix(fakeDriver(null, (req) => ({ method: req.method, params: req.params })));
    expect(await driverCall({ endpoint, token: null }, 'context', { pid: 42 })).toEqual({ method: 'context', params: { pid: 42 } });
  });

  it('sends the launch token over loopback TCP, and a wrong token is refused', async () => {
    const endpoint = await listenTcp(fakeDriver('s3cret', () => ({ ok: true })));
    expect(await driverCall({ endpoint, token: 's3cret' }, 'ping')).toEqual({ ok: true });
    await expect(driverCall({ endpoint, token: 'nope' }, 'ping')).rejects.toMatchObject({ code: 'unauthorized' });
  });

  it('turns a driver error into its code', async () => {
    const endpoint = await listenTcp(fakeDriver('t', () => Object.assign(new Error('Terminal isn\'t running'), { name: 'app_not_running' })));
    const error: DriverRpcError = await driverCall({ endpoint, token: "t" }, "read_ui", { pid: 1 }).catch((e: DriverRpcError) => e) as DriverRpcError;
    expect(error).toBeInstanceOf(DriverRpcError);
    expect(error.code).toBe('app_not_running');
  });

  it('waits for its own id across split packets', async () => {
    const endpoint = await listenTcp(fakeDriver('t', () => ({ split: true, value: 7 })));
    expect(await driverCall({ endpoint, token: 't' }, 'status')).toEqual({ split: true, value: 7 });
  });

  it('gives up after the timeout when the driver never answers', async () => {
    const endpoint = await listenTcp(fakeDriver('t', () => 'hang'));
    const started = Date.now();
    await expect(driverCall({ endpoint, token: 't' }, 'read_ui', {}, 150)).rejects.toMatchObject({ code: 'timeout' });
    expect(Date.now() - started).toBeLessThan(1000);
  });

  it('rejects when nothing listens, and never dials a non-loopback host', async () => {
    await expect(driverCall({ endpoint: 'unix:/nonexistent/driver.sock', token: null }, 'ping')).rejects.toMatchObject({ code: 'connect' });
    await expect(driverCall({ endpoint: 'tcp:10.0.0.5:9000', token: 't' }, 'ping')).rejects.toMatchObject({ code: 'bad_endpoint' });
    expect(parseDriverEndpoint('tcp:127.0.0.1:0')).toBeNull();
    expect(parseDriverEndpoint('tcp:127.0.0.1:5123')).toEqual({ kind: 'tcp', host: '127.0.0.1', port: 5123 });
  });
});
