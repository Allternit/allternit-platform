// Route desktop-app-ax for the ChatGPT and Kimi desktop apps (+ Claude's route declaration), offline only:
// hand-built AX trees and a scripted app behind AxReplayDriver. Never touches a running app, the AX permission or a vendor.
// One describe block per §3 contract item, run against both adapters.
import { existsSync, mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { describe, expect, it } from "vitest";
import { runConformance } from "@allternit/agent-gateway";
import {
  AX_CHAT_CONTRACT, AX_ROUTE_ELIGIBLE, DEFAULT_PATTERNS, AxChatAppProvider, AxReplayDriver, DESKTOP_APP_AX_ROUTE, assertPackCompatible, chatAppSnapshot,
  exactText, missingCritical, readChatState, resolveKey, retryAfterFromBanner, scriptedChatApp, unavailableAxDriver,
  type AxChatAppSpec, type AxSnapshot, type ChatScene, type ChatScriptOptions, type ChatSkin,
} from "../adapters/_shared/ax/index.js";
import { CHATGPT_DESKTOP_SPEC, CHATGPT_DESKTOP_SKIN, CHATGPT_DESKTOP_MANIFEST, CHATGPT_NATIVE_BUNDLE_ID, CHATGPT_ELECTRON_BUNDLE_ID, chatgptDesktop, ROUTE_IDS as GPT_ROUTES } from "../adapters/chatgpt-desktop/index.js";
import { KIMI_DESKTOP_SPEC, KIMI_DESKTOP_SKIN, KIMI_DESKTOP_MANIFEST, kimiDesktop, ROUTE_IDS as KIMI_ROUTES } from "../adapters/kimi-desktop/index.js";
import { ROUTE_IDS as CLAUDE_ROUTES, CLAUDE_DESKTOP_MANIFEST } from "../adapters/claude-desktop/manifest.js";
import { createAaiRegistration as gptReg } from "../adapters/chatgpt-desktop/aai.js";
import { createAaiRegistration as kimiReg } from "../adapters/kimi-desktop/aai.js";

const APPS: Array<{ name: string; dir: string; spec: AxChatAppSpec; skin: ChatSkin }> = [
  { name: "chatgpt-desktop", dir: "chatgpt-desktop", spec: CHATGPT_DESKTOP_SPEC, skin: CHATGPT_DESKTOP_SKIN },
  { name: "kimi-desktop", dir: "kimi-desktop", spec: KIMI_DESKTOP_SPEC, skin: KIMI_DESKTOP_SKIN },
];

const mk = (spec: AxChatAppSpec, skin: ChatSkin, o: ChatScriptOptions = {}, extra: { consented?: boolean; replyTimeoutMs?: number } = {}) => {
  const app = scriptedChatApp(skin, o);
  const p = new AxChatAppProvider(spec, { ax: app.ax, consented: extra.consented ?? true, pacing: false, pollMs: 1, replyTimeoutMs: extra.replyTimeoutMs ?? 2000 });
  return { ...app, p };
};
const okv = <T>(r: { ok: true; value: T } | { ok: false; error: unknown }): T => { if (!r.ok) throw new Error(JSON.stringify(r.error)); return r.value; };

/** Named fixture scenes, written to adapters/<id>/fixtures/*.json (UPDATE_AX_FIXTURES=1 regenerates). */
const SCENES: Record<string, ChatScene> = {
  idle: { model: "Fast", threads: ["Trip plan", "Tax notes"] },
  streaming: { model: "Fast", turns: [{ role: "user", text: "hi" }, { role: "assistant", text: "Hel" }], streaming: true },
  complete: { model: "Fast", turns: [{ role: "user", text: "hi" }, { role: "assistant", text: "Hello.\nSecond line." }] },
  "model-picker": { model: "Fast", models: ["Fast", "Thinking"], pickerOpen: true },
  "rate-limit": { banner: "You've reached your usage limit. Try again in 2 hours." },
  verification: { banner: "Unusual activity detected. Verify you are human to continue." },
  "logged-out": { loggedOut: true },
  drift: { drift: true },
};

describe("route declaration (route registry keys on these exact ids)", () => {
  it("chatgpt, kimi and claude declare desktop-app-ax only; nobody declares a vendor-blocked or unprobed cdp route", () => {
    expect(GPT_ROUTES).toEqual(["desktop-app-ax"]);
    expect(KIMI_ROUTES).toEqual(["desktop-app-ax"]);
    expect(CLAUDE_ROUTES).toEqual(["desktop-app-ax"]);
    for (const m of [CHATGPT_DESKTOP_MANIFEST, KIMI_DESKTOP_MANIFEST, CLAUDE_DESKTOP_MANIFEST]) expect((m as unknown as { routes: string[] }).routes).toEqual([DESKTOP_APP_AX_ROUTE]);
  });
  it("manifests are ui_bridge + localRequired (what the router maps to the desktop routes) and best_effort", () => {
    for (const { spec } of APPS) {
      expect(spec.capabilities).toMatchObject({ lane: "ui_bridge", guarantee: "best_effort", runtime: { localRequired: true } });
      expect(() => assertPackCompatible(spec.pack)).not.toThrow();
      for (const k of Object.values(spec.pack.keys)) expect(k.confidence).toBe("unverified");
    }
    expect(CHATGPT_DESKTOP_SPEC.bundleIds).toEqual([CHATGPT_NATIVE_BUNDLE_ID, CHATGPT_ELECTRON_BUNDLE_ID]);
  });
  it("the contract table is honest: attachments + generated files are not implemented, so the route is not eligible yet", () => {
    const by = Object.fromEntries(AX_CHAT_CONTRACT.map((c) => [c.item, c.status]));
    expect(by).toEqual({
      "chat.start_continue": "implemented", "reply.stream_read": "implemented", "reply.stop_midstream": "implemented",
      "attachments.send": "blocked", "outputs.exact_and_files": "partial", "threads.list_reopen": "implemented", "errors.typed_states": "implemented",
    });
    expect(AX_ROUTE_ELIGIBLE).toBe(false);
  });
});

for (const { name, dir, spec, skin } of APPS) {
  const agentId = spec.agentId;
  describe(`${name} fixtures`, () => {
    it("recorded snapshot JSON stays in sync with the builder and reads to the expected state", () => {
      const fdir = fileURLToPath(new URL(`../adapters/${dir}/fixtures/`, import.meta.url));
      const expected: Record<string, string> = { idle: "ok", streaming: "ok", complete: "ok", "model-picker": "ok", "rate-limit": "rate_limited", verification: "blocked", "logged-out": "logged_out", drift: "drift" };
      for (const [scene, kind] of Object.entries(expected)) {
        const file = `${fdir}${scene}.json`; const snap = chatAppSnapshot(skin, SCENES[scene]);
        if (process.env.UPDATE_AX_FIXTURES === "1") { mkdirSync(fdir, { recursive: true }); writeFileSync(file, JSON.stringify(snap, null, 1) + "\n"); }
        expect(existsSync(file)).toBe(true);
        const loaded = JSON.parse(readFileSync(file, "utf8")) as AxSnapshot;
        expect(loaded).toEqual(snap);
        expect(readChatState(loaded.root, spec.pack, 0, { ...DEFAULT_PATTERNS, ...spec.patterns }).kind).toBe(kind);
      }
    });
    it("every critical key resolves on the idle skin (pack and skin agree)", () => {
      expect(missingCritical(chatAppSnapshot(skin, SCENES.idle).root, spec.pack)).toEqual([]);
      expect(missingCritical(chatAppSnapshot(skin, SCENES.drift).root, spec.pack)).toEqual(["composer"]);
    });
  });

  describe(`${name} contract: start, continue, pick the model`, () => {
    it("passes the AAI conformance suite through the AX route", async () => {
      const report = await runConformance(mk(spec, skin).p, {
        agentId, settleMs: 10,
        faulty: {
          vendor_down: () => mk(spec, skin, { mode: "down" }).p, rate_limited: () => mk(spec, skin, { mode: "rate_limited" }).p,
          auth_revoked: () => mk(spec, skin, { mode: "logged_out" }).p, account_banned: () => mk(spec, skin, { mode: "blocked" }).p,
          ui_changed: () => mk(spec, skin, { mode: "drift" }).p,
        },
      });
      expect(report.areas.flatMap((a) => a.checks.filter((c) => c.status === "fail").map((c) => `${a.area}: ${c.name}: ${c.reason}`))).toEqual([]);
      expect(report.ok).toBe(true);
    });
    it("open presses New chat; two messages continue the same chat; replies come back", async () => {
      const { p, state } = mk(spec, skin);
      const { contextId } = okv(await p.contextOpen({ agentId, threadId: "t1" }));
      expect(state.newChats).toBe(1);
      expect(okv(await p.contextMessage({ contextId, correlationId: "c1", text: "one" })).reply).toBe("Echo: one");
      expect(okv(await p.contextMessage({ contextId, correlationId: "c2", text: "two" })).reply).toBe("Echo: two");
      expect(state.sends).toBe(2);
      expect(okv(await p.transcript(contextId)).map((t) => t.text)).toEqual(["one", "Echo: one", "two", "Echo: two"]);
    });
    it("lists models, selects one, and open can pick a model up front", async () => {
      const { p, state } = mk(spec, skin, { models: ["Fast", "Thinking", "Pro"] });
      expect(okv(await p.models())).toEqual({ current: "Fast", options: ["Fast", "Thinking", "Pro"] });
      expect(state.pickerOpen).toBe(false); // picker closed again after reading
      expect(okv(await p.selectModel("thinking")).model).toBe("Thinking");
      const bad = await p.selectModel("Ultra");
      expect(bad.ok === false && bad.error.code).toBe("UNKNOWN");
      expect(state.pickerOpen).toBe(false);
      okv(await p.contextOpen({ agentId, model: "Pro" } as never));
      expect(state.model).toBe("Pro");
    });
  });

  describe(`${name} contract: read the reply as it streams, know when it finished`, () => {
    it("emits deltas while Stop is showing and one completed event when Stop leaves the tree", async () => {
      const { p } = mk(spec, skin, { frames: 4, reply: () => "abcdefgh" });
      const { contextId } = okv(await p.contextOpen({ agentId }));
      const r = okv(await p.contextMessage({ contextId, correlationId: "s1", text: "go" }));
      expect(r.reply).toBe("abcdefgh");
      const evs = okv(await p.events({ contextId })).events.map((e) => e.event);
      const deltas = evs.filter((e) => e.type === "agent.message.delta").map((e) => e.payload.text);
      expect(deltas.length).toBeGreaterThan(1);
      expect(deltas.join("")).toBe("abcdefgh");
      const done = evs.filter((e) => e.type === "agent.message.completed");
      expect(done).toHaveLength(1);
      expect(done[0].payload).toMatchObject({ text: "abcdefgh", finishedBy: "stop_control_gone" });
      expect(evs.every((e) => e.guarantee !== "exact" && e.correlationId)).toBe(true);
    });
    it("a reply still streaming at the timeout returns without a reply; events() keeps reading", async () => {
      const { p } = mk(spec, skin, { frames: 1000 }, { replyTimeoutMs: 0 });
      const { contextId } = okv(await p.contextOpen({ agentId }));
      expect(okv(await p.contextMessage({ contextId, correlationId: "s2", text: "long" })).reply).toBeUndefined();
      const evs = okv(await p.events({ contextId })).events.map((e) => e.event.type);
      expect(evs).toContain("agent.message.delta");
      expect(evs).not.toContain("agent.message.completed");
    });
  });

  describe(`${name} contract: stop mid-stream`, () => {
    it("cancel presses Stop and confirms; no deltas after", async () => {
      const { p, state } = mk(spec, skin, { frames: 1000, reply: () => "x".repeat(5000) }, { replyTimeoutMs: 0 });
      const { contextId } = okv(await p.contextOpen({ agentId }));
      await p.contextMessage({ contextId, correlationId: "k1", text: "long" });
      expect(okv(await p.contextCancel({ contextId })).confirmed).toBe(true);
      expect(state.stopped).toBe(true);
      expect(state.presses).toContain(skin.stop);
      const n1 = okv(await p.events({ contextId })).events.length;
      const n2 = okv(await p.events({ contextId })).events.length;
      expect(n2).toBe(n1);
    });
  });

  describe(`${name} contract: outputs`, () => {
    it("text comes back verbatim from the tree, line breaks kept", async () => {
      const { p } = mk(spec, skin, { reply: () => "Line one.\nLine **two** stays raw.\n- item" });
      const { contextId } = okv(await p.contextOpen({ agentId }));
      expect(okv(await p.contextMessage({ contextId, correlationId: "o1", text: "x" })).reply).toBe("Line one.\nLine **two** stays raw.\n- item");
    });
    it("attachments and generated files answer UNSUPPORTED naming the contract item (never quietly skipped)", async () => {
      const { p } = mk(spec, skin);
      const a = await p.sendAttachment({ contextId: "x", path: "/tmp/a.png" });
      expect(a.ok === false && a.error.code).toBe("UNSUPPORTED");
      if (!a.ok) expect(a.error.details).toMatchObject({ contractItem: "attachments.send", route: "desktop-app-ax" });
      const f = await p.outputFiles({ contextId: "x" });
      if (!f.ok) expect(f.error.details).toMatchObject({ contractItem: "outputs.exact_and_files" });
    });
  });

  describe(`${name} contract: list and reopen past threads`, () => {
    it("lists sidebar threads, reopens one by id and continues it", async () => {
      const { p, state } = mk(spec, skin, { threads: { "Trip plan": [{ role: "user", text: "plan a trip" }, { role: "assistant", text: "Sure." }], "Tax notes": [] } });
      const threads = okv(await p.threads());
      expect(threads).toEqual([{ id: "trip-plan", title: "Trip plan" }, { id: "tax-notes", title: "Tax notes" }]);
      const o = okv(await p.contextOpen({ agentId, adoptContextId: "thread:trip-plan" }));
      expect(o.resumed).toBe(true);
      expect(state.opened).toBe("Trip plan");
      expect(state.newChats).toBe(0);
      expect(okv(await p.transcript(o.contextId)).length).toBe(2);
      expect(okv(await p.contextMessage({ contextId: o.contextId, correlationId: "r1", text: "and back" })).reply).toBe("Echo: and back");
      const evs = okv(await p.events({ contextId: o.contextId })).events.map((e) => e.event);
      expect(evs.find((e) => e.type === "agent.context.opened")?.payload).toMatchObject({ reopened: "trip-plan", turns: 2 });
      // history from before the reopen is not re-emitted as new replies
      expect(evs.filter((e) => e.type === "agent.message.completed").map((e) => e.payload.text)).toEqual(["Echo: and back"]);
      const miss = await p.contextOpen({ agentId, adoptContextId: "thread:nope" });
      expect(miss.ok === false && miss.error.code).toBe("CONTEXT_NOT_FOUND");
    });
  });

  describe(`${name} contract: typed error states`, () => {
    const open = (o: ChatScriptOptions, extra = {}) => mk(spec, skin, o, extra).p.contextOpen({ agentId });
    it("usage limit -> RATE_LIMITED with retryAfterMs from the banner", async () => {
      const r = await open({ mode: "rate_limited", rateBanner: "You've reached your usage limit. Try again in 3 hours." });
      expect(r.ok === false && r.error).toMatchObject({ code: "RATE_LIMITED", retryable: true, retryAfterMs: 3 * 3_600_000 });
    });
    it("signed out -> AUTH_REQUIRED (first run) / AUTH_REVOKED (was signed in)", async () => {
      expect((await open({ mode: "logged_out" })) as unknown).toMatchObject({ ok: false, error: { code: "AUTH_REQUIRED" } });
      let out = false;
      const ax = new AxReplayDriver({ bundleId: skin.bundleId, frame: () => chatAppSnapshot(skin, out ? { loggedOut: true } : SCENES.idle).root });
      const p = new AxChatAppProvider(spec, { ax, consented: true, pacing: false });
      okv(await p.contextOpen({ agentId }));
      out = true;
      expect((await p.contextOpen({ agentId })) as unknown).toMatchObject({ ok: false, error: { code: "AUTH_REVOKED" } });
    });
    it("verification wall -> LANE_BLOCKED, latched until cleared", async () => {
      const { p } = mk(spec, skin, { mode: "blocked" });
      const r = await p.contextOpen({ agentId });
      expect(r.ok === false && r.error.code).toBe("LANE_BLOCKED");
      expect((await p.health({})).ok && ((await p.health({})) as { value: { status: string } }).value.status).toBe("down");
    });
    it("untrusted helper -> AUTH_REQUIRED with how-to; app not running -> VENDOR_UNAVAILABLE; no consent -> LANE_BLOCKED, bridge untouched", async () => {
      const u = await open({ mode: "untrusted" });
      expect(u.ok === false && u.error.code).toBe("AUTH_REQUIRED");
      if (!u.ok) expect(u.error.humanMessage).toMatch(/System Settings.*Accessibility/s);
      const d = await open({ mode: "down" });
      expect(d.ok === false && d.error).toMatchObject({ code: "VENDOR_UNAVAILABLE", retryable: true });
      const { p, ax } = mk(spec, skin, {}, { consented: false });
      const b = await p.contextOpen({ agentId });
      expect(b.ok === false && b.error.code).toBe("LANE_BLOCKED");
      expect(ax.attached).toBe(false);
    });
    it("selector drift -> ADAPTER_DRIFT with the missing key and pack version", async () => {
      const { p } = mk(spec, skin, { mode: "drift" });
      const r = await p.contextOpen({ agentId });
      expect(r.ok === false && r.error.code).toBe("ADAPTER_DRIFT");
      expect(p.lastDrift).toMatchObject({ missing: ["composer"], packVersion: spec.pack.packVersion });
    });
  });

  describe(`${name} registration`, () => {
    it("aai.ts registers lazily: no consent -> LANE_BLOCKED; consent without a bridge -> VENDOR_UNAVAILABLE", async () => {
      const reg = name === "chatgpt-desktop" ? gptReg : kimiReg;
      const consentVar = name === "chatgpt-desktop" ? "SUBS_GATEWAY_CHATGPT_DESKTOP_AX_CONSENT" : "SUBS_GATEWAY_KIMI_DESKTOP_AX_CONSENT";
      const a = await reg({}).provider.contextOpen({ agentId });
      expect(a.ok === false && a.error.code).toBe("LANE_BLOCKED");
      const b = await reg({ [consentVar]: "1" }).provider.contextOpen({ agentId });
      expect(b.ok === false && b.error.code).toBe("VENDOR_UNAVAILABLE");
      expect(reg({}).provider.adapterId).toBe(name);
      const factory = name === "chatgpt-desktop" ? chatgptDesktop : kimiDesktop;
      expect(() => factory.create({})).toThrow(/needs `ax` or `axBinPath`/);
    });
  });
}

describe("chatgpt-desktop attaches to whichever ChatGPT build is running", () => {
  it("native com.openai.chat not running -> falls through to the Electron com.openai.codex build", async () => {
    const { p } = mk(CHATGPT_DESKTOP_SPEC, CHATGPT_DESKTOP_SKIN, { runningBundleId: CHATGPT_ELECTRON_BUNDLE_ID });
    okv(await p.contextOpen({ agentId: CHATGPT_DESKTOP_SPEC.agentId }));
    expect(p.attachedBundleId).toBe(CHATGPT_ELECTRON_BUNDLE_ID);
    const n = mk(CHATGPT_DESKTOP_SPEC, CHATGPT_DESKTOP_SKIN, { runningBundleId: CHATGPT_NATIVE_BUNDLE_ID });
    okv(await n.p.contextOpen({ agentId: CHATGPT_DESKTOP_SPEC.agentId }));
    expect(n.p.attachedBundleId).toBe(CHATGPT_NATIVE_BUNDLE_ID);
  });
});

describe("kimi-desktop reads Chinese limit and verification banners", () => {
  it("额度/上限 -> RATE_LIMITED; 安全验证 -> LANE_BLOCKED", async () => {
    const r = await mk(KIMI_DESKTOP_SPEC, KIMI_DESKTOP_SKIN, { mode: "rate_limited", rateBanner: "今日额度已达上限，请稍后再试" }).p.contextOpen({ agentId: KIMI_DESKTOP_SPEC.agentId });
    expect(r.ok === false && r.error.code).toBe("RATE_LIMITED");
    const b = await mk(KIMI_DESKTOP_SPEC, KIMI_DESKTOP_SKIN, { mode: "blocked", blockedBanner: "请完成安全验证" }).p.contextOpen({ agentId: KIMI_DESKTOP_SPEC.agentId });
    expect(b.ok === false && b.error.code).toBe("LANE_BLOCKED");
  });
});

describe("shared helpers", () => {
  it("retryAfterFromBanner parses durations and clock times, bounded", () => {
    expect(retryAfterFromBanner("try again in 15 minutes", 0)).toBe(15 * 60_000);
    expect(retryAfterFromBanner("resets in 2 hours", 0)).toBe(2 * 3_600_000);
    expect(retryAfterFromBanner("in 5 seconds", 0)).toBe(60_000);
    const now = new Date(2026, 9, 17, 13, 0).getTime();
    expect(retryAfterFromBanner("Your limit resets at 2:30 PM", now)).toBe(90 * 60_000);
    expect(retryAfterFromBanner("resets at 14:00", now)).toBe(60 * 60_000);
    expect(retryAfterFromBanner("no hint here", now)).toBeUndefined();
  });
  it("exactText keeps text runs within a paragraph joined and breaks between paragraphs", () => {
    const root = chatAppSnapshot(CHATGPT_DESKTOP_SKIN, { turns: [{ role: "assistant", text: "a\nb" }] }).root;
    const turn = resolveKey(root, CHATGPT_DESKTOP_SPEC.pack, "assistantTurn")[0];
    expect(exactText(turn)).toBe("a\nb");
  });
  it("unavailableAxDriver fails every call as bridge_unavailable", async () => {
    await expect(unavailableAxDriver("nope").trust()).rejects.toMatchObject({ fault: "bridge_unavailable" });
  });
});
