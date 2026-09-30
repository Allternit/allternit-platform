// Grok Bot adapter: offline conformance (replay driver over fixture markup), observation, drift/bot/rate-limit
// handling, launch consent gate. No app, no network, no CDP.
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { describe, expect, it } from "vitest";
import { runConformance } from "@allternit/agent-gateway";
import { lookProfileSchema } from "@allternit/subscription-fabric-contracts";
import { GROK_BOT_MANIFEST, AGENT_ID } from "../adapters/grok-bot/manifest.js";
import { GrokBotProvider, ReplayGrokDriver, launchWithDebugPort, grokBot } from "../adapters/grok-bot/index.js";
import type { ReplayMode } from "../adapters/grok-bot/index.js";
import { classify } from "../adapters/grok-bot/observe.js";
import { parseHtml, queryAll } from "../adapters/grok-bot/minidom.js";
import { SCENARIOS, renderPage } from "../adapters/grok-bot/fixtures/markup.js";

const fixture = (n: string) => readFileSync(fileURLToPath(new URL(`../adapters/grok-bot/fixtures/${n}.html`, import.meta.url)), "utf8");
const mk = (mode: ReplayMode = "normal", extra: ConstructorParameters<typeof ReplayGrokDriver>[0] = {}) => {
  const driver = new ReplayGrokDriver({ mode, ...extra });
  return { driver, p: new GrokBotProvider({ driver, pacing: false, pollMs: 1, replyTimeoutMs: 2000 }) };
};

describe("fixtures", () => {
  it("static html fixtures match markup.ts", () => {
    for (const [n, s] of Object.entries(SCENARIOS)) expect(fixture(n).trim()).toBe(renderPage(s).trim());
  });
  it("classifies each fixture", () => {
    const k = (n: string) => classify(fixture(n));
    expect(k("idle")).toMatchObject({ kind: "ok", composer: true, streaming: false, turns: [] });
    expect(k("picker")).toMatchObject({ kind: "ok", picker: true });
    expect(k("idle").picker).toBe(false);
    expect(k("streaming")).toMatchObject({ kind: "ok", streaming: true });
    expect(k("streaming").turns.map((t) => t.role)).toEqual(["user", "assistant"]);
    const c = k("complete");
    expect(c).toMatchObject({ kind: "ok", streaming: false, routineCues: ["Routine Daily inbox digest"] });
    expect(c.turns[1].text).toContain("12 unread");
    expect(k("approval").approvals).toHaveLength(1);
    expect(k("rate-limit")).toMatchObject({ kind: "rate_limited", retryAfterMs: 3_600_000 });
    expect(k("bot-check").kind).toBe("blocked");
    expect(k("logged-out").kind).toBe("logged_out");
    expect(k("drift").kind).toBe("drift");
    expect(k("drift-turn")).toMatchObject({ kind: "drift", missing: ["turnContent"] });
    expect(classify("").kind).toBe("unreachable");
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
    const raw = JSON.parse(readFileSync(fileURLToPath(new URL("../adapters/grok-bot/look-profile.json", import.meta.url)), "utf8"));
    expect(lookProfileSchema.parse(raw).vendorId).toBe("grok-bot");
    expect(readFileSync(fileURLToPath(new URL("../adapters/grok-bot/" + raw.iconAssets.app, import.meta.url))).subarray(1, 4).toString()).toBe("PNG");
  });
});

describe("manifest", () => {
  it("declares ui_bridge / best_effort / desktop_session", () => {
    const a = GROK_BOT_MANIFEST.agent!;
    expect(a.capabilities).toMatchObject({ lane: "ui_bridge", guarantee: "best_effort", context: { maxParallel: 1, parallel: false } });
    expect(a.authDescriptors[0]).toMatchObject({ authType: "desktop_session", lane: "ui_bridge", guarantee: "best_effort" });
    expect(a.authDescriptors[0].termsWarning).toMatch(/not an official API/);
    expect(GROK_BOT_MANIFEST.interface).toBe("ui_bridge_desktop");
  });
});

describe("conformance (offline replay)", () => {
  it("passes every declared area; undeclared areas are UNSUPPORTED", async () => {
    const { driver, p } = mk("normal", { approval: "Send email to sam@example.com" });
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
    const { p, driver } = mk("normal", { approval: "Send email to sam@example.com" });
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
    expect(grokBot.adapterId).toBe("grok-bot"); expect(typeof grokBot.create).toBe("function");
  });
});

describe("launchWithDebugPort", () => {
  const deps = (running: boolean) => { const calls: number[] = []; return { calls, d: { isRunning: async () => running, launch: async (p: number) => void calls.push(p) } }; };
  it("refuses without explicit consent", async () => {
    const x = deps(false);
    expect(await launchWithDebugPort({ port: 9333 }, x.d)).toMatchObject({ ok: false, error: { code: "POLICY_DENIED" } });
    expect(await launchWithDebugPort({ port: 9333, userConsented: false }, x.d)).toMatchObject({ ok: false });
    expect(x.calls).toEqual([]);
  });
  it("never touches a running instance: LANE_BLOCKED asking the user to quit", async () => {
    const x = deps(true);
    const r = await launchWithDebugPort({ port: 9333, userConsented: true }, x.d);
    expect(r).toMatchObject({ ok: false, error: { code: "LANE_BLOCKED" } });
    if (!r.ok) expect(r.error.humanMessage).toMatch(/quit Grok Bot yourself/);
    expect(x.calls).toEqual([]);
  });
  it("launches when consented and not running", async () => {
    const x = deps(false);
    expect(await launchWithDebugPort({ port: 9333, userConsented: true }, x.d)).toMatchObject({ ok: true, value: { port: 9333 } });
    expect(x.calls).toEqual([9333]);
  });
});

describe("per-binding Bot selection", () => {
  it("accepts grok-bot:<Bot name> agent ids and rejects other agents", async () => {
    const { p } = mk();
    const good = await p.contextOpen({ agentId: `${AGENT_ID}:Allternit Dev Bot` });
    expect(good.ok).toBe(true);
    const { p: p2 } = mk();
    const bad = await p2.contextOpen({ agentId: "someone-else:Bot" });
    expect(bad).toMatchObject({ ok: false, error: { code: "CONTEXT_NOT_FOUND" } });
  });
});

describe("Bot discovery (New-chat picker)", () => {
  const bots = ["Research Bot", "Ops Bot"];
  it("pickerBots excludes the picker's own controls", async () => {
    const { pickerBots } = await import("../adapters/grok-bot/observe.js");
    expect(pickerBots(renderPage({ picker: true, bots }))).toEqual(bots);
    expect(pickerBots(renderPage({ picker: true }))).toEqual(["Example Bot"]);
    expect(pickerBots(renderPage({}))).toEqual([]);
  });
  it("list() returns the generic agent plus grok-bot:<name> per Bot, then closes the picker", async () => {
    const { driver, p } = mk("normal", { bots });
    const r = await p.list();
    expect(r.ok && r.value.map((a) => a.agentId)).toEqual(["grok-bot", "grok-bot:Research Bot", "grok-bot:Ops Bot"]);
    expect(driver.pickerOpen).toBe(false);
    expect(driver.sends).toBe(0);
    // discovered agent ids open directly
    const c = await p.contextOpen({ agentId: "grok-bot:Ops Bot" });
    expect(c.ok).toBe(true);
    expect(driver.chosenBot).toBe("Ops Bot");
  });
  it("list() degrades to the generic agent when not attached or a chat is open", async () => {
    const down = mk("down", { bots });
    const r = await down.p.list();
    expect(r.ok && r.value.map((a) => a.agentId)).toEqual(["grok-bot"]);
    const { p } = mk("normal", { bots });
    const c = await p.contextOpen({ agentId: "grok-bot:Ops Bot" });
    expect(c.ok).toBe(true);
    const r2 = await p.list();
    expect(r2.ok && r2.value).toHaveLength(1);
  });
});
