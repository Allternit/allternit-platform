import { createHmac } from "node:crypto";
import { beforeEach, describe, expect, it } from "vitest";
import type { IncidentsDoc, StatusDoc } from "../contract/types";
import type { Mailer } from "../src/alerts";
import { type Env, handleFetch, runTick } from "../src/app";
import { TARGETS } from "../src/targets";
import { TestD1 } from "./d1";

const SECRET = "test-ingest-secret";
const TOKEN = "test-admin-token-0123456789";
const T0 = Date.parse("2026-11-12T10:00:00Z");
const at = (min: number) => new Date(T0 + min * 60_000);

let db: TestD1;
let env: Env;
let mail: { subject: string; text: string }[];
let failing: Set<string>; // target ids that fail
const mailer: Mailer = async (subject, text) => {
  mail.push({ subject, text });
  return "sent";
};
// Several targets can share one URL (e.g. cloud computers "reachable" + "healthy"),
// so a healthy response carries every marker for that URL, as the real one does.
const byUrl = new Map<string, (typeof TARGETS.targets)[number][]>();
for (const t of TARGETS.targets) byUrl.set(t.url, [...(byUrl.get(t.url) ?? []), t]);
const fetcher = async (url: string) => {
  const ts = byUrl.get(url)!;
  if (ts.some((t) => failing.has(t.id))) return new Response("bad gateway", { status: 502 });
  return new Response(ts.map((t) => t.body_marker ?? "").join(" "), { status: ts[0].expect_status });
};
const tick = (min: number) => runTick(env, at(min), { fetcher, mailer });

/** The standby vantage reports through the real signed ingest route. */
async function standby(min: number) {
  const body = JSON.stringify({
    schema: "allternit.ingest/v1",
    vantage: "allternit-standby",
    ts: at(min).toISOString(),
    results: TARGETS.targets.map((t) => ({ target_id: t.id, ok: !failing.has(t.id), status_code: failing.has(t.id) ? 502 : t.expect_status, latency_ms: 90 })),
  });
  const sig = `sha256=${createHmac("sha256", SECRET).update(body).digest("hex")}`;
  const r = await handleFetch(new Request("https://status.test/api/v1/ingest", { method: "POST", body, headers: { "x-allternit-signature": sig } }), env, undefined, at(min));
  expect(r.status).toBe(202);
}
const both = async (min: number) => {
  await standby(min);
  return tick(min);
};

beforeEach(() => {
  db = new TestD1();
  env = { DB: db, INGEST_HMAC_SECRET: SECRET, ADMIN_TOKEN: TOKEN, AUTO_INCIDENTS: "on" };
  mail = [];
  failing = new Set();
});

const get = async <T,>(path: string, now = at(0)) => {
  const r = await handleFetch(new Request(`https://status.test${path}`), env, undefined, now);
  return { status: r.status, headers: r.headers, body: (await r.json()) as T };
};
const admin = (method: string, path: string, body?: unknown, token: string | null = TOKEN, now = at(0)) =>
  handleFetch(
    new Request(`https://status.test${path}`, {
      method,
      headers: { ...(token ? { authorization: `Bearer ${token}` } : {}), "content-type": "application/json", "cf-connecting-ip": "203.0.113.9" },
      body: body === undefined ? undefined : JSON.stringify(body),
    }),
    env,
    undefined,
    now,
  );

describe("ingest HMAC", () => {
  const payload = (ts: string) =>
    JSON.stringify({ schema: "allternit.ingest/v1", vantage: "allternit-standby", ts, results: [{ target_id: "api-live", ok: true, status_code: 200, latency_ms: 120, marker_ok: true, error: null }] });
  const sign = (body: string, secret = SECRET) => `sha256=${createHmac("sha256", secret).update(body).digest("hex")}`;
  const post = (body: string, sig: string | null, now = at(0)) =>
    handleFetch(
      new Request("https://status.test/api/v1/ingest", { method: "POST", body, headers: sig ? { "x-allternit-signature": sig } : {} }),
      env,
      undefined,
      now,
    );

  it("accepts a correctly signed report and stores it as a standby tick", async () => {
    const body = payload(at(0).toISOString());
    const r = await post(body, sign(body));
    expect(r.status).toBe(202);
    const rows = db.rows<{ vantage: string; minute: string }>("SELECT vantage, minute FROM ticks");
    expect(rows).toEqual([{ vantage: "allternit-standby", minute: "2026-11-12T10:00" }]);
  });

  it("rejects a wrong secret, a tampered body, a missing header and an old timestamp", async () => {
    const body = payload(at(0).toISOString());
    expect((await post(body, sign(body, "other-secret"))).status).toBe(401);
    expect((await post(body.replace("120", "121"), sign(body))).status).toBe(401);
    expect((await post(body, null)).status).toBe(401);
    expect((await post(body, "sha256=zz")).status).toBe(401);
    const old = payload(at(-6).toISOString());
    expect((await post(old, sign(old))).status).toBe(400);
    expect(db.rows("SELECT * FROM ticks")).toEqual([]);
  });
});

describe("admin auth", () => {
  it("401 without or with a wrong token, audit rows written, then 429", async () => {
    expect((await admin("POST", "/api/admin/incidents", {}, null)).status).toBe(401);
    expect((await admin("GET", "/api/admin/incidents", undefined, "wrong")).status).toBe(401);
    expect(db.rows<{ action: string; ip: string }>("SELECT action, ip FROM audit")).toEqual([
      { action: "auth_fail", ip: "203.0.113.9" },
      { action: "auth_fail", ip: "203.0.113.9" },
    ]);
    for (let i = 0; i < 8; i++) await admin("GET", "/api/admin/incidents", undefined, "wrong");
    expect((await admin("GET", "/api/admin/incidents")).status).toBe(429); // even the right token, for 15 min
    expect((await admin("GET", "/api/admin/incidents", undefined, TOKEN, at(16))).status).toBe(200);
  });

  it("503 when ADMIN_TOKEN is not set", async () => {
    env.ADMIN_TOKEN = undefined;
    expect((await admin("GET", "/api/admin/incidents")).status).toBe(503);
  });
});

describe("incidents", () => {
  it("a manual edit survives the automatic resolve; automation only appends Resolved", async () => {
    await both(0);
    failing = new Set(["api-live", "api-ready"]);
    for (let m = 1; m <= 3; m++) await both(m); // 3 Cloudflare failures + 2 standby failures -> down
    let status = (await get<StatusDoc>("/api/v1/status.json", at(7))).body;
    expect(status.components.find((c) => c.id === "api")!.status).toBe("down");
    expect(status.overall).toBe("down");
    const inc = status.incidents[0];
    expect(inc).toMatchObject({ title: "API is down", auto: true, state: "open", component_ids: ["api"] });

    const patch = await admin("PATCH", `/api/admin/incidents/${inc.id}`, {
      schema: "allternit.admin.incident.patch/v1",
      title: "API outage after database failover",
      status: "identified",
      body: "The database failed over. We are moving traffic back.",
    });
    expect(patch.status).toBe(200);

    failing = new Set();
    for (let m = 6; m <= 8; m++) await both(m);
    status = (await get<StatusDoc>("/api/v1/status.json", at(8))).body;
    expect(status.components.find((c) => c.id === "api")!.status).toBe("operational");
    expect(status.incidents).toEqual([]);

    const all = (await get<IncidentsDoc>("/api/v1/incidents.json", at(8))).body.incidents;
    expect(all).toHaveLength(1);
    expect(all[0]).toMatchObject({ title: "API outage after database failover", auto: false, state: "resolved", resolved_at: at(8).toISOString() });
    expect(all[0].updates.map((u) => [u.status, u.body])).toEqual([
      ["investigating", "Our checks cannot reach API. We are looking into it."],
      ["identified", "The database failed over. We are moving traffic back."],
      ["resolved", "API is working again. Our checks have passed for the last few minutes."],
    ]);
    expect(db.rows<{ action: string }>("SELECT action FROM audit WHERE actor = 'admin'")).toEqual([{ action: "incident.update" }]);
  });

  it("AUTO_INCIDENTS=off keeps auto incidents as admin-only drafts until published", async () => {
    env.AUTO_INCIDENTS = "off";
    await both(0);
    failing = new Set(["signin"]);
    for (let m = 1; m <= 5; m++) await both(m);
    expect((await get<StatusDoc>("/api/v1/status.json", at(5))).body.incidents).toEqual([]);
    expect((await get<IncidentsDoc>("/api/v1/incidents.json", at(5))).body.incidents).toEqual([]);
    const list = (await (await admin("GET", "/api/admin/incidents", undefined, TOKEN, at(5))).json()) as { incidents: { id: string; draft: boolean; title: string }[] };
    expect(list.incidents).toMatchObject([{ draft: true, title: "Sign-in is down" }]);
    expect(mail.map((m) => m.subject)).toEqual(["[Allternit status] Down: Sign-in"]); // alerts are on regardless
    expect(mail[0].text).toContain("/admin");

    expect((await admin("POST", `/api/admin/incidents/${list.incidents[0].id}/publish`, undefined, TOKEN, at(5))).status).toBe(200);
    const pub = (await get<StatusDoc>("/api/v1/status.json", at(6))).body.incidents;
    expect(pub.map((i) => i.title)).toEqual(["Sign-in is down"]);
  });

  it("AUTO_INCIDENTS set to a time keeps drafts before it and publishes after it", async () => {
    env.AUTO_INCIDENTS = at(12).toISOString(); // soak ends at minute 12
    await both(0);
    failing = new Set(["signin"]);
    for (let m = 1; m <= 5; m++) await both(m); // Down during the soak: draft
    failing = new Set();
    for (let m = 6; m <= 11; m++) await both(m);
    failing = new Set(["api-ready", "api-live"]);
    for (let m = 12; m <= 17; m++) await both(m); // Down after the soak: public
    const list = (await (await admin("GET", "/api/admin/incidents", undefined, TOKEN, at(17))).json()) as { incidents: { draft: boolean; title: string }[] };
    expect(list.incidents.filter((i) => i.draft).map((i) => i.title)).toEqual(["Sign-in is down"]);
    const pub = (await get<IncidentsDoc>("/api/v1/incidents.json", at(17))).body.incidents;
    expect(pub.map((i) => i.title)).toEqual(["API is down"]);
  });

  it("admin opens an incident and schedules maintenance; both show in the public JSON", async () => {
    await tick(0);
    const r = await admin("POST", "/api/admin/incidents", {
      schema: "allternit.admin.incident.create/v1", title: "Slow sign-in for some users", impact: "degraded", component_ids: ["signin"], body: "Some sign-ins take longer than normal.",
    });
    expect(r.status).toBe(201);
    const m = await admin("POST", "/api/admin/maintenance", {
      schema: "allternit.admin.maintenance.create/v1", title: "Database upgrade", component_ids: ["api"], body: "The API may be slow.",
      starts_at: at(10).toISOString(), ends_at: at(40).toISOString(),
    });
    expect(m.status).toBe(201);
    let s = (await get<StatusDoc>("/api/v1/status.json", at(1))).body;
    expect(s.incidents.map((i) => i.title)).toEqual(["Slow sign-in for some users"]); // maintenance not started yet
    s = (await get<StatusDoc>("/api/v1/status.json", at(15))).body;
    expect(s.components.find((c) => c.id === "api")!.status).toBe("maintenance");
    // the window closes itself
    await tick(41);
    const inc = (await get<IncidentsDoc>("/api/v1/incidents.json", at(41))).body.incidents.find((i) => i.title === "Database upgrade")!;
    expect(inc).toMatchObject({ state: "resolved", resolved_at: at(40).toISOString() });
    expect(inc.updates.at(-1)!.body).toBe("The maintenance is complete.");
    // automation never resolves an incident a human opened
    expect((await get<StatusDoc>("/api/v1/status.json", at(41))).body.incidents.map((i) => i.title)).toEqual(["Slow sign-in for some users"]);
  });
});

describe("alerts", () => {
  it("standby silent: Degraded at 3 failures, Down at 5, a reminder every 60 min, then Recovered", async () => {
    await tick(0);
    failing = new Set(["phone-push"]);
    for (let m = 1; m <= 70; m++) await tick(m);
    failing = new Set();
    for (let m = 71; m <= 73; m++) await tick(m);
    expect(mail.map((m) => m.subject)).toEqual([
      "[Allternit status] Degraded: Phone Link and push",
      "[Allternit status] Down: Phone Link and push",
      "[Allternit status] Still down: Phone Link and push",
      "[Allternit status] Recovered: Phone Link and push",
    ]);
  });

  it("a failed send is retried on the next tick", async () => {
    let calls = 0;
    const flaky: Mailer = async (subject) => {
      calls++;
      if (calls === 1) return "failed";
      mail.push({ subject, text: "" });
      return "sent";
    };
    await runTick(env, at(0), { fetcher, mailer: flaky });
    failing = new Set(["docs"]);
    for (let m = 1; m <= 5; m++) await runTick(env, at(m), { fetcher, mailer: flaky });
    expect(mail.map((m) => m.subject)).toEqual(["[Allternit status] Still degraded: Docs"]);
  });
});

describe("daily rollup", () => {
  it("runs once at 00:05 UTC: worst status, uptime, uptime_90d, history", async () => {
    // Yesterday: 100 ticks, API down for 10 of them, degraded for 5.
    const ins = db.raw.prepare("INSERT INTO ticks (vantage, minute, ts, results, states) VALUES ('cloudflare', ?, ?, '[]', ?)");
    for (let i = 0; i < 100; i++) {
      const d = new Date(Date.parse("2026-11-11T12:00:00Z") + i * 60_000);
      const api = i < 10 ? "down" : i < 15 ? "degraded" : "operational";
      ins.run(d.toISOString().slice(0, 16), d.toISOString(), JSON.stringify({ api, docs: "operational" }));
    }
    const s1 = await runTick(env, new Date("2026-11-12T00:04:00Z"), { fetcher, mailer });
    expect(s1.rollup).toBeNull();
    const s2 = await runTick(env, new Date("2026-11-12T00:05:00Z"), { fetcher, mailer });
    expect(s2.rollup).toBe("2026-11-11");
    const s3 = await runTick(env, new Date("2026-11-12T00:06:00Z"), { fetcher, mailer });
    expect(s3.rollup).toBeNull();

    expect(db.rows("SELECT component_id, date, status, uptime, checks, up_checks FROM daily WHERE date = '2026-11-11' ORDER BY component_id")).toEqual([
      { component_id: "api", date: "2026-11-11", status: "down", uptime: 0.9, checks: 100, up_checks: 90 },
      { component_id: "docs", date: "2026-11-11", status: "operational", uptime: 1, checks: 100, up_checks: 100 },
    ]);
    const status = (await get<StatusDoc>("/api/v1/status.json", new Date("2026-11-12T00:07:00Z"))).body;
    expect(status.components.find((c) => c.id === "api")!.uptime_90d).toBeCloseTo(93 / 103, 10); // 100 ticks yesterday + 3 healthy ticks today
    expect(status.components.find((c) => c.id === "cloud-computers")).toMatchObject({ status: "operational", uptime_90d: 1 });

    const h = await get<{ days: number; components: { id: string; days: { date: string; status: string | null; uptime: number | null }[] }[] }>(
      "/api/v1/history.json?days=3", new Date("2026-11-12T00:07:00Z"),
    );
    expect(h.headers.get("cache-control")).toBe("public, max-age=30");
    expect(h.headers.get("access-control-allow-origin")).toBe("*");
    expect(h.body.components.find((c) => c.id === "api")!.days).toEqual([
      { date: "2026-11-10", status: null, uptime: null },
      { date: "2026-11-11", status: "down", uptime: 0.9 },
      { date: "2026-11-12", status: "operational", uptime: 1 },
    ]);
  });

  it("fills today's cell and uptime before the rollup has run", async () => {
    await both(0);
    failing = new Set(["api-live", "api-ready"]);
    for (let m = 1; m <= 5; m++) await both(m); // enough failures to go down
    failing = new Set();
    await both(6);
    const h = (await get<{ components: { id: string; days: { date: string; status: string | null; uptime: number | null }[] }[] }>("/api/v1/history.json?days=2", at(7))).body;
    const api = h.components.find((c) => c.id === "api")!.days;
    expect(api[1].date).toBe("2026-11-12");
    expect(api[1].status).toBe("down"); // worst so far
    const row = db.rows("SELECT checks, up_checks FROM daily WHERE component_id = 'api' AND date = '2026-11-12'")[0] as { checks: number; up_checks: number };
    expect(row.checks).toBe(7);
    expect(row.up_checks).toBeLessThan(7);
    expect(api[1].uptime).toBeCloseTo(row.up_checks / 7, 10);
    const st = (await get<StatusDoc>("/api/v1/status.json", at(7))).body;
    expect(st.components.find((c) => c.id === "api")!.uptime_90d).toBeCloseTo(row.up_checks / 7, 10);
  });

  it("prunes ticks older than 14 days", async () => {
    db.raw.prepare("INSERT INTO ticks (vantage, minute, ts, results, states) VALUES ('cloudflare', '2026-10-01T00:00', '2026-10-01T00:00:00Z', '[]', '{}')").run();
    await runTick(env, new Date("2026-11-12T00:05:00Z"), { fetcher, mailer });
    expect(db.rows("SELECT minute FROM ticks WHERE minute < '2026-11-01'")).toEqual([]);
  });
});

describe("health", () => {
  it("ok within 300 s of the last tick, 503 stale after", async () => {
    await tick(0);
    const ok = await get<{ status: string; age_seconds: number }>("/api/health", new Date(T0 + 300_000));
    expect([ok.status, ok.body.status, ok.headers.get("cache-control")]).toEqual([200, "ok", "no-store"]);
    const stale = await get<{ status: string }>("/api/health", new Date(T0 + 301_000));
    expect([stale.status, stale.body.status]).toEqual([503, "stale"]);
  });
});
