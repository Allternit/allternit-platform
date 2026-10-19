/** Tests for the per-phone Phone Link cloud socket: lifecycle, hello, backoff, viewer control, data plane. */

import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { PHONE_AUDIO_FRAME_TAG, PhoneLinkPhoneSocket, SOCKET_OPEN, type PhoneLinkSocket, type PhoneLinkSocketDeps } from './phone-link-cloud.js';

class FakeSocket implements PhoneLinkSocket {
  sent: (string | Uint8Array)[] = [];
  closed: { code?: number; reason?: string } | null = null;
  state = 0;
  private openCbs: (() => void)[] = [];
  private messageCbs: ((data: string) => void)[] = [];
  private closeCbs: ((code: number, reason: string) => void)[] = [];
  private errorCbs: ((error: Error) => void)[] = [];

  send(data: string | Uint8Array): void {
    this.sent.push(data);
  }
  close(code?: number, reason?: string): void {
    this.state = 3;
    this.closed = { code, reason };
    this.closeCbs.forEach((cb) => cb(code ?? 1_000, reason ?? ''));
  }
  onOpen(cb: () => void): void {
    this.openCbs.push(cb);
  }
  onMessage(cb: (data: string) => void): void {
    this.messageCbs.push(cb);
  }
  onClose(cb: (code: number, reason: string) => void): void {
    this.closeCbs.push(cb);
  }
  onError(cb: (error: Error) => void): void {
    this.errorCbs.push(cb);
  }
  readyState(): number {
    return this.state;
  }

  simulateOpen(): void {
    this.state = SOCKET_OPEN;
    this.openCbs.forEach((cb) => cb());
  }
  simulateServerMessage(frame: Record<string, unknown>): void {
    this.messageCbs.forEach((cb) => cb(JSON.stringify(frame)));
  }
}

function textFrames(socket: FakeSocket): Record<string, unknown>[] {
  return socket.sent.filter((s) => typeof s === 'string').map((s) => JSON.parse(s as string) as Record<string, unknown>);
}

describe('PhoneLinkPhoneSocket', () => {
  beforeEach(() => {
    vi.useFakeTimers();
  });
  afterEach(() => {
    vi.useRealTimers();
  });

  function makeDeps(overrides: Partial<PhoneLinkSocketDeps> = {}) {
    const sockets: FakeSocket[] = [];
    const deps: PhoneLinkSocketDeps = {
      apiUrl: () => 'http://127.0.0.1:18013',
      headers: () => ({ 'x-allternit-desktop-access-token': 'tok', 'x-allternit-user-id': 'user-1' }),
      phone: () => ({ serial: 'ser1', model: 'Pixel 8', name: 'Pixel' }),
      onViewerAttached: async () => ({ ok: true }),
      onViewerDetached: () => undefined,
      onInput: () => ({ ok: true }),
      onDisplayPower: () => ({ ok: true }),
      retryDelaysMs: [100, 200],
      createSocket: () => {
        const s = new FakeSocket();
        sockets.push(s);
        return s;
      },
      ...overrides,
    };
    return { deps, sockets };
  }

  it('connects to the phone-link session route and helloes its one phone', async () => {
    const seen: string[] = [];
    const { deps, sockets } = makeDeps({
      createSocket: (url) => {
        seen.push(url);
        const s = new FakeSocket();
        sockets.push(s);
        return s;
      },
    });
    const client = new PhoneLinkPhoneSocket(deps);
    client.start();
    await vi.advanceTimersByTimeAsync(0);
    expect(seen[0]).toBe('ws://127.0.0.1:18013/ws/phone-link/session');
    sockets[0].simulateOpen();
    expect(textFrames(sockets[0])[0]).toEqual({ type: 'hello', phones: [{ serial: 'ser1', model: 'Pixel 8', name: 'Pixel' }] });
    client.stop();
  });

  it('heartbeats, and re-helloes (debounced) after notePhoneChanged', async () => {
    let name = 'Pixel';
    const { deps, sockets } = makeDeps({ phone: () => ({ serial: 'ser1', model: 'Pixel 8', name }) });
    const client = new PhoneLinkPhoneSocket(deps);
    client.start();
    await vi.advanceTimersByTimeAsync(0);
    sockets[0].simulateOpen();
    const pingsBefore = textFrames(sockets[0]).filter((f) => f.type === 'ping').length;
    await vi.advanceTimersByTimeAsync(25_000);
    expect(textFrames(sockets[0]).filter((f) => f.type === 'ping').length).toBeGreaterThan(pingsBefore);
    name = 'Pixel (work)';
    client.notePhoneChanged();
    await vi.advanceTimersByTimeAsync(1_100);
    const hellos = textFrames(sockets[0]).filter((f) => f.type === 'hello');
    expect(hellos.at(-1)?.phones).toEqual([{ serial: 'ser1', model: 'Pixel 8', name: 'Pixel (work)' }]);
    client.stop();
  });

  it('re-helloes with the Call audio summary when it changes (no device ids)', async () => {
    let callBridge: { enabled: boolean; botName: string | null } | undefined;
    const { deps, sockets } = makeDeps({ phone: () => ({ serial: 'ser1', model: 'Pixel 8', name: 'Pixel', ...(callBridge ? { callBridge } : {}) }) });
    const client = new PhoneLinkPhoneSocket(deps);
    client.start();
    await vi.advanceTimersByTimeAsync(0);
    sockets[0].simulateOpen();
    callBridge = { enabled: true, botName: 'Receptionist' };
    client.notePhoneChanged();
    await vi.advanceTimersByTimeAsync(1_100);
    const hellos = textFrames(sockets[0]).filter((f) => f.type === 'hello');
    expect(hellos).toHaveLength(2);
    expect(hellos.at(-1)?.phones).toEqual([{ serial: 'ser1', model: 'Pixel 8', name: 'Pixel', callBridge: { enabled: true, botName: 'Receptionist' } }]);
    client.stop();
  });

  it('reconnects with backoff after close; stop() prevents reconnects', async () => {
    const { deps, sockets } = makeDeps();
    const client = new PhoneLinkPhoneSocket(deps);
    client.start();
    await vi.advanceTimersByTimeAsync(0);
    sockets[0].simulateOpen();
    sockets[0].close(1_006, 'gone');
    await vi.advanceTimersByTimeAsync(150);
    expect(sockets).toHaveLength(2);
    client.stop();
    sockets[1].close(1_006, 'gone');
    await vi.advanceTimersByTimeAsync(10_000);
    expect(sockets).toHaveLength(2);
  });

  it('startStream failure sends an end frame with the reason', async () => {
    const { deps, sockets } = makeDeps({ onViewerAttached: async () => ({ ok: false, message: 'That phone is not connected.' }) });
    const client = new PhoneLinkPhoneSocket(deps);
    client.start();
    await vi.advanceTimersByTimeAsync(0);
    sockets[0].simulateOpen();
    sockets[0].simulateServerMessage({ type: 'viewer_attached', serial: 'ser1' });
    await vi.advanceTimersByTimeAsync(0);
    const end = textFrames(sockets[0]).find((f) => f.type === 'end');
    expect(end).toEqual({ type: 'end', reason: 'That phone is not connected.' });
    client.stop();
  });

  it('forwards input/displayPower to the service hooks and answers ping with pong', async () => {
    const inputs: unknown[][] = [];
    let power: boolean | null = null;
    const { deps, sockets } = makeDeps({
      onInput: (_s, events) => {
        inputs.push(events);
        return { ok: true };
      },
      onDisplayPower: (_s, on) => {
        power = on;
        return { ok: true };
      },
    });
    const client = new PhoneLinkPhoneSocket(deps);
    client.start();
    await vi.advanceTimersByTimeAsync(0);
    sockets[0].simulateOpen();
    sockets[0].simulateServerMessage({ type: 'ping' });
    sockets[0].simulateServerMessage({ type: 'input', serial: 'ser1', events: [{ type: 'home' }] });
    sockets[0].simulateServerMessage({ type: 'displayPower', serial: 'ser1', on: false });
    await vi.advanceTimersByTimeAsync(0);
    expect(textFrames(sockets[0]).some((f) => f.type === 'pong')).toBe(true);
    expect(inputs).toEqual([[{ type: 'home' }]]);
    expect(power).toBe(false);
    client.stop();
  });

  it('notifies onViewerDetached only when the last viewer leaves', async () => {
    const detached: string[] = [];
    const { deps, sockets } = makeDeps({ onViewerDetached: (s) => detached.push(s) });
    const client = new PhoneLinkPhoneSocket(deps);
    client.start();
    await vi.advanceTimersByTimeAsync(0);
    sockets[0].simulateOpen();
    sockets[0].simulateServerMessage({ type: 'viewer_attached', serial: 'ser1' });
    sockets[0].simulateServerMessage({ type: 'viewer_attached', serial: 'ser1' });
    await vi.advanceTimersByTimeAsync(0);
    sockets[0].simulateServerMessage({ type: 'viewer_detached', serial: 'ser1' });
    expect(detached).toEqual([]);
    sockets[0].simulateServerMessage({ type: 'viewer_detached', serial: 'ser1' });
    expect(detached).toEqual(['ser1']);
    client.stop();
  });

  it('push() writes meta JSON, raw binary chunks, and end JSON', async () => {
    const { deps, sockets } = makeDeps();
    const client = new PhoneLinkPhoneSocket(deps);
    client.start();
    await vi.advanceTimersByTimeAsync(0);
    sockets[0].simulateOpen();
    client.push({ kind: 'meta', meta: { deviceName: 'Pixel', width: 1080, height: 2400, codec: 'h264' } });
    client.push({ kind: 'chunk', chunk: new Uint8Array([1, 2, 3]) });
    client.push({ kind: 'end', reason: 'done' });
    const sent = sockets[0].sent.slice(1); // [0] is the hello
    expect(sent[0]).toBe(JSON.stringify({ type: 'meta', deviceName: 'Pixel', width: 1080, height: 2400, codec: 'h264' }));
    expect(sent[1]).toBeInstanceOf(Uint8Array);
    expect(sent[2]).toBe(JSON.stringify({ type: 'end', reason: 'done' }));
    client.stop();
  });
  it('sends audio only after a viewer opts in: audioMeta JSON first, then 0xA0-tagged binary frames', async () => {
    const { deps, sockets } = makeDeps();
    const client = new PhoneLinkPhoneSocket(deps);
    client.start();
    await vi.advanceTimersByTimeAsync(0);
    sockets[0].simulateOpen();
    sockets[0].simulateServerMessage({ type: 'viewer_attached', serial: 'ser1' });
    await vi.advanceTimersByTimeAsync(0);
    const audioMeta = { codec: 'opus' as const, sampleRate: 48000, channels: 2, description: 'T3B1c0hlYWQ=' };
    client.push({ kind: 'audioMeta', meta: audioMeta });
    client.push({ kind: 'audioChunk', chunk: new Uint8Array([1, 2]) });
    client.push({ kind: 'chunk', chunk: new Uint8Array([0, 0, 0, 1, 0x65]) });
    // Not opted in: only the video frame went out (old clients see no audio).
    let sent = sockets[0].sent.slice(1);
    expect(sent).toHaveLength(1);
    expect(Array.from(sent[0] as Uint8Array)).toEqual([0, 0, 0, 1, 0x65]);

    sockets[0].simulateServerMessage({ type: 'audio', on: true });
    await vi.advanceTimersByTimeAsync(0);
    client.push({ kind: 'audioChunk', chunk: new Uint8Array([3, 4]) });
    client.push({ kind: 'chunk', chunk: new Uint8Array([0, 0, 1, 0x41]) });
    sent = sockets[0].sent.slice(2);
    expect(sent[0]).toBe(JSON.stringify({ type: 'audioMeta', ...audioMeta }));
    expect(Array.from(sent[1] as Uint8Array)).toEqual([PHONE_AUDIO_FRAME_TAG, 3, 4]);
    expect(Array.from(sent[2] as Uint8Array)).toEqual([0, 0, 1, 0x41]);

    sockets[0].simulateServerMessage({ type: 'audio', on: false });
    await vi.advanceTimersByTimeAsync(0);
    const before = sockets[0].sent.length;
    client.push({ kind: 'audioChunk', chunk: new Uint8Array([5]) });
    expect(sockets[0].sent.length).toBe(before);
    client.stop();
  });

  it('opting in before the format waits for it; the last viewer leaving or an end turns audio off', async () => {
    const { deps, sockets } = makeDeps();
    const client = new PhoneLinkPhoneSocket(deps);
    client.start();
    await vi.advanceTimersByTimeAsync(0);
    sockets[0].simulateOpen();
    sockets[0].simulateServerMessage({ type: 'viewer_attached', serial: 'ser1' });
    sockets[0].simulateServerMessage({ type: 'audio', on: true });
    await vi.advanceTimersByTimeAsync(0);
    client.push({ kind: 'audioChunk', chunk: new Uint8Array([1]) }); // no format yet: dropped
    expect(sockets[0].sent.slice(1)).toEqual([]);
    const audioMeta = { codec: 'aac' as const, sampleRate: 48000, channels: 2, description: 'EZA=' };
    client.push({ kind: 'audioMeta', meta: audioMeta });
    expect(textFrames(sockets[0]).filter((f) => f.type === 'audioMeta')).toHaveLength(1);
    client.push({ kind: 'end', reason: 'done' });
    client.push({ kind: 'audioChunk', chunk: new Uint8Array([2]) }); // format forgotten with the stream
    expect(sockets[0].sent.filter((f) => f instanceof Uint8Array)).toHaveLength(0);

    client.push({ kind: 'audioMeta', meta: audioMeta });
    sockets[0].simulateServerMessage({ type: 'viewer_detached', serial: 'ser1' });
    await vi.advanceTimersByTimeAsync(0);
    client.push({ kind: 'audioChunk', chunk: new Uint8Array([3]) });
    expect(sockets[0].sent.filter((f) => f instanceof Uint8Array)).toHaveLength(0);
    client.stop();
  });
});
