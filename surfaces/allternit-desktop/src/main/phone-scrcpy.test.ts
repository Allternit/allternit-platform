import { EventEmitter } from 'node:events';
import { describe, expect, it, vi } from 'vitest';
import {
  AUDIO_CODEC_OPUS,
  buildServerCommand,
  opusHeadChannels,
  ScrcpyAudioDemuxer,
  DEVICE_SERVER_PATH,
  encodeControlMessage,
  encodeSetDisplayPower,
  floatToI16fp,
  floatToU16fp,
  parseDeviceMessage,
  parseDeviceName,
  parseSessionPacket,
  ScrcpySession,
  SCRCPY_SERVER_SHA256,
  verifyServerBinary,
  type PhoneAudioMeta,
  type PhoneStreamMeta,
  type ScrcpyAdb,
  type ScrcpyDeviceMessage,
  type ScrcpySocket,
} from './phone-scrcpy.js';

const VIDEO_1080x2400 = { width: 1080, height: 2400 };

// ── Fixed-byte fixtures ─────────────────────────────────────────────────────

function sessionPacket(width: number, height: number, clientResized = false): Buffer {
  const p = Buffer.alloc(12);
  p.writeUInt32BE((0x80000000 | (clientResized ? 1 : 0)) >>> 0, 0);
  p.writeUInt32BE(width, 4);
  p.writeUInt32BE(height, 8);
  return p;
}

function mediaPacket(payload: Buffer, pts = 0, keyFrame = false): Buffer {
  const h = Buffer.alloc(12);
  h.writeBigUInt64BE(BigInt(pts) | (keyFrame ? 0x2000000000000000n : 0n), 0);
  h.writeUInt32BE(payload.length, 8);
  return Buffer.concat([h, payload]);
}

function headerBytes(deviceName: string, width = 1080, height = 2400): Buffer {
  const name = Buffer.alloc(64);
  Buffer.from(deviceName, 'utf8').copy(name);
  const codec = Buffer.from([0x68, 0x32, 0x36, 0x34]); // "h264"
  return Buffer.concat([Buffer.from([0]), name, codec, sessionPacket(width, height)]);
}

describe('float fixed-point', () => {
  it('matches sc_float_to_u16fp / sc_float_to_i16fp', () => {
    expect(floatToU16fp(1)).toBe(0xffff);
    expect(floatToU16fp(0)).toBe(0);
    expect(floatToU16fp(0.5)).toBe(0x8000);
    expect(floatToI16fp(1)).toBe(0x7fff);
    expect(floatToI16fp(-1)).toBe(-0x8000);
    expect(floatToI16fp(0.0625)).toBe(2048);
    expect(floatToI16fp(-0.125)).toBe(-4096);
  });
});

describe('header field parsing', () => {
  it('parses the 64-byte NUL-padded device name', () => {
    const field = Buffer.alloc(64);
    Buffer.from('Pixel 8', 'utf8').copy(field);
    expect(parseDeviceName(field)).toBe('Pixel 8');
    expect(parseDeviceName(Buffer.alloc(64))).toBe('');
    // 80 bytes copied into a 64-byte field lands on a character boundary (64)
    const long = Buffer.alloc(64);
    Buffer.from('é'.repeat(40), 'utf8').copy(long);
    expect(parseDeviceName(long)).toBe('é'.repeat(32));
  });

  it('refuses a mis-sized device name field', () => {
    expect(() => parseDeviceName(Buffer.alloc(63))).toThrow(/64 bytes/);
  });

  it('parses session packets (rotation/size changes)', () => {
    expect(parseSessionPacket(sessionPacket(1080, 2400))).toEqual({ width: 1080, height: 2400, clientResized: false });
    expect(parseSessionPacket(sessionPacket(2400, 1080, true))).toEqual({ width: 2400, height: 1080, clientResized: true });
    expect(() => parseSessionPacket(Buffer.alloc(11))).toThrow(/12 bytes/);
    expect(() => parseSessionPacket(Buffer.alloc(12))).toThrow(/session packet/);
  });
});

describe('control message encodings (control_msg.c v5.0.1)', () => {
  const hex = (bufs: Buffer[]) => bufs.map((b) => b.toString('hex')).join(' ');

  it('touch down: primary button, full pressure', () => {
    expect(hex(encodeControlMessage({ type: 'touch', action: 'down', pointerId: 0, x: 100, y: 200 }, VIDEO_1080x2400)))
      .toBe('0200' + '0000000000000000' + '00000064' + '000000c8' + '0438' + '0960' + 'ffff' + '00000001' + '00000001');
  });

  it('touch up: no buttons, no pressure, no action button', () => {
    expect(hex(encodeControlMessage({ type: 'touch', action: 'up', pointerId: 0, x: 100, y: 200 }, VIDEO_1080x2400)))
      .toBe('0201' + '0000000000000000' + '00000064' + '000000c8' + '0438' + '0960' + '0000' + '00000000' + '00000000');
  });

  it('touch move: buttons held, action_button only on down', () => {
    expect(hex(encodeControlMessage({ type: 'touch', action: 'move', pointerId: 7, x: 0, y: 0 }, VIDEO_1080x2400)))
      .toBe('0202' + '0000000000000007' + '00000000' + '00000000' + '0438' + '0960' + 'ffff' + '00000000' + '00000001');
  });

  it('touch honors an explicit pressure (0.5 → 0x8000)', () => {
    const out = encodeControlMessage({ type: 'touch', action: 'down', pointerId: 1, x: 1, y: 1, pressure: 0.5 }, VIDEO_1080x2400)[0];
    expect(out.readUInt16BE(22)).toBe(0x8000);
    expect(() => encodeControlMessage({ type: 'touch', action: 'down', pointerId: 1, x: 0, y: 0, pressure: 2 }, VIDEO_1080x2400)).toThrow(/pressure/);
    expect(() => encodeControlMessage({ type: 'touch', action: 'down', pointerId: 1, x: -1, y: 0 }, VIDEO_1080x2400)).toThrow(/x/);
  });

  it('key down/up with keycode and meta state', () => {
    expect(hex(encodeControlMessage({ type: 'key', action: 'down', keyCode: 66, metaState: 4096 }, VIDEO_1080x2400)))
      .toBe('00' + '00' + '00000042' + '00000000' + '00001000');
    expect(hex(encodeControlMessage({ type: 'key', action: 'up', keyCode: 66 }, VIDEO_1080x2400)))
      .toBe('00' + '01' + '00000042' + '00000000' + '00000000');
  });

  it('text: u32 length + UTF-8', () => {
    expect(hex(encodeControlMessage({ type: 'text', text: 'héllo' }, VIDEO_1080x2400))).toBe('01' + '00000006' + '68c3a96c6c6f');
  });

  it('text is truncated to 300 bytes on a character boundary', () => {
    const text = 'a'.repeat(299) + 'é'; // 299 ASCII + 2 UTF-8 bytes = 301
    const out = encodeControlMessage({ type: 'text', text }, VIDEO_1080x2400)[0];
    // byte 300 is a UTF-8 continuation byte → back off to 299
    expect(out.readUInt32BE(1)).toBe(299);
    expect(out.length).toBe(304);
  });

  it('scroll: position + i16 fixed-point amounts / 16, no buttons', () => {
    expect(hex(encodeControlMessage({ type: 'scroll', x: 540, y: 1200, hScroll: 1, vScroll: -2 }, VIDEO_1080x2400)))
      .toBe('03' + '0000021c' + '000004b0' + '0438' + '0960' + '0800' + 'f000' + '00000000');
    // no amounts → zeros
    expect(hex(encodeControlMessage({ type: 'scroll', x: 0, y: 0 }, VIDEO_1080x2400)))
      .toBe('03' + '00000000' + '00000000' + '0438' + '0960' + '0000' + '0000' + '00000000');
  });

  it('back is a down/up pair of BACK_OR_SCREEN_ON', () => {
    expect(hex(encodeControlMessage({ type: 'back' }, VIDEO_1080x2400))).toBe('0400 0401');
  });

  it('home/app_switch/power are key taps with the Android keycodes', () => {
    expect(hex(encodeControlMessage({ type: 'home' }, VIDEO_1080x2400))).toBe('00' + '00' + '00000003' + '00000000' + '00000000' + ' ' + '00' + '01' + '00000003' + '00000000' + '00000000');
    expect(hex(encodeControlMessage({ type: 'app_switch' }, VIDEO_1080x2400))).toContain('000000bb');
    expect(hex(encodeControlMessage({ type: 'power' }, VIDEO_1080x2400))).toContain('0000001a');
  });

  it('unknown events throw', () => {
    expect(() => encodeControlMessage({ type: 'nope' } as never, VIDEO_1080x2400)).toThrow(/Unknown input event/);
  });

  it('SET_DISPLAY_POWER is 2 bytes', () => {
    expect(encodeSetDisplayPower(true)).toEqual(Buffer.from([10, 1]));
    expect(encodeSetDisplayPower(false)).toEqual(Buffer.from([10, 0]));
  });
});

describe('device message decoding (device_msg.c v5.0.1)', () => {
  it('parses clipboard, ack-clipboard and uhid-output', () => {
    const clip = Buffer.concat([Buffer.from([0]), Buffer.from([0, 0, 0, 5]), Buffer.from('hello')]);
    expect(parseDeviceMessage(clip)).toEqual({ message: { type: 'clipboard', text: 'hello' }, consumed: 10 });

    const ack = Buffer.concat([Buffer.from([1]), Buffer.from([0, 0, 0, 0, 0, 0, 0, 42])]);
    const parsedAck = parseDeviceMessage(ack);
    expect('message' in parsedAck && parsedAck.message.type).toBe('ackClipboard');
    expect(parsedAck).toMatchObject({ consumed: 9 });

    const uhid = Buffer.concat([Buffer.from([2, 0, 7, 0, 3]), Buffer.from([1, 2, 3])]);
    expect(parseDeviceMessage(uhid)).toEqual({ message: { type: 'uhidOutput', id: 7, data: Buffer.from([1, 2, 3]) }, consumed: 8 });
  });

  it('reports needMore on truncated messages', () => {
    expect(parseDeviceMessage(Buffer.alloc(0))).toEqual({ needMore: true });
    expect(parseDeviceMessage(Buffer.from([0, 0, 0]))).toEqual({ needMore: true });
    expect(parseDeviceMessage(Buffer.from([0, 0, 0, 0, 5, 104]))).toEqual({ needMore: true }); // clipboard body short
    expect(parseDeviceMessage(Buffer.from([1, 0, 0]))).toEqual({ needMore: true });
    expect(parseDeviceMessage(Buffer.from([2, 0, 7, 0, 3, 1]))).toEqual({ needMore: true }); // uhid data short
  });

  it('fails on unknown types and absurd sizes', () => {
    expect(parseDeviceMessage(Buffer.from([9]))).toMatchObject({ error: expect.stringContaining('Unknown device message') });
    const huge = Buffer.from([0, 0xff, 0xff, 0xff, 0xff]);
    expect(parseDeviceMessage(huge)).toMatchObject({ error: expect.stringContaining('too large') });
  });
});

describe('pinned server binary', () => {
  it('verifies against the recorded sha256/size', () => {
    expect(SCRCPY_SERVER_SHA256).toMatch(/^[0-9a-f]{64}$/);
    expect(verifyServerBinary(Buffer.alloc(100))).toBe(false);
  });
});

describe('server command', () => {
  it('pins the version, tunnel and h264, and asks for opus audio by default', () => {
    const cmd = buildServerCommand({});
    expect(cmd).toContain('audio=true audio_codec=opus');
    expect(cmd).not.toContain('audio_dup'); // scrcpy's default: sound moves to the computer
    expect(buildServerCommand({ audio: false })).toContain('audio=false');
    expect(buildServerCommand({ audio: false })).not.toContain('audio_codec');
    expect(cmd).toContain('app_process / com.genymobile.scrcpy.Server 5.0.1');
    expect(cmd).toContain('tunnel_forward=true');
    expect(cmd).toContain('video_codec=h264');
    expect(cmd).toContain('stay_awake=true');
    expect(cmd).toContain('max_size=1080');
    expect(cmd).toContain('video_bit_rate=5000000');
    expect(cmd).toContain('max_fps=30');
    expect(cmd).not.toContain('send_frame_meta=false');
  });

  it('honours explicit options and omits zero values', () => {
    expect(buildServerCommand({ maxSize: 720, videoBitrate: 2_000_000, maxFps: 15 })).toContain('max_size=720 video_bit_rate=2000000 max_fps=15');
    expect(buildServerCommand({ maxSize: 0, videoBitrate: 0, maxFps: 0 })).not.toContain('max_size=');
  });
});

// ── Session with fake adb + fake sockets ────────────────────────────────────

class FakeSocket extends EventEmitter {
  written: Buffer[] = [];
  destroyed = false;
  write(data: Buffer): boolean {
    this.written.push(Buffer.from(data));
    return true;
  }
  destroy(): void {
    if (this.destroyed) return;
    this.destroyed = true;
    queueMicrotask(() => this.emit('close'));
  }
}

function fakeAdb() {
  const calls: string[][] = [];
  let shellDone: (code: number | null) => void = () => undefined;
  const kill = vi.fn();
  const adb: ScrcpyAdb = {
    push: async (local, remote) => void calls.push(['push', local, remote]),
    shell: (command) => {
      calls.push(['shell', command]);
      const done = new Promise<number | null>((r) => (shellDone = r));
      return { done, kill };
    },
    forward: async (port, remote) => void calls.push(['forward', String(port), remote]),
    removeForward: async (port) => void calls.push(['removeForward', String(port)]),
    remove: async (remote) => void calls.push(['remove', remote]),
  };
  return { adb, calls, kill, endShell: (code: number | null) => shellDone(code) };
}

function makeSession(opts: { deviceName?: string; width?: number; height?: number; startTimeoutMs?: number; audio?: boolean; onConnect?: (s: FakeSocket, n: number) => void } = {}) {
  const { adb, calls, kill, endShell } = fakeAdb();
  const sockets: FakeSocket[] = [];
  const session = new ScrcpySession(
    {
      adb,
      serial: '192.168.1.20:41234',
      serverPath: '/tmp/fake/scrcpy-server.jar',
      connect: async () => {
        const s = new FakeSocket();
        sockets.push(s);
        opts.onConnect?.(s, sockets.length);
        return s as unknown as ScrcpySocket;
      },
      port: 27183,
      startTimeoutMs: opts.startTimeoutMs,
    },
    // The video/control socket tests run audio-off (two sockets); the audio
    // tests below run the production default (video, audio, control).
    { audio: opts.audio ?? false },
  );
  const meta = vi.fn();
  const chunks: Buffer[] = [];
  const messages: ScrcpyDeviceMessage[] = [];
  const ended = vi.fn();
  session.on('meta', meta);
  session.on('videoChunk', (c) => chunks.push(Buffer.from(c)));
  session.on('deviceMessage', (m) => messages.push(m));
  session.on('ended', ended);
  // Server order (DesktopConnection.open → sendDeviceMeta): dummy byte on
  // accept, then the control accept, and only then the rest of the header.
  const goLive = async () => {
    const started = session.start();
    await vi.waitFor(() => expect(sockets.length).toBeGreaterThan(0)); // video socket connected
    const header = headerBytes(opts.deviceName ?? 'Pixel 8', opts.width ?? 1080, opts.height ?? 2400);
    sockets[0].emit('data', header.subarray(0, 1));
    await vi.waitFor(() => expect(sockets.length).toBe(2)); // control socket connected
    sockets[0].emit('data', header.subarray(1));
    await started;
    expect(meta).toHaveBeenCalledWith({ deviceName: opts.deviceName ?? 'Pixel 8', width: opts.width ?? 1080, height: opts.height ?? 2400, codec: 'h264' } satisfies PhoneStreamMeta);
  };
  return { session, sockets, calls, kill, endShell, meta, chunks, messages, ended, goLive };
}

describe('ScrcpySession', () => {
  it('pushes, forwards, launches the server and goes live on the header', async () => {
    const { session, calls, ended, goLive } = makeSession({ deviceName: 'Pixel 9 Pro' });
    await goLive();
    expect(calls[0]).toEqual(['push', '/tmp/fake/scrcpy-server.jar', DEVICE_SERVER_PATH]);
    expect(calls[1]).toEqual(['forward', '27183', 'localabstract:scrcpy']);
    expect(calls[2][0]).toBe('shell');
    expect(calls[2][1]).toContain('com.genymobile.scrcpy.Server 5.0.1');
    expect(ended).not.toHaveBeenCalled();
    session.close();
    expect(ended).toHaveBeenCalledWith({ reason: 'closed' });
    await vi.waitFor(() => expect(calls.some((c) => c[0] === 'removeForward')).toBe(true));
    await vi.waitFor(() => expect(calls.some((c) => c[0] === 'remove')).toBe(true));
  });

  it('handles a fully fragmented header (byte-by-byte reads)', async () => {
    const { session, sockets } = makeSession();
    const started = session.start();
    await vi.waitFor(() => expect(sockets.length).toBe(1));
    const header = headerBytes('Pixel 8');
    sockets[0].emit('data', header.subarray(0, 1));
    await vi.waitFor(() => expect(sockets.length).toBe(2));
    for (const byte of header.subarray(1)) sockets[0].emit('data', Buffer.from([byte]));
    await started; // resolves only once live
    session.close();
  });

  it('retries when the adb tunnel accepts and closes before the server listens', async () => {
    // adb forward accepts the TCP connect even when nothing listens on the
    // phone yet, then closes it (server.c connect_and_read_byte).
    const { session, sockets, ended, meta } = makeSession({ onConnect: (s, n) => n <= 2 && queueMicrotask(() => s.destroy()) });
    const started = session.start();
    await vi.waitFor(() => expect(sockets.length).toBe(3), { timeout: 3000 });
    expect(ended).not.toHaveBeenCalled();
    const header = headerBytes('moto g play 2024');
    sockets[2].emit('data', header.subarray(0, 1));
    await vi.waitFor(() => expect(sockets.length).toBe(4)); // control
    sockets[2].emit('data', header.subarray(1));
    await started;
    expect(meta).toHaveBeenCalledWith(expect.objectContaining({ deviceName: 'moto g play 2024' }));
    expect(ended).not.toHaveBeenCalled();
    session.close();
  });

  it('connects the control socket after the dummy byte, before the device name', async () => {
    const { session, sockets, meta } = makeSession();
    const started = session.start();
    await vi.waitFor(() => expect(sockets.length).toBe(1));
    sockets[0].emit('data', Buffer.from([0])); // the server sends nothing more until control is accepted
    await vi.waitFor(() => expect(sockets.length).toBe(2));
    expect(meta).not.toHaveBeenCalled();
    sockets[0].emit('data', headerBytes('Pixel 8').subarray(1));
    await started;
    expect(meta).toHaveBeenCalledTimes(1);
    session.close();
  });

  it('strips 12-byte frame headers and forwards Annex-B payloads', async () => {
    const { session, sockets, chunks, goLive } = makeSession();
    await goLive();
    const a = Buffer.from([0, 0, 0, 1, 0x67, 1, 2]); // SPS-ish
    const b = Buffer.from([0, 0, 0, 1, 0x68, 3, 4]); // PPS-ish
    sockets[0].emit('data', Buffer.concat([mediaPacket(a, 0, false), mediaPacket(b, 33333, true)]));
    expect(chunks.map((c) => c.toString('hex'))).toEqual([a.toString('hex'), b.toString('hex')]);
    session.close();
  });

  it('waits for a split media packet before emitting it', async () => {
    const { session, sockets, chunks, goLive } = makeSession();
    await goLive();
    const payload = Buffer.from([0, 0, 0, 1, 9, 9, 9, 9]);
    const packet = mediaPacket(payload);
    sockets[0].emit('data', packet.subarray(0, 14)); // header + half payload
    expect(chunks).toEqual([]);
    sockets[0].emit('data', packet.subarray(14));
    expect(chunks).toEqual([payload]);
    session.close();
  });

  it('emits size deviceMessages from mid-stream session packets (rotation)', async () => {
    const { session, sockets, messages, goLive } = makeSession();
    await goLive();
    sockets[0].emit('data', Buffer.concat([mediaPacket(Buffer.from([1])), sessionPacket(2400, 1080), mediaPacket(Buffer.from([2]))]));
    expect(messages).toEqual([{ type: 'size', width: 2400, height: 1080, clientResized: false }]);
    session.close();
  });

  it('connects the control socket once even with back-to-back session packets', async () => {
    const { session, sockets, messages, meta } = makeSession();
    const started = session.start();
    await vi.waitFor(() => expect(sockets.length).toBe(1));
    sockets[0].emit('data', Buffer.from([0]));
    await vi.waitFor(() => expect(sockets.length).toBe(2));
    // rest of the header with TWO session packets coalesced into a single read
    const name = Buffer.alloc(64);
    Buffer.from('Pixel 8').copy(name);
    sockets[0].emit('data', Buffer.concat([name, Buffer.from([0x68, 0x32, 0x36, 0x34]), sessionPacket(1080, 2400), sessionPacket(2400, 1080)]));
    await started;
    await new Promise((r) => setTimeout(r, 50));
    expect(sockets.length).toBe(2); // no duplicate control connection
    expect(meta).toHaveBeenCalledTimes(1);
    expect(messages).toEqual([{ type: 'size', width: 2400, height: 1080, clientResized: false }]);
    session.close();
  });

  it('parses fragmented device messages on the control socket', async () => {
    const { session, sockets, messages, goLive } = makeSession();
    await goLive();
    const clip = Buffer.concat([Buffer.from([0]), Buffer.from([0, 0, 0, 5]), Buffer.from('hello')]);
    const control = sockets[1];
    for (const byte of clip) control.emit('data', Buffer.from([byte]));
    expect(messages).toEqual([{ type: 'clipboard', text: 'hello' }]);
    session.close();
  });

  it('ends when the phone disconnects mid-stream, and tears everything down', async () => {
    const { sockets, ended, kill, goLive } = makeSession();
    await goLive();
    sockets[0].destroy(); // phone unplugged
    await vi.waitFor(() => expect(ended).toHaveBeenCalled());
    expect(ended.mock.calls[0][0].reason).toMatch(/video stream closed|closed/);
    expect(kill).toHaveBeenCalled();
  });

  it('ends when the server process dies', async () => {
    const { ended, endShell, goLive } = makeSession();
    await goLive();
    endShell(1);
    await vi.waitFor(() => expect(ended).toHaveBeenCalled());
    expect(ended.mock.calls[0][0].reason).toMatch(/server on the phone stopped/);
  });

  it('rejects an unsupported codec with a clear error', async () => {
    const { session, sockets, ended } = makeSession();
    const started = session.start();
    await vi.waitFor(() => expect(sockets.length).toBe(1));
    const name = Buffer.alloc(64);
    sockets[0].emit('data', Buffer.concat([Buffer.from([0]), name, Buffer.from([0x68, 0x32, 0x36, 0x35])])); // "h265"
    await expect(started).rejects.toThrow(/unsupported video codec/);
    await vi.waitFor(() => expect(ended).toHaveBeenCalled());
  });

  it('times out when the phone never sends the header', async () => {
    vi.useFakeTimers();
    try {
      const { session, ended } = makeSession({ startTimeoutMs: 1000 });
      const started = session.start();
      const verdict = expect(started).rejects.toThrow(/Timed out/); // attach before the timer fires
      await vi.advanceTimersByTimeAsync(1500);
      await verdict;
      expect(ended).toHaveBeenCalledWith({ reason: 'Timed out waiting for the phone to start streaming.' });
    } finally {
      vi.useRealTimers();
    }
  });

  it('sendControl refuses before live and writes encoded events after live', async () => {
    const { session, sockets, goLive } = makeSession();
    expect(session.sendControl([{ type: 'home' }])).toMatchObject({ ok: false });
    await goLive();
    expect(session.sendControl([{ type: 'home' }])).toEqual({ ok: true });
    const control = sockets[1];
    // key tap: two INJECT_KEYCODE messages, KEYCODE_HOME (3), down then up
    expect(control.written).toHaveLength(2);
    for (const [i, msg] of control.written.entries()) {
      expect(msg.length).toBe(14);
      expect(msg[0]).toBe(0); // CONTROL_MSG_TYPE_INJECT_KEYCODE
      expect(msg[1]).toBe(i === 0 ? 0 : 1); // down, then up
      expect(msg.readUInt32BE(2)).toBe(3); // KEYCODE_HOME
      expect(msg.readUInt32BE(6)).toBe(0); // repeat
      expect(msg.readUInt32BE(10)).toBe(0); // metaState
    }
    // invalid events: nothing written, error reported
    control.written.length = 0;
    expect(session.sendControl([{ type: 'touch', action: 'down', pointerId: 0, x: -5, y: 0 }])).toMatchObject({ ok: false, message: expect.stringContaining('x') });
    expect(control.written).toEqual([]);
    session.close();
  });

  it('setDisplayPower writes the 2-byte message after live only', async () => {
    const { session, sockets, goLive } = makeSession();
    expect(session.setDisplayPower(true)).toMatchObject({ ok: false });
    await goLive();
    expect(session.setDisplayPower(false)).toEqual({ ok: true });
    expect(sockets[1].written.map((b) => b.toString('hex'))).toEqual(['0a00']);
    session.close();
  });

  it('close is idempotent and ends exactly once', async () => {
    const { session, ended, goLive } = makeSession();
    await goLive();
    session.close();
    session.close();
    expect(ended).toHaveBeenCalledTimes(1);
  });
});

// ── Audio (opus over the audio socket) ──────────────────────────────────────

const OPUS_HEAD = Buffer.concat([Buffer.from('OpusHead', 'latin1'), Buffer.from([1, 2, 0x38, 0x01, 0x80, 0xbb, 0, 0, 0, 0, 0])]);

function audioPacket(payload: Buffer, opts: { config?: boolean; pts?: number } = {}): Buffer {
  const h = Buffer.alloc(12);
  h.writeBigUInt64BE(opts.config ? 0x4000000000000000n : BigInt(opts.pts ?? 0), 0);
  h.writeUInt32BE(payload.length, 8);
  return Buffer.concat([h, payload]);
}

function codecId(id: number): Buffer {
  const b = Buffer.alloc(4);
  b.writeUInt32BE(id >>> 0, 0);
  return b;
}

const OPUS_META: PhoneAudioMeta = { codec: 'opus', sampleRate: 48000, channels: 2, description: OPUS_HEAD.toString('base64') };

describe('ScrcpyAudioDemuxer', () => {
  it('reads the codec id, the OpusHead config packet and opus frames across splits', () => {
    const d = new ScrcpyAudioDemuxer();
    const bytes = Buffer.concat([codecId(AUDIO_CODEC_OPUS), audioPacket(OPUS_HEAD, { config: true }), audioPacket(Buffer.from([0xfc, 1, 2]), { pts: 20000 }), audioPacket(Buffer.from([0xfc, 3]))]);
    const events = [];
    for (const byte of bytes) events.push(...d.push(Buffer.from([byte])));
    expect(events).toEqual([
      { type: 'meta', meta: OPUS_META },
      { type: 'chunk', data: Buffer.from([0xfc, 1, 2]) },
      { type: 'chunk', data: Buffer.from([0xfc, 3]) },
    ]);
  });

  it('codec 0 (no capture, Android < 11) and 1 (error) disable audio and swallow the rest', () => {
    for (const [id, reason] of [[0, /Android 11/], [1, /configuration error/], [0x00616163, /unsupported audio codec 0x616163/]] as const) {
      const d = new ScrcpyAudioDemuxer();
      const events = d.push(Buffer.concat([codecId(id), audioPacket(Buffer.from([1]))]));
      expect(events).toHaveLength(1);
      expect(events[0]).toMatchObject({ type: 'disabled' });
      expect((events[0] as { reason: string }).reason).toMatch(reason);
      expect(d.disabled).toBe(true);
      expect(d.push(audioPacket(Buffer.from([2])))).toEqual([]);
    }
  });

  it('a corrupt packet disables audio instead of throwing', () => {
    const d = new ScrcpyAudioDemuxer();
    const bad = Buffer.alloc(12);
    bad.writeUInt32BE(0xffffffff, 8); // absurd size
    expect(d.push(Buffer.concat([codecId(AUDIO_CODEC_OPUS), bad]))).toEqual([{ type: 'disabled', reason: 'corrupt audio packet' }]);
  });

  it('reads the channel count from the OpusHead', () => {
    const mono = Buffer.from(OPUS_HEAD);
    mono[9] = 1;
    expect(opusHeadChannels(mono)).toBe(1);
    expect(opusHeadChannels(Buffer.from('nope'))).toBe(2);
  });
});

describe('ScrcpySession audio', () => {
  function audioSession() {
    const s = makeSession({ audio: true });
    const audioMetas: PhoneAudioMeta[] = [];
    const audioChunks: Buffer[] = [];
    s.session.on('audioMeta', (m) => audioMetas.push(m));
    s.session.on('audioChunk', (c) => audioChunks.push(Buffer.from(c)));
    // Server order with audio on: video (dummy byte), audio, control, then the header.
    const goLive = async () => {
      const started = s.session.start();
      await vi.waitFor(() => expect(s.sockets.length).toBe(1));
      const header = headerBytes('Pixel 8');
      s.sockets[0].emit('data', header.subarray(0, 1));
      await vi.waitFor(() => expect(s.sockets.length).toBe(3));
      s.sockets[0].emit('data', header.subarray(1));
      await started;
      expect(s.meta).toHaveBeenCalledTimes(1);
    };
    return { ...s, audioMetas, audioChunks, goLive };
  }

  it('opens video, audio, control in order and demuxes opus beside the video', async () => {
    const s = audioSession();
    await s.goLive();
    expect(s.calls[2][1]).toContain('audio=true audio_codec=opus');
    const [video, audio, control] = s.sockets;
    audio.emit('data', Buffer.concat([codecId(AUDIO_CODEC_OPUS), audioPacket(OPUS_HEAD, { config: true }), audioPacket(Buffer.from([0xfc, 9]))]));
    video.emit('data', mediaPacket(Buffer.from([0, 0, 0, 1, 0x65])));
    expect(s.audioMetas).toEqual([OPUS_META]);
    expect(s.audioChunks).toEqual([Buffer.from([0xfc, 9])]);
    expect(s.chunks).toEqual([Buffer.from([0, 0, 0, 1, 0x65])]);
    // Input goes to the third socket (control), never the audio one.
    expect(s.session.sendControl([{ type: 'home' }])).toEqual({ ok: true });
    expect(control.written).toHaveLength(2);
    expect(audio.written).toHaveLength(0);
    s.session.close();
    expect(audio.destroyed).toBe(true);
  });

  it('a phone without audio capture (codec 0) keeps streaming video', async () => {
    const s = audioSession();
    await s.goLive();
    s.sockets[1].emit('data', codecId(0));
    s.sockets[0].emit('data', mediaPacket(Buffer.from([1])));
    expect(s.audioMetas).toEqual([]);
    expect(s.chunks).toEqual([Buffer.from([1])]);
    expect(s.ended).not.toHaveBeenCalled();
  });

  it('audio socket errors/closes and throwing audio listeners never end the video', async () => {
    const s = audioSession();
    s.session.on('audioChunk', () => {
      throw new Error('decoder blew up');
    });
    await s.goLive();
    const [video, audio] = s.sockets;
    audio.emit('data', Buffer.concat([codecId(AUDIO_CODEC_OPUS), audioPacket(OPUS_HEAD, { config: true }), audioPacket(Buffer.from([1]))]));
    audio.emit('error', new Error('ECONNRESET'));
    audio.emit('close');
    video.emit('data', mediaPacket(Buffer.from([2])));
    expect(s.chunks).toEqual([Buffer.from([2])]);
    expect(s.ended).not.toHaveBeenCalled();
    expect(s.session.sendControl([{ type: 'home' }])).toEqual({ ok: true });
  });
});
