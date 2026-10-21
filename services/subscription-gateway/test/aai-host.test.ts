// AAI host: /aai/call dispatch, error envelope, auth, cursor passthrough, kill switch, pacing.
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import request from "supertest";
import { MemoryProvider } from "@allternit/agent-gateway";
import { AaiHost } from "../src/aai/registry.js";
import { issueToken } from "../src/security/tokens.js";
import { cleanupDir, makeDeps, tmpStateDir, type TestDeps } from "./helpers.js";
import { createServer } from "../src/http/server.js";

const binding = (over: Record<string, unknown> = {}) => ({
  id: "b1", botId: "bot-1", type: "vendor", mode: "hosted", adapterId: "memory-test",
  externalAgentId: "mem-agent", state: "BOUND", ...over,
});

let dir: string;
let deps: TestDeps;
let host: AaiHost;
let app: ReturnType<typeof createServer>;
let tok: string;

function setup(reg: Partial<Parameters<AaiHost["register"]>[0]> = {}, disabled: string[] = []) {
  host = new AaiHost(disabled).register({ provider: new MemoryProvider(), ...reg });
  app = createServer({ ...deps, aai: host });
}

beforeEach(() => {
  dir = tmpStateDir();
  deps = makeDeps(dir);
  tok = issueToken(deps.db, "api", "test", ["tasks:submit", "tasks:read"]).token;
  setup();
});
afterEach(() => { deps.cleanup(); cleanupDir(dir); });

const call = (op: string, input: object = {}, b: object = binding(), token = tok) =>
  request(app).post("/aai/call").set("Authorization", `Bearer ${token}`).send({ op, binding: b, input });

describe("POST /aai/call", () => {
  it("requires auth and scope", async () => {
    await request(app).post("/aai/call").send({ op: "agent.list", binding: binding(), input: {} }).expect(401);
    const weak = issueToken(deps.db, "x", "weak", ["tasks:read"]).token;
    await call("agent.list", {}, binding(), weak).expect(403);
  });

  it("dispatches ops through the router (list, capabilities, open/message)", async () => {
    const list = await call("agent.list").expect(200);
    expect(list.body.ok).toBe(true);
    expect(list.body.value[0].agentId).toBe("mem-agent");
    const caps = await call("agent.capabilities", {}).expect(200);
    expect(caps.body.ok).toBe(true);
    const open = await call("agent.context.open", { agentId: "mem-agent" });
    expect(open.body.ok).toBe(true);
    const msg = await call("agent.context.message", { contextId: open.body.value.contextId, correlationId: "c1", text: "hi" });
    expect(msg.body.ok).toBe(true);
  });

  it("unknown op -> UNSUPPORTED envelope with HTTP 200", async () => {
    const r = await call("agent.bogus").expect(200);
    expect(r.body).toMatchObject({ ok: false, error: { code: "UNSUPPORTED" } });
  });

  it("AAI errors are envelopes (200): unregistered adapter, dead binding", async () => {
    const a = await call("agent.list", {}, binding({ adapterId: "nope" })).expect(200);
    expect(a.body.error.code).toBe("UNSUPPORTED");
    const b = await call("agent.list", {}, binding({ state: "NEEDS_AUTH" })).expect(200);
    expect(b.body).toMatchObject({ ok: false, error: { code: "AUTH_REQUIRED" } });
  });

  it("malformed body is a transport 400", async () => {
    await request(app).post("/aai/call").set("Authorization", `Bearer ${tok}`).send({ op: "agent.list" }).expect(400);
  });

  it("agent.events passes cursor through and returns {events, cursor}", async () => {
    const open = await call("agent.context.open", { agentId: "mem-agent" });
    const contextId = open.body.value.contextId;
    await call("agent.context.message", { contextId, correlationId: "c1", text: "hi" });
    const first = await call("agent.events", { contextId });
    expect(first.body.ok).toBe(true);
    expect(typeof first.body.value.cursor).toBe("string");
    expect(Array.isArray(first.body.value.events)).toBe(true);
    const second = await call("agent.events", { contextId, cursor: first.body.value.cursor });
    expect(second.body.ok).toBe(true);
    expect(second.body.value.events.length).toBe(0);
  });
});

describe("kill switch + pacing", () => {
  it("disabled adapter blocks vendor-touching ops with LANE_BLOCKED, keeps discovery", async () => {
    setup({}, ["memory-test"]);
    const open = await call("agent.context.open", { agentId: "mem-agent" }).expect(200);
    expect(open.body).toMatchObject({ ok: false, error: { code: "LANE_BLOCKED" } });
    expect((await call("agent.capabilities")).body.ok).toBe(true);
    host.setDisabled("memory-test", false);
    expect((await call("agent.context.open", { agentId: "mem-agent" })).body.ok).toBe(true);
  });

  it("pacing: min gap and hourly cap answer LANE_BLOCKED (retryable)", async () => {
    setup({ pacing: { minGapMs: 60_000 } });
    expect((await call("agent.context.open", { agentId: "mem-agent" })).body.ok).toBe(true);
    const r = await call("agent.context.open", { agentId: "mem-agent" });
    expect(r.body).toMatchObject({ ok: false, error: { code: "LANE_BLOCKED", retryable: true } });
    setup({ pacing: { maxPerHour: 1 } });
    await call("agent.context.open", { agentId: "mem-agent" });
    expect((await call("agent.context.open", { agentId: "mem-agent" })).body.error.code).toBe("LANE_BLOCKED");
  });
});

describe("GET /aai/providers + POST /aai/conformance", () => {
  it("lists providers with manifests (auth required)", async () => {
    await request(app).get("/aai/providers").expect(401);
    const r = await request(app).get("/aai/providers").set("Authorization", `Bearer ${tok}`).expect(200);
    expect(r.body.providers[0]).toMatchObject({ adapterId: "memory-test", disabled: false });
    expect(r.body.providers[0].manifest.adapterId).toBe("memory-test");
  });

  it("runs conformance for a registered provider, 404 otherwise", async () => {
    setup({ fixtures: { agentId: "mem-agent", approvalId: "appr-1" } });
    const r = await request(app).post("/aai/conformance/memory-test").set("Authorization", `Bearer ${tok}`).expect(200);
    expect(r.body.adapterId).toBe("memory-test");
    // Harness runs against the raw registered provider; pass/fail semantics are covered in agent-gateway tests.
    expect(r.body.areas.map((a: any) => a.area)).toContain("identity");
    expect(r.body.summary.pass).toBeGreaterThan(0);
    await request(app).post("/aai/conformance/nope").set("Authorization", `Bearer ${tok}`).expect(404);
  });
});

describe("vendor adapter registration", () => {
  it("loads adapters/<id>/aai.ts at boot (grok-bot) without touching the app", async () => {
    const { AaiHost, registerVendorAdapters } = await import("../src/aai/registry.js");
    const { defaultAdaptersDir } = await import("../src/adapters/registry.js");
    const host = new AaiHost();
    const ids = await registerVendorAdapters(host, defaultAdaptersDir(), {});
    expect(ids).toContain("grok-bot");
  });
});

describe("wire shapes sent by allternit-api (gateway_runner.rs / agent_gateway_routes.rs)", () => {
  it("accepts SQL-null binding columns and a transient discovery binding", async () => {
    // exec row as rows() emits it: NULL columns are JSON null
    const row = binding({ vendor: null, accountBindingId: null, preferredLane: null, capabilities: null, health: null, owner: "u1", createdAt: "t" });
    expect((await call("agent.list", {}, row).expect(200)).body.ok).toBe(true);
    // discover_agents: no id/botId/mode/state, no adapterId (vendor is the adapter id)
    const r = await call("agent.list", {}, { type: "vendor", vendor: "memory-test", accountBindingId: "acct1" }).expect(200);
    expect(r.body.ok).toBe(true);
  });
  it("agent.context.open takes externalAgentId, and agent.events are flat with snake_case ids", async () => {
    const open = await call("agent.context.open", { threadId: "t1", generation: 1, correlationId: "corr1", externalAgentId: "mem-agent" }).expect(200);
    expect(open.body.ok).toBe(true);
    const contextId = open.body.value.contextId;
    await call("agent.context.message", { contextId, correlationId: "c1", text: "hi" });
    const ev = await call("agent.events", { contextId, limit: 200 }).expect(200);
    const first = ev.body.value.events[0];
    expect(typeof first.type).toBe("string");            // not nested under `event`
    expect(first.event).toBeUndefined();
    expect(typeof first.remote_event_id).toBe("string");  // read by gateway_runner::bridge_event
    expect(first.remote_event_id).toBe(first.remoteEventId);
  });
});

// Wizard bindings and discovery carry the vendor library id, never an adapter id. Before
// adapterForVendor every one of them answered UNSUPPORTED "no provider registered for adapter xai".
describe("vendor library ids resolve to their adapter", () => {
  const route = (adapterIds: string[]) => {
    host = new AaiHost();
    for (const id of adapterIds) host.register({ provider: new MemoryProvider("mem-agent", 2, {}, id) });
    app = createServer({ ...deps, aai: host });
  };
  const wire = { type: "vendor", accountBindingId: "acct-1", adapterId: null, id: null, botId: null, mode: null, state: null };
  it.each([
    ["xai", "grok-bot"], ["grok", "grok-bot"], ["openai", "chatgpt-dots"], ["anthropic", "claude-desktop"], ["openclaw", "openclaw"],
  ])("%s → %s (discovery-shaped binding with SQL nulls)", async (vendor, adapter) => {
    route([adapter]);
    const r = await call("agent.list", {}, { ...wire, vendor }).expect(200);
    expect(r.body.ok).toBe(true);
  });
  it("anthropic with the user's API key goes to Claude Managed Agents", async () => {
    route(["claude-managed-agents"]);
    const r = await request(app).post("/aai/call").set("Authorization", `Bearer ${tok}`)
      .send({ op: "agent.list", binding: { ...wire, vendor: "anthropic" }, input: {}, credential: { apiKey: "sk-test" } }).expect(200);
    expect(r.body.ok).toBe(true);
  });
  it("an explicit adapterId always wins", async () => {
    route(["claude-managed-agents"]);
    const r = await call("agent.list", {}, { ...wire, vendor: "anthropic", adapterId: "claude-managed-agents" }).expect(200);
    expect(r.body.ok).toBe(true);
  });
});
