// macOS Accessibility (AX) transport: selector format, bridge client, claude-desktop + chatgpt-app over AX fixtures.
// Offline only: hand-built AX trees, a fake bridge child process. Never touches a running app or the AX permission.
import { EventEmitter } from "node:events";
import { PassThrough } from "node:stream";
import { existsSync, mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { describe, expect, it } from "vitest";
import { runConformance } from "@allternit/agent-gateway";
import {
  AX_SELECTOR_FORMAT, AxBridgeDriver, AxError, AxHintBuffer, AxReplayDriver, assertPackCompatible, build, missingCritical, n, normalizeAxEvent, resolveKey, resolveSelector,
  type AxSelectorPack, type AxSnapshot,
} from "../adapters/_shared/ax/index.js";
import { AxClaudeDesktopDriver, ClaudeDesktopProvider, claudeDesktop, CLAUDE_AX_PACK } from "../adapters/claude-desktop/index.js";
import { AGENT_ID } from "../adapters/claude-desktop/manifest.js";
import { classify } from "../adapters/claude-desktop/observe.js";
import { SCENARIOS, renderPage } from "../adapters/claude-desktop/fixtures/markup.js";
import { claudeAxSnapshot, scriptedClaudeAx, type AxScriptMode } from "../adapters/claude-desktop/ax/fixtures.js";
import { ChatGPTDotsProvider, AxChatGptAppDriver, chatgptDots } from "../adapters/chatgpt-dots/index.js";
import { AGENT_ID as DOTS_AGENT_ID } from "../adapters/chatgpt-dots/manifest.js";
import { classify as classifyDots } from "../adapters/chatgpt-dots/observe.js";
import { renderPage as renderDots } from "../adapters/chatgpt-dots/fixtures/markup.js";
import { chatgptAppAxSnapshot, scriptedChatGptApp } from "../adapters/chatgpt-dots/ax/fixtures.js";

const mkClaude = (mode: AxScriptMode = "normal", extra: { approval?: string } = {}) => {
  const app = scriptedClaudeAx({ mode, ...extra });
  const driver = new AxClaudeDesktopDriver(app.ax);
  return { ...app, driver, p: new ClaudeDesktopProvider({ driver, pacing: false, pollMs: 1, replyTimeoutMs: 2000 }) };
};

describe("AX selector format v1", () => {
  const tree = build(n("AXApplication", {}, [n("AXWindow", { title: "W" }, [
    n("AXGroup", { description: "Conversation" }, [n("AXButton", { title: "Send message" }), n("AXButton", { title: "Stop" })]),
    n("AXGroup", { description: "Sidebar" }, [n("AXButton", { title: "Send message" })]),
  ])]));
  it("matches role + label regex + ancestor path (gaps allowed) + nth", () => {
    const base = { v: AX_SELECTOR_FORMAT, role: "AXButton" } as const;
    expect(resolveSelector(tree, { ...base, label: "^send" })).toHaveLength(2);
    expect(resolveSelector(tree, { ...base, label: "^send", path: [{ role: "AXGroup", label: "conversation" }] })).toHaveLength(1);
    expect(resolveSelector(tree, { ...base, path: [{ role: "AXWindow" }, { role: "AXGroup", label: "sidebar" }] })).toHaveLength(1);
    expect(resolveSelector(tree, { ...base, path: [{ role: "AXGroup" }, { role: "AXWindow" }] })).toHaveLength(0); // order matters
    expect(resolveSelector(tree, { ...base, nth: 1 })[0].title).toBe("Stop");
  });
  it("refuses an unknown selector format and a bad regex", () => {
    const pack = (format: number, label = "ok"): AxSelectorPack => ({ format, packVersion: "t", bundleId: "x", keys: { k: { critical: true, confidence: "unverified", alternatives: [{ v: 1, role: "AXButton", label }] } } });
    expect(() => assertPackCompatible(pack(1))).not.toThrow();
    expect(() => assertPackCompatible(pack(2))).toThrow(/format 2/);
    expect(() => assertPackCompatible(pack(1, "("))).toThrow();
    expect(() => assertPackCompatible(CLAUDE_AX_PACK)).not.toThrow();
  });
  it("every Claude key is marked unverified", () => {
    for (const k of Object.values(CLAUDE_AX_PACK.keys)) expect(k.confidence).toBe("unverified");
  });
});

describe("AxBridgeDriver (fake child process)", () => {
  function fake(script: (req: Record<string, unknown>, out: PassThrough) => void) {
    const stdin = new PassThrough(); const stdout = new PassThrough();
    const child = Object.assign(new EventEmitter(), { stdin, stdout, kill: () => true }) as never;
    let buf = "";
    stdin.on("data", (d) => { buf += d; let i; while ((i = buf.indexOf("\n")) >= 0) { const l = buf.slice(0, i); buf = buf.slice(i + 1); script(JSON.parse(l), stdout); } });
    return child;
  }
  const line = (o: PassThrough, m: unknown) => o.write(JSON.stringify(m) + "\n");
  it("request/response, snapshot mapping, observer stream", async () => {
    const root = { role: "AXApplication", path: [] };
    const d = new AxBridgeDriver({ binPath: "x", spawnFn: () => fake((r, o) => {
      if (r.cmd === "trust") line(o, { id: r.id, ok: true, trusted: true });
      if (r.cmd === "attach") line(o, { id: r.id, ok: true, pid: 7, manualAccessibility: true });
      if (r.cmd === "snapshot") line(o, { id: r.id, ok: true, root, capturedAt: 5 });
      if (r.cmd === "observe") { line(o, { id: r.id, ok: true }); line(o, { event: "AXValueChanged", role: "AXStaticText", value: "hi", ts: 9 }); }
    }) });
    expect(await d.trust()).toBe(true);
    expect(await d.attach("com.x")).toEqual({ pid: 7, manualAccessibility: true });
    const s = await d.snapshot();
    expect(s).toMatchObject({ formatVersion: 1, bundleId: "com.x", root });
    const seen: unknown[] = [];
    await d.observe((e) => seen.push(e));
    await new Promise((r) => setTimeout(r, 5));
    expect(seen).toEqual([{ event: "AXValueChanged", role: "AXStaticText", value: "hi", ts: 9 }]);
    await d.dispose();
  });
  it("NOT_TRUSTED maps to AxError not_trusted; silence times out", async () => {
    const d = new AxBridgeDriver({ binPath: "x", requestTimeoutMs: 20, spawnFn: () => fake((r, o) => {
      if (r.cmd === "attach") line(o, { id: r.id, ok: false, error: { code: "NOT_TRUSTED", message: "no" } });
    }) });
    await expect(d.attach("com.x")).rejects.toMatchObject({ fault: "not_trusted" });
    await expect(d.trust()).rejects.toMatchObject({ fault: "timeout" });
    await d.dispose();
  });
});

describe("observer-driven event normalization", () => {
  it("maps AX notifications to hints and buffers them", () => {
    expect(normalizeAxEvent({ event: "AXValueChanged", value: " streaming… ", ts: 1 })).toEqual({ kind: "text_changed", role: undefined, text: "streaming…", at: 1 });
    expect(normalizeAxEvent({ event: "AXUIElementCreated", ts: 2 }).kind).toBe("element_added");
    expect(normalizeAxEvent({ event: "AXFocusedUIElementChanged", ts: 3 }).kind).toBe("focus_moved");
    expect(normalizeAxEvent({ event: "AXWeird", ts: 4 }).kind).toBe("other");
    const b = new AxHintBuffer(2);
    for (let i = 1; i <= 3; i++) b.push({ event: "AXValueChanged", value: String(i), ts: i });
    expect(b.size).toBe(2);
    expect(b.changedSince(2)).toBe(true);
    expect(b.drain().map((h) => h.text)).toEqual(["2", "3"]);
    expect(b.changedSince(0)).toBe(false);
  });
  it("replay observers receive emitted events; streaming read through AX classifies as streaming", async () => {
    const { ax, p } = mkClaude();
    const got: string[] = []; const buf = new AxHintBuffer();
    await ax.observe((e) => { got.push(e.event); buf.push(e); });
    ax.emit({ event: "AXValueChanged", value: "x", ts: 1 });
    expect(got).toEqual(["AXValueChanged"]); expect(ax.observed).toContain("AXUIElementCreated");
    const c = await p.contextOpen({ agentId: AGENT_ID, threadId: "t" });
    if (!c.ok) throw new Error(JSON.stringify(c.error));
    const before = await p.get(AGENT_ID);
    expect(before.ok).toBe(true);
    // a streaming frame observed through AX surfaces as a normalized hint source; the provider stays on the same classify path
    expect(classify(await new AxClaudeDesktopDriver(new AxReplayDriver({ snapshots: [claudeAxSnapshot(SCENARIOS.streaming)] })).html()).streaming).toBe(true);
  });
});

describe("claude-desktop over AX", () => {
  it("passes conformance through the AX transport (best_effort, inferred)", async () => {
    const { p, state } = mkClaude("normal", { approval: "Finder" });
    const approvalId = classify(renderPage({ approval: "Finder" })).approvals[0].id;
    const report = await runConformance(p, {
      agentId: AGENT_ID, approvalId, settleMs: 10,
      faulty: {
        vendor_down: () => mkClaude("down").p, rate_limited: () => mkClaude("rate_limited").p, auth_revoked: () => mkClaude("logged_out").p,
        account_banned: () => mkClaude("blocked").p, ui_changed: () => mkClaude("drift").p,
      },
    });
    expect(report.areas.flatMap((a) => a.checks.filter((c) => c.status === "fail").map((c) => `${a.area}: ${c.name}: ${c.reason}`))).toEqual([]);
    expect(report.ok).toBe(true);
    expect(state.resolution).toBe("approved");
    expect((await p.capabilities(AGENT_ID) as { ok: true; value: { guarantee: string } }).value.guarantee).toBe("best_effort");
  });
  it("untrusted helper -> AUTH_REQUIRED with a how-to-grant humanMessage", async () => {
    const { p } = mkClaude("untrusted");
    const r = await p.contextOpen({ agentId: AGENT_ID, threadId: "t" });
    expect(r.ok).toBe(false);
    if (!r.ok) { expect(r.error.code).toBe("AUTH_REQUIRED"); expect(r.error.humanMessage).toMatch(/System Settings.*Accessibility/s); }
  });
  it("without the per-app consent every call is LANE_BLOCKED and the bridge is never touched", async () => {
    const ax = new AxReplayDriver({ frame: () => claudeAxSnapshot({}).root });
    const p = claudeDesktop.create({ transport: "ax", ax, pacing: false });
    const r = await p.contextOpen({ agentId: AGENT_ID, threadId: "t" });
    expect(r.ok === false && r.error.code).toBe("LANE_BLOCKED");
    expect(ax.attached).toBe(false);
  });
  it("factory: ax transport needs a bridge; default stays CDP", () => {
    expect(() => claudeDesktop.create({ transport: "ax" })).toThrow(/needs `ax` or `axBinPath`/);
    expect(claudeDesktop.create({})).toBeInstanceOf(ClaudeDesktopProvider);
  });
  it("AX fixtures classify like the DOM fixtures", async () => {
    for (const name of ["idle", "cowork", "streaming", "complete", "approval", "rate-limit", "logged-out"] as const) {
      const scn = SCENARIOS[name];
      const d = new AxClaudeDesktopDriver(new AxReplayDriver({ snapshots: [claudeAxSnapshot(scn)] }));
      const viaAx = classify(await d.html()); const viaDom = classify(renderPage(scn));
      expect({ ...viaAx, detail: "" }).toEqual({ ...viaDom, detail: "" });
    }
  });
  it("recorded snapshot JSON fixtures stay in sync with the builder", async () => {
    const dir = fileURLToPath(new URL("../adapters/claude-desktop/ax/fixtures/", import.meta.url));
    for (const name of ["idle", "streaming", "approval", "rate-limit", "logged-out", "drift"]) {
      const file = `${dir}${name}.json`; const snap = claudeAxSnapshot(SCENARIOS[name]);
      if (process.env.UPDATE_AX_FIXTURES === "1") { mkdirSync(dir, { recursive: true }); writeFileSync(file, JSON.stringify(snap, null, 1) + "\n"); }
      expect(existsSync(file)).toBe(true);
      const loaded = JSON.parse(readFileSync(file, "utf8")) as AxSnapshot;
      expect(loaded).toEqual(snap);
      const kind = classify(await new AxClaudeDesktopDriver(new AxReplayDriver({ snapshots: [loaded] })).html()).kind;
      expect(kind).toBe(classify(renderPage(SCENARIOS[name])).kind);
    }
  });
});

describe("selector versioning and drift -> ADAPTER_DRIFT", () => {
  const drift = async (driver: AxClaudeDesktopDriver) => {
    const p = new ClaudeDesktopProvider({ driver, pacing: false, pollMs: 1 });
    const r = await p.contextOpen({ agentId: AGENT_ID, threadId: "t" });
    return { r, p };
  };
  it("a moved composer label latches ADAPTER_DRIFT and records the missing critical key", async () => {
    const d = new AxClaudeDesktopDriver(new AxReplayDriver({ snapshots: [claudeAxSnapshot({ drift: "composer" })] }));
    const { r } = await drift(d);
    expect(r.ok === false && r.error.code).toBe("ADAPTER_DRIFT");
    expect(d.lastDrift).toMatchObject({ missing: ["composer"], packVersion: "claude-ax-v1" });
    expect(missingCritical(claudeAxSnapshot({ drift: "composer" }).root, CLAUDE_AX_PACK)).toEqual(["composer"]);
    expect(resolveKey(claudeAxSnapshot({}).root, CLAUDE_AX_PACK, "composer")).toHaveLength(1);
  });
  it("a pack written in an unsupported selector format reads as drift, never as a guess", async () => {
    const pack = { ...CLAUDE_AX_PACK, format: 2 };
    const d = new AxClaudeDesktopDriver(new AxReplayDriver({ snapshots: [claudeAxSnapshot({})] }), true, pack);
    const { r } = await drift(d);
    expect(r.ok === false && r.error.code).toBe("ADAPTER_DRIFT");
    expect(d.lastDrift?.reason).toMatch(/format 2/);
  });
});

describe("chatgpt-app transport (native ChatGPT.app, com.openai.chat)", () => {
  const mk = (mode: Parameters<typeof scriptedChatGptApp>[0] = {}) => {
    const app = scriptedChatGptApp(mode);
    return { ...app, p: new ChatGPTDotsProvider({ driver: new AxChatGptAppDriver(app.ax), pacing: false, pollMs: 1, replyTimeoutMs: 2000 }) };
  };
  const CARD = { policy: "ask_first" as const, text: "Send email to sam@example.com" };
  it("AX trees classify like the DOM fixtures (list view, dot view, confirmation, tasks)", async () => {
    const cases = [
      { view: "list" as const, dots: [{ id: "nova-dot", name: "Nova", handle: "@nova-dot" }] },
      { turns: [{ role: "user" as const, text: "hi" }, { role: "assistant" as const, text: "hello" }], streaming: true, activity: "Working" },
      { confirmation: CARD, turns: [{ role: "user" as const, text: "Email Sam" }] },
      { showTasks: true, tasks: [{ title: "Compile vendor notes", state: "in_progress" as const }, { title: "Weekly digest", state: "scheduled" as const }] },
      { banner: "You've reached your usage limit. Your quota resets at 14:00." },
      { loggedOut: true },
    ];
    for (const scn of cases) {
      const d = new AxChatGptAppDriver(new AxReplayDriver({ snapshots: [chatgptAppAxSnapshot(scn)] }));
      const strip = (s: ReturnType<typeof classifyDots>) => ({ ...s, detail: "", dots: s.dots.map(({ avatar, ...r }) => r), header: s.header ? { name: s.header.name, handle: s.header.handle } : undefined });
      expect(strip(classifyDots(await d.html()))).toEqual(strip(classifyDots(renderDots(scn))));
    }
  });
  it("passes conformance through the AX transport", async () => {
    const { p, state } = mk({ confirmation: CARD });
    const confirmationId = classifyDots(renderDots({ confirmation: CARD })).confirmations[0].id;
    const report = await runConformance(p, {
      agentId: `${DOTS_AGENT_ID}:nova-dot`, approvalId: confirmationId, settleMs: 10,
      faulty: {
        vendor_down: () => mk({ mode: "down" }).p, rate_limited: () => mk({ mode: "rate_limited" }).p, auth_revoked: () => mk({ mode: "logged_out" }).p,
        account_banned: () => mk({ mode: "blocked" }).p, ui_changed: () => mk({ mode: "drift" }).p,
      },
    });
    expect(report.areas.flatMap((a) => a.checks.filter((c) => c.status === "fail").map((c) => `${a.area}: ${c.name}: ${c.reason}`))).toEqual([]);
    expect(report.ok).toBe(true);
    expect(state.resolution).toBe("approved");
  });
  it("untrusted -> AUTH_REQUIRED; no consent -> LANE_BLOCKED; factory wiring", async () => {
    const r = await mk({ mode: "untrusted" }).p.contextOpen({ agentId: `${DOTS_AGENT_ID}:nova-dot`, threadId: "t" });
    expect(r.ok === false && r.error.code).toBe("AUTH_REQUIRED");
    if (!r.ok) expect(r.error.humanMessage).toMatch(/Accessibility/);
    const app = scriptedChatGptApp();
    const p = chatgptDots.create({ transport: "chatgpt-app", ax: app.ax, pacing: false });
    const blocked = await p.contextOpen({ agentId: `${DOTS_AGENT_ID}:nova-dot`, threadId: "t" });
    expect(blocked.ok === false && blocked.error.code).toBe("LANE_BLOCKED");
    expect(app.ax.attached).toBe(false);
    expect(() => chatgptDots.create({ transport: "chatgpt-app" })).toThrow(/needs `ax` or `axBinPath`/);
  });
});
