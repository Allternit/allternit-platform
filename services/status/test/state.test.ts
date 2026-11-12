import { describe, expect, it } from "vitest";
import type { ProbeResult, TargetsDoc } from "../contract/types";
import { type ComponentRecord, type StandbyReport, step } from "../src/state";

// Small table: API (critical, live + ready), Docs (non-critical), Other (critical target, capped at degraded).
const DOC: TargetsDoc = {
  schema: "allternit.targets/v1",
  version: 1,
  components: [
    { id: "api", name: "API", group: "api", critical: true },
    { id: "docs", name: "Docs", group: "docs", critical: false },
    { id: "other", name: "Other sites", group: "other", critical: true, max_status: "degraded" },
  ],
  targets: [
    { id: "api-live", component_id: "api", name: "live", url: "https://a/live", method: "GET", expect_status: 200, body_marker: null, critical: true, role: "live", vantages: ["cloudflare", "allternit-standby"] },
    { id: "api-ready", component_id: "api", name: "ready", url: "https://a/ready", method: "GET", expect_status: 200, body_marker: null, critical: true, role: "ready", vantages: ["cloudflare", "allternit-standby"] },
    { id: "docs", component_id: "docs", name: "docs", url: "https://d/", method: "GET", expect_status: 200, body_marker: null, critical: false, vantages: ["cloudflare", "allternit-standby"] },
    { id: "labs", component_id: "other", name: "labs", url: "https://l/", method: "GET", expect_status: 200, body_marker: null, critical: true, vantages: ["cloudflare", "allternit-standby"] },
  ],
};

const T0 = Date.parse("2026-11-12T10:00:00Z");
type Fail = Set<string>;
const res = (fail: Fail, latency = 100): ProbeResult[] =>
  DOC.targets.map((t) => ({ target_id: t.id, ok: !fail.has(t.id), status_code: fail.has(t.id) ? 502 : 200, latency_ms: latency }));

/** Runs minute ticks. cf/sb give the failing target ids per tick; sb = null means the standby sent nothing. */
function run(ticks: { cf: Fail; sb?: Fail | null; latency?: number }[], start: Map<string, ComponentRecord> = new Map()) {
  let prev = start;
  const standby: StandbyReport[] = [];
  const statuses: Record<string, string>[] = [];
  ticks.forEach((tk, i) => {
    const now = new Date(T0 + i * 60_000);
    if (tk.sb !== null && tk.sb !== undefined) standby.unshift({ ts: now.toISOString(), results: res(tk.sb) });
    const out = step({ now, targets: DOC, cf: res(tk.cf, tk.latency), standby: standby.slice(0, 3), prev });
    prev = out.next;
    statuses.push(Object.fromEntries([...out.next].map(([id, r]) => [id, r.status])));
  });
  return { statuses, last: prev };
}

const none: Fail = new Set();
const api: Fail = new Set(["api-live", "api-ready"]);
const rep = <T,>(n: number, x: T) => Array.from({ length: n }, () => x);

describe("state machine", () => {
  it("flapping checks never change the status", () => {
    const flap = Array.from({ length: 12 }, (_, i) => (i % 2 ? { cf: none, sb: none } : { cf: api, sb: api }));
    const { statuses } = run([{ cf: none, sb: none }, ...flap]);
    expect(statuses.map((s) => s.api)).toEqual(rep(13, "operational"));
    // two in a row is still not enough
    const { statuses: s2 } = run([{ cf: none, sb: none }, { cf: api, sb: api }, { cf: api, sb: api }, { cf: none, sb: none }]);
    expect(s2.map((s) => s.api)).toEqual(rep(4, "operational"));
  });

  it("goes down after 3 Cloudflare failures confirmed by the last 2 standby reports", () => {
    const { statuses } = run([{ cf: none, sb: none }, { cf: api, sb: none }, { cf: api, sb: api }, { cf: api, sb: api }]);
    // third Cloudflare failure (tick 3) with standby failing on ticks 2 and 3
    expect(statuses.map((s) => s.api)).toEqual(["operational", "operational", "operational", "down"]);
  });

  it("only one vantage failing is degraded, not down", () => {
    const { statuses } = run([{ cf: none, sb: none }, ...rep(6, { cf: api, sb: none })]);
    expect(statuses.at(-1)!.api).toBe("degraded");
    const sbOnly = run([{ cf: none, sb: none }, ...rep(6, { cf: none, sb: api })]);
    expect(sbOnly.statuses.map((s) => s.api)).toEqual([...rep(3, "operational"), ...rep(4, "degraded")]);
  });

  it("stale standby needs 5 Cloudflare failures for down", () => {
    // standby reported once, then went silent (stale after 3 min)
    const { statuses } = run([{ cf: none, sb: none }, ...rep(6, { cf: api, sb: null })]);
    expect(statuses.map((s) => s.api)).toEqual([
      "operational", "operational", "operational", "degraded", "degraded", "down", "down",
    ]);
  });

  it("recovers only after 3 passes on both vantages", () => {
    const down = run([{ cf: none, sb: none }, ...rep(3, { cf: api, sb: api })]);
    expect(down.statuses.at(-1)!.api).toBe("down");
    const two = run(rep(2, { cf: none, sb: none }), down.last);
    expect(two.statuses.map((s) => s.api)).toEqual(["down", "down"]);
    const three = run(rep(3, { cf: none, sb: none }), down.last);
    expect(three.statuses.map((s) => s.api)).toEqual(["down", "down", "operational"]);
    // Cloudflare passes but standby still fails: not recovered to operational
    const half = run(rep(5, { cf: none, sb: api }), down.last);
    expect(half.statuses.at(-1)!.api).not.toBe("operational");
  });

  it("median latency over 3 s for 5 ticks is degraded, and recovers", () => {
    const four = run([{ cf: none, sb: none }, ...rep(4, { cf: none, sb: none, latency: 3500 })]);
    expect(four.statuses.at(-1)!.api).toBe("operational");
    const five = run([{ cf: none, sb: none }, ...rep(5, { cf: none, sb: none, latency: 3500 })]);
    expect(five.statuses.at(-1)!.api).toBe("degraded");
    const back = run(rep(3, { cf: none, sb: none, latency: 200 }), five.last);
    expect(back.statuses.map((s) => s.api)).toEqual(["degraded", "degraded", "operational"]);
  });

  it("ready failing while live passes is degraded", () => {
    const ready: Fail = new Set(["api-ready"]);
    const { statuses } = run([{ cf: none, sb: none }, ...rep(5, { cf: ready, sb: ready })]);
    expect(statuses.at(-1)!.api).toBe("degraded");
  });

  it("non-critical targets and max_status components never go past degraded", () => {
    const f: Fail = new Set(["docs", "labs"]);
    const { statuses } = run([{ cf: none, sb: none }, ...rep(8, { cf: f, sb: f })]);
    expect(statuses.at(-1)).toMatchObject({ docs: "degraded", other: "degraded", api: "operational" });
  });

  it("records last_change only on a real transition", () => {
    const { last } = run([{ cf: none, sb: none }, ...rep(3, { cf: api, sb: api })]);
    expect(last.get("api")!.last_change).toBe(new Date(T0 + 3 * 60_000).toISOString());
    expect(last.get("docs")!.last_change).toBeNull();
  });
});
