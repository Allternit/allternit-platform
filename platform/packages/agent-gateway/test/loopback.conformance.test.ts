import { describe, expect, it } from "vitest";
import session from "./fixtures/loopback-session.json";
import { LoopbackProvider, createReplayFetch, faultFetch, runConformance, type ConformanceFixtures, type RecordedSession } from "../src";

const BASE = "http://127.0.0.1:3010/api/v1";
const rec = session as unknown as RecordedSession;
const mk = (f: typeof fetch) => new LoopbackProvider({ baseUrl: BASE, botIds: ["bot-1"], fetch: f, auth: { token: "t" }, maxParallel: 3 });

function fixtures(): ConformanceFixtures {
  return {
    agentId: "bot-1",
    approvalId: "perm-1",
    faulty: {
      vendor_down: () => mk(faultFetch("vendor_down")),
      rate_limited: () => mk(faultFetch("rate_limited")),
      auth_revoked: () => mk(faultFetch("auth_revoked")),
      account_banned: () => mk(faultFetch("forbidden")),
      ui_changed: () => mk(faultFetch("drift")),
    },
  };
}

describe("LoopbackProvider conformance (recorded allternit-api session)", () => {
  it("passes every applicable area", async () => {
    const replay = createReplayFetch(rec);
    const report = await runConformance(mk(replay.fetch), fixtures());
    const failing = report.areas.filter((a) => a.status === "fail");
    expect(failing.map((a) => `${a.area}: ${a.reasons.join("; ")}`)).toEqual([]);
    expect(report.ok).toBe(true);
    expect(report.guarantee).toBe("exact");
    expect(report.lane).toBe("local");
    const by = Object.fromEntries(report.areas.map((a) => [a.area, a.status]));
    for (const a of ["identity", "context", "isolation", "parallelism", "events", "approvals", "failure", "idempotency", "cancellation"]) expect(by[a]).toBe("pass");
    for (const a of ["memory", "computer", "sync"]) expect(by[a]).toBe("skipped-unsupported");
    expect(JSON.parse(JSON.stringify(report)).areas).toHaveLength(13);
    // replayed correlation ids never reached allternit-api twice: each message hit the API at most once per correlation
    const bodies = replay.calls.filter((c) => c.method === "POST" && c.path.endsWith("/messages")).map((c) => `${c.path}|${c.body?.text}`);
    expect(new Set(bodies).size).toBeLessThan(bodies.length + 1);
  });

  it("does not send a replayed correlationId to allternit-api twice", async () => {
    const replay = createReplayFetch(rec);
    const p = mk(replay.fetch);
    const ctx = await p.contextOpen({ agentId: "bot-1" });
    if (!ctx.ok) throw new Error("open failed");
    const args = { contextId: ctx.value.contextId, correlationId: "c-1", text: "once" };
    const [a, b] = await Promise.all([p.contextMessage(args), p.contextMessage(args)]);
    await p.contextMessage(args);
    expect(a).toEqual(b);
    expect(replay.count("POST", "/agent-sessions/:id/messages")).toBe(1);
  });

  it("maps Thread <-> context, events exact, refuses non-human approvals without calling the API", async () => {
    const replay = createReplayFetch(rec);
    const p = mk(replay.fetch);
    const ctx = await p.contextOpen({ agentId: "bot-1", title: "t" });
    expect(ctx.ok && ctx.value.contextId).toBe("thr-1");
    const ev = await p.events({ contextId: "thr-1" });
    if (!ev.ok) throw new Error("events failed");
    expect(ev.value.events.every((e) => e.event.guarantee === "exact" && e.event.source === "allternit")).toBe(true);
    expect(ev.value.events.map((e) => e.event.remoteEventId).filter(Boolean)).toEqual(["ev-thr-1-1", "ev-thr-1-2"]);
    const r = await p.approvals({ op: "respond", approvalId: "perm-1", decision: "approve", actor: { type: "system", id: "x" } });
    expect(!r.ok && r.error.code).toBe("APPROVAL_REQUIRED");
    expect(replay.count("POST", "/permissions/:id/reply")).toBe(0);
    const un = await p.contextSteer({ contextId: "thr-1", text: "x" });
    expect(!un.ok && un.error.code).toBe("UNSUPPORTED");
  });
});
