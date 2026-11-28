import { describe, expect, it, vi } from "vitest";
import {
  BUFFER_PACKETS,
  CloseReason,
  PacketType,
  StreamType,
  UserRelay,
  WS_CLOSE_REPLACED,
  decodePacket,
  encodeClose,
  encodeConnect,
  encodeData,
  type Limits,
  type SocketLike,
  type Usage,
  type UsageStore,
} from "../src/wisp";

const flush = async () => {
  for (let i = 0; i < 20; i++) await new Promise((r) => setImmediate(r));
};

function fakeSocket() {
  let push!: (b: Uint8Array) => void;
  let end!: () => void;
  const readable = new ReadableStream<Uint8Array>({
    start(c) {
      push = (b) => c.enqueue(b);
      end = () => c.close();
    },
  });
  const written: string[] = [];
  const writable = new WritableStream<Uint8Array>({
    write(chunk) {
      written.push(new TextDecoder().decode(chunk));
    },
  });
  const state = { closed: false };
  const socket: SocketLike = {
    readable,
    writable,
    opened: Promise.resolve(),
    close: async () => {
      state.closed = true;
    },
  };
  return { socket, push: (s: string) => push(new TextEncoder().encode(s)), end: () => end(), written, state };
}

function fakeWs() {
  const sent: { type: number; streamId: number; payload: Uint8Array }[] = [];
  const ws: { closed?: { code?: number; reason?: string }; send(d: Uint8Array): void; close(c?: number, r?: string): void } = {
    closed: undefined,
    send(data) {
      const p = decodePacket(data);
      if (p) sent.push({ ...p, payload: p.payload.slice() });
    },
    close(code, reason) {
      ws.closed = { code, reason };
    },
  };
  const closes = (id: number) =>
    sent.filter((p) => p.type === PacketType.CLOSE && p.streamId === id).map((p) => p.payload[0]);
  return { ws, sent, closes };
}

function memoryStore(): UsageStore & { value?: Usage } {
  const store: UsageStore & { value?: Usage } = {
    value: undefined,
    load: async () => store.value,
    save: async (u) => {
      store.value = { ...u };
    },
  };
  return store;
}

function setup(opts: { limits?: Partial<Limits>; resolve?: (h: string) => Promise<string[]>; store?: UsageStore } = {}) {
  const clock = { t: Date.UTC(2026, 10, 27, 12) };
  const sockets: ReturnType<typeof fakeSocket>[] = [];
  const connect = vi.fn(() => {
    const s = fakeSocket();
    sockets.push(s);
    return s.socket;
  });
  const logs: Record<string, string | number>[] = [];
  const relay = new UserRelay({
    connect,
    resolve: opts.resolve ?? (async () => ["104.16.1.1"]),
    store: opts.store ?? memoryStore(),
    log: (e) => logs.push(e),
    now: () => clock.t,
    limits: opts.limits,
  });
  const { ws, sent, closes } = fakeWs();
  const session = relay.open(ws);
  return { relay, session, ws, sent, closes, connect, sockets, logs, clock };
}

describe("Wisp session", () => {
  it("announces the stream buffer on stream 0 when the socket opens", () => {
    const { sent } = setup();
    expect(sent[0]?.type).toBe(PacketType.CONTINUE);
    expect(sent[0]?.streamId).toBe(0);
    expect(new DataView(sent[0]!.payload.buffer).getUint32(0, true)).toBe(BUFFER_PACKETS);
  });

  it("connects an allowed host on 443 to the checked address and pipes both ways", async () => {
    const { session, connect, sockets, sent, closes } = setup();
    session.onMessage(encodeConnect(1, StreamType.TCP, 443, "registry.npmjs.org"));
    session.onMessage(encodeData(1, new TextEncoder().encode("hello")));
    await flush();

    expect(connect).toHaveBeenCalledWith(
      { hostname: "104.16.1.1", port: 443 },
      { secureTransport: "off", allowHalfOpen: false },
    );
    expect(sockets[0]!.written).toEqual(["hello"]);

    sockets[0]!.push("world");
    await flush();
    const data = sent.find((p) => p.type === PacketType.DATA && p.streamId === 1);
    expect(new TextDecoder().decode(data!.payload)).toBe("world");

    // Remote finished: no half-close in Wisp v1, the whole stream closes.
    sockets[0]!.end();
    await flush();
    expect(closes(1)).toEqual([CloseReason.VOLUNTARY]);
  });

  it.each([
    ["example.com", 443],
    ["evil.github.com", 443],
    ["github.com.evil.net", 443],
    ["github.com.", 443],
    ["github.com", 80],
    ["github.com", 22],
  ])("refuses %s:%i with CLOSE 0x48 and never connects", async (host, port) => {
    const { session, connect, closes } = setup();
    session.onMessage(encodeConnect(7, StreamType.TCP, port, host));
    await flush();
    expect(closes(7)).toEqual([CloseReason.BLOCKED]);
    expect(connect).not.toHaveBeenCalled();
  });

  it.each(["140.82.112.3", "127.0.0.1", "[::1]", "::1", "2130706433", "0x7f.1"])(
    "refuses the IP literal %s",
    async (host) => {
      const { session, connect, closes } = setup();
      session.onMessage(encodeConnect(3, StreamType.TCP, 443, host));
      await flush();
      expect(closes(3)).toEqual([CloseReason.BLOCKED]);
      expect(connect).not.toHaveBeenCalled();
    },
  );

  it("refuses UDP streams", async () => {
    const { session, connect, closes } = setup();
    session.onMessage(encodeConnect(2, StreamType.UDP, 443, "github.com"));
    await flush();
    expect(closes(2)).toEqual([CloseReason.INVALID_INFO]);
    expect(connect).not.toHaveBeenCalled();
  });

  it("refuses a listed name that resolves to a private address (rebinding guard)", async () => {
    const { session, connect, closes } = setup({ resolve: async () => ["104.16.1.1", "10.0.0.5"] });
    session.onMessage(encodeConnect(4, StreamType.TCP, 443, "github.com"));
    await flush();
    expect(closes(4)).toEqual([CloseReason.BLOCKED]);
    expect(connect).not.toHaveBeenCalled();
  });

  it("flushes queued data before closing the socket on a client CLOSE", async () => {
    const { session, sockets } = setup();
    session.onMessage(encodeConnect(5, StreamType.TCP, 443, "pypi.org"));
    session.onMessage(encodeData(5, new TextEncoder().encode("GET / HTTP/1.1\r\n\r\n")));
    session.onMessage(encodeClose(5, CloseReason.VOLUNTARY));
    await flush();
    expect(sockets[0]!.written).toEqual(["GET / HTTP/1.1\r\n\r\n"]);
    expect(sockets[0]!.state.closed).toBe(true);
  });
});

describe("Per-user caps", () => {
  it("allows 32 open streams and throttles the 33rd", async () => {
    const { session, closes } = setup();
    for (let id = 1; id <= 33; id++) session.onMessage(encodeConnect(id, StreamType.TCP, 443, "github.com"));
    await flush();
    expect(session.openStreams).toBe(32);
    expect(closes(32)).toEqual([]);
    expect(closes(33)).toEqual([CloseReason.THROTTLED]);
  });

  it("allows 100 new streams a minute, then throttles until the minute passes", async () => {
    const { session, closes, clock } = setup();
    for (let id = 1; id <= 100; id++) {
      session.onMessage(encodeConnect(id, StreamType.TCP, 443, "github.com"));
      session.onMessage(encodeClose(id, CloseReason.VOLUNTARY));
    }
    session.onMessage(encodeConnect(101, StreamType.TCP, 443, "github.com"));
    await flush();
    expect(closes(101)).toEqual([CloseReason.THROTTLED]);

    clock.t += 61_000;
    session.onMessage(encodeConnect(102, StreamType.TCP, 443, "github.com"));
    await flush();
    expect(closes(102)).toEqual([]);
    expect(session.openStreams).toBe(1);
  });

  it("closes a stream with 0x49 past the daily byte budget, and it survives a restart", async () => {
    const store = memoryStore();
    const first = setup({ limits: { maxBytesPerDay: 10 }, store });
    first.session.onMessage(encodeConnect(1, StreamType.TCP, 443, "crates.io"));
    first.session.onMessage(encodeData(1, new Uint8Array(20)));
    await flush();
    expect(first.closes(1)).toEqual([CloseReason.THROTTLED]);

    first.session.onMessage(encodeConnect(2, StreamType.TCP, 443, "crates.io"));
    await flush();
    expect(first.closes(2)).toEqual([CloseReason.THROTTLED]);

    // The Durable Object restarts: usage is loaded back from storage.
    first.ws.close();
    first.session.onSocketClosed();
    await flush();
    const relay = new UserRelay({
      connect: vi.fn(),
      resolve: async () => ["104.16.1.1"],
      store,
      log: () => {},
      now: () => first.clock.t,
      limits: { maxBytesPerDay: 10 },
    });
    await relay.loadUsage();
    const { ws, closes } = fakeWs();
    relay.open(ws).onMessage(encodeConnect(1, StreamType.TCP, 443, "crates.io"));
    await flush();
    expect(closes(1)).toEqual([CloseReason.THROTTLED]);
  });

  it("closes the first socket with 4001 when the same user opens a second", async () => {
    const { relay, ws, session, sockets } = setup();
    session.onMessage(encodeConnect(1, StreamType.TCP, 443, "github.com"));
    await flush();
    const second = fakeWs();
    relay.open(second.ws);
    expect(ws.closed?.code).toBe(WS_CLOSE_REPLACED);
    expect(sockets[0]!.state.closed).toBe(true);
    expect(relay.session).not.toBe(session);
    expect(second.ws.closed).toBeUndefined();
  });

  it("ends a socket older than 2 hours", () => {
    const { session, ws, clock } = setup();
    clock.t += 2 * 60 * 60 * 1000 + 1;
    session.onMessage(encodeConnect(1, StreamType.TCP, 443, "github.com"));
    expect(ws.closed?.code).toBe(1000);
  });

  it("logs hostnames and counts, never payload", async () => {
    const { session, sockets, logs } = setup();
    session.onMessage(encodeConnect(1, StreamType.TCP, 443, "github.com"));
    session.onMessage(encodeData(1, new TextEncoder().encode("secret-token-in-body")));
    await flush();
    sockets[0]!.end();
    await flush();
    const text = JSON.stringify(logs);
    expect(text).toContain("github.com");
    expect(text).not.toContain("secret-token-in-body");
  });
});
