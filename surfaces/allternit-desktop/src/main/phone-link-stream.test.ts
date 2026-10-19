import { EventEmitter } from 'node:events';
import { PassThrough } from 'node:stream';
import { describe, expect, it, vi } from 'vitest';
import type { RequestEnvelope } from './ictrl-client.js';
import type { LinkedPhone } from './phone-link-health.js';
import {
  buildHelperStreamRequest,
  HelperStreamError,
  NO_MESH_MESSAGE,
  openHelperStream,
  relaySerials,
  resolveHelperStreamHost,
  streamLaneFor,
} from './phone-link-stream.js';
import { helperServerArgs, randomScid, ScrcpySession, type ScrcpyHelperStreamRequest, type ScrcpySocket } from './phone-scrcpy.js';

const ENVELOPE: RequestEnvelope = { v: 1, tok: 't', ts: 1, nonce: 'n', ct: 'c', tag: 'g' };

/** A fake TCP duplex: what Desktop writes lands in `sent`; `reply` plays the phone. */
function fakeConn() {
  const conn = new PassThrough();
  const sent: Buffer[] = [];
  const toPhone = conn.write.bind(conn);
  // Writes from Desktop are captured, not echoed back as phone data.
  (conn as unknown as { write: (d: Buffer) => boolean }).write = (d: Buffer) => {
    sent.push(Buffer.from(d));
    return true;
  };
  const reply = (data: Buffer | string) => toPhone(typeof data === 'string' ? Buffer.from(data, 'latin1') : data);
  return { conn, sent, reply };
}

describe('openHelperStream', () => {
  it('posts the sealed body and hands the socket over on 101, leftover bytes first', async () => {
    const { conn, sent, reply } = fakeConn();
    const seal = vi.fn((_body: string) => ENVELOPE);
    const opening = openHelperStream({ host: '127.0.0.1:41000', seal, dial: () => conn }, { kind: 'video', scid: '0badc0de', args: ['audio=false'] });
    expect(JSON.parse(seal.mock.calls[0][0])).toEqual({ kind: 'video', scid: '0badc0de', args: ['audio=false'] });
    const req = Buffer.concat(sent).toString('utf8');
    expect(req.startsWith('POST /stream HTTP/1.1\r\nHost: 127.0.0.1:41000\r\n')).toBe(true);
    expect(req.endsWith(`\r\n\r\n${JSON.stringify(ENVELOPE)}`)).toBe(true);
    expect(req).toContain(`Content-Length: ${Buffer.byteLength(JSON.stringify(ENVELOPE))}`);
    reply('HTTP/1.1 101 Switching Protocols\r\nContent-Length: 0\r\nConnection: Upgrade\r\n\r\n\u0000AB');
    const socket = await opening;
    const got: Buffer[] = [];
    socket.on('data', (c) => got.push(Buffer.from(c)));
    reply('CD');
    await vi.waitFor(() => expect(Buffer.concat(got).toString('latin1')).toBe('\u0000ABCD'));
    socket.write(Buffer.from('ctl'));
    expect(sent.at(-1)?.toString()).toBe('ctl');
  });

  it('control requests carry no args', async () => {
    const { conn, reply } = fakeConn();
    const seal = vi.fn((_body: string) => ENVELOPE);
    const opening = openHelperStream({ host: '127.0.0.1:41000', seal, dial: () => conn }, { kind: 'control', scid: '0badc0de' });
    expect(JSON.parse(seal.mock.calls[0][0])).toEqual({ kind: 'control', scid: '0badc0de' });
    reply('HTTP/1.1 101 Switching Protocols\r\n\r\n');
    await expect(opening).resolves.toBeDefined();
  });

  it('turns 403 and 503 into the helper message', async () => {
    const forbidden = fakeConn();
    const a = openHelperStream({ host: 'h:1', seal: () => ENVELOPE, dial: () => forbidden.conn }, { kind: 'video', scid: '0badc0de' });
    const body403 = JSON.stringify({ error: { code: 'stream_forbidden', message: 'Live screen streams are available over the Allternit mesh only' } });
    forbidden.reply(`HTTP/1.1 403 Forbidden\r\nContent-Type: application/json\r\nContent-Length: ${body403.length}\r\n\r\n${body403}`);
    await expect(a).rejects.toMatchObject({ status: 403, code: 'stream_forbidden', message: 'Live screen streams are available over the Allternit mesh only' });

    const down = fakeConn();
    const b = openHelperStream({ host: 'h:1', seal: () => ENVELOPE, dial: () => down.conn }, { kind: 'video', scid: '0badc0de' });
    const body503 = JSON.stringify({ error: { code: 'stream_unavailable', message: 'Waiting for Wi-Fi to start the screen service.' } });
    // Split across packets: the body arrives after the head.
    down.reply(`HTTP/1.1 503 Service Unavailable\r\nContent-Length: ${body503.length}\r\n\r\n`);
    down.reply(body503);
    const err = await b.catch((e) => e);
    expect(err).toBeInstanceOf(HelperStreamError);
    expect(err.message).toBe("The phone's screen service isn't running: Waiting for Wi-Fi to start the screen service.");
  });

  it('says to update Phone Link when the route is missing, and fails on an early close', async () => {
    const old = fakeConn();
    const a = openHelperStream({ host: 'h:1', seal: () => ENVELOPE, dial: () => old.conn }, { kind: 'video', scid: '0badc0de' });
    old.reply('HTTP/1.1 404 Not Found\r\nContent-Length: 9\r\n\r\nNot Found');
    await expect(a).rejects.toThrow(/too old for the live screen/);

    const gone = fakeConn();
    const b = openHelperStream({ host: 'h:1', seal: () => ENVELOPE, dial: () => gone.conn }, { kind: 'video', scid: '0badc0de' });
    gone.conn.destroy();
    await expect(b).rejects.toThrow(/closed the connection before answering/);
  });

  it('builds a plain HTTP/1.1 request', () => {
    const req = buildHelperStreamRequest('127.0.0.1:9', ENVELOPE).toString();
    expect(req).toMatch(/^POST \/stream HTTP\/1\.1\r\n/);
    expect(req).toContain('Content-Type: application/json\r\n');
  });
});

// ── ScrcpySession in helper mode ─────────────────────────────────────────────

class FakeSocket extends EventEmitter {
  written: Buffer[] = [];
  write(data: Buffer): boolean {
    this.written.push(Buffer.from(data));
    return true;
  }
  destroy(): void {
    queueMicrotask(() => this.emit('close'));
  }
}

function header(): Buffer {
  const name = Buffer.alloc(64);
  Buffer.from('moto g', 'utf8').copy(name);
  const packet = Buffer.alloc(12);
  packet.writeUInt32BE(0x80000000, 0);
  packet.writeUInt32BE(720, 4);
  packet.writeUInt32BE(1600, 8);
  return Buffer.concat([Buffer.from([0]), name, Buffer.from('h264'), packet]);
}

describe('ScrcpySession helper mode', () => {
  async function run(audio: boolean) {
    const requests: ScrcpyHelperStreamRequest[] = [];
    const sockets: FakeSocket[] = [];
    const session = new ScrcpySession(
      {
        serial: 'link:dev-1',
        newScid: () => '1234abcd',
        openStream: async (req) => {
          requests.push(req);
          const s = new FakeSocket();
          sockets.push(s);
          return s as unknown as ScrcpySocket;
        },
      },
      { audio },
    );
    const meta = vi.fn();
    session.on('meta', meta);
    const started = session.start();
    // The video reader attaches right after the open resolves (the real socket buffers until then).
    await vi.waitFor(() => expect(sockets[0]?.listenerCount('data')).toBe(1));
    sockets[0].emit('data', header().subarray(0, 1)); // dummy byte: the server accepted
    await vi.waitFor(() => expect(sockets.length).toBe(audio ? 3 : 2));
    sockets[0].emit('data', header().subarray(1));
    await started;
    expect(meta).toHaveBeenCalledWith({ deviceName: 'moto g', width: 720, height: 1600, codec: 'h264' });
    return { requests, sockets, session };
  }

  it('opens video, then audio, then control, all on one scid', async () => {
    const { requests, sockets, session } = await run(true);
    expect(requests.map((r) => r.kind)).toEqual(['video', 'control', 'control']);
    expect(new Set(requests.map((r) => r.scid))).toEqual(new Set(['1234abcd']));
    expect(requests[0].args).toEqual(helperServerArgs({ audio: true }));
    // Input goes to the third socket (control), not the audio one.
    expect(session.sendControl([{ type: 'home' }]).ok).toBe(true);
    expect(sockets[2].written.length).toBeGreaterThan(0);
    expect(sockets[1].written.length).toBe(0);
    session.close();
  });

  it('opens video then control when audio is off', async () => {
    const { requests, session } = await run(false);
    expect(requests.map((r) => r.kind)).toEqual(['video', 'control']);
    expect(requests[0].args).toContain('audio=false');
    session.close();
  });

  it('fails start with the helper error and opens nothing else', async () => {
    const openStream = vi.fn(async () => {
      throw new Error("The phone's screen service isn't running: Stopped");
    });
    const session = new ScrcpySession({ serial: 'link:dev-1', openStream });
    const ended = vi.fn();
    session.on('ended', ended);
    await expect(session.start()).rejects.toThrow(/screen service isn't running/);
    expect(openStream).toHaveBeenCalledTimes(1);
    expect(ended).toHaveBeenCalledTimes(1);
  });
});

describe('helper server args', () => {
  it('drops the options the helper reserves and keeps the rest in its ARG alphabet', () => {
    const args = helperServerArgs({});
    for (const a of args) {
      expect(a).toMatch(/^[a-z_]{1,40}=[A-Za-z0-9_.:,+-]{0,200}$/);
      expect(['scid', 'tunnel_forward', 'log_level']).not.toContain(a.split('=')[0]);
    }
    expect(args).toContain('video_codec=h264');
    expect(args).toContain('audio_codec=opus');
  });

  it('makes 31-bit scids scrcpy can parse', () => {
    for (let i = 0; i < 50; i += 1) expect(randomScid()).toMatch(/^[0-7][0-9a-f]{7}$/);
  });
});

// ── Routing ─────────────────────────────────────────────────────────────────

const ANDROID: LinkedPhone = { deviceId: 'dev-a', platform: 'android', name: 'moto', meshHost: '100.64.0.9:8200', storeKey: 'k-a' };
const IPHONE: LinkedPhone = { deviceId: 'dev-i', platform: 'ios', name: 'iPhone', storeKey: 'k-i' };

describe('streamLaneFor', () => {
  it('routes link: serials by platform and adb serials to adb', () => {
    expect(streamLaneFor('link:dev-a', ANDROID)).toBe('android-helper');
    expect(streamLaneFor('link:dev-i', IPHONE)).toBe('ios-video');
    expect(streamLaneFor('192.168.1.20:5555', null)).toBe('adb');
    expect(streamLaneFor('link:nobody', null)).toBe('unlinked');
    expect(streamLaneFor('link:dev-x', ANDROID)).toBe('unlinked');
  });
});

describe('resolveHelperStreamHost', () => {
  const deps = {
    meshEnabled: true,
    meshProxy: async () => '127.0.0.1:41000',
    fetchWhoami: async () => ({ device_id: 'dev-a' }),
  };

  it('always uses the mesh proxy, verified as this phone', async () => {
    await expect(resolveHelperStreamHost(ANDROID, deps)).resolves.toBe('127.0.0.1:41000');
  });

  it('says so when the phone has no mesh address yet', async () => {
    await expect(resolveHelperStreamHost({ ...ANDROID, meshHost: undefined, lanHost: '192.168.1.5:8200' }, deps)).rejects.toThrow(NO_MESH_MESSAGE);
  });

  it('refuses a proxy that answers as another phone, or none', async () => {
    await expect(resolveHelperStreamHost(ANDROID, { ...deps, fetchWhoami: async () => ({ device_id: 'other' }) })).rejects.toThrow(/different phone/);
    await expect(resolveHelperStreamHost(ANDROID, { ...deps, fetchWhoami: async () => null })).rejects.toThrow(/over the Allternit mesh/);
    await expect(resolveHelperStreamHost(ANDROID, { ...deps, meshProxy: async () => Promise.reject(new Error('not signed in')) })).rejects.toThrow(/not signed in/);
  });

  it('restarts the phone mesh off the mesh when the route is dead, then retries once', async () => {
    let healed = 0;
    let alive = false;
    const host = await resolveHelperStreamHost(ANDROID, {
      ...deps,
      fetchWhoami: async () => (alive ? { device_id: 'dev-a' } : null),
      healMesh: async () => {
        healed++;
        alive = true;
        return { meshState: 'running', meshHost: ANDROID.meshHost ?? null };
      },
    });
    expect(host).toBe('127.0.0.1:41000');
    expect(healed).toBe(1);
  });

  it('gives the usual error when the heal is not possible or does not help', async () => {
    await expect(resolveHelperStreamHost(ANDROID, { ...deps, fetchWhoami: async () => null, healMesh: async () => null })).rejects.toThrow(/over the Allternit mesh/);
    let heals = 0;
    await expect(
      resolveHelperStreamHost(ANDROID, { ...deps, fetchWhoami: async () => null, healMesh: async () => (heals++, { meshState: 'starting', meshHost: null }) }),
    ).rejects.toThrow(/over the Allternit mesh/);
    expect(heals).toBe(1);
  });

  it('never heals when this computer cannot open the mesh', async () => {
    let heals = 0;
    await expect(
      resolveHelperStreamHost(ANDROID, { ...deps, meshProxy: async () => Promise.reject(new Error('not signed in')), healMesh: async () => (heals++, null) }),
    ).rejects.toThrow(/not signed in/);
    expect(heals).toBe(0);
  });
});

describe('relaySerials', () => {
  const link = { deviceId: 'dev-a', serial: 'ZY22ABC' };

  it('relays a reachable linked Android when its adb twin is not online', () => {
    expect(
      relaySerials({
        adb: [{ serial: '192.168.1.20:41234', id: 'ZY22ABC', state: 'lost' }],
        iosDeviceIds: ['dev-i'],
        androidLinks: [link],
        linkReachable: () => true,
      }),
    ).toEqual(['link:dev-i', 'link:dev-a']);
  });

  it('keeps the adb serial while adb is online, and skips unreachable links', () => {
    expect(
      relaySerials({
        adb: [{ serial: '192.168.1.20:41234', id: 'ZY22ABC', state: 'online' }],
        iosDeviceIds: [],
        androidLinks: [link],
        linkReachable: () => true,
      }),
    ).toEqual(['192.168.1.20:41234']);
    expect(
      relaySerials({
        adb: [{ serial: '192.168.1.20:41234', id: 'ZY22ABC', state: 'lost' }],
        iosDeviceIds: [],
        androidLinks: [link],
        linkReachable: () => false,
      }),
    ).toEqual(['192.168.1.20:41234']);
  });

  it('relays a linked Android with no adb phone at all', () => {
    expect(relaySerials({ adb: [], iosDeviceIds: [], androidLinks: [link], linkReachable: () => true })).toEqual(['link:dev-a']);
  });
});
