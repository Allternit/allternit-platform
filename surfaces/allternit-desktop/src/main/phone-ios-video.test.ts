import { PassThrough } from 'node:stream';
import { afterEach, describe, expect, it, vi } from 'vitest';
import {
  encodeIosVideoPacket,
  IOS_PACKET_AUDIO,
  IOS_PACKET_AUDIO_CONFIG,
  IOS_PACKET_CONFIG,
  IOS_PACKET_ENDED,
  IOS_PACKET_FRAME,
  IOS_PACKET_STATUS,
  IOS_HELPER_LOST_MESSAGE,
  IOS_START_TIMEOUT_MESSAGE,
  IosGestureMapper,
  IosVideoPacketParser,
  IosVideoSession,
  parseHttpHead,
  type IosVideoConfig,
} from './phone-ios-video.js';
import type { PhoneAudioMeta, PhoneStreamMeta, ScrcpyDeviceMessage } from './phone-scrcpy.js';

const OK_HEAD = 'HTTP/1.1 200 OK\r\nContent-Type: application/x-ictrl-h264\r\nCache-Control: no-store\r\nConnection: close\r\n\r\n';
const CONFIG = { width: 1170, height: 2532, scale: 3, rotation: 0 };
const IDR = Buffer.from([0, 0, 0, 1, 0x67, 1, 2, 0, 0, 0, 1, 0x68, 3, 0, 0, 0, 1, 0x65, 9, 9]);

const status = (broadcasting: boolean) => encodeIosVideoPacket(IOS_PACKET_STATUS, { broadcasting });
const config = (c: object = CONFIG) => encodeIosVideoPacket(IOS_PACKET_CONFIG, c);
const frame = (data: Buffer = IDR) => encodeIosVideoPacket(IOS_PACKET_FRAME, data);
const ended = (reason: string) => encodeIosVideoPacket(IOS_PACKET_ENDED, { reason });

const flush = () => new Promise<void>((resolve) => setImmediate(resolve));

/** A session over a fake socket; the test writes the runner's bytes into `socket`. */
function makeSession(opts: { startTimeoutMs?: number; startLiveScreen?: () => Promise<unknown> } = {}) {
  const socket = new PassThrough();
  const calls: Array<{ method: string; params: Record<string, unknown> }> = [];
  const call = vi.fn(async (method: string, params: Record<string, unknown>) => {
    calls.push({ method, params });
    if (method === 'stream_ticket') return { ticket: 'T-1', expires_in: 60 };
    if (method === 'start_live_screen') return (opts.startLiveScreen ?? (async () => ({ ok: true, broadcasting: false, needsTap: true })))();
    return { ok: true };
  });
  const connect = vi.fn((_host: string, _path: string) => socket);
  let clock = 1_000;
  const session = new IosVideoSession({
    call,
    host: async () => '127.0.0.1:52001',
    connect,
    deviceName: 'iPhone 15',
    now: () => clock,
    ...(opts.startTimeoutMs !== undefined ? { startTimeoutMs: opts.startTimeoutMs } : {}),
  });
  const metas: PhoneStreamMeta[] = [];
  const chunks: Buffer[] = [];
  const messages: ScrcpyDeviceMessage[] = [];
  const ends: string[] = [];
  session.on('meta', (m) => metas.push(m));
  session.on('videoChunk', (c) => chunks.push(c));
  session.on('deviceMessage', (m) => messages.push(m));
  session.on('ended', (e) => ends.push(e.reason));
  return {
    session,
    socket,
    call,
    calls,
    connect,
    metas,
    chunks,
    messages,
    ends,
    tick: (ms: number) => {
      clock += ms;
    },
  };
}

async function goLive(s: ReturnType<typeof makeSession>) {
  const started = s.session.start();
  await flush();
  s.socket.write(Buffer.concat([Buffer.from(OK_HEAD), status(true), config()]));
  await started;
}

afterEach(() => {
  vi.useRealTimers();
});

describe('IosVideoPacketParser', () => {
  it('parses packets across arbitrary chunk splits', () => {
    const stream = Buffer.concat([status(false), config(), frame(), frame(Buffer.from([0, 0, 0, 1, 0x41, 7])), ended('user stopped')]);
    for (const step of [1, 2, 3, 5, 7, stream.length]) {
      const parser = new IosVideoPacketParser();
      const out = [];
      for (let i = 0; i < stream.length; i += step) out.push(...parser.push(stream.subarray(i, i + step)));
      expect(out).toEqual([
        { type: 'status', broadcasting: false },
        { type: 'config', width: 1170, height: 2532, scale: 3, rotation: 0 },
        { type: 'frame', data: IDR },
        { type: 'frame', data: Buffer.from([0, 0, 0, 1, 0x41, 7]) },
        { type: 'ended', reason: 'user stopped' },
      ]);
    }
  });

  it('skips malformed and unknown packets, rejects oversized ones', () => {
    const parser = new IosVideoPacketParser();
    const out = parser.push(
      Buffer.concat([encodeIosVideoPacket(IOS_PACKET_CONFIG, 'not json'), encodeIosVideoPacket(9, 'x'), config({ width: 0, height: 10 }), status(true)]),
    );
    expect(out).toEqual([{ type: 'status', broadcasting: true }]);
    const huge = Buffer.alloc(5);
    huge[0] = IOS_PACKET_FRAME;
    huge.writeUInt32BE(64 * 1024 * 1024, 1);
    expect(() => new IosVideoPacketParser().push(huge)).toThrow(/too large/);
  });

  it('parses the HTTP head and leaves the packet bytes after it', () => {
    expect(parseHttpHead(Buffer.from('HTTP/1.1 200 OK\r\nX: 1'))).toBeNull();
    const head = parseHttpHead(Buffer.concat([Buffer.from(OK_HEAD), status(true)]))!;
    expect(head.status).toBe(200);
    expect(head.headers['content-type']).toBe('application/x-ictrl-h264');
    expect(head.rest).toEqual(status(true));
    expect(() => parseHttpHead(Buffer.from('garbage\r\n\r\n'))).toThrow(/malformed/);
  });
});

describe('IosGestureMapper', () => {
  const screen: IosVideoConfig = { width: 1170, height: 2532, scale: 3, rotation: 0 };
  const touch = (action: 'down' | 'move' | 'up', x: number, y: number, pointerId = 0) => ({ type: 'touch' as const, action, pointerId, x, y });

  it('maps a short still touch to a tap in points', () => {
    const m = new IosGestureMapper();
    expect(m.map(touch('down', 300, 600), screen, 0)).toEqual({ ok: true, rpcs: [] });
    expect(m.map(touch('move', 304, 603), screen, 50)).toEqual({ ok: true, rpcs: [] });
    expect(m.map(touch('up', 304, 603), screen, 120)).toEqual({ ok: true, rpcs: [{ method: 'tap', params: { x: 100, y: 200 } }] });
  });

  it('maps a held still touch to long_press with its duration in seconds', () => {
    const m = new IosGestureMapper();
    m.map(touch('down', 300, 600), screen, 0);
    expect(m.map(touch('up', 302, 601), screen, 900)).toEqual({
      ok: true,
      rpcs: [{ method: 'long_press', params: { x: 100, y: 200, duration: 0.9 } }],
    });
  });

  it('maps a moving touch to a drag from start to end', () => {
    const m = new IosGestureMapper();
    m.map(touch('down', 600, 1800), screen, 0);
    m.map(touch('move', 600, 1200), screen, 80);
    expect(m.map(touch('up', 600, 600), screen, 160)).toEqual({
      ok: true,
      rpcs: [{ method: 'drag', params: { from_x: 200, from_y: 600, to_x: 200, to_y: 200, duration: 0.16 } }],
    });
  });

  it('ignores a second pointer while one is down', () => {
    const m = new IosGestureMapper();
    m.map(touch('down', 300, 600, 0), screen, 0);
    expect(m.map(touch('down', 900, 900, 1), screen, 10)).toEqual({ ok: true, rpcs: [] });
    expect(m.map(touch('up', 900, 900, 1), screen, 20)).toEqual({ ok: true, rpcs: [] });
    expect(m.map(touch('up', 300, 600, 0), screen, 30).ok).toBe(true);
  });

  it('maps scroll, back, text and home; refuses Android-only keys', () => {
    const m = new IosGestureMapper();
    expect(m.map({ type: 'scroll', x: 300, y: 600, vScroll: 1 }, screen, 0)).toEqual({
      ok: true,
      rpcs: [{ method: 'swipe', params: { x: 100, y: 200, direction: 'down', distance: 120 } }],
    });
    expect(m.map({ type: 'scroll', x: 300, y: 600, vScroll: -2 }, screen, 0)).toMatchObject({ rpcs: [{ params: { direction: 'up' } }] });
    expect(m.map({ type: 'back' }, screen, 0)).toEqual({
      ok: true,
      rpcs: [{ method: 'drag', params: { from_x: 2, from_y: 422, to_x: 234, to_y: 422, duration: 0.3 } }],
    });
    expect(m.map({ type: 'text', text: 'hi' }, screen, 0)).toEqual({ ok: true, rpcs: [{ method: 'type', params: { text: 'hi' } }] });
    expect(m.map({ type: 'home' }, screen, 0)).toEqual({ ok: true, rpcs: [{ method: 'home', params: {} }] });
    for (const event of [{ type: 'key', action: 'down', keyCode: 4 }, { type: 'app_switch' }, { type: 'power' }] as const) {
      expect(m.map(event, screen, 0)).toEqual({ ok: false, message: 'Not available on iPhone.' });
    }
  });
});

describe('IosVideoSession', () => {
  it('mints a ticket, opens /video, goes live on the first CONFIG and forwards frames', async () => {
    const s = makeSession();
    const started = s.session.start();
    await flush();
    expect(s.calls[0]).toEqual({ method: 'stream_ticket', params: {} });
    expect(s.connect).toHaveBeenCalledWith('127.0.0.1:52001', '/video?ticket=T-1');

    // The head and packets arrive split mid-header and mid-packet.
    const bytes = Buffer.concat([Buffer.from(OK_HEAD), status(true), config(), frame()]);
    s.socket.write(bytes.subarray(0, 20));
    s.socket.write(bytes.subarray(20, OK_HEAD.length + 3));
    s.socket.write(bytes.subarray(OK_HEAD.length + 3));
    await started;
    await flush();
    expect(s.metas).toEqual([{ deviceName: 'iPhone 15', width: 1170, height: 2532, codec: 'h264' }]);
    expect(s.chunks).toEqual([IDR]);
    expect(s.calls.some((c) => c.method === 'start_live_screen')).toBe(false);
  });

  it('asks the runner to start the live screen when not broadcasting, and tolerates its failure', async () => {
    const s = makeSession({ startLiveScreen: async () => Promise.reject(new Error('picker not found')) });
    const started = s.session.start();
    await flush();
    s.socket.write(Buffer.concat([Buffer.from(OK_HEAD), status(false), status(false)]));
    await flush();
    expect(s.calls.filter((c) => c.method === 'start_live_screen')).toHaveLength(1);
    s.socket.write(config());
    await started;
    expect(s.metas).toHaveLength(1);
  });

  it('emits a size message on a new CONFIG size, ends on ENDED after live', async () => {
    const s = makeSession();
    await goLive(s);
    s.socket.write(config());
    s.socket.write(config({ width: 2532, height: 1170, scale: 3, rotation: 90 }));
    await flush();
    expect(s.messages).toEqual([{ type: 'size', width: 2532, height: 1170, clientResized: false }]);
    s.socket.write(ended('Stopped from Control Center'));
    await flush();
    expect(s.ends).toEqual(['Stopped from Control Center']);
    s.session.close('again');
    expect(s.ends).toHaveLength(1);
  });

  it('rejects with the body text on a non-200 response', async () => {
    const s = makeSession();
    const started = s.session.start();
    await flush();
    s.socket.end(Buffer.from('HTTP/1.1 401 Unauthorized\r\nConnection: close\r\n\r\nUnauthorized: the ticket expired.'));
    await expect(started).rejects.toThrow('Unauthorized: the ticket expired.');
    expect(s.ends).toHaveLength(1);
  });

  it('rejects after the start timeout when nobody starts the broadcast', async () => {
    vi.useFakeTimers();
    const s = makeSession();
    const started = s.session.start();
    const failed = expect(started).rejects.toThrow(IOS_START_TIMEOUT_MESSAGE);
    await vi.advanceTimersByTimeAsync(0);
    s.socket.write(Buffer.concat([Buffer.from(OK_HEAD), status(false)]));
    await vi.advanceTimersByTimeAsync(179_000);
    expect(s.ends).toEqual([]);
    await vi.advanceTimersByTimeAsync(1_500);
    await failed;
    expect(s.ends).toEqual([IOS_START_TIMEOUT_MESSAGE]);
  });

  it('queues gestures one RPC at a time and answers immediately', async () => {
    const s = makeSession();
    await goLive(s);
    let release: () => void = () => undefined;
    s.call.mockImplementationOnce(async (method, params) => {
      s.calls.push({ method, params });
      await new Promise<void>((resolve) => (release = resolve));
      return { ok: true };
    });
    expect(s.session.sendControl([{ type: 'touch', action: 'down', pointerId: 0, x: 300, y: 600 }])).toEqual({ ok: true });
    s.tick(100);
    expect(s.session.sendControl([{ type: 'touch', action: 'up', pointerId: 0, x: 300, y: 600 }, { type: 'home' }])).toEqual({ ok: true });
    await flush();
    const gestures = () => s.calls.filter((c) => c.method !== 'stream_ticket').map((c) => c.method);
    expect(gestures()).toEqual(['tap']);
    release();
    await flush();
    expect(gestures()).toEqual(['tap', 'home']);
    expect(s.session.sendControl([{ type: 'power' }])).toEqual({ ok: false, message: 'Not available on iPhone.' });
    expect(s.session.setDisplayPower(false)).toEqual({ ok: false, message: 'Not available on iPhone.' });
  });

  it('ends the start with the helper\'s message when another screen recording holds the screen', async () => {
    const msg = 'Another screen recording is running on the iPhone. Stop it from the red indicator at the top of the screen, then try again.';
    const s = makeSession({ startLiveScreen: async () => ({ ok: false, broadcasting: false, needsTap: false, otherCapture: true, message: msg }) });
    const started = s.session.start();
    const failed = expect(started).rejects.toThrow(msg);
    await flush();
    s.socket.write(Buffer.concat([Buffer.from(OK_HEAD), status(false)]));
    await failed;
    expect(s.ends).toEqual([msg]);
  });

  it('refuses input before live', () => {
    const s = makeSession();
    expect(s.session.sendControl([{ type: 'home' }])).toEqual({ ok: false, message: 'The stream is not live.' });
  });

  it('a live connection that closes or goes quiet reconnects with a new ticket and stays live', async () => {
    vi.useFakeTimers();
    const sockets: PassThrough[] = [];
    const call = vi.fn(async (method: string) => (method === 'stream_ticket' ? { ticket: `T-${sockets.length + 1}` } : { ok: true }));
    const session = new IosVideoSession({
      call,
      host: async () => '127.0.0.1:52001',
      connect: () => {
        const sock = new PassThrough();
        sockets.push(sock);
        return sock;
      },
      deviceName: 'iPhone 15',
    });
    const metas: PhoneStreamMeta[] = [];
    const chunks: Buffer[] = [];
    const ends: string[] = [];
    session.on('meta', (m) => metas.push(m));
    session.on('videoChunk', (c) => chunks.push(c));
    session.on('ended', (e) => ends.push(e.reason));
    const started = session.start();
    await vi.advanceTimersByTimeAsync(0);
    sockets[0].write(Buffer.concat([Buffer.from(OK_HEAD), status(true), config()]));
    await started;

    // The runner was killed: no FIN, just silence. 7 s later the session reconnects.
    await vi.advanceTimersByTimeAsync(7_000 + 2_000);
    expect(sockets).toHaveLength(2);
    sockets[1].write(Buffer.concat([Buffer.from(OK_HEAD), status(true), config(), frame()]));
    await vi.advanceTimersByTimeAsync(0);
    expect(chunks).toHaveLength(1);
    expect(metas).toHaveLength(1); // still the same live stream
    expect(ends).toEqual([]);

    // A clean close reconnects too.
    sockets[1].end();
    await vi.advanceTimersByTimeAsync(2_000);
    expect(sockets).toHaveLength(3);
    expect(ends).toEqual([]);
  });

  it('gives up with a plain message when the helper stays gone', async () => {
    vi.useFakeTimers();
    let clock = 0;
    const s = new IosVideoSession({
      call: async (method: string) => {
        if (method === 'stream_ticket' && clock > 0) throw new Error("Can't reach the phone");
        return { ticket: 'T' };
      },
      host: async () => '127.0.0.1:52001',
      connect: () => {
        const sock = new PassThrough();
        if (clock === 0) setImmediate(() => sock.write(Buffer.concat([Buffer.from(OK_HEAD), status(true), config()])));
        return sock;
      },
      deviceName: 'iPhone 15',
      now: () => clock,
    });
    const ends: string[] = [];
    s.on('ended', (e) => ends.push(e.reason));
    const started = s.start();
    await vi.advanceTimersByTimeAsync(1);
    await started;
    for (let i = 0; i < 30 && ends.length === 0; i++) {
      clock += 2_000;
      await vi.advanceTimersByTimeAsync(2_000);
    }
    expect(ends).toEqual([IOS_HELPER_LOST_MESSAGE]);
  });
});

describe('IosVideoSession audio (packet types 4/5)', () => {
  const ASC = Buffer.from([0x11, 0x90]); // AAC-LC, 48 kHz, stereo
  const audioConfig = (o: object = { codec: 'aac', sampleRate: 48000, channels: 2, asc: ASC.toString('base64') }) =>
    encodeIosVideoPacket(IOS_PACKET_AUDIO_CONFIG, o);
  const audio = (data: Buffer) => encodeIosVideoPacket(IOS_PACKET_AUDIO, data);

  function listen(s: ReturnType<typeof makeSession>) {
    const audioMetas: PhoneAudioMeta[] = [];
    const audioChunks: Buffer[] = [];
    s.session.on('audioMeta', (m) => audioMetas.push(m));
    s.session.on('audioChunk', (c) => audioChunks.push(c));
    return { audioMetas, audioChunks };
  }

  it('turns AUDIO_CONFIG into audioMeta and AUDIO into audioChunks, beside the video', async () => {
    const s = makeSession();
    const a = listen(s);
    await goLive(s);
    s.socket.write(Buffer.concat([audioConfig(), frame(), audio(Buffer.from([0x21, 1, 2])), audio(Buffer.from([0x21, 3]))]));
    await flush();
    expect(a.audioMetas).toEqual([{ codec: 'aac', sampleRate: 48000, channels: 2, description: ASC.toString('base64') }]);
    expect(a.audioChunks).toEqual([Buffer.from([0x21, 1, 2]), Buffer.from([0x21, 3])]);
    expect(s.chunks).toEqual([IDR]);
  });

  it('drops audio before a valid AUDIO_CONFIG and skips malformed configs', async () => {
    const s = makeSession();
    const a = listen(s);
    await goLive(s);
    s.socket.write(Buffer.concat([
      audio(Buffer.from([1])),
      audioConfig({ codec: 'opus', sampleRate: 48000, channels: 2, asc: ASC.toString('base64') }),
      audioConfig({ codec: 'aac', sampleRate: 0, channels: 2, asc: ASC.toString('base64') }),
      audioConfig({ codec: 'aac', sampleRate: 48000, channels: 2, asc: '' }),
      encodeIosVideoPacket(IOS_PACKET_AUDIO_CONFIG, 'not json'),
      audio(Buffer.from([2])),
      frame(),
    ]));
    await flush();
    expect(a.audioMetas).toEqual([]);
    expect(a.audioChunks).toEqual([]);
    expect(s.chunks).toEqual([IDR]);
    expect(s.ends).toEqual([]);
  });

  it('a throwing audio listener never ends or stalls the video', async () => {
    const s = makeSession();
    s.session.on('audioChunk', () => {
      throw new Error('decoder blew up');
    });
    await goLive(s);
    s.socket.write(Buffer.concat([audioConfig(), audio(Buffer.from([7])), frame()]));
    await flush();
    expect(s.chunks).toEqual([IDR]);
    expect(s.ends).toEqual([]);
  });
});
