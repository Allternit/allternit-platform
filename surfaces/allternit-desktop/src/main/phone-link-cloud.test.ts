/** Tests for the Phone Link cloud session client: lifecycle, hello, backoff, stream-control dispatch. */

import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { PhoneLinkCloudClient, SOCKET_OPEN, type PhoneLinkSocket, type PhoneLinkCloudDeps } from './phone-link-cloud.js';

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

function jsonSent(socket: FakeSocket): Record<string, unknown>[] {
  return socket.sent.filter((s) => typeof s === 'string').map((s) => JSON.parse(s as string) as Record<string, unknown>);
}

describe('PhoneLinkCloudClient', () => {
  beforeEach(() => {
    vi.useFakeTimers();
  });
  afterEach(() => {
    vi.useRealTimers();
  });

  function makeDeps(overrides: Partial<PhoneLinkCloudDeps> = {}) {
    const sockets: FakeSocket[] = [];
    const deps: PhoneLinkCloudDeps = {
      apiUrl: () => 'http://127.0.0.1:18013',
      getSession: async () => ({ accessToken: 'tok', userId: 'user-1', runtimeId: 'rt-1' }),
      phones: () => [{ serial: 'ser1', model: 'Pixel 8', name: 'Pixel' }],
      onStreamControl: async () => ({ ok: true }),
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

  it('connects, sends hello with phones, and heartbeats', async () => {
    const { deps, sockets } = makeDeps();
    const client = new PhoneLinkCloudClient(deps);
    client.start();
    await vi.advanceTimersByTimeAsync(0);
    const socket = sockets[0];
    socket.simulateOpen();
    const frames = jsonSent(socket);
    expect(frames[0]).toEqual({ type: 'hello', phones: [{ serial: 'ser1', model: 'Pixel 8', name: 'Pixel' }] });
    const pingsBefore = jsonSent(socket).filter((f) => f.type === 'ping').length;
    await vi.advanceTimersByTimeAsync(25_000);
    expect(jsonSent(socket).filter((f) => f.type === 'ping').length).toBeGreaterThan(pingsBefore);
    client.stop();
  });

  it('re-sends hello (debounced) when the phone inventory changes', async () => {
    let phones = [{ serial: 'ser1', model: 'Pixel 8', name: 'Pixel' }];
    const { deps, sockets } = makeDeps({ phones: () => phones });
    const client = new PhoneLinkCloudClient(deps);
    client.start();
    await vi.advanceTimersByTimeAsync(0);
    sockets[0].simulateOpen();
    expect(jsonSent(sockets[0]).filter((f) => f.type === 'hello')).toHaveLength(1);
    phones = [...phones, { serial: 'ser2', model: 'Pixel 8', name: 'Pixel 2' }];
    client.notePhonesChanged();
    await vi.advanceTimersByTimeAsync(1_100);
    const hellos = jsonSent(sockets[0]).filter((f) => f.type === 'hello');
    expect(hellos).toHaveLength(2);
    expect((hellos[1].phones as unknown[]).length).toBe(2);
    client.stop();
  });

  it('reconnects with backoff after a close, and stop() prevents further reconnects', async () => {
    const { deps, sockets } = makeDeps();
    const client = new PhoneLinkCloudClient(deps);
    client.start();
    await vi.advanceTimersByTimeAsync(0);
    sockets[0].simulateOpen();
    sockets[0].close(1_006, 'gone');
    await vi.advanceTimersByTimeAsync(150);
    expect(sockets).toHaveLength(2);
    client.stop();
    sockets[1].simulateOpen();
    sockets[1].close(1_006, 'gone');
    await vi.advanceTimersByTimeAsync(10_000);
    expect(sockets).toHaveLength(2); // no third socket after stop
  });

  it('retries slowly while signed out instead of hammering', async () => {
    const { deps, sockets } = makeDeps({ getSession: async () => null });
    const client = new PhoneLinkCloudClient(deps);
    client.start();
    await vi.advanceTimersByTimeAsync(0);
    expect(sockets).toHaveLength(0);
    await vi.advanceTimersByTimeAsync(150);
    expect(sockets).toHaveLength(0); // first retry also has no session
    client.stop();
  });

  it('dispatches start/stop-stream control frames and acks them', async () => {
    const controls: Record<string, unknown>[] = [];
    const { deps, sockets } = makeDeps({
      onStreamControl: async (frame) => {
        controls.push(frame as unknown as Record<string, unknown>);
        return { ok: true };
      },
    });
    const client = new PhoneLinkCloudClient(deps);
    client.start();
    await vi.advanceTimersByTimeAsync(0);
    sockets[0].simulateOpen();
    sockets[0].simulateServerMessage({ type: 'start-stream', serial: 'ser1', opts: { maxSize: 720 }, requestId: 'r1' });
    await vi.advanceTimersByTimeAsync(0);
    expect(controls[0]).toMatchObject({ type: 'start-stream', serial: 'ser1', opts: { maxSize: 720 } });
    await vi.advanceTimersByTimeAsync(0);
    const ack = jsonSent(sockets[0]).find((f) => f.type === 'ack');
    expect(ack).toMatchObject({ requestId: 'r1', ok: true });
    client.stop();
  });

  it('sends auth headers with the device token at connect', async () => {
    const seen: { url: string; headers: Record<string, string> }[] = [];
    const { deps } = makeDeps({
      createSocket: (url, headers) => {
        seen.push({ url, headers });
        return new FakeSocket();
      },
    });
    const client = new PhoneLinkCloudClient(deps);
    client.start();
    await vi.advanceTimersByTimeAsync(0);
    expect(seen[0].url).toBe('ws://127.0.0.1:18013/ws/phone-link/session');
    expect(seen[0].headers.authorization).toBe('Bearer tok');
    expect(seen[0].headers['x-allternit-runtime']).toBe('rt-1');
    client.stop();
  });
});
