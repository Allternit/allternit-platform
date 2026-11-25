import { describe, expect, test, vi } from "vitest";
import { createShareTunnel, ShareTunnelError, type ShareGate, type TunnelClient } from "../src/index.js";

const link = { id: "shl_1", subdomain: "k3x9q2m7v8w4p5ra" };

function fakes(heartbeatStatus: () => number, heartbeatBody: () => unknown = () => ({ status: "active" })) {
  const gate: ShareGate & { closed: string[] } = {
    closed: [],
    open: vi.fn(async () => ({ id: "gate_1", target: "127.0.0.1:41234" })),
    async close(id) {
      this.closed.push(id);
    },
  };
  const close = vi.fn(async () => undefined);
  const remove = vi.fn(async () => undefined);
  const apis: string[] = [];
  const openTunnel = (api: string): TunnelClient => {
    apis.push(api);
    return {
      dispose: async () => undefined,
      tunnel: {
        get: async () => undefined,
        remove,
        connect: (async () => ({
          tunnel: { id: link.subdomain, hostname: `${link.subdomain}.live.example.test` },
          closed: new Promise<void>(() => undefined),
          close,
        })) as unknown as TunnelClient["tunnel"]["connect"],
      },
    };
  };
  const fetch = vi.fn(async () => new Response(JSON.stringify(heartbeatBody()), { status: heartbeatStatus() }));
  const tunnel = createShareTunnel({
    relayApi: "https://relay.example.test",
    cloudApi: "https://api.example.test",
    getAuthToken: async () => "session",
    gate,
    heartbeatMs: 20,
    openTunnel,
    fetch: fetch as unknown as typeof globalThis.fetch,
  });
  return { tunnel, gate, close, remove, apis, fetch };
}

describe("share tunnel", () => {
  test("a killed link (heartbeat 410) stops serving and forgets the tunnel", async () => {
    let status = 200;
    const f = fakes(() => status, () => (status === 410 ? { error: "killed" } : { status: "active" }));
    const share = await f.tunnel.start({ link, port: 3000, admission: "ota1.p.s" });
    expect(share.url).toBe(`https://${link.subdomain}.live.example.test`);
    expect(f.apis.at(-1)).toBe("https://relay.example.test/admit/ota1.p.s");
    status = 410;
    await expect(share.stopped).resolves.toBe("killed");
    expect(f.close).toHaveBeenCalled();
    expect(f.gate.closed).toEqual(["gate_1"]);
    expect(f.remove).toHaveBeenCalled();
    expect(f.tunnel.list()).toEqual([]);
  });

  test("a paused link stops but keeps its tunnel for resume", async () => {
    const f = fakes(() => 200, () => ({ status: "paused" }));
    const share = await f.tunnel.start({ link, port: 3000, admission: "ota1.p.s" });
    await expect(share.stopped).resolves.toBe("paused");
    expect(f.remove).not.toHaveBeenCalled();
  });

  test("Allternit's own ports are refused before the gate opens", async () => {
    const f = fakes(() => 200);
    for (const port of [8013, 4096]) {
      await expect(f.tunnel.start({ link, port, admission: "x" })).rejects.toMatchObject({ code: "port_denied" });
    }
    expect(f.gate.open).not.toHaveBeenCalled();
  });

  test("a first start without a credential is refused", async () => {
    const f = fakes(() => 200);
    const error = await f.tunnel.start({ link, port: 3000 }).catch((e: unknown) => e);
    expect(error).toBeInstanceOf(ShareTunnelError);
    expect((error as ShareTunnelError).code).toBe("admission_required");
  });
});
