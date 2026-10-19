import { describe, expect, it, vi } from 'vitest';
import { PhoneStreamManager, type StreamConsumer, type StreamSessionHandle } from './phone-stream.js';
import type { PhoneAudioMeta, PhoneStreamMeta, ScrcpyDeviceMessage } from './phone-scrcpy.js';

const META: PhoneStreamMeta = { deviceName: 'Pixel 8', width: 1080, height: 2400, codec: 'h264' };

type Listener = (...args: any[]) => void;

/** A scriptable fake session: the test drives meta/chunk/ended, close() ends it like the real one. */
function fakeSession() {
  const listeners: Record<string, Listener[]> = { meta: [], videoChunk: [], deviceMessage: [], ended: [] };
  const emit = (event: string, ...args: any[]) => {
    for (const cb of listeners[event] ?? []) cb(...args);
  };
  const handle = {
    start: vi.fn((): Promise<void> => Promise.resolve()),
    on: (event: string, cb: Listener) => {
      (listeners[event] ??= []).push(cb);
      return () => {
        const i = (listeners[event] ?? []).indexOf(cb);
        if (i >= 0) listeners[event].splice(i, 1);
      };
    },
    sendControl: vi.fn(() => ({ ok: true })),
    setDisplayPower: vi.fn(() => ({ ok: true })),
    close: vi.fn((reason = 'closed') => {
      queueMicrotask(() => emit('ended', { reason }));
    }),
  } satisfies StreamSessionHandle;
  return { handle, emit, listeners };
}

function makeManager() {
  const sessions = new Map<string, ReturnType<typeof fakeSession>>();
  const createSession = vi.fn((serial: string) => {
    const s = fakeSession();
    sessions.set(serial, s);
    return s.handle;
  });
  const manager = new PhoneStreamManager({ createSession });
  return { manager, sessions, createSession };
}

async function goLive(manager: PhoneStreamManager, sessions: Map<string, ReturnType<typeof fakeSession>>, serial = '192.168.1.20:41234') {
  const starting = manager.start(serial);
  expect(manager.status(serial)).toEqual({ state: 'starting' });
  await starting;
  sessions.get(serial)!.emit('meta', META);
  expect(manager.status(serial)).toEqual({ state: 'live', meta: META });
}

describe('PhoneStreamManager lifecycle', () => {
  it('idle → starting → live, then stop → ended with reason', async () => {
    const { manager, sessions } = makeManager();
    expect(manager.status('192.168.1.20:41234')).toEqual({ state: 'idle' });
    await goLive(manager, sessions);

    const stops: string[] = [];
    const consumer: StreamConsumer = { onEnd: (_s, info) => stops.push(info.reason) };
    manager.subscribe('192.168.1.20:41234', consumer);

    await manager.stop('192.168.1.20:41234');
    expect(manager.status('192.168.1.20:41234')).toEqual({ state: 'ended', reason: 'stopped' });
    expect(stops).toEqual(['stopped']);
  });

  it('shares one session across consumers and start calls', async () => {
    const { manager, sessions, createSession } = makeManager();
    const first = manager.start('192.168.1.20:41234');
    const second = manager.start('192.168.1.20:41234');
    await Promise.all([first, second]);
    expect(createSession).toHaveBeenCalledTimes(1);
    const a = makeConsumer();
    const b = makeConsumer();
    manager.subscribe('192.168.1.20:41234', a);
    manager.subscribe('192.168.1.20:41234', b);
    sessions.get('192.168.1.20:41234')!.emit('meta', META);
    sessions.get('192.168.1.20:41234')!.emit('videoChunk', Buffer.from([1, 2, 3]));
    expect(a.meta).toHaveBeenCalledWith('192.168.1.20:41234', META);
    expect(b.meta).toHaveBeenCalledWith('192.168.1.20:41234', META);
    expect(a.chunk).toHaveBeenCalledWith('192.168.1.20:41234', Buffer.from([1, 2, 3]));
    expect(b.chunk).toHaveBeenCalledTimes(1);
  });

  it('stop is idempotent, with or without a stream', async () => {
    const { manager, sessions } = makeManager();
    await manager.stop('192.168.1.20:41234'); // nothing there
    await goLive(manager, sessions);
    await manager.stop('192.168.1.20:41234');
    await manager.stop('192.168.1.20:41234');
    expect(manager.status('192.168.1.20:41234')).toEqual({ state: 'ended', reason: 'stopped' });
  });

  it('a device disconnect ends the stream with a clear reason and it can be restarted', async () => {
    const { manager, sessions, createSession } = makeManager();
    await goLive(manager, sessions);
    manager.deviceDisconnected('192.168.1.20:41234');
    expect(sessions.get('192.168.1.20:41234')!.handle.close).toHaveBeenCalledWith('the phone disconnected');
    await vi.waitFor(() => expect(manager.status('192.168.1.20:41234')).toEqual({ state: 'ended', reason: 'the phone disconnected' }));
    expect(manager.activeSerials()).toEqual([]);

    const again = await manager.start('192.168.1.20:41234');
    expect(again).toEqual({ ok: true });
    expect(createSession).toHaveBeenCalledTimes(2);
  });

  it('start failure surfaces as error status', async () => {
    const manager = new PhoneStreamManager({
      createSession: () => {
        const s = fakeSession();
        s.handle.start.mockRejectedValue(new Error('server version mismatch'));
        return s.handle;
      },
    });
    await expect(manager.start('192.168.1.20:41234')).resolves.toEqual({ ok: false, message: expect.stringContaining('version') });
    expect(manager.status('192.168.1.20:41234')).toMatchObject({ state: 'error', message: expect.stringContaining('version') });
  });

  it('routes input and display-power to the live session only', async () => {
    const { manager, sessions } = makeManager();
    expect(manager.sendInput('192.168.1.20:41234', [{ type: 'home' }])).toMatchObject({ ok: false });
    expect(manager.setDisplayPower('192.168.1.20:41234', true)).toMatchObject({ ok: false });
    await goLive(manager, sessions);
    const events = [{ type: 'touch', action: 'down', pointerId: 0, x: 10, y: 20 } as const];
    expect(manager.sendInput('192.168.1.20:41234', [...events])).toEqual({ ok: true });
    expect(sessions.get('192.168.1.20:41234')!.handle.sendControl).toHaveBeenCalledWith([...events]);
    expect(manager.setDisplayPower('192.168.1.20:41234', false)).toEqual({ ok: true });
    expect(sessions.get('192.168.1.20:41234')!.handle.setDisplayPower).toHaveBeenCalledWith(false);
  });

  it('deviceMessages fan out to subscribers (future relay consumer)', async () => {
    const { manager, sessions } = makeManager();
    const c = makeConsumer();
    manager.subscribe('192.168.1.20:41234', c); // subscribe before start
    await goLive(manager, sessions);
    const size: ScrcpyDeviceMessage = { type: 'size', width: 2400, height: 1080, clientResized: false };
    sessions.get('192.168.1.20:41234')!.emit('deviceMessage', size);
    expect(c.message).toHaveBeenCalledWith('192.168.1.20:41234', size);
  });

  it('stopAll closes everything (desktop shutdown)', async () => {
    const { manager, sessions } = makeManager();
    await goLive(manager, sessions);
    manager.stopAll();
    expect(sessions.get('192.168.1.20:41234')!.handle.close).toHaveBeenCalledWith('desktop is shutting down');
  });
});

function makeConsumer(): StreamConsumer & { meta: ReturnType<typeof vi.fn>; chunk: ReturnType<typeof vi.fn>; message: ReturnType<typeof vi.fn>; end: ReturnType<typeof vi.fn> } {
  const c = {
    meta: vi.fn(),
    chunk: vi.fn(),
    message: vi.fn(),
    end: vi.fn(),
  };
  return { ...c, onMeta: c.meta, onChunk: c.chunk, onDeviceMessage: c.message, onEnd: c.end };
}

describe('PhoneStreamManager audio', () => {
  const SERIAL = '192.168.1.20:41234';
  const AUDIO: PhoneAudioMeta = { codec: 'opus', sampleRate: 48000, channels: 2, description: 'T3B1c0hlYWQ=' };

  it('fans audio meta/chunks out and replays the last audioMeta to a late subscriber', async () => {
    const { manager, sessions } = makeManager();
    const early = { metas: [] as PhoneAudioMeta[], chunks: [] as Buffer[] };
    manager.subscribe(SERIAL, {
      onAudioMeta: (_s, m) => early.metas.push(m),
      onAudioChunk: (_s, c) => early.chunks.push(c),
    });
    await goLive(manager, sessions);
    const session = sessions.get(SERIAL)!;
    session.emit('audioMeta', AUDIO);
    session.emit('audioChunk', Buffer.from([1]));
    expect(early.metas).toEqual([AUDIO]);
    expect(early.chunks).toEqual([Buffer.from([1])]);
    expect(manager.audioMeta(SERIAL)).toEqual(AUDIO);

    const late = { metas: [] as PhoneAudioMeta[], chunks: [] as Buffer[] };
    manager.subscribe(SERIAL, {
      onAudioMeta: (_s, m) => late.metas.push(m),
      onAudioChunk: (_s, c) => late.chunks.push(c),
    });
    expect(late.metas).toEqual([AUDIO]);
    session.emit('audioChunk', Buffer.from([2]));
    expect(late.chunks).toEqual([Buffer.from([2])]);
  });

  it('drops audio chunks before live or before an audioMeta, and forgets the meta when the stream ends', async () => {
    const { manager, sessions } = makeManager();
    const chunks: Buffer[] = [];
    manager.subscribe(SERIAL, { onAudioChunk: (_s, c) => chunks.push(c) });
    await manager.start(SERIAL);
    const session = sessions.get(SERIAL)!;
    session.emit('audioMeta', AUDIO);
    session.emit('audioChunk', Buffer.from([1])); // not live yet
    session.emit('meta', META);
    session.emit('audioChunk', Buffer.from([2]));
    expect(chunks).toEqual([Buffer.from([2])]);
    await manager.stop(SERIAL);
    await vi.waitFor(() => expect(manager.status(SERIAL).state).toBe('ended'));
    expect(manager.audioMeta(SERIAL)).toBeNull();
    const replayed: PhoneAudioMeta[] = [];
    manager.subscribe(SERIAL, { onAudioMeta: (_s, m) => replayed.push(m) });
    expect(replayed).toEqual([]);
  });

  it('video without audio never calls the audio hooks; a throwing audio consumer does not stop video', async () => {
    const { manager, sessions } = makeManager();
    const video: Buffer[] = [];
    const onAudioMeta = vi.fn();
    manager.subscribe(SERIAL, {
      onAudioMeta,
      onAudioChunk: () => {
        throw new Error('boom');
      },
      onChunk: (_s, c) => video.push(c),
    });
    await goLive(manager, sessions);
    const session = sessions.get(SERIAL)!;
    session.emit('videoChunk', Buffer.from([9]));
    expect(onAudioMeta).not.toHaveBeenCalled();
    session.emit('audioMeta', AUDIO);
    expect(() => session.emit('audioChunk', Buffer.from([1]))).not.toThrow();
    session.emit('videoChunk', Buffer.from([10]));
    expect(video).toEqual([Buffer.from([9]), Buffer.from([10])]);
    expect(manager.status(SERIAL).state).toBe('live');
  });
});
