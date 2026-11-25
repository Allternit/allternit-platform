import { describe, expect, test, vi } from "vitest";
import { createShareTunnel, ShareTunnelError, type ShareGate, type TunnelClient } from "../src/index.js";

const link = { id: "shl_1", subdomain: "k3x9q2m7v8w4p5ra" };

function fakes(heartbeatStatus: () => number, heartbeatBody: () => unknown = () => ({ status: "active" })) {
  const gate: ShareGate & { closed: string[] } = {
    closed: [],
    open: vi.fn(async () => ({ id: "gate_1", target: "127.0.0.1:41234", token: "ast_t1", authMode: "token" as const, paused: false })),
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

  test("a paused link stops and drops its identity (cloud-api deleted the relay tunnel)", async () => {
    const f = fakes(() => 200, () => ({ status: "paused" }));
    const share = await f.tunnel.start({ link, port: 3000, admission: "ota1.p.s" });
    await expect(share.stopped).resolves.toBe("paused");
    expect(f.remove).toHaveBeenCalled();
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

describe("httpShareGate against the allternit-api /shares contract", () => {
  test("open sends share_link_id, returns target + token; pause/rotate/stop use the id routes", async () => {
    const { httpShareGate } = await import("../src/index.js");
    const calls: { url: string; method: string; headers: Record<string, string>; body?: unknown }[] = [];
    const share = { id: "shr_1", gate_url: "http://127.0.0.1:45001", gate_port: 45001, auth_mode: "token", status: "active" };
    const fetch = (async (url: string, init: RequestInit) => {
      calls.push({ url, method: init.method!, headers: init.headers as Record<string, string>, body: init.body && JSON.parse(String(init.body)) });
      const path = new URL(url).pathname;
      const body = path.endsWith("/stop") ? { stopped: true, id: "shr_1" } : path.endsWith("/rotate") ? { share, token: "ast_new" } : path.endsWith("/pause") ? { share: { ...share, status: "paused" } } : { share, token: "ast_t1" };
      return new Response(JSON.stringify(body), { status: path === "/shares" ? 201 : 200 });
    }) as unknown as typeof globalThis.fetch;
    const gate = httpShareGate({ baseUrl: "http://127.0.0.1:8013/", auth: { bearer: "sess" }, fetch });
    const opened = await gate.open({ linkId: "shl_1", port: 3000, authMode: "token" });
    expect(opened).toMatchObject({ id: "shr_1", target: "127.0.0.1:45001", token: "ast_t1" });
    expect(calls[0]).toMatchObject({ url: "http://127.0.0.1:8013/shares", method: "POST", body: { port: 3000, share_link_id: "shl_1", auth_mode: "token", subject: "app" } });
    expect(calls[0]!.headers.authorization).toBe("Bearer sess");
    expect(await gate.pause!("shr_1")).toBe(true);
    expect(await gate.rotate!("shr_1")).toBe("ast_new");
    await gate.close("shr_1");
    expect(calls.map((c) => c.url.replace("http://127.0.0.1:8013", ""))).toEqual(["/shares", "/shares/shr_1/pause", "/shares/shr_1/rotate", "/shares/shr_1/stop"]);
  });

  test("a refused port surfaces as port_denied", async () => {
    const { httpShareGate } = await import("../src/index.js");
    const fetch = (async () => new Response(JSON.stringify({ error: "port_refused", message: "no" }), { status: 403 })) as unknown as typeof globalThis.fetch;
    const gate = httpShareGate({ baseUrl: "http://x", auth: { userId: "u", desktopAccessToken: "t" }, fetch });
    await expect(gate.open({ linkId: "l", port: 8013, authMode: "token" })).rejects.toMatchObject({ code: "port_denied" });
  });
});
