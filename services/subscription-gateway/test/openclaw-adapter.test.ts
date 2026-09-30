// OpenClaw adapter: offline conformance against a real local fake gateway, streaming, isolation via client-side
// history, error mapping (ECONNREFUSED/404/401/429), registration reads only env config.
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { afterAll, beforeAll, describe, expect, it } from "vitest";
import { runConformance, type AaiResult } from "@allternit/agent-gateway";
import { adapterManifestSchema, lookProfileSchema } from "@allternit/subscription-fabric-contracts";
import { CAPABILITIES, OPENCLAW_MANIFEST } from "../adapters/openclaw/manifest.js";
import { OpenClawProvider } from "../adapters/openclaw/index.js";
import { createAaiRegistration } from "../adapters/openclaw/aai.js";
import { startFakeOpenClaw, type FakeOpenClaw } from "../adapters/openclaw/fixtures/fake-server.js";

const AGENT = "openclaw:openclaw";
const val = <T>(r: AaiResult<T>): T => { if (!r.ok) throw new Error(`${r.error.code}: ${r.error.humanMessage}`); return r.value; };
const err = <T>(r: AaiResult<T>) => { if (r.ok) throw new Error("expected error"); return r.error; };

let good: FakeOpenClaw, limited: FakeOpenClaw, locked: FakeOpenClaw, noModels: FakeOpenClaw, dead: string;
beforeAll(async () => {
  [good, limited, locked, noModels] = await Promise.all([
    startFakeOpenClaw(), startFakeOpenClaw({ fault: "rate_limit" }), startFakeOpenClaw({ fault: "unauthorized" }), startFakeOpenClaw({ models: "absent" }),
  ]);
  const tmp = await startFakeOpenClaw(); dead = tmp.url; await tmp.close(); // a port with nothing listening
});
afterAll(async () => { await Promise.all([good, limited, locked, noModels].map((s) => s.close())); });

describe("manifest + registration", () => {
  it("valid, local/exact, isolated", () => {
    expect(adapterManifestSchema.safeParse(OPENCLAW_MANIFEST).success).toBe(true);
    expect(CAPABILITIES).toMatchObject({ lane: "local", guarantee: "exact", context: { isolation: "isolated" }, messaging: { stream: true } });
    expect(OPENCLAW_MANIFEST.agent!.authDescriptors[0]).toMatchObject({ authType: "local_endpoint", lane: "local" });
  });
  it("look profile parses", () => {
    const lp = lookProfileSchema.parse(JSON.parse(readFileSync(fileURLToPath(new URL("../adapters/openclaw/look-profile.json", import.meta.url)), "utf8")));
    expect(lp.iconAssets?.app).toMatch(/^TODO:/);
  });
  it("createAaiRegistration reads URL/token/agent/max from env", async () => {
    const reg = createAaiRegistration({ SUBS_GATEWAY_OPENCLAW_URL: good.url, SUBS_GATEWAY_OPENCLAW_MAX_PARALLEL: "2" });
    expect(reg.provider.adapterId).toBe("openclaw");
    expect(val(await reg.provider.list())[0].agentId).toBe(AGENT);
  });
});

describe("conformance (offline)", () => {
  it("passes every declared area", async () => {
    const provider = new OpenClawProvider({ baseUrl: good.url, replyTimeoutMs: 5000 });
    const report = await runConformance(provider, {
      agentId: AGENT, settleMs: 10,
      faulty: {
        vendor_down: () => new OpenClawProvider({ baseUrl: dead }),
        rate_limited: () => new OpenClawProvider({ baseUrl: limited.url }),
        auth_revoked: () => new OpenClawProvider({ baseUrl: locked.url, token: "stale" }),
      },
    });
    const bad = report.areas.flatMap((a) => a.checks.filter((c) => c.status === "fail").map((c) => `${a.area}: ${c.name}: ${c.reason}`));
    expect(bad).toEqual([]);
    expect(report.ok).toBe(true);
    const by = Object.fromEntries(report.areas.map((a) => [a.area, a.status]));
    for (const a of ["identity", "context", "isolation", "parallelism", "events", "failure", "idempotency", "cancellation"]) expect(by[a]).toBe("pass");
  });
});

describe("behaviour", () => {
  it("streams SSE deltas into events and replays client-side history per context", async () => {
    const p = new OpenClawProvider({ baseUrl: good.url });
    const A = val(await p.contextOpen({ agentId: AGENT })).contextId, B = val(await p.contextOpen({ agentId: AGENT })).contextId;
    const a1 = val(await p.contextMessage({ contextId: A, correlationId: "a1", text: "hello" }));
    const a2 = val(await p.contextMessage({ contextId: A, correlationId: "a2", text: "again" }));
    const b1 = val(await p.contextMessage({ contextId: B, correlationId: "b1", text: "solo" }));
    expect(a1.reply).toBe("echo[1]: hello");
    expect(a2.reply).toBe("echo[2]: again"); // history replayed
    expect(b1.reply).toBe("echo[1]: solo");  // B never saw A
    const evs = val(await p.events({ contextId: A })).events.map((e) => e.event.type);
    expect(evs.filter((t) => t === "agent.message.delta").length).toBeGreaterThan(2); // real streaming
    expect(evs.filter((t) => t === "agent.message.completed")).toHaveLength(2);
    expect(good.requests.some((r) => r.body?.stream === true && r.body?.model === "openclaw")).toBe(true);
  });
  it("falls back to plain JSON when the server does not stream", async () => {
    const s = await startFakeOpenClaw({ jsonOnly: true });
    const p = new OpenClawProvider({ baseUrl: s.url });
    const c = val(await p.contextOpen({ agentId: AGENT })).contextId;
    expect(val(await p.contextMessage({ contextId: c, correlationId: "j", text: "x" })).reply).toBe("echo[1]: x");
    await s.close();
  });
  it("agent.list uses /v1/models, else the configured agent; unknown agent is rejected", async () => {
    const multi = await startFakeOpenClaw({ models: ["openclaw", "openclaw/research"] });
    const p = new OpenClawProvider({ baseUrl: multi.url });
    expect(val(await p.list()).map((a) => a.agentId)).toEqual(["openclaw:openclaw", "openclaw:openclaw/research"]);
    const c = val(await p.contextOpen({ agentId: "openclaw:openclaw/research" }));
    expect(c.contextId).toBeTruthy();
    expect(multi.requests.filter((r) => r.path === "/v1/models").length).toBeGreaterThan(0);
    expect(err(await p.get("openclaw:nope")).code).toBe("CONTEXT_NOT_FOUND");
    await multi.close();
    const q = new OpenClawProvider({ baseUrl: noModels.url, defaultAgent: "main" });
    expect(val(await q.list()).map((a) => a.agentId)).toEqual(["openclaw:main"]);
  });
  it("bearer token is sent when configured and never appears in errors", async () => {
    const s = await startFakeOpenClaw({ token: "s3cret-token" });
    expect(err(await new OpenClawProvider({ baseUrl: s.url }).list()).code).toBe("AUTH_REQUIRED");
    const p = new OpenClawProvider({ baseUrl: s.url, token: "s3cret-token" });
    expect(val(await p.list())).toHaveLength(1);
    expect(s.requests.at(-1)?.auth).toBe("Bearer s3cret-token");
    const bad = err(await new OpenClawProvider({ baseUrl: s.url, token: "wrong" }).list());
    expect(bad.code).toBe("AUTH_REVOKED");
    expect(JSON.stringify(bad)).not.toContain("wrong");
    await s.close();
  });
  it("ECONNREFUSED and 404 map to VENDOR_UNAVAILABLE with a start-OpenClaw message", async () => {
    const down = new OpenClawProvider({ baseUrl: dead });
    const e1 = err(await down.contextOpen({ agentId: AGENT }));
    expect(e1).toMatchObject({ code: "VENDOR_UNAVAILABLE", retryable: true });
    expect(e1.humanMessage).toMatch(/Start it/);
    expect(val(await down.health({})).status).toBe("down");
    const s = await startFakeOpenClaw({ fault: "not_found" });
    // /v1/models 404 only means "no listing" (configured agent used); the chat endpoint 404 is what surfaces.
    const q = new OpenClawProvider({ baseUrl: s.url });
    const c = val(await q.contextOpen({ agentId: AGENT })).contextId;
    const e2 = err(await q.contextMessage({ contextId: c, correlationId: "nf", text: "x" }));
    expect(e2).toMatchObject({ code: "VENDOR_UNAVAILABLE", retryable: true });
    expect(e2.humanMessage).toMatch(/Start it/);
    expect(val(await q.health({})).status).toBe("degraded");
    await s.close();
  });
  it("cancel aborts the in-flight request, keeps history clean, and the context stays usable", async () => {
    const s = await startFakeOpenClaw({ chunkDelayMs: 40, chunks: 20 });
    const p = new OpenClawProvider({ baseUrl: s.url });
    const c = val(await p.contextOpen({ agentId: AGENT })).contextId;
    const pending = p.contextMessage({ contextId: c, correlationId: "slow", text: "long" });
    await new Promise((r) => setTimeout(r, 120));
    expect(val(await p.contextCancel({ contextId: c })).confirmed).toBe(true);
    expect(err(await pending).details).toMatchObject({ cancelled: true });
    await new Promise((r) => setTimeout(r, 100));
    expect(s.aborted).toBe(1);
    const n = val(await p.events({ contextId: c })).events.length;
    await new Promise((r) => setTimeout(r, 150));
    expect(val(await p.events({ contextId: c })).events.length).toBe(n); // no deltas after cancel
    const next = val(await p.contextMessage({ contextId: c, correlationId: "next", text: "again" }));
    expect(next.reply).toMatch(/^echo\[1\]/); // cancelled turn was not committed to history
    await s.close();
  });
});
