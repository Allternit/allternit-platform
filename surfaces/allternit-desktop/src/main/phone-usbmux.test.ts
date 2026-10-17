import { afterEach, describe, expect, it } from 'vitest';
import * as fs from 'node:fs';
import * as net from 'node:net';
import * as os from 'node:os';
import * as path from 'node:path';
import { decodePlist, encodePlist, listUsbmuxDevices, readPairRecord, startLoopbackForward, type PlistValue } from './phone-usbmux.js';

/** A usbmuxd stand-in on a unix socket: ListDevices, ReadPairRecord, Connect → echo server. */
function fakeUsbmuxd(dir: string, echoPort: number) {
  const socketPath = path.join(dir, 'usbmuxd');
  const connects: { DeviceID: number; PortNumber: number }[] = [];
  const server = net.createServer((sock) => {
    let buf = Buffer.alloc(0);
    const onData = (chunk: Buffer) => {
      buf = Buffer.concat([buf, chunk]);
      if (buf.length < 16 || buf.length < buf.readUInt32LE(0)) return;
      const len = buf.readUInt32LE(0);
      const msg = decodePlist(buf.subarray(16, len).toString()) as Record<string, PlistValue>;
      sock.off('data', onData);
      const reply = (body: Record<string, PlistValue>) => {
        const payload = Buffer.from(encodePlist(body));
        const header = Buffer.alloc(16);
        header.writeUInt32LE(16 + payload.length, 0);
        header.writeUInt32LE(1, 4);
        header.writeUInt32LE(8, 8);
        header.writeUInt32LE(1, 12);
        sock.write(Buffer.concat([header, payload]));
      };
      if (msg.MessageType === 'ListDevices') {
        reply({
          DeviceList: [
            { DeviceID: 7, MessageType: 'Attached', Properties: { SerialNumber: 'UDID-1', ConnectionType: 'Network', DeviceID: 7 } },
            { DeviceID: 3, MessageType: 'Attached', Properties: { SerialNumber: 'UDID-1', ConnectionType: 'USB', DeviceID: 3 } },
          ],
        });
        sock.end();
      } else if (msg.MessageType === 'ReadPairRecord') {
        reply({ PairRecordData: Buffer.from('<plist>pair</plist>') });
        sock.end();
      } else if (msg.MessageType === 'Connect') {
        connects.push({ DeviceID: Number(msg.DeviceID), PortNumber: Number(msg.PortNumber) });
        reply({ MessageType: 'Result', Number: 0 });
        const upstream = net.createConnection(echoPort, '127.0.0.1');
        sock.pipe(upstream);
        upstream.pipe(sock);
      }
    };
    sock.on('data', onData);
  });
  return { socketPath, server, connects };
}

const cleanups: (() => Promise<void> | void)[] = [];
afterEach(async () => {
  for (const fn of cleanups.splice(0)) await fn();
});

describe('plist subset', () => {
  it('round-trips the shapes usbmuxd uses', () => {
    const value = { a: 'x & <y>', n: 42, t: true, f: false, d: Buffer.from([1, 2, 3]), arr: [{ k: 'v' }, 'z'], e: {} };
    expect(decodePlist(encodePlist(value))).toEqual(value);
  });
});

describe('usbmuxd client', () => {
  it('lists devices, reads the pair record, and forwards on 127.0.0.1 only (USB preferred)', async () => {
    const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'mux-'));
    const echo = net.createServer((s) => s.pipe(s));
    await new Promise<void>((r) => echo.listen(0, '127.0.0.1', r));
    const mux = fakeUsbmuxd(dir, (echo.address() as net.AddressInfo).port);
    await new Promise<void>((r) => mux.server.listen(mux.socketPath, r));
    cleanups.push(() => new Promise((r) => echo.close(() => r())), () => new Promise((r) => mux.server.close(() => r())), () => fs.rmSync(dir, { recursive: true, force: true }));

    expect(await listUsbmuxDevices(mux.socketPath)).toEqual([
      { deviceId: 7, udid: 'UDID-1', connectionType: 'Network' },
      { deviceId: 3, udid: 'UDID-1', connectionType: 'USB' },
    ]);
    expect((await readPairRecord('UDID-1', mux.socketPath))?.toString()).toBe('<plist>pair</plist>');

    const fwd = await startLoopbackForward('UDID-1', 8200, { socketPath: mux.socketPath });
    cleanups.unshift(() => fwd.close());
    expect(fwd.host).toBe('127.0.0.1');
    const reply = await new Promise<string>((resolve, reject) => {
      const c = net.createConnection(fwd.port, '127.0.0.1', () => c.write('ping'));
      c.once('data', (d) => {
        resolve(d.toString());
        c.destroy();
      });
      c.once('error', reject);
    });
    expect(reply).toBe('ping');
    // usbmuxd wants the port big-endian: 8200 = 0x2008 → 0x0820.
    expect(mux.connects).toEqual([{ DeviceID: 3, PortNumber: 0x0820 }]);
  });
});
