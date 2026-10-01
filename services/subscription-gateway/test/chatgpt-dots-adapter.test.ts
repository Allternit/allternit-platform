// ChatGPT dots adapter: offline conformance (replay driver over fixture markup), observation, drift/challenge/limit
// handling, Ask first vs Hand off, tasks, consent gate, reuse of chatgpt-web. No browser, no network, no ChatGPT.
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { describe, expect, it } from "vitest";
import { runConformance } from "@allternit/agent-gateway";
import { lookProfileSchema } from "@allternit/subscription-fabric-contracts";
import { loadManifest } from "../adapters/chatgpt-web/adapter.js";
import { CHATGPT_DOTS_MANIFEST, AGENT_ID, PACING } from "../adapters/chatgpt-dots/manifest.js";
import { ChatGPTDotsProvider, ReplayDotsDriver, BrowserDotsDriver, chatgptDots } from "../adapters/chatgpt-dots/index.js";
import type { ReplayMode, ReplayOptions } from "../adapters/chatgpt-dots/index.js";
import { classify } from "../adapters/chatgpt-dots/observe.js";
import { SELECTORS, SELECTORS_VERSION } from "../adapters/chatgpt-dots/selectors.js";
import { SCENARIOS, renderPage } from "../adapters/chatgpt-dots/fixtures/markup.js";
import { createAaiRegistration } from "../adapters/chatgpt-dots/aai.js";

const fixture = (n: string) => readFileSync(fileURLToPath(new URL(`../adapters/chatgpt-dots/fixtures/${n}.html`, import.meta.url)), "utf8");
const DOT = `${AGENT_ID}:nova-dot`;
const CARD = { policy: "ask_first" as const, text: "Send email to sam@example.com" };
const mk = (mode: ReplayMode = "normal", extra: ReplayOptions = {}) => {
  const driver = new ReplayDotsDriver({ mode, ...extra });
  return { driver, p: new ChatGPTDotsProvider({ driver, pacing: false, pollMs: 1, replyTimeoutMs: 2000 }) };
};
const open = async (p: ChatGPTDotsProvider, agentId = DOT) => { const c = await p.contextOpen({ agentId, threadId: "t1" }); if (!c.ok) throw new Error(JSON.stringify(c.error)); return c.value.contextId; };

describe("fixtures", () => {
  it("static html fixtures match markup.ts", () => {
    for (const [n, s] of Object.entries(SCENARIOS)) expect(fixture(n).trim()).toBe(renderPage(s).trim());
  });
  it("classifies each fixture", () => {
    const k = (n: string) => classify(fixture(n));
    expect(k("dot-list")).toMatchObject({ kind: "ok", view: "list", dots: [{ id: "nova-dot", name: "Nova", handle: "@nova-dot" }] });
    expect(k("idle")).toMatchObject({ kind: "ok", view: "dot", composer: true, streaming: false, turns: [], header: { name: "Nova", handle: "@nova-dot" } });
    expect(k("streaming")).toMatchObject({ kind: "ok", streaming: true, activity: "Working" });
    expect(k("streaming").turns.map((t) => t.role)).toEqual(["user", "assistant"]);
    expect(k("complete").turns[1].text).toContain("12 unread");
    expect(k("ask-first").confirmations).toMatchObject([{ policy: "ask_first", text: "Send email to sam@example.com" }]);
    expect(k("hand-off").confirmations).toMatchObject([{ policy: "hand_off", text: "Change the password for the hosting account" }]);
    expect(k("tasks").tasks.map((t) => [t.title, t.state])).toEqual([["Compile vendor notes", "in_progress"], ["Weekly inbox digest", "scheduled"], ["Book travel research", "completed"]]);
    expect(k("rate-limit")).toMatchObject({ kind: "rate_limited", retryAfterMs: 3_600_000 });
    expect(k("paused").kind).toBe("paused");
    expect(k("challenge").kind).toBe("blocked");
    expect(k("logged-out").kind).toBe("logged_out");
    expect(k("plan-required")).toMatchObject({ kind: "plan_required", detail: "Dots require a Pro plan" });
    expect(k("drift")).toMatchObject({ kind: "drift", missing: ["composer", "dotRow"] });
    expect(classify("").kind).toBe("unreachable");
  });
});

describe("reuse of chatgpt-web", () => {
  it("inherits origins, login, probe, pacing from the chatgpt-web manifest", () => {
    const web = loadManifest();
    expect(CHATGPT_DOTS_MANIFEST.origins).toEqual(web.origins);
    expect(CHATGPT_DOTS_MANIFEST.auth.login_url).toBe(web.auth.login_url);
    expect(CHATGPT_DOTS_MANIFEST.auth.logged_in_probe).toBe(web.auth.logged_in_probe);
    expect(PACING).toEqual(web.pacing);
  });
  it("shared chat selectors come from chatgpt-web's pack; dots-specific ones are marked inferred", () => {
    expect(SELECTORS.composer.css).toContain("div#prompt-textarea[contenteditable=true]");
    expect(SELECTORS.assistantTurn.css).toContain("[data-markdown-text-style='assistant-message']");
    for (const k of ["composer", "assistantTurn", "userTurn", "loggedInProbe"]) expect(SELECTORS[k].confidence).toBe("live");
    for (const k of ["dotRow", "confirmation", "tasksPanel", "activity", "dotHeader"]) expect(SELECTORS[k].confidence).toBe("inferred");
    expect(SELECTORS_VERSION).toBe("dots-v1");
  });
});

describe("look profile", () => {
  it("is a valid LookProfile with the dots vocabulary and a real PNG icon", () => {
    const raw = JSON.parse(readFileSync(fileURLToPath(new URL("../adapters/chatgpt-dots/look-profile.json", import.meta.url)), "utf8"));
    expect(lookProfileSchema.parse(raw).vendorId).toBe("chatgpt-dots");
    expect(raw.statusVocabulary).toMatchObject({ ask_first: "Ask first", hand_off: "Hand off" });
    expect(readFileSync(fileURLToPath(new URL("../adapters/chatgpt-dots/" + raw.iconAssets.app, import.meta.url))).subarray(1, 4).toString()).toBe("PNG");
  });
});

describe("manifest", () => {
  it("declares ui_bridge / best_effort / browser_session, honest parallelism, terms warning", () => {
    const a = CHATGPT_DOTS_MANIFEST.agent!;
    expect(a.capabilities).toMatchObject({ lane: "ui_bridge", guarantee: "best_effort", vendor: "openai", context: { maxParallel: 1, parallel: false, isolation: "shared" }, tasks: { list: true, schedule: false, cancel: false } });
    expect(a.authDescriptors[0]).toMatchObject({ authType: "browser_session", lane: "ui_bridge", loginUrl: "https://chatgpt.com/auth/login", requiresUserOwnedSubscription: true });
    expect(a.authDescriptors[0].termsWarning).toMatch(/not an official API/);
    expect(a.authDescriptors[0].permissionDescription.join(" ")).toMatch(/never asks for or stores your password or cookies/);
    expect(CHATGPT_DOTS_MANIFEST.interface).toBe("ui_bridge_web");
  });
});

describe("conformance (offline replay)", () => {
  it("passes every declared area; undeclared areas are UNSUPPORTED", async () => {
    const { driver, p } = mk("normal", { confirmation: CARD, tasks: [{ title: "Compile vendor notes", state: "in_progress" }] });
    const report = await runConformance(p, {
      agentId: DOT, approvalId: driver.confirmationId, settleMs: 10,
      faulty: {
        vendor_down: () => mk("down").p, rate_limited: () => mk("rate_limited").p, auth_revoked: () => mk("logged_out").p,
        account_banned: () => mk("blocked").p, ui_changed: () => mk("drift").p,
      },
    });
    const by = Object.fromEntries(report.areas.map((a) => [a.area, a.status]));
    expect(report.areas.flatMap((a) => a.checks.filter((c) => c.status === "fail").map((c) => `${a.area}: ${c.name}: ${c.reason}`))).toEqual([]);
    expect(report.ok).toBe(true);
    for (const a of ["identity", "context", "parallelism", "events", "approvals", "failure", "idempotency", "cancellation"]) expect(by[a]).toBe("pass");
    for (const a of ["memory", "computer", "resources"]) expect(by[a]).toMatch(/pass|skipped-unsupported/);
    expect(by.isolation).toBe("skipped-unsupported");
    expect(by.sync).toBe("skipped-unsupported");
    expect(driver.confirmationResolution).toBe("approved");
  });
});

describe("agent list / identity / per-binding dot", () => {
  it("lists the user's dots from the dots list view and reads identity", async () => {
    const { p } = mk("normal", { dots: [{ id: "nova-dot", name: "Nova", handle: "@nova-dot" }, { id: "atlas-dot", name: "Atlas" }] });
    const l = await p.list(); if (!l.ok) throw new Error();
    expect(l.value.map((a) => [a.agentId, a.displayName])).toEqual([[`${AGENT_ID}:nova-dot`, "Nova"], [`${AGENT_ID}:atlas-dot`, "Atlas"]]);
    expect(await p.identity(`${AGENT_ID}:nova-dot`)).toMatchObject({ ok: true, value: { displayName: "Nova", vendor: "openai", lookPack: "chatgpt-dots" } });
  });
  it("opens the named dot (id or name); several dots need an explicit choice; rejects other agents", async () => {
    const two = { dots: [{ id: "nova-dot", name: "Nova" }, { id: "atlas-dot", name: "Atlas" }] };
    expect((await mk("normal", two).p.contextOpen({ agentId: `${AGENT_ID}:Atlas` })).ok).toBe(true);
    expect(await mk("normal", two).p.contextOpen({ agentId: AGENT_ID })).toMatchObject({ ok: false, error: { code: "POLICY_DENIED" } });
    expect(await mk("normal", two).p.contextOpen({ agentId: `${AGENT_ID}:Nope` })).toMatchObject({ ok: false, error: { code: "CONTEXT_NOT_FOUND" } });
    expect((await mk().p.contextOpen({ agentId: AGENT_ID })).ok).toBe(true); // exactly one dot: picked automatically
    expect(await mk().p.contextOpen({ agentId: "someone-else:x" })).toMatchObject({ ok: false, error: { code: "CONTEXT_NOT_FOUND" } });
  });
  it("a dot's conversation persists: reopening keeps history out of new events", async () => {
    const { p } = mk();
    let id = await open(p);
    await p.contextMessage({ contextId: id, correlationId: "a", text: "one" });
    await p.contextClose({ contextId: id });
    id = await open(p);
    const ev = await p.events({ contextId: id }); if (!ev.ok) throw new Error();
    expect(ev.value.events.map((e) => e.event.type)).toEqual(["agent.context.opened"]);
  });
});

describe("behaviour", () => {
  it("streams deltas then completes; events never claim exact and carry the dot as botId", async () => {
    const { p } = mk();
    const id = await open(p);
    const r = await p.contextMessage({ contextId: id, correlationId: "c1", text: "hello" });
    expect(r).toMatchObject({ ok: true, value: { reply: "Echo: hello", guarantee: "best_effort" } });
    const ev = await p.events({ contextId: id }); if (!ev.ok) throw new Error();
    const types = ev.value.events.map((e) => e.event.type);
    expect(types).toContain("agent.message.delta"); expect(types.filter((t) => t === "agent.message.completed")).toHaveLength(1);
    expect(ev.value.events.every((e) => e.event.guarantee !== "exact" && e.event.threadId === "t1" && e.event.botId === DOT && e.event.vendor === "openai")).toBe(true);
  });
  it("idempotent by correlation id (one UI send)", async () => {
    const { p, driver } = mk();
    const id = await open(p);
    const [a, b] = await Promise.all([1, 2].map(() => p.contextMessage({ contextId: id, correlationId: "same", text: "x" })));
    expect(a).toEqual(b); expect(driver.sends).toBe(1);
  });
  it("a new conversation replaces an idle open one, and waits while a reply is in flight", async () => {
    const { p } = mk();
    const first = await open(p);
    const sending = p.contextMessage({ contextId: first, correlationId: "busy-1", text: "x" });
    expect(await p.contextOpen({ agentId: DOT, threadId: "t2" })).toMatchObject({ ok: false, error: { code: "CONTEXT_BUSY" } });
    expect(await sending).toMatchObject({ ok: true });
    const second = await open(p);
    expect(second).not.toBe(first);
    // The replaced conversation is no longer driven: its thread reopens on its next turn.
    expect(await p.contextMessage({ contextId: first, correlationId: "late", text: "x" })).toMatchObject({ ok: false, error: { code: "CONTEXT_NOT_FOUND" } });
    expect(await p.contextMessage({ contextId: second, correlationId: "now", text: "x" })).toMatchObject({ ok: true });
  });
  it("drift stops the provider (ADAPTER_DRIFT) until cleared", async () => {
    const { p, driver } = mk();
    const id = await open(p);
    driver.mode = "drift";
    expect(await p.contextMessage({ contextId: id, correlationId: "d1", text: "x" })).toMatchObject({ ok: false, error: { code: "ADAPTER_DRIFT", retryable: false } });
    driver.mode = "normal";
    expect(await p.events({ contextId: id })).toMatchObject({ ok: false, error: { code: "ADAPTER_DRIFT" } });
    p.clearHalt();
    expect((await p.events({ contextId: id })).ok).toBe(true);
  });
  it("challenge latches LANE_BLOCKED and never retries; usage limit sets a cooldown; logged out is AUTH_REQUIRED", async () => {
    const b = mk("blocked");
    expect(await b.p.contextOpen({ agentId: DOT })).toMatchObject({ ok: false, error: { code: "LANE_BLOCKED" } });
    b.driver.mode = "normal";
    expect(await b.p.contextOpen({ agentId: DOT })).toMatchObject({ ok: false, error: { code: "LANE_BLOCKED" } });
    const r = mk("rate_limited");
    expect(await r.p.contextOpen({ agentId: DOT })).toMatchObject({ ok: false, error: { code: "RATE_LIMITED", retryable: true } });
    r.driver.mode = "normal";
    expect(await r.p.contextOpen({ agentId: DOT })).toMatchObject({ ok: false, error: { code: "RATE_LIMITED" } });
    expect(await mk("logged_out").p.contextOpen({ agentId: DOT })).toMatchObject({ ok: false, error: { code: "AUTH_REQUIRED" } });
    expect(await mk("paused").p.contextOpen({ agentId: DOT })).toMatchObject({ ok: false, error: { code: "LANE_BLOCKED" } });
  });
  it("Allternit's own pacing caps apply (SDK Pacer): hourly cap => RATE_LIMITED", async () => {
    const { p } = mk();
    const id = await open(p);
    let last: unknown;
    for (let i = 0; i <= PACING.max_tasks_per_hour; i++) last = await p.contextMessage({ contextId: id, correlationId: `m${i}`, text: "x" });
    expect(last).toMatchObject({ ok: false, error: { code: "RATE_LIMITED" } });
  });
  it("Ask first => vendor approval.requested; only a human answers; robots are refused", async () => {
    const { p, driver } = mk("normal", { confirmation: CARD });
    const id = await open(p);
    const ev = await p.events({ contextId: id }); if (!ev.ok) throw new Error();
    const req = ev.value.events.find((e) => e.event.type === "agent.approval.requested")!;
    expect(req.event).toMatchObject({ source: "vendor", payload: { policy: "ask_first", label: "Ask first", needsYou: false } });
    const ref = (req.event.payload as { approvalId: string }).approvalId;
    expect(await p.approvals({ op: "respond", approvalId: ref, decision: "approve", actor: { type: "system", id: "bot" } })).toMatchObject({ ok: false, error: { code: "APPROVAL_REQUIRED" } });
    expect(driver.confirmationResolution).toBeUndefined();
    expect(await p.approvals({ op: "respond", approvalId: ref, decision: "deny", actor: { type: "human", id: "eoj" } })).toMatchObject({ ok: true, value: { resolved: { state: "denied" } } });
    expect(driver.confirmationResolution).toBe("denied");
  });
  it("Hand off => needs-you event; even a human cannot answer it from Allternit", async () => {
    const { p, driver } = mk("normal", { confirmation: { policy: "hand_off", text: "Change the password for the hosting account" } });
    const id = await open(p);
    const ev = await p.events({ contextId: id }); if (!ev.ok) throw new Error();
    const req = ev.value.events.find((e) => e.event.type === "agent.approval.requested")!;
    expect(req.event.payload).toMatchObject({ policy: "hand_off", label: "Hand off", needsYou: true });
    const ref = (req.event.payload as { approvalId: string }).approvalId;
    expect(await p.approvals({ op: "respond", approvalId: ref, decision: "approve", actor: { type: "human", id: "eoj" } })).toMatchObject({ ok: false, error: { code: "POLICY_DENIED" } });
    expect(driver.confirmationResolution).toBeUndefined();
  });
  it("card answered inside ChatGPT => approval.resolved outcome unknown", async () => {
    const { p, driver } = mk("normal", { confirmation: CARD });
    const id = await open(p);
    await p.events({ contextId: id });
    driver.confirmation = undefined;
    const ev = await p.events({ contextId: id }); if (!ev.ok) throw new Error();
    expect(ev.value.events.find((e) => e.event.type === "agent.approval.resolved")?.event.payload).toMatchObject({ outcome: "unknown", where: "in_app" });
  });
  it("tasks are read from the profile panel and emit agent.task.updated", async () => {
    const tasks = [{ title: "Compile vendor notes", state: "in_progress" as const }, { title: "Book travel research", state: "completed" as const }];
    const { p } = mk("normal", { tasks });
    const id = await open(p);
    const t = await p.tasks({ agentId: DOT }); if (!t.ok) throw new Error();
    expect(t.value.map((x) => [x.title, x.state])).toEqual([["Compile vendor notes", "in_progress"], ["Book travel research", "completed"]]);
    const ev = await p.events({ contextId: id }); if (!ev.ok) throw new Error();
    expect(ev.value.events.filter((e) => e.event.type === "agent.task.updated")).toHaveLength(2);
    const again = await p.events({ contextId: id, cursor: ev.value.nextCursor }); if (!again.ok) throw new Error();
    expect(again.value.events).toHaveLength(0);
    // no open conversation: opens the dot just to read tasks
    expect((await mk("normal", { tasks }).p.tasks({ agentId: DOT })).ok).toBe(true);
  });
  it("undeclared ops are UNSUPPORTED", async () => {
    const { p } = mk();
    for (const r of [await p.memory({ op: "snapshot" }), await p.contextSteer({ contextId: "x", text: "y" }), await p.contextOpen({ agentId: DOT, adoptContextId: "z" })])
      expect(r).toMatchObject({ ok: false, error: { code: "UNSUPPORTED" } });
  });
  it("factory + registration shape", () => {
    expect(chatgptDots.adapterId).toBe("chatgpt-dots"); expect(typeof chatgptDots.create).toBe("function");
    const reg = createAaiRegistration({});
    expect(reg.provider.adapterId).toBe("chatgpt-dots");
    expect(reg.pacing.minGapMs).toBe(PACING.min_task_gap_s * 1000);
  });
});

describe("live driver consent gate (no browser is ever launched here)", () => {
  it("refuses to open a browser without explicit consent, and without a configured profile", async () => {
    await expect(new BrowserDotsDriver({ profileDir: "/nonexistent" }).connect()).rejects.toMatchObject({ fault: "consent_required" });
    await expect(new BrowserDotsDriver({ userConsented: true }).connect()).rejects.toMatchObject({ fault: "not_running" });
    const p = createAaiRegistration({}).provider;
    expect(await p.contextOpen({ agentId: DOT })).toMatchObject({ ok: false, error: { code: "LANE_BLOCKED" } });
  });
});

describe("dots use the ChatGPT subscription account (one login system)", () => {
  it("asks the gateway for the subscription profile at connect time; none ready → sign-in hint", async () => {
    const { BrowserDotsDriver } = await import("../adapters/chatgpt-dots/browser-driver.js");
    const asked: string[] = [];
    const driver = new BrowserDotsDriver({
      userConsented: true,
      resolveProfileDir: async () => {
        asked.push("chatgpt");
        return undefined;
      },
    });
    await expect(driver.connect()).rejects.toThrow(/Settings → Subscriptions/);
    expect(asked).toEqual(["chatgpt"]);
  });

  it("the registration resolves the preferred ChatGPT subscription through the gateway context", async () => {
    const { createAaiRegistration } = await import("../adapters/chatgpt-dots/aai.js");
    const providers: string[] = [];
    const reg = createAaiRegistration({ SUBS_GATEWAY_DOTS_CONSENT: "1" } as NodeJS.ProcessEnv, {
      subscriptionProfile: async (p) => {
        providers.push(p);
        return null;
      },
    });
    expect(reg.provider).toBeTruthy();
    expect(providers).toEqual([]); // lazy: nothing resolved until a dot is opened
  });
});

describe("plan without dots releases the ChatGPT login", () => {
  it("closes the browser on the plan gate and answers from memory for an hour without reopening it", async () => {
    const { ChatGPTDotsProvider } = await import("../adapters/chatgpt-dots/provider.js");
    const { renderPage } = await import("../adapters/chatgpt-dots/fixtures/markup.js");
    let connects = 0, disposes = 0, now = 1_000_000;
    const driver = {
      connect: async () => { connects += 1; }, html: async () => renderPage({ planRequired: true }), dispose: async () => { disposes += 1; },
      isAppRunning: async () => true, showDotList: async () => true, openDot: async () => true, showTasks: async () => true, typeText: async () => true, clickButton: async () => true,
    } as never;
    const p = new ChatGPTDotsProvider({ driver, now: () => now } as never);
    const a = await p.contextOpen({ agentId: "chatgpt-dots" });
    expect(a).toMatchObject({ ok: false, error: { code: "LANE_BLOCKED" } });
    expect(disposes).toBe(1);
    const b = await p.contextOpen({ agentId: "chatgpt-dots" });
    expect(b).toMatchObject({ ok: false, error: { code: "LANE_BLOCKED" } });
    expect(connects).toBe(1); // no relaunch inside the hour
    now += 61 * 60_000;
    await p.contextOpen({ agentId: "chatgpt-dots" });
    expect(connects).toBe(2);
  });
});
