/**
 * usbmuxd client for Phone Link (iPhone): list devices, read the Mac's pair
 * record, and forward a device TCP port to 127.0.0.1.
 *
 * Why not `ios forward`: go-ios binds its forward to 0.0.0.0 (ios/forward,
 * net.Listen("tcp", "0.0.0.0:<port>")). The runner treats anything arriving
 * through usbmux as loopback, and during setup it holds a one-shot
 * auto-approved pairing (ICTRL_ALLOW_AUTH_ONCE), so a LAN-wide forward would
 * let anyone on the Mac's network race us for the key. This forward listens
 * on 127.0.0.1 only.
 *
 * Protocol: 16-byte little-endian header (length incl. header, version 1,
 * type 8 = plist, tag) + an XML plist; after a successful Connect the socket
 * becomes a raw TCP stream to the device port (PortNumber is big-endian).
 */

import * as net from 'node:net';

export const USBMUXD_SOCKET = '/var/run/usbmuxd';

export type PlistValue = string | number | boolean | Buffer | PlistValue[] | { [key: string]: PlistValue };

// ─── Minimal XML plist (the subset usbmuxd speaks) ───────────────────────────

const esc = (s: string) => s.replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;');

function encodeValue(value: PlistValue): string {
  if (Buffer.isBuffer(value)) return `<data>${value.toString('base64')}</data>`;
  if (typeof value === 'string') return `<string>${esc(value)}</string>`;
  if (typeof value === 'boolean') return value ? '<true/>' : '<false/>';
  if (typeof value === 'number') return Number.isInteger(value) ? `<integer>${value}</integer>` : `<real>${value}</real>`;
  if (Array.isArray(value)) return `<array>${value.map(encodeValue).join('')}</array>`;
  return `<dict>${Object.entries(value)
    .map(([k, v]) => `<key>${esc(k)}</key>${encodeValue(v)}`)
    .join('')}</dict>`;
}

export function encodePlist(value: PlistValue): string {
  return `<?xml version="1.0" encoding="UTF-8"?>\n<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">\n<plist version="1.0">${encodeValue(value)}</plist>\n`;
}

const unesc = (s: string) => s.replace(/&lt;/g, '<').replace(/&gt;/g, '>').replace(/&quot;/g, '"').replace(/&apos;/g, "'").replace(/&amp;/g, '&');

export function decodePlist(xml: string): PlistValue {
  const tokens = xml
    .replace(/<\?xml[^>]*>|<!DOCTYPE[^>]*>|<\/?plist[^>]*>/g, '')
    .match(/<[^>]+>|[^<]+/g)
    ?.filter((t) => t.trim() !== '') ?? [];
  let i = 0;
  const textUntil = (close: string): string => {
    let text = '';
    while (i < tokens.length && tokens[i] !== close) text += tokens[i++];
    i++; // the close tag
    return unesc(text);
  };
  const parse = (): PlistValue => {
    const tag = tokens[i++];
    switch (tag) {
      case '<dict/>':
        return {};
      case '<array/>':
        return [];
      case '<string/>':
        return '';
      case '<true/>':
        return true;
      case '<false/>':
        return false;
      case '<string>':
        return textUntil('</string>');
      case '<integer>':
        return Number(textUntil('</integer>'));
      case '<real>':
        return Number(textUntil('</real>'));
      case '<date>':
        return textUntil('</date>');
      case '<data>':
        return Buffer.from(textUntil('</data>').replace(/\s+/g, ''), 'base64');
      case '<array>': {
        const out: PlistValue[] = [];
        while (tokens[i] !== '</array>') out.push(parse());
        i++;
        return out;
      }
      case '<dict>': {
        const out: Record<string, PlistValue> = {};
        while (tokens[i] !== '</dict>') {
          if (tokens[i++] !== '<key>') throw new Error('plist: expected <key>');
          const key = textUntil('</key>');
          out[key] = parse();
        }
        i++;
        return out;
      }
      default:
        throw new Error(`plist: unexpected ${tag}`);
    }
  };
  return parse();
}

// ─── Wire ────────────────────────────────────────────────────────────────────

export function frameMessage(body: Record<string, PlistValue>, tag: number): Buffer {
  const payload = Buffer.from(encodePlist({ ProgName: 'Allternit', ClientVersionString: 'allternit-phone-link', ...body }), 'utf8');
  const header = Buffer.alloc(16);
  header.writeUInt32LE(16 + payload.length, 0);
  header.writeUInt32LE(1, 4);
  header.writeUInt32LE(8, 8);
  header.writeUInt32LE(tag, 12);
  return Buffer.concat([header, payload]);
}

/** One request/response on a fresh connection; resolves with the socket still open (for Connect). */
function exchange(socketPath: string, body: Record<string, PlistValue>, timeoutMs = 5000): Promise<{ reply: Record<string, PlistValue>; socket: net.Socket }> {
  return new Promise((resolve, reject) => {
    const socket = net.createConnection(socketPath);
    let buffer = Buffer.alloc(0);
    const timer = setTimeout(() => {
      socket.destroy();
      reject(new Error('usbmuxd did not answer'));
    }, timeoutMs);
    const onData = (chunk: Buffer) => {
      buffer = Buffer.concat([buffer, chunk]);
      if (buffer.length < 16) return;
      const length = buffer.readUInt32LE(0);
      if (buffer.length < length) return;
      clearTimeout(timer);
      socket.off('data', onData);
      socket.pause(); // until the caller pipes it (Connect) or closes it
      const rest = buffer.subarray(length);
      if (rest.length) socket.unshift(rest);
      try {
        resolve({ reply: decodePlist(buffer.subarray(16, length).toString('utf8')) as Record<string, PlistValue>, socket });
      } catch (error) {
        socket.destroy();
        reject(error);
      }
    };
    socket.on('data', onData);
    socket.once('error', (error) => {
      clearTimeout(timer);
      reject(error);
    });
    socket.once('connect', () => socket.write(frameMessage(body, 1)));
  });
}

export interface UsbmuxDevice {
  deviceId: number;
  udid: string;
  /** 'USB' or 'Network' (Wi-Fi sync). */
  connectionType: string;
}

export async function listUsbmuxDevices(socketPath = USBMUXD_SOCKET): Promise<UsbmuxDevice[]> {
  const { reply, socket } = await exchange(socketPath, { MessageType: 'ListDevices' });
  socket.destroy();
  const list = Array.isArray(reply.DeviceList) ? reply.DeviceList : [];
  return list.flatMap((entry) => {
    const e = entry as Record<string, PlistValue>;
    const props = (e.Properties ?? {}) as Record<string, PlistValue>;
    const deviceId = Number(e.DeviceID ?? props.DeviceID);
    const udid = String(props.SerialNumber ?? '');
    return udid && Number.isFinite(deviceId) ? [{ deviceId, udid, connectionType: String(props.ConnectionType ?? 'USB') }] : [];
  });
}

/**
 * The Mac's lockdown pair record for this phone (plist bytes), the "pairing
 * file" StikDebug/iCTRL's on-device mounter imports. Contains the Mac's host
 * private key for this phone: only ever written into the iCTRL app's own
 * container on that same phone, never to disk here.
 */
export async function readPairRecord(udid: string, socketPath = USBMUXD_SOCKET): Promise<Buffer | null> {
  const { reply, socket } = await exchange(socketPath, { MessageType: 'ReadPairRecord', PairRecordID: udid });
  socket.destroy();
  return Buffer.isBuffer(reply.PairRecordData) ? reply.PairRecordData : null;
}

export async function usbmuxConnect(deviceId: number, port: number, socketPath = USBMUXD_SOCKET): Promise<net.Socket> {
  const { reply, socket } = await exchange(socketPath, {
    MessageType: 'Connect',
    DeviceID: deviceId,
    PortNumber: ((port & 0xff) << 8) | ((port >> 8) & 0xff),
  });
  if (reply.MessageType !== 'Result' || Number(reply.Number) !== 0) {
    socket.destroy();
    throw new Error(`usbmuxd refused the connection to port ${port} (result ${String(reply.Number)})`);
  }
  return socket;
}

export interface LoopbackForward {
  port: number;
  host: '127.0.0.1';
  close(): Promise<void>;
}

/** 127.0.0.1:<random> → device:<devicePort>, resolving the device per connection (USB preferred). */
export async function startLoopbackForward(
  udid: string,
  devicePort: number,
  opts: { socketPath?: string; hostPort?: number } = {},
): Promise<LoopbackForward> {
  const socketPath = opts.socketPath ?? USBMUXD_SOCKET;
  const sockets = new Set<net.Socket>();
  const server = net.createServer(async (local) => {
    sockets.add(local);
    local.once('close', () => sockets.delete(local));
    local.pause();
    try {
      const devices = (await listUsbmuxDevices(socketPath)).filter((d) => d.udid === udid);
      const device = devices.find((d) => d.connectionType === 'USB') ?? devices[0];
      if (!device) throw new Error('phone not connected');
      const remote = await usbmuxConnect(device.deviceId, devicePort, socketPath);
      sockets.add(remote);
      remote.once('close', () => {
        sockets.delete(remote);
        local.destroy();
      });
      remote.on('error', () => local.destroy());
      local.on('error', () => remote.destroy());
      local.once('close', () => remote.destroy());
      local.pipe(remote);
      remote.pipe(local);
      local.resume();
    } catch {
      local.destroy();
    }
  });
  await new Promise<void>((resolve, reject) => {
    server.once('error', reject);
    server.listen(opts.hostPort ?? 0, '127.0.0.1', () => resolve());
  });
  const port = (server.address() as net.AddressInfo).port;
  return {
    port,
    host: '127.0.0.1',
    close: () =>
      new Promise<void>((resolve) => {
        for (const s of sockets) s.destroy();
        server.close(() => resolve());
      }),
  };
}
