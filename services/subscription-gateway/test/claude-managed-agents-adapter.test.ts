// Claude Managed Agents adapter: offline conformance (fake SDK client over documented event shapes), credential
// handling (user-owned key, per-call, never leaked), error mapping via SDK classes, event log/replay, approvals.
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { describe, expect, it, vi } from "vitest";
import { APIConnectionError, NotFoundError } from "@anthropic-ai/sdk";
import { runConformance, type AaiResult } from "@allternit/agent-gateway";
import { adapterManifestSchema, lookProfileSchema } from "@allternit/subscription-fabric-contracts";
import { CLAUDE_MANAGED_AGENTS_MANIFEST, CAPABILITIES } from "../adapters/claude-managed-agents/manifest.js";
import { ClaudeManagedAgentsProvider } from "../adapters/claude-managed-agents/index.js";
import { createAaiRegistration } from "../adapters/claude-managed-agents/aai.js";
import { normalize } from "../adapters/claude-managed-agents/normalize.js";
import { FakeManagedAgents } from "../adapters/claude-managed-agents/fixtures/fake-client.js";
import { RECORDED_APPROVAL_PAUSE, RECORDED_APPROVAL_RESOLVED, RECORDED_TERMINATED, RECORDED_TURN } from "../adapters/claude-managed-agents/fixtures/recorded.js";
import { AGENT_ID, PENDING_APPROVAL, TEST_KEY, createOfflineRig, makeProvider } from "../adapters/claude-managed-agents/fixtures/rig.js";
import { parseCredential, runWithCallScope } from "../src/aai/call-scope.js";

const human = { type: "human" as const, id: "eoj" };
const val = <T>(r: AaiResult<T>): T => { if (!r.ok) throw new Error(`${r.error.code}: ${r.error.humanMessage}`); return r.value; };

describe("manifest + registration", () => {
  it("manifest is valid, official/exact, user-owned api_key", () => {
    expect(adapterManifestSchema.safeParse(CLAUDE_MANAGED_AGENTS_MANIFEST).success).toBe(true);
    expect(CAPABILITIES).toMatchObject({ lane: "official", guarantee: "exact", context: { isolation: "isolated", resume: true, parallel: true }, approvals: { exact: true }, memory: { opaque: true } });
    const auth = CLAUDE_MANAGED_AGENTS_MANIFEST.agent!.authDescriptors[0];
    expect(auth).toMatchObject({ authType: "api_key", requiresUserOwnedSubscription: true, lane: "official", guarantee: "exact" });
  });
  it("look profile parses; icon is a TODO key (no logo asset in repo)", () => {
    const lp = lookProfileSchema.parse(JSON.parse(readFileSync(fileURLToPath(new URL("../adapters/claude-managed-agents/look-profile.json", import.meta.url)), "utf8")));
    expect(lp.iconAssets?.app).toMatch(/^TODO:/);
    expect(lp.contentRenderers).toEqual(expect.arrayContaining(["tool_use_block", "file_artifact"]));
  });
  it("createAaiRegistration reads only non-secret config", () => {
    const reg = createAaiRegistration({ SUBS_GATEWAY_CLAUDE_MA_ENVIRONMENT_ID: "env_1", SUBS_GATEWAY_CLAUDE_MA_MAX_PARALLEL: "2", ANTHROPIC_API_KEY: "sk-ant-should-never-be-read" });
    expect(reg.provider.adapterId).toBe("claude-managed-agents");
  });
});

describe("conformance (offline)", () => {
  it("passes every declared area", async () => {
    const { provider, fixtures } = createOfflineRig();
    const report = await runConformance(provider, fixtures);
    const by = Object.fromEntries(report.areas.map((a) => [a.area, a.status]));
    // eslint-disable-next-line no-console
    console.log(JSON.stringify(by), report.areas.flatMap((a) => a.checks.filter((c) => c.status === "fail").map((c) => `${a.area}: ${c.name}: ${c.reason}`)));
    expect(report.areas.filter((a) => a.status === "fail")).toEqual([]);
    expect(report.ok).toBe(true);
    for (const a of ["identity", "context", "isolation", "parallelism", "events", "approvals", "failure", "idempotency", "cancellation", "resources"]) expect(by[a]).toBe("pass");
    for (const a of ["memory", "computer", "sync"]) expect(by[a]).toMatch(/pass|skipped-unsupported/);
  });
});

describe("credentials (user-owned key)", () => {
  it("no credential -> AUTH_REQUIRED telling the user to connect their Anthropic API key", async () => {
    const p = new ClaudeManagedAgentsProvider({ clientFactory: () => new FakeManagedAgents(), environmentId: "env_fake" });
    for (const r of [await p.list(), await p.get(AGENT_ID), await p.contextOpen({ agentId: AGENT_ID })]) {
      expect(r.ok).toBe(false);
      if (!r.ok) { expect(r.error.code).toBe("AUTH_REQUIRED"); expect(r.error.humanMessage).toMatch(/Connect your Anthropic API key/); }
    }
    const h = val(await p.health({}));
    expect(h.status).toBe("down");
  });

  it("default resolver reads the per-call credential; one client per binding; contexts never cross bindings", async () => {
    const fake = new FakeManagedAgents();
    const built: string[] = [];
    const p = new ClaudeManagedAgentsProvider({ clientFactory: (k) => { built.push(k); return fake; }, environmentId: "env_fake", pollMs: 1, sleep: async () => undefined });
    const as = <T>(id: string, key: string | undefined, fn: () => Promise<T>) => runWithCallScope({ binding: { id }, credential: key ? { apiKey: key } : undefined }, fn);
    const opened = val(await as("b1", "sk-ant-KEY-ONE-aaaaaa", () => p.contextOpen({ agentId: AGENT_ID })));
    await as("b1", "sk-ant-KEY-ONE-aaaaaa", () => p.list());
    await as("b2", "sk-ant-KEY-TWO-bbbbbb", () => p.list());
    expect(built).toEqual(["sk-ant-KEY-ONE-aaaaaa", "sk-ant-KEY-TWO-bbbbbb"]); // reused within a binding, never across
    const cross = await as("b2", "sk-ant-KEY-TWO-bbbbbb", () => p.events({ contextId: opened.contextId }));
    expect(!cross.ok && cross.error.code).toBe("CONTEXT_NOT_FOUND");
    const noKey = await as("b1", undefined, () => p.list());
    expect(!noKey.ok && noKey.error.code).toBe("AUTH_REQUIRED");
    expect(parseCredential({ apiKey: "  " })).toBeUndefined();
    expect(parseCredential({ apiKey: " k " })).toEqual({ apiKey: "k" });
  });

  it("401 after a good call -> AUTH_REVOKED and the client is dropped; a fresh call rebuilds it", async () => {
    const fake = new FakeManagedAgents();
    let builds = 0;
    const p = new ClaudeManagedAgentsProvider({ resolveCredential: () => ({ apiKey: "sk-ant-x-123456" }), clientFactory: () => { builds++; return fake; }, environmentId: "env_fake" });
    val(await p.list());
    fake.fault = "unauthorized";
    const r = await p.list();
    expect(!r.ok && r.error.code).toBe("AUTH_REVOKED");
    fake.fault = undefined;
    val(await p.list());
    expect(builds).toBe(2);
  });

  it("the key never appears in events, errors, results or logs", async () => {
    const spies = (["log", "warn", "error", "info", "debug"] as const).map((m) => vi.spyOn(console, m).mockImplementation(() => undefined));
    const secret = "sk-ant-api03-LEAKCHECK-9f8e7d6c5b4a";
    const fake = new FakeManagedAgents({ apiKey: secret });
    const p = makeProvider(fake);
    const out: unknown[] = [];
    const ctx = val(await p.contextOpen({ agentId: AGENT_ID })); out.push(ctx);
    out.push(await p.contextMessage({ contextId: ctx.contextId, correlationId: "c1", text: "run tool please" }));
    out.push(await p.approvals({ op: "list" }), await p.events({ contextId: ctx.contextId }), await p.list(), await p.get(AGENT_ID), await p.health({}), await p.artifacts({ agentId: AGENT_ID }));
    // an SDK error whose message echoes the key must be scrubbed
    fake.beta.agents.list = () => (async function* () { throw new NotFoundError(404, { type: "error" }, `bad key ${secret}`, new Headers()); })();
    out.push(await p.list());
    fake.beta.sessions.create = async () => { throw new APIConnectionError({ message: `connect failed for ${secret}` }); };
    out.push(await p.contextOpen({ agentId: AGENT_ID }));
    for (const f of ["unauthorized", "forbidden", "rate_limit", "unavailable"] as const) out.push(await makeProvider(new FakeManagedAgents({ apiKey: secret, fault: f })).list());
    expect(JSON.stringify(out)).not.toContain(secret);
    expect(JSON.stringify(out)).not.toMatch(/sk-ant-/);
    spies.forEach((s) => { expect(JSON.stringify(s.mock.calls)).not.toContain(secret); s.mockRestore(); });
  });
});

describe("error mapping (SDK typed errors)", () => {
  const code = async (fault: NonNullable<FakeManagedAgents["fault"]>) => {
    const r = await makeProvider(new FakeManagedAgents({ fault })).contextOpen({ agentId: AGENT_ID });
    return r.ok ? undefined : r.error;
  };
  it("401/403/429/5xx", async () => {
    expect((await code("unauthorized"))?.code).toBe("AUTH_REQUIRED");
    expect((await code("forbidden"))?.code).toBe("AUTH_REQUIRED");
    expect(await code("rate_limit")).toMatchObject({ code: "RATE_LIMITED", retryAfterMs: 7000, retryable: true });
    expect(await code("unavailable")).toMatchObject({ code: "VENDOR_UNAVAILABLE", retryable: true });
  });
  it("connection failure -> VENDOR_UNAVAILABLE; missing session -> CONTEXT_NOT_FOUND; missing agent -> UNKNOWN", async () => {
    const fake = new FakeManagedAgents();
    const p = makeProvider(fake);
    const ctx = val(await p.contextOpen({ agentId: AGENT_ID }));
    fake.beta.sessions.events.list = () => (async function* () { throw new APIConnectionError({ message: "down" }); })();
    const ev = await p.events({ contextId: ctx.contextId });
    expect(!ev.ok && ev.error.code).toBe("VENDOR_UNAVAILABLE");
    const q = makeProvider(new FakeManagedAgents());
    const ghost = await q.contextOpen({ agentId: AGENT_ID, adoptContextId: "sesn_missing" });
    expect(!ghost.ok && ghost.error.code).toBe("CONTEXT_NOT_FOUND");
    const noAgent = await q.get("agent_nope");
    expect(!noAgent.ok && noAgent.error.code).toBe("UNKNOWN");
  });
});

describe("events: exact, lossless, replayable", () => {
  it("normalizes a recorded turn; noise events are dropped", () => {
    expect(RECORDED_TURN.flatMap(normalize).map((d) => d.type)).toEqual([
      "agent.activity.started", "agent.task.updated", "agent.activity.started", "agent.message.delta", "agent.message.completed", "agent.task.updated"]);
    expect(RECORDED_TERMINATED.flatMap(normalize).map((d) => d.type)).toEqual(["agent.activity.started", "agent.health.changed", "agent.task.updated"]);
    const pause = RECORDED_APPROVAL_PAUSE.flatMap(normalize);
    expect(pause.find((d) => d.type === "agent.approval.requested")?.payload).toMatchObject({ approvalId: "sevt_a3", authority: "vendor" });
    expect(RECORDED_APPROVAL_RESOLVED.flatMap(normalize).some((d) => d.type === "agent.approval.resolved")).toBe(true);
  });

  it("re-reading history never duplicates; a restarted provider adopts the session and rebuilds the same log", async () => {
    const fake = new FakeManagedAgents();
    const p = makeProvider(fake);
    const ctx = val(await p.contextOpen({ agentId: AGENT_ID, threadId: "t-1" }));
    const r = val(await p.contextMessage({ contextId: ctx.contextId, correlationId: "k1", text: "hello" }));
    expect(r.reply).toBe("Echo: hello");
    const a = val(await p.events({ contextId: ctx.contextId }));
    const b = val(await p.events({ contextId: ctx.contextId }));
    expect(b.events).toEqual(a.events);
    expect(new Set(a.events.map((e) => e.event.remoteEventId)).size).toBe(a.events.length);
    expect(a.events.every((e) => e.event.guarantee === "exact" && e.event.lane === "official")).toBe(true);
    expect(a.events.filter((e) => e.event.correlationId === "k1").length).toBeGreaterThan(2);
    const tail = val(await p.events({ contextId: ctx.contextId, cursor: a.events[2].cursor }));
    expect(tail.events.map((e) => e.cursor)).toEqual(a.events.slice(3).map((e) => e.cursor));
    const q = makeProvider(fake); // "gateway restart"
    const resumed = val(await q.contextOpen({ agentId: AGENT_ID, adoptContextId: ctx.contextId }));
    expect(resumed).toMatchObject({ contextId: ctx.contextId, resumed: true });
    const rebuilt = val(await q.events({ contextId: ctx.contextId }));
    expect(rebuilt.events.filter((e) => e.event.type === "agent.message.completed").map((e) => e.event.payload?.text)).toEqual(["Echo: hello"]);
  });

  it("close archives the session (nothing deleted) and rejects further use; steer interrupts then messages; cancel is verified", async () => {
    const fake = new FakeManagedAgents();
    const p = makeProvider(fake);
    const ctx = val(await p.contextOpen({ agentId: AGENT_ID }));
    expect(val(await p.contextSteer({ contextId: ctx.contextId, text: "change course" })).accepted).toBe(true);
    expect(val(await p.contextCancel({ contextId: ctx.contextId })).confirmed).toBe(true);
    expect(val(await p.contextClose({ contextId: ctx.contextId })).closed).toBe(true);
    expect(fake.sessions.get(ctx.contextId)?.archived_at).toBeTruthy();
    expect(fake.calls).not.toContain("sessions.delete");
    const after = await p.contextMessage({ contextId: ctx.contextId, correlationId: "z", text: "x" });
    expect(!after.ok && after.error.code).toBe("CONTEXT_NOT_FOUND");
  });
});

describe("approvals: vendor authority, human only", () => {
  it("only a human can answer; tool_confirmation carries the tool event id; second answer conflicts", async () => {
    const { provider, fake } = createOfflineRig();
    const list = val(await provider.approvals({ op: "list" })).approvals!;
    expect(list).toHaveLength(1);
    expect(list[0]).toMatchObject({ authority: "vendor", remoteRef: PENDING_APPROVAL, state: "pending" });
    const sys = await provider.approvals({ op: "respond", approvalId: PENDING_APPROVAL, decision: "approve", actor: { type: "system", id: "bot" } });
    expect(!sys.ok && sys.error.code).toBe("APPROVAL_REQUIRED");
    expect(fake.sessions.get("sesn_pending")!.events.some((e) => e.type === "user.tool_confirmation")).toBe(false);
    const sent = vi.spyOn(fake.beta.sessions.events, "send");
    expect(val(await provider.approvals({ op: "respond", approvalId: PENDING_APPROVAL, decision: "deny", actor: human })).resolved?.state).toBe("denied");
    expect(sent).toHaveBeenCalledWith("sesn_pending", { events: [expect.objectContaining({ type: "user.tool_confirmation", tool_use_id: PENDING_APPROVAL, result: "deny" })] });
    const again = await provider.approvals({ op: "respond", approvalId: PENDING_APPROVAL, decision: "approve", actor: human });
    expect(!again.ok && again.error.code).toBe("SYNC_CONFLICT");
    const evs = val(await provider.events({ contextId: "sesn_pending" }));
    expect(evs.events.find((e) => e.event.type === "agent.approval.resolved")?.event.payload).toMatchObject({ approvalId: PENDING_APPROVAL, outcome: "denied", actorId: "eoj" });
  });

  it("a tool call raised mid-conversation surfaces as agent.approval.requested (authority vendor)", async () => {
    const p = makeProvider(new FakeManagedAgents());
    const ctx = val(await p.contextOpen({ agentId: AGENT_ID }));
    const r = val(await p.contextMessage({ contextId: ctx.contextId, correlationId: "t", text: "run tool" }));
    expect(r.reply).toBeUndefined(); // paused at requires_action, not a finished turn
    const evs = val(await p.events({ contextId: ctx.contextId })).events;
    expect(evs.find((e) => e.event.type === "agent.approval.requested")?.event.payload).toMatchObject({ authority: "vendor" });
    const [ap] = val(await p.approvals({ op: "list", contextId: ctx.contextId })).approvals!;
    expect(val(await p.approvals({ op: "respond", approvalId: ap.remoteRef!, decision: "approve", actor: human })).resolved?.state).toBe("approved");
  });
});

describe("misc", () => {
  it("maxParallel is configurable and enforced; environment required", async () => {
    const p = makeProvider(new FakeManagedAgents(), { maxParallel: 1 });
    val(await p.contextOpen({ agentId: AGENT_ID }));
    const busy = await p.contextOpen({ agentId: AGENT_ID });
    expect(!busy.ok && busy.error.code).toBe("CONTEXT_BUSY");
    const noEnv = await makeProvider(new FakeManagedAgents(), { environmentId: undefined }).contextOpen({ agentId: AGENT_ID });
    expect(noEnv.ok).toBe(false);
    void TEST_KEY;
  });
  it("lists only non-archived agents and exposes session files as artifacts", async () => {
    const fake = new FakeManagedAgents();
    const p = makeProvider(fake);
    expect(val(await p.list()).map((a) => a.agentId)).toEqual(["agent_01"]);
    const ctx = val(await p.contextOpen({ agentId: AGENT_ID }));
    fake.files.set(ctx.contextId, [{ id: "file_1", filename: "report.md" }]);
    expect(val(await p.artifacts({ agentId: AGENT_ID, contextId: ctx.contextId }))).toEqual([{ artifactId: "file_1", name: "report.md" }]);
  });
});
