// Claude adapter: offline conformance (replay driver over fixture markup), observation, drift/bot/rate-limit
// handling, launch consent gate. No app, no network, no CDP.
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { describe, expect, it } from "vitest";
import { runConformance } from "@allternit/agent-gateway";
import { lookProfileSchema } from "@allternit/subscription-fabric-contracts";
import { CLAUDE_DESKTOP_MANIFEST, AGENT_ID, COWORK_AGENT_ID } from "../adapters/claude-desktop/manifest.js";
import { ClaudeDesktopProvider, ReplayClaudeDesktopDriver, launchWithDebugPort, claudeDesktop } from "../adapters/claude-desktop/index.js";
import type { ReplayMode } from "../adapters/claude-desktop/index.js";
import { nameRe } from "../adapters/claude-desktop/selectors.js";
import { classify, retryHint } from "../adapters/claude-desktop/observe.js";
import { parseHtml, queryAll } from "../adapters/claude-desktop/minidom.js";
import { SCENARIOS, renderPage } from "../adapters/claude-desktop/fixtures/markup.js";

const fixture = (n: string) => readFileSync(fileURLToPath(new URL(`../adapters/claude-desktop/fixtures/${n}.html`, import.meta.url)), "utf8");
const mk = (mode: ReplayMode = "normal", extra: ConstructorParameters<typeof ReplayClaudeDesktopDriver>[0] = {}) => {
  const driver = new ReplayClaudeDesktopDriver({ mode, ...extra });
  return { driver, p: new ClaudeDesktopProvider({ driver, pacing: false, pollMs: 1, replyTimeoutMs: 2000 }) };
};

describe("fixtures", () => {
  it("static html fixtures match markup.ts", () => {
    for (const [n, s] of Object.entries(SCENARIOS)) expect(fixture(n).trim()).toBe(renderPage(s).trim());
  });
  it("classifies each fixture", () => {
    const k = (n: string) => classify(fixture(n));
    expect(k("idle")).toMatchObject({ kind: "ok", composer: true, streaming: false, turns: [] });
    expect(k("cowork")).toMatchObject({ kind: "ok", cowork: true });
    expect(k("idle").cowork).toBe(false);
    expect(k("streaming")).toMatchObject({ kind: "ok", streaming: true });
    expect(k("streaming").turns.map((t) => t.role)).toEqual(["user", "assistant"]);
    const c = k("complete");
    expect(c).toMatchObject({ kind: "ok", streaming: false, toolCues: ["Read files"], artifactCues: ["inbox-summary.md"] });
    expect(c.turns[1].text).toContain("12 unread");
    expect(k("approval").approvals).toHaveLength(1);
    expect(k("rate-limit")).toMatchObject({ kind: "rate_limited", retryAfterMs: 7_200_000 });
    expect(k("bot-check").kind).toBe("blocked");
    expect(k("logged-out").kind).toBe("logged_out");
    expect(k("drift").kind).toBe("drift");
    expect(k("drift-turn")).toMatchObject({ kind: "drift", missing: ["turnContent"] });
    expect(classify("").kind).toBe("unreachable");
  });
  it("retry hints parse relative and clock times", () => {
    expect(retryHint("resets in 45 minutes", 0)).toBe(45 * 60_000);
    const noon = new Date(2026, 0, 1, 12, 0, 0).getTime();
    expect(retryHint("Your limit resets at 3:00 PM", noon)).toBe(3 * 3_600_000);
    expect(retryHint("no hint", 0)).toBeUndefined();
  });
  it("minidom selector engine", () => {
    const r = parseHtml('<div id="a" class="x y"><p class="z" data-k="v w">hi<br>there</p><button aria-label="Go">G</button></div>');
    expect(queryAll(r, "div.x > p.z[data-k~=w]")).toHaveLength(1);
    expect(queryAll(r, "#a button, p.nope")).toHaveLength(1);
    expect(queryAll(r, "p[data-k^=q]")).toHaveLength(0);
  });
});

describe("look profile", () => {
  it("is a valid LookProfile and its icon exists", () => {
    const raw = JSON.parse(readFileSync(fileURLToPath(new URL("../adapters/claude-desktop/look-profile.json", import.meta.url)), "utf8"));
    expect(lookProfileSchema.parse(raw).vendorId).toBe("claude-desktop");
    expect(readFileSync(fileURLToPath(new URL("../adapters/claude-desktop/" + raw.iconAssets.app, import.meta.url))).subarray(1, 4).toString()).toBe("PNG");
  });
});

describe("manifest", () => {
  it("declares ui_bridge / best_effort / desktop_session", () => {
    const a = CLAUDE_DESKTOP_MANIFEST.agent!;
    expect(a.capabilities).toMatchObject({ lane: "ui_bridge", guarantee: "best_effort", context: { maxParallel: 1, parallel: false } });
    expect(a.authDescriptors[0]).toMatchObject({ authType: "desktop_session", lane: "ui_bridge", guarantee: "best_effort" });
    expect(a.authDescriptors[0].termsWarning).toMatch(/not an official API/);
    expect(a.capabilities.vendor).toBe("claude");
    expect(CLAUDE_DESKTOP_MANIFEST.origins).toEqual(["https://claude.ai"]);
    expect(CLAUDE_DESKTOP_MANIFEST.interface).toBe("ui_bridge_desktop");
  });
});

describe("conformance (offline replay)", () => {
  it("passes every declared area; undeclared areas are UNSUPPORTED", async () => {
    const { driver, p } = mk("normal", { approval: "Finder" });
    const report = await runConformance(p, {
      agentId: AGENT_ID, approvalId: driver.approvalId, settleMs: 10,
      faulty: {
        vendor_down: () => mk("down").p, rate_limited: () => mk("rate_limited").p, auth_revoked: () => mk("logged_out").p,
        account_banned: () => mk("blocked").p, ui_changed: () => mk("drift").p,
      },
    });
    const by = Object.fromEntries(report.areas.map((a) => [a.area, a.status]));
    // eslint-disable-next-line no-console
    console.log(JSON.stringify(by), report.areas.flatMap((a) => a.checks.filter((c) => c.status === "fail").map((c) => `${a.area}: ${c.name}: ${c.reason}`)));
    expect(report.areas.filter((a) => a.status === "fail")).toEqual([]);
    expect(report.ok).toBe(true);
    for (const a of ["identity", "context", "parallelism", "events", "approvals", "failure", "idempotency", "cancellation"]) expect(by[a]).toBe("pass");
    for (const a of ["memory", "computer", "resources"]) expect(by[a]).toMatch(/pass|skipped-unsupported/);
    expect(by.isolation).toBe("skipped-unsupported"); // declared shared, isolation not promised
    expect(by.sync).toBe("skipped-unsupported");
    expect(driver.approvalResolution).toBe("approved");
  });
});

describe("behaviour", () => {
  it("streams deltas then completes; events never claim exact", async () => {
    const { p } = mk();
    const ctx = (await p.contextOpen({ agentId: AGENT_ID, threadId: "t1" }));
    if (!ctx.ok) throw new Error("open");
    const r = await p.contextMessage({ contextId: ctx.value.contextId, correlationId: "c1", text: "hello" });
    expect(r).toMatchObject({ ok: true, value: { reply: "Echo: hello", guarantee: "best_effort" } });
    const ev = await p.events({ contextId: ctx.value.contextId });
    if (!ev.ok) throw new Error("ev");
    const types = ev.value.events.map((e) => e.event.type);
    expect(types).toContain("agent.message.delta"); expect(types.filter((t) => t === "agent.message.completed")).toHaveLength(1);
    expect(ev.value.events.every((e) => e.event.guarantee !== "exact" && e.event.threadId === "t1")).toBe(true);
  });
  it("idempotent by correlation id (one UI send)", async () => {
    const { p, driver } = mk();
    const c = await p.contextOpen({ agentId: AGENT_ID }); if (!c.ok) throw new Error();
    const [a, b] = await Promise.all([1, 2].map(() => p.contextMessage({ contextId: c.value.contextId, correlationId: "same", text: "x" })));
    expect(a).toEqual(b); expect(driver.sends).toBe(1);
  });
  it("drift stops the provider (ADAPTER_DRIFT) until cleared", async () => {
    const { p, driver } = mk();
    const c = await p.contextOpen({ agentId: AGENT_ID }); if (!c.ok) throw new Error();
    driver.mode = "drift";
    const r = await p.contextMessage({ contextId: c.value.contextId, correlationId: "d1", text: "x" });
    expect(r).toMatchObject({ ok: false, error: { code: "ADAPTER_DRIFT", retryable: false } });
    driver.mode = "normal";
    expect(await p.events({ contextId: c.value.contextId })).toMatchObject({ ok: false, error: { code: "ADAPTER_DRIFT" } });
    p.clearHalt();
    expect((await p.events({ contextId: c.value.contextId })).ok).toBe(true);
  });
  it("bot-check latches LANE_BLOCKED; rate limit sets a cooldown", async () => {
    const b = mk("blocked");
    expect(await b.p.contextOpen({ agentId: AGENT_ID })).toMatchObject({ ok: false, error: { code: "LANE_BLOCKED" } });
    b.driver.mode = "normal";
    expect(await b.p.contextOpen({ agentId: AGENT_ID })).toMatchObject({ ok: false, error: { code: "LANE_BLOCKED" } });
    const r = mk("rate_limited");
    expect(await r.p.contextOpen({ agentId: AGENT_ID })).toMatchObject({ ok: false, error: { code: "RATE_LIMITED", retryable: true } });
    r.driver.mode = "normal";
    expect(await r.p.contextOpen({ agentId: AGENT_ID })).toMatchObject({ ok: false, error: { code: "RATE_LIMITED" } }); // cooldown honoured
  });
  it("only a human answers approvals; robots are refused and state stays pending", async () => {
    const { p, driver } = mk("normal", { approval: "Finder" });
    const l = await p.approvals({ op: "list" }); if (!l.ok) throw new Error();
    const id = l.value.approvals![0].remoteRef!;
    expect(await p.approvals({ op: "respond", approvalId: id, decision: "approve", actor: { type: "system", id: "bot" } })).toMatchObject({ ok: false, error: { code: "APPROVAL_REQUIRED" } });
    expect(driver.approvalResolution).toBeUndefined();
    expect(await p.approvals({ op: "respond", approvalId: id, decision: "deny", actor: { type: "human", id: "eoj" } })).toMatchObject({ ok: true, value: { resolved: { state: "denied" } } });
  });
  it("undeclared ops are UNSUPPORTED", async () => {
    const { p } = mk();
    for (const r of [await p.tasks({ agentId: AGENT_ID }), await p.memory({ op: "snapshot" }), await p.contextSteer({ contextId: "x", text: "y" }), await p.contextOpen({ agentId: AGENT_ID, adoptContextId: "z" })])
      expect(r).toMatchObject({ ok: false, error: { code: "UNSUPPORTED" } });
  });
  it("factory + registration shape", () => {
    expect(claudeDesktop.adapterId).toBe("claude-desktop"); expect(typeof claudeDesktop.create).toBe("function");
  });
});

describe("launchWithDebugPort", () => {
  const deps = (running: boolean) => { const calls: number[] = []; return { calls, d: { isRunning: async () => running, launch: async (p: number) => void calls.push(p) } }; };
  const auth = { authToken: "1.dGVzdA==.sig", userDataDir: "/tmp/x" };
  it("refuses without explicit consent", async () => {
    const x = deps(false);
    expect(await launchWithDebugPort({ port: 9333, ...auth }, x.d)).toMatchObject({ ok: false, error: { code: "POLICY_DENIED" } });
    expect(await launchWithDebugPort({ port: 9333, userConsented: false, ...auth }, x.d)).toMatchObject({ ok: false });
    expect(x.calls).toEqual([]);
  });
  it("without a vendor-signed token the lane is LANE_BLOCKED (Claude refuses the flag)", async () => {
    const x = deps(false);
    const r = await launchWithDebugPort({ port: 9333, userConsented: true }, x.d);
    expect(r).toMatchObject({ ok: false, error: { code: "LANE_BLOCKED", retryable: false } });
    if (!r.ok) expect(r.error.humanMessage).toMatch(/signed developer token/);
    expect(x.calls).toEqual([]);
  });
  it("never touches a running instance: LANE_BLOCKED asking the user to quit", async () => {
    const x = deps(true);
    const r = await launchWithDebugPort({ port: 9333, userConsented: true, ...auth }, x.d);
    expect(r).toMatchObject({ ok: false, error: { code: "LANE_BLOCKED" } });
    if (!r.ok) expect(r.error.humanMessage).toMatch(/quit Claude yourself/);
    expect(x.calls).toEqual([]);
  });
  it("launches when consented, token supplied and not running", async () => {
    const x = deps(false);
    expect(await launchWithDebugPort({ port: 9333, userConsented: true, ...auth }, x.d)).toMatchObject({ ok: true, value: { port: 9333 } });
    expect(x.calls).toEqual([9333]);
  });
});

describe("chat vs Cowork entry points", () => {
  it("claude-desktop opens a chat; claude-desktop:cowork switches to Cowork and starts a task", async () => {
    const chat = mk(); expect((await chat.p.contextOpen({ agentId: AGENT_ID })).ok).toBe(true);
    expect(chat.driver.cowork).toBe(false);
    const cw = mk();
    const r = await cw.p.contextOpen({ agentId: COWORK_AGENT_ID }); if (!r.ok) throw new Error("open");
    expect(cw.driver).toMatchObject({ cowork: true, newTasks: 1 });
    const ev = await cw.p.events({ contextId: r.value.contextId }); if (!ev.ok) throw new Error();
    expect(ev.value.events[0].event.payload).toMatchObject({ mode: "cowork" });
  });
  it("rejects other agent ids", async () => {
    expect(await mk().p.contextOpen({ agentId: "someone-else:x" })).toMatchObject({ ok: false, error: { code: "CONTEXT_NOT_FOUND" } });
    expect(await mk().p.contextOpen({ agentId: "claude-desktop:other" })).toMatchObject({ ok: false, error: { code: "CONTEXT_NOT_FOUND" } });
  });
  it("tool/artifact cues become inferred tool.called events", async () => {
    const { p } = mk("normal", { tool: "Read files", artifact: "notes.md" });
    const c = await p.contextOpen({ agentId: AGENT_ID }); if (!c.ok) throw new Error();
    await p.contextMessage({ contextId: c.value.contextId, correlationId: "k", text: "go" });
    const ev = await p.events({ contextId: c.value.contextId }); if (!ev.ok) throw new Error();
    const tools = ev.value.events.filter((e) => e.event.type === "agent.tool.called");
    expect(tools.map((e) => e.event.payload.kind)).toEqual(["tool_use", "artifact"]);
    expect(tools.every((e) => e.event.guarantee === "inferred")).toBe(true);
  });
  it("approve never selects Always allow", () => {
    expect(nameRe("approve").test("Always allow")).toBe(false);
    expect(nameRe("approve").test("Allow once")).toBe(true);
  });
});
