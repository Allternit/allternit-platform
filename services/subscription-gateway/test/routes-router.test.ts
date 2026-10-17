// Deterministic route router (spec §3–§6): the selection table, the
// conformance store keyed by app version, binding + between-turn failover,
// the typed NO_ROUTE, the /v1/routes API shape (a contract for the Settings
// UI task), conformance recording through the real /aai/conformance route,
// and the AAI turn-start seam. Phone device shapes mirror Desktop's real
// `phone.devices` response (link:<id> serials, route/health fields).
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import request from "supertest";
import { existsSync } from "node:fs";
import { join } from "node:path";
import { pathToFileURL } from "node:url";
import { fail, ok, type AaiProvider, type AaiResult } from "@allternit/agent-gateway";
import type { Account, BotExecutionBinding } from "@allternit/subscription-fabric-contracts";
import { AaiHost } from "../src/aai/registry.js";
import { defaultAdaptersDir } from "../src/adapters/registry.js";
import { createServer } from "../src/http/server.js";
import { issueToken } from "../src/security/tokens.js";
import { openDatabase, type Db } from "../src/store/db.js";
import { upsertAccount } from "../src/store/queries.js";
import {
  RouteDirector,
  type RouteChoiceLogEntry,
  type VendorLaneInfo,
} from "../src/routes/director.js";
import {
  CONTRACT_ITEM_IDS,
  contractFromReport,
  type ContractResult,
} from "../src/routes/contract.js";
import { contractForVersion, recordContractResult } from "../src/routes/store.js";
import {
  ROUTE_ORDER,
  phoneDeviceOnline,
  routeForPhoneDevice,
  type RouteId,
} from "../src/routes/registry.js";
import { selectRoute, type RouteCandidateState } from "../src/routes/select.js";
import { cleanupDir, makeDeps, tmpStateDir, type TestDeps } from "./helpers.js";

// ---------------------------------------------------------------------------
// fixtures
// ---------------------------------------------------------------------------

function allPassedResult(appVersion: string, checkedAt = "2026-10-17T00:00:00.000Z"): ContractResult {
  const items = CONTRACT_ITEM_IDS.map((item) => ({
    item,
    passed: true,
    reason: "",
    evidence: [`${item} proven by route conformance`],
  }));
  return {
    appVersion,
    checkedAt,
    items,
    passed: [...CONTRACT_ITEM_IDS],
    failed: [],
    ok: true,
  };
}

function candidate(route: RouteId, over: Partial<RouteCandidateState> = {}): RouteCandidateState {
  return { route, eligible: true, signedIn: true, online: true, ...over };
}

function allCandidates(over: Partial<RouteCandidateState> = {}): RouteCandidateState[] {
  return ROUTE_ORDER.map((route) => candidate(route, over));
}

function readyAccount(provider: string, id = `${provider}-acct`): Account {
  return {
    account_id: id,
    provider: provider as Account["provider"],
    label: provider,
    plan: null,
    plan_observed_at: null,
    profile_ref: `profiles/${id}`,
    session_health: "ready",
    enabled: true,
    identity: null,
    usage: null,
    agents: null,
    preferred: false,
  };
}

function lanesFor(vendor: string, routes: RouteId[], appVersion = "1.0.0", online = true): VendorLaneInfo[] {
  return [{ vendor, adapterId: `${vendor}-adapter`, appVersion, routes, online }];
}

// ---------------------------------------------------------------------------
// §4.3 selection — the table: every order/override/eligibility combination
// that matters. Pure: same inputs → same outputs.
// ---------------------------------------------------------------------------

describe("selectRoute (§4)", () => {
  it("picks the first eligible route in the FIXED order, not the input order", () => {
    const reversed = [...allCandidates()].reverse();
    const r = selectRoute({ vendor: "acme", candidates: reversed });
    expect(r).toEqual({ ok: true, route: "android-app", reason: expect.stringContaining("fixed order") });
  });

  it("skips ineligible routes to the next eligible one in the order", () => {
    const r = selectRoute({
      vendor: "acme",
      candidates: allCandidates({ eligible: false }).map((c) =>
        c.route === "desktop-app-ax" ? { ...c, eligible: true } : c
      ),
    });
    expect(r.ok && r.route).toBe("desktop-app-ax");
  });

  it("honours an eligible override even when a higher-order route is eligible", () => {
    const r = selectRoute({ vendor: "acme", candidates: allCandidates(), override: "web" });
    expect(r).toEqual({ ok: true, route: "web", reason: expect.stringContaining("override") });
  });

  it("an ineligible override falls through to the default order, saying why", () => {
    const r = selectRoute({
      vendor: "acme",
      candidates: allCandidates().map((c) => (c.route === "web" ? { ...c, eligible: false, online: false } : c)),
      override: "web",
    });
    expect(r.ok && r.route).toBe("android-app");
    expect(r.ok && r.reason).toContain("override web ineligible (offline)");
  });

  it("an ineligible override with nothing else eligible is NO_ROUTE", () => {
    const r = selectRoute({
      vendor: "acme",
      candidates: allCandidates({ eligible: false, online: false }),
      override: "web",
    });
    expect(r.ok).toBe(false);
    if (!r.ok) {
      expect(r.error).toBe("NO_ROUTE");
      expect(r.reason).toContain("override web ineligible (offline)");
    }
  });

  it("NO_ROUTE with nothing eligible: plain per-route reasons (§4.5)", () => {
    const r = selectRoute({
      vendor: "acme",
      candidates: [
        candidate("android-app", { eligible: false, online: false }),
        candidate("iphone-app-ictrl", { eligible: false, signedIn: false }),
        candidate("web", { eligible: false, online: true, signedIn: true }),
      ],
    });
    expect(r).toEqual({
      ok: false,
      error: "NO_ROUTE",
      reason:
        "android-app: offline; iphone-app-ictrl: not signed in; desktop-app-cdp: not offered; desktop-app-ax: not offered; web: contract not passed; iphone-app-usb-agent: not offered; no eligible route",
    });
  });

  it("eligible is authoritative at this layer; the director ANDs the flags upstream", () => {
    // selectRoute trusts the eligible flag verbatim (the RouteDirector
    // computes it as signedIn && online && contract.ok).
    const trusted = selectRoute({ vendor: "acme", candidates: [candidate("web", { signedIn: false })] });
    expect(trusted.ok && trusted.route).toBe("web");
    for (const over of [
      { eligible: false, signedIn: false },
      { eligible: false, online: false },
      { eligible: false, signedIn: false, online: false },
    ] as Array<Partial<RouteCandidateState>>) {
      const r = selectRoute({ vendor: "acme", candidates: [candidate("web", over)] });
      expect(r.ok).toBe(false);
    }
  });

  it("null override behaves like no override", () => {
    const a = selectRoute({ vendor: "acme", candidates: allCandidates(), override: null });
    const b = selectRoute({ vendor: "acme", candidates: allCandidates() });
    expect(a).toEqual(b);
  });

  it("same inputs → same output (determinism, §4.3)", () => {
    const input = { vendor: "acme", candidates: allCandidates(), override: "web" as const };
    expect(JSON.stringify(selectRoute(input))).toBe(JSON.stringify(selectRoute(input)));
  });

  it("a custom order is respected verbatim", () => {
    const r = selectRoute({
      vendor: "acme",
      candidates: allCandidates(),
      order: ["web", "android-app"],
    });
    expect(r.ok && r.route).toBe("web");
  });
});

// ---------------------------------------------------------------------------
// Phone device shapes — mirrored from Desktop's real phone.devices output
// ---------------------------------------------------------------------------

describe("phone device → route mapping (real phone.devices shapes)", () => {
  it("classifies the three linked-device shapes", () => {
    expect(
      routeForPhoneDevice({ serial: "link:dev-1", platform: "android", via: "phone-link", route: "android-app" })
    ).toBe("android-app");
    expect(
      routeForPhoneDevice({ serial: "link:dev-2", platform: "ios", via: "phone-link", route: "iphone-app-ictrl" })
    ).toBe("iphone-app-ictrl");
    expect(
      routeForPhoneDevice({ serial: "usb:abc123", platform: "ios", via: "usb-agent" })
    ).toBe("iphone-app-usb-agent");
    expect(routeForPhoneDevice({ serial: "10.0.0.5:5555", platform: "android" })).toBeNull();
  });

  it("online gate matches phone-link-health.ts (HEALTHY/DEVICE_LOCKED/unchecked online)", () => {
    expect(phoneDeviceOnline({ serial: "link:a", platform: "ios" })).toBe(true);
    expect(phoneDeviceOnline({ serial: "link:a", platform: "ios", health: { state: "HEALTHY" } })).toBe(true);
    expect(phoneDeviceOnline({ serial: "link:a", platform: "ios", health: { state: "DEVICE_LOCKED" } })).toBe(true);
    expect(phoneDeviceOnline({ serial: "link:a", platform: "ios", health: { state: "TRANSPORT_OFFLINE" } })).toBe(false);
    expect(phoneDeviceOnline({ serial: "link:a", platform: "ios", health: { state: "PAIRING_INVALID" } })).toBe(false);
    expect(phoneDeviceOnline({ serial: "link:a", platform: "ios", health: { state: "HELPER_NOT_RESPONDING" } })).toBe(false);
  });
});

// ---------------------------------------------------------------------------
// §3 contract mapping from a ConformanceReport (real check names)
// ---------------------------------------------------------------------------

function reportWith(areas: Array<{ area: string; checks: Array<{ name: string; status: string; reason?: string }> }>) {
  const all = areas.flatMap((a) => a.checks);
  return {
    adapterId: "x",
    guarantee: "best_effort",
    lane: "ui_bridge",
    ok: all.every((c) => c.status !== "fail"),
    areas: areas.map((a) => ({
      area: a.area,
      status: a.checks.some((c) => c.status === "fail") ? "fail" : "pass",
      reasons: [],
      checks: a.checks,
    })),
    summary: {
      pass: all.filter((c) => c.status === "pass").length,
      fail: all.filter((c) => c.status === "fail").length,
      skippedUnsupported: 0,
    },
  } as const;
}

const PASSING_REPORT = reportWith([
  { area: "context", checks: [
    { name: "send", status: "pass" },
    { name: "terminate: close", status: "pass" },
    { name: "send after close", status: "pass" },
  ] },
  { area: "events", checks: [
    { name: "events schema-valid", status: "pass" },
    { name: "events have cursors", status: "pass" },
    { name: "events reflect sent message (correlationId)", status: "pass" },
  ] },
  { area: "cancellation", checks: [{ name: "cancel", status: "pass" }] },
  { area: "idempotency", checks: [{ name: "replay returns the first result", status: "pass" }] },
  { area: "resources", checks: [
    { name: "agent.tasks", status: "pass" },
    { name: "agent.artifacts", status: "pass" },
  ] },
  { area: "failure", checks: [
    { name: "vendor_down: maps to VENDOR_UNAVAILABLE", status: "pass" },
    { name: "rate_limited: maps to RATE_LIMITED", status: "pass" },
    { name: "auth_revoked: maps to AUTH_REVOKED|AUTH_REQUIRED", status: "pass" },
    { name: "account_banned: maps to LANE_BLOCKED|POLICY_DENIED", status: "pass" },
  ] },
]);

describe("contractFromReport (§3)", () => {
  it("a fully green report still fails attachments.send — never assumed", () => {
    const r = contractFromReport(PASSING_REPORT as never, { appVersion: "1.0.0", checkedAt: "2026-10-17T00:00:00.000Z" });
    expect(r.passed).toEqual([
      "chat.start_continue",
      "reply.stream_read",
      "reply.stop_midstream",
      "outputs.exact_and_files",
      "threads.list_reopen",
      "errors.typed_states",
    ]);
    expect(r.failed).toEqual(["attachments.send"]);
    expect(r.ok).toBe(false);
    expect(r.items.find((i) => i.item === "attachments.send")?.reason).toContain("not covered");
  });

  it("a missing check fails its item as 'check not run', not a pass", () => {
    const partial = reportWith([
      { area: "context", checks: [{ name: "send", status: "pass" }] }, // no "terminate: close"
    ]);
    const r = contractFromReport(partial as never, { appVersion: "1.0.0" });
    const item = r.items.find((i) => i.item === "chat.start_continue");
    expect(item?.passed).toBe(false);
    expect(item?.reason).toContain("check not run");
  });

  it("a failed check fails its item with the reason", () => {
    const bad = reportWith([
      { area: "cancellation", checks: [{ name: "cancel", status: "fail", reason: "cancel returned VENDOR_UNAVAILABLE" }] },
    ]);
    const r = contractFromReport(bad as never, { appVersion: "1.0.0" });
    const item = r.items.find((i) => i.item === "reply.stop_midstream");
    expect(item?.passed).toBe(false);
    expect(item?.reason).toContain("cancel returned VENDOR_UNAVAILABLE");
  });

  it("a skipped fault check (bare kind name) is 'check not run', not a pass", () => {
    const skipped = reportWith([
      { area: "failure", checks: [{ name: "vendor_down", status: "skipped", reason: "no fault fixture" }] },
    ]);
    const r = contractFromReport(skipped as never, { appVersion: "1.0.0" });
    const item = r.items.find((i) => i.item === "errors.typed_states");
    expect(item?.passed).toBe(false);
    expect(item?.reason).toContain("failure: vendor_down: maps to VENDOR_UNAVAILABLE — check not run");
  });
});

// ---------------------------------------------------------------------------
// Store: conformance keyed by app version (§3: re-run when the app updates)
// ---------------------------------------------------------------------------

describe("conformance store keyed by app version", () => {
  let db: Db;
  beforeEach(() => { db = openDatabase(":memory:"); });
  afterEach(() => { db.close(); });

  it("records and reads back per (vendor, route, appVersion)", () => {
    recordContractResult(db, "acme", "web", "1.0.0", allPassedResult("1.0.0"));
    const r = contractForVersion(db, "acme", "web", "1.0.0");
    expect(r?.ok).toBe(true);
    expect(r?.appVersion).toBe("1.0.0");
    expect(r?.checkedAt).toBe("2026-10-17T00:00:00.000Z");
  });

  it("a new app version reads as absent — ineligible until the suite re-runs", () => {
    recordContractResult(db, "acme", "web", "1.0.0", allPassedResult("1.0.0"));
    expect(contractForVersion(db, "acme", "web", "2.0.0")).toBeNull();
    // the old version's rows stay for history
    expect(contractForVersion(db, "acme", "web", "1.0.0")?.ok).toBe(true);
  });

  it("re-recording the same version replaces the items", () => {
    recordContractResult(db, "acme", "web", "1.0.0", allPassedResult("1.0.0"));
    const regressed: ContractResult = {
      ...allPassedResult("1.0.0"),
      items: CONTRACT_ITEM_IDS.map((item) => ({
        item,
        passed: item !== "chat.start_continue",
        reason: item === "chat.start_continue" ? "send timed out" : "",
        evidence: [],
      })),
      passed: CONTRACT_ITEM_IDS.filter((i) => i !== "chat.start_continue"),
      failed: ["chat.start_continue"],
      ok: false,
    };
    recordContractResult(db, "acme", "web", "1.0.0", regressed);
    const r = contractForVersion(db, "acme", "web", "1.0.0");
    expect(r?.ok).toBe(false);
    expect(r?.failed).toEqual(["chat.start_continue"]);
  });
});

// ---------------------------------------------------------------------------
// RouteDirector: eligibility, binding, between-turn failover, NO_ROUTE,
// overrides, the §2 probe gate, and choice logging.
// ---------------------------------------------------------------------------

describe("RouteDirector", () => {
  let db: Db;
  let choices: RouteChoiceLogEntry[];
  let state: string;
  let deps: TestDeps;

  const newDirector = (lanes: VendorLaneInfo[], phones?: { linkedDevices(): Promise<Array<Record<string, unknown>>> }) =>
    new RouteDirector({ db, lanes: { lanes: async () => lanes }, phones: phones as never, onChoice: (e) => choices.push(e) });

  beforeEach(() => {
    db = openDatabase(":memory:");
    choices = [];
    state = tmpStateDir();
    deps = makeDeps(state);
  });
  afterEach(() => {
    deps.cleanup();
    cleanupDir(state);
    db.close();
  });

  function eligibleWebSetup(appVersion = "1.0.0") {
    upsertAccount(db, readyAccount("acme"));
    recordContractResult(db, "acme", "web", appVersion, allPassedResult(appVersion));
    return lanesFor("acme", ["web"], appVersion);
  }

  it("binds the first eligible route at turn start and logs the choice", async () => {
    const d = newDirector(eligibleWebSetup());
    const r = await d.routeForTurn("acme");
    expect(r.ok && r.route).toBe("web");
    expect(r.ok && r.rebound).toBe(true);
    expect(choices.at(-1)).toMatchObject({ vendor: "acme", route: "web", kind: "bind" });
    expect(choices.at(-1)?.reason).toContain("fixed order");
  });

  it("a second turn on a healthy binding keeps the route (bound, not re-selected)", async () => {
    const d = newDirector(eligibleWebSetup());
    await d.routeForTurn("acme");
    choices.length = 0;
    const r = await d.routeForTurn("acme");
    expect(r.ok && r.route).toBe("web");
    expect(r.ok && r.rebound).toBe(false);
    expect(choices.at(-1)).toMatchObject({ kind: "turn" });
  });

  it("failover happens only between turns: bound route goes offline → next turn uses the next eligible route", async () => {
    upsertAccount(db, readyAccount("acme"));
    recordContractResult(db, "acme", "web", "1.0.0", allPassedResult("1.0.0"));
    recordContractResult(db, "acme", "desktop-app-cdp", "1.0.0", allPassedResult("1.0.0"));
    const lanes = [
      ...lanesFor("acme", ["desktop-app-cdp"], "1.0.0"),
      ...lanesFor("acme", ["web"], "1.0.0"),
    ];
    const d = newDirector(lanes);
    // desktop routes need the §2 launch-time probe
    d.recordProbe("acme", "desktop-app-cdp", "1.0.0", true, "app accepts its debug flag");
    d.recordProbe("acme", "desktop-app-ax", "1.0.0", false, "cdp applies");

    const t1 = await d.routeForTurn("acme");
    expect(t1.ok && t1.route).toBe("desktop-app-cdp"); // §4.1: cdp before web

    // The phone (bound route's computer) goes offline before the next turn.
    const lanesNowOffline = lanes.map((l) => (l.routes.includes("desktop-app-cdp") ? { ...l, online: false } : l));
    const d2 = newDirector(lanesNowOffline);
    const t2 = await d2.routeForTurn("acme");
    expect(t2.ok && t2.route).toBe("web");
    expect(t2.ok && t2.rebound).toBe(true);
    expect(choices.at(-1)).toMatchObject({ kind: "failover", route: "web" });
  });

  it("an app update makes the bound route ineligible until conformance re-runs (§3)", async () => {
    const lanes = eligibleWebSetup("1.0.0");
    const d = newDirector(lanes);
    const t1 = await d.routeForTurn("acme");
    expect(t1.ok && t1.route).toBe("web");

    // the app updates: adapter now reports 2.0.0, conformance rows are 1.0.0
    const d2 = newDirector(lanesFor("acme", ["web"], "2.0.0"));
    const t2 = await d2.routeForTurn("acme");
    expect(t2.ok).toBe(false);
    if (!t2.ok) expect(t2.error).toBe("NO_ROUTE");
  });

  it("NO_ROUTE when nothing is eligible: typed error, binding cleared, logged", async () => {
    const d = newDirector(eligibleWebSetup());
    await d.routeForTurn("acme");
    const dOffline = newDirector(lanesFor("acme", ["web"], "1.0.0", false));
    const r = await dOffline.routeForTurn("acme");
    expect(r.ok).toBe(false);
    if (!r.ok) {
      expect(r.error).toBe("NO_ROUTE");
      expect(r.reason).toContain("web: offline");
      expect(r.reason).toContain("no eligible route");
    }
    const { getBinding } = await import("../src/routes/store.js");
    expect(getBinding(db, "acme")?.boundRoute).toBeNull();
    expect(choices.at(-1)).toMatchObject({ kind: "no_route", route: null });
  });

  it("§4.6 override: pins an eligible route; refuses an ineligible one (400)", async () => {
    const d = newDirector(eligibleWebSetup());
    const bad = await d.setOverride("acme", "android-app");
    expect(bad).toMatchObject({ ok: false, status: 400 });
    if (!bad.ok) expect(bad.body.error).toBe("route_not_eligible");

    const unknown = await d.setOverride("acme", "not-a-route");
    expect(unknown).toMatchObject({ ok: false, status: 400 });
    if (!unknown.ok) expect(unknown.body.error).toBe("unknown_route");

    const good = await d.setOverride("acme", "web");
    expect(good.ok).toBe(true);
    const t = await d.routeForTurn("acme");
    expect(t.ok && t.route).toBe("web");
    expect(choices.at(-1)).toMatchObject({ kind: "override", route: "web" });

    // clearing the pin
    expect((await d.setOverride("acme", null)).ok).toBe(true);
    const { getOverride } = await import("../src/routes/store.js");
    expect(getOverride(db, "acme")).toBeNull();
  });

  it("§2 probe gate: desktop-app-cdp is not eligible without an applying probe for the current version", async () => {
    upsertAccount(db, readyAccount("acme"));
    recordContractResult(db, "acme", "desktop-app-cdp", "1.0.0", allPassedResult("1.0.0"));
    const lanes = lanesFor("acme", ["desktop-app-cdp"], "1.0.0");

    const noProbe = await newDirector(lanes).status(["acme"]);
    const cdp = noProbe[0].routes.find((r) => r.route === "desktop-app-cdp");
    expect(cdp?.eligible).toBe(false);

    const d = newDirector(lanes);
    d.recordProbe("acme", "desktop-app-cdp", "1.0.0", false, "vendor disabled the debug flag");
    const probedFalse = (await d.status(["acme"]))[0].routes.find((r) => r.route === "desktop-app-cdp");
    expect(probedFalse?.eligible).toBe(false);

    d.recordProbe("acme", "desktop-app-cdp", "1.0.0", true, "app accepts its debug flag unmodified");
    const probedTrue = (await d.status(["acme"]))[0].routes.find((r) => r.route === "desktop-app-cdp");
    expect(probedTrue?.eligible).toBe(true);
  });

  it("phone routes: registered but never assumed eligible (task item 7)", async () => {
    const phones = {
      linkedDevices: async () => [
        { serial: "link:dev-android", platform: "android", via: "phone-link", route: "android-app", health: { state: "HEALTHY", checkedAt: "2026-10-17T00:00:00.000Z", recovery: "automatic" } },
        { serial: "link:dev-iphone", platform: "ios", via: "phone-link", route: "iphone-app-ictrl", health: { state: "TRANSPORT_OFFLINE", checkedAt: "2026-10-17T00:00:00.000Z", recovery: "automatic" } },
      ],
    };
    const d = newDirector([], phones as never);
    upsertAccount(db, readyAccount("acme"));
    const view = (await d.status(["acme"]))[0];
    const android = view.routes.find((r) => r.route === "android-app")!;
    const ictrl = view.routes.find((r) => r.route === "iphone-app-ictrl")!;
    expect(android.signedIn).toBe(true);
    expect(android.online).toBe(true);
    expect(android.eligible).toBe(false);
    expect(android.contract.failed).toEqual(CONTRACT_ITEM_IDS); // nothing proven
    expect(android.contract.appVersion).toBeUndefined();
    expect(ictrl.online).toBe(false); // TRANSPORT_OFFLINE drops the phone
    const r = await d.routeForTurn("acme");
    expect(r.ok).toBe(false);
  });

  it("status view matches the §6 shape exactly (optional keys omitted)", async () => {
    upsertAccount(db, readyAccount("acme"));
    recordContractResult(db, "acme", "web", "1.0.0", allPassedResult("1.0.0"));
    const d = newDirector(lanesFor("acme", ["web"], "1.0.0"));
    await d.routeForTurn("acme");
    const view = (await d.status(["acme"]))[0];
    expect(Object.keys(view).sort()).toEqual(["boundRoute", "routes", "vendor"]);
    const web = view.routes.find((r) => r.route === "web")!;
    expect(Object.keys(web).sort()).toEqual(["contract", "eligible", "online", "route", "signedIn"]);
    expect(Object.keys(web.contract).sort()).toEqual(["appVersion", "checkedAt", "failed", "passed"]);
    expect(web.contract.passed).toEqual([...CONTRACT_ITEM_IDS]);
    expect(web.contract.failed).toEqual([]);
    const android = view.routes.find((r) => r.route === "android-app")!;
    expect(Object.keys(android.contract).sort()).toEqual(["failed", "passed"]); // unchecked: no appVersion/checkedAt
  });
});

// ---------------------------------------------------------------------------
// /v1/routes HTTP API (§6) — the shape the Settings UI builds against
// ---------------------------------------------------------------------------

describe("GET /v1/routes + PUT /v1/routes/:vendor/override", () => {
  let state: string, deps: TestDeps, app: ReturnType<typeof createServer>, tok: string;

  beforeEach(() => {
    state = tmpStateDir();
    deps = makeDeps(state);
    const director = new RouteDirector({ db: deps.db });
    app = createServer({ ...deps, routeDirector: director });
    tok = issueToken(deps.db, "api", "test", ["accounts:manage", "tasks:submit", "tasks:read"]).token;
  });
  afterEach(() => { deps.cleanup(); cleanupDir(state); });

  it("answers 401 without a token", async () => {
    await request(app).get("/v1/routes").expect(401);
  });

  it("empty state: no vendors, routes shape intact", async () => {
    const r = await request(app).get("/v1/routes").set("Authorization", `Bearer ${tok}`).expect(200);
    expect(r.body).toEqual({ vendors: [] });
  });

  it("exact §6 shape for a vendor with everything recorded", async () => {
    upsertAccount(deps.db, readyAccount("acme"));
    recordContractResult(deps.db, "acme", "web", "1.0.0", allPassedResult("1.0.0"));
    const director = new RouteDirector({
      db: deps.db,
      lanes: { lanes: async () => lanesFor("acme", ["web"], "1.0.0") },
    });
    app = createServer({ ...deps, routeDirector: director });

    const r = await request(app).get("/v1/routes").set("Authorization", `Bearer ${tok}`).expect(200);
    expect(r.body.vendors).toHaveLength(1);
    const v = r.body.vendors[0];
    expect(v.vendor).toBe("acme");
    expect(Array.isArray(v.routes)).toBe(true);
    expect(v.routes).toHaveLength(6);
    const web = v.routes.find((x: { route: string }) => x.route === "web");
    expect(web).toEqual({
      route: "web",
      eligible: true,
      signedIn: true,
      online: true,
      contract: {
        passed: [...CONTRACT_ITEM_IDS],
        failed: [],
        appVersion: "1.0.0",
        checkedAt: "2026-10-17T00:00:00.000Z",
      },
    });
    const phone = v.routes.find((x: { route: string }) => x.route === "android-app");
    expect(phone.eligible).toBe(false);
    expect(phone.contract.appVersion).toBeUndefined();
  });

  it("PUT override: pins an eligible route, rejects an ineligible one with 400, null clears", async () => {
    upsertAccount(deps.db, readyAccount("acme"));
    recordContractResult(deps.db, "acme", "web", "1.0.0", allPassedResult("1.0.0"));
    const director = new RouteDirector({
      db: deps.db,
      lanes: { lanes: async () => lanesFor("acme", ["web"], "1.0.0") },
    });
    app = createServer({ ...deps, routeDirector: director });

    const bad = await request(app)
      .put("/v1/routes/acme/override")
      .set("Authorization", `Bearer ${tok}`)
      .send({ route: "android-app" })
      .expect(400);
    expect(bad.body.error).toBe("route_not_eligible");

    const nonsense = await request(app)
      .put("/v1/routes/acme/override")
      .set("Authorization", `Bearer ${tok}`)
      .send({ route: "bogus" })
      .expect(400);
    expect(nonsense.body.error).toBe("unknown_route");

    await request(app)
      .put("/v1/routes/acme/override")
      .set("Authorization", `Bearer ${tok}`)
      .send({ route: "web" })
      .expect(200);

    const after = await request(app).get("/v1/routes").set("Authorization", `Bearer ${tok}`).expect(200);
    expect(after.body.vendors[0].override).toBe("web");

    await request(app)
      .put("/v1/routes/acme/override")
      .set("Authorization", `Bearer ${tok}`)
      .send({ route: null })
      .expect(200);
    const cleared = await request(app).get("/v1/routes").set("Authorization", `Bearer ${tok}`).expect(200);
    expect(cleared.body.vendors[0].override).toBeUndefined();

    await request(app)
      .put("/v1/routes/acme/override")
      .set("Authorization", `Bearer ${tok}`)
      .send({ route: "web", extra: 1 })
      .expect(400); // strict body
  });
});

// ---------------------------------------------------------------------------
// AAI turn-start seam: the director is consulted on open/message only,
// NO_ROUTE answers typed through the AAI error, adapterId pins the route.
// ---------------------------------------------------------------------------

function stubProvider(adapterId: string): AaiProvider & { messages: number } {
  const p: AaiProvider & { messages: number } = {
    adapterId,
    messages: 0,
    list: async () => ok([{ agentId: "agent-1", displayName: "stub", vendor: "stub", state: "ready" }]),
    get: async () => fail("UNSUPPORTED", "no get", { retryable: false }),
    capabilities: async () => fail("UNSUPPORTED", "no capabilities", { retryable: false }),
    identity: async () => fail("UNSUPPORTED", "no identity", { retryable: false }),
    contextOpen: async () => ok({ contextId: "ctx-1", isolation: "shared", guarantee: "best_effort" }),
    contextMessage: async (input) => {
      p.messages += 1;
      return ok({ messageId: `m-${p.messages}`, correlationId: input.correlationId, reply: "hi", guarantee: "best_effort" });
    },
    contextSteer: async () => ok({ accepted: true }),
    contextCancel: async () => ok({ confirmed: true }),
    contextClose: async () => ok({ closed: true }),
    events: async () => ok({ events: [], nextCursor: "0" }),
    tasks: async () => ok([]),
    memory: async () => fail("UNSUPPORTED", "no memory", { retryable: false }),
    computer: async () => fail("UNSUPPORTED", "no computer", { retryable: false }),
    artifacts: async () => ok([]),
    approvals: async () => ok({}),
    snapshot: async () => ok({ agentId: "agent-1", fields: {} }),
    sync: async () => ok({ fields: [] }),
    health: async () => ok({ status: "healthy" }),
  };
  return p;
}

function vendorBinding(vendor: string): BotExecutionBinding {
  return { id: "b-1", botId: "bot-1", type: "vendor", vendor, mode: "hosted", state: "READY" };
}

describe("AaiHost route-director seam (turn start only)", () => {
  it("pins the binding's adapterId to the chosen route and runs the turn on it", async () => {
    const host = new AaiHost();
    const stub = stubProvider("stub-adapter");
    host.register({ provider: stub });
    host.routeDirector = {
      routeForTurn: async () => ({ ok: true, route: "web", adapterId: "stub-adapter", rebound: true, reason: "test" }),
    };
    const r = await host.call("agent.context.message", vendorBinding("stub"), { contextId: "ctx-1", correlationId: "c-1", text: "hello" });
    expect(r.ok).toBe(true);
    expect(stub.messages).toBe(1);
  });

  it("NO_ROUTE answers as a typed error with the reason preserved", async () => {
    const host = new AaiHost();
    host.register({ provider: stubProvider("stub-adapter") });
    host.routeDirector = {
      routeForTurn: async () => ({ ok: false, error: "NO_ROUTE", reason: "phone offline, no other signed-in route" }),
    };
    const r = await host.call("agent.context.message", vendorBinding("stub"), { contextId: "ctx-1", correlationId: "c-1", text: "hello" });
    expect(r.ok).toBe(false);
    if (!r.ok) {
      expect(r.error.code).toBe("LANE_BLOCKED");
      expect(r.error.retryable).toBe(false);
      expect(r.error.details?.no_route).toBe(true);
      expect(r.error.humanMessage).toContain("NO_ROUTE for stub: phone offline, no other signed-in route");
    }
  });

  it("allternit (loopback) bindings and non-turn ops never consult the director", async () => {
    const host = new AaiHost();
    host.register({ provider: stubProvider("allternit-loopback") });
    let consulted = 0;
    host.routeDirector = {
      routeForTurn: async () => {
        consulted += 1;
        return { ok: false, error: "NO_ROUTE", reason: "must not be consulted" };
      },
    };
    const allternitBinding: BotExecutionBinding = { id: "b-2", botId: "bot-1", type: "allternit", mode: "hosted", state: "READY" };
    await host.call("agent.list", allternitBinding, {});
    await host.call("agent.list", vendorBinding("stub"), {});
    expect(consulted).toBe(0);
  });
});

// ---------------------------------------------------------------------------
// /aai/conformance extended per route: a REAL run (chatgpt-dots offline
// fixtures) records contract items per (vendor, route, appVersion).
// ---------------------------------------------------------------------------

describe("POST /aai/conformance/:adapterId records per-route contract items", () => {
  let state: string, deps: TestDeps, app: ReturnType<typeof createServer>, tok: string, closer: (() => Promise<void>) | undefined;

  beforeEach(async () => {
    state = tmpStateDir();
    deps = makeDeps(state);
    tok = issueToken(deps.db, "api", "test", ["accounts:manage", "tasks:submit", "tasks:read"]).token;
    const fixtureFile = join(defaultAdaptersDir(), "chatgpt-dots", "fixtures", "offline.ts");
    if (!existsSync(fixtureFile)) throw new Error("chatgpt-dots offline fixtures missing");
    const mod = (await import(pathToFileURL(fixtureFile).href)) as {
      createOfflineAaiRegistration: () => Promise<{ registration: Parameters<AaiHost["register"]>[0]; close?: () => Promise<void> }>;
    };
    const { registration, close } = await mod.createOfflineAaiRegistration();
    closer = close;
    const host = new AaiHost();
    host.register(registration);
    const director = new RouteDirector({
      db: deps.db,
      lanes: {
        lanes: async () => [
          { vendor: "openai", adapterId: "chatgpt-dots", appVersion: "0.0.0", routes: ["desktop-app-cdp", "desktop-app-ax"], online: true },
        ],
      },
    });
    app = createServer({ ...deps, aai: host, routeDirector: director });
  });
  afterEach(async () => {
    await closer?.();
    deps.cleanup();
    cleanupDir(state);
  });

  it("a real offline run records the §3 items for both desktop routes", async () => {
    const r = await request(app)
      .post("/aai/conformance/chatgpt-dots?offline=1")
      .set("Authorization", `Bearer ${tok}`)
      .expect(200);
    expect(r.body.ok).toBe(true);
    expect(r.body.adapterId).toBe("chatgpt-dots");

    const view = await request(app).get("/v1/routes").set("Authorization", `Bearer ${tok}`).expect(200);
    const openai = view.body.vendors.find((v: { vendor: string }) => v.vendor === "openai");
    expect(openai).toBeDefined();
    const cdp = openai.routes.find((x: { route: string }) => x.route === "desktop-app-cdp");
    expect(cdp.contract.appVersion).toBe("0.0.0");
    expect(typeof cdp.contract.checkedAt).toBe("string");
    expect(cdp.contract.failed).toContain("attachments.send"); // the honest current gap
    expect(cdp.eligible).toBe(false); // contract not fully passed + no probe + no signed-in account
  }, 60_000);

  it("?route= rejects a route the adapter does not implement", async () => {
    const r = await request(app)
      .post("/aai/conformance/chatgpt-dots?offline=1&route=web")
      .set("Authorization", `Bearer ${tok}`)
      .expect(400);
    expect(r.body.error).toBe("route_not_implemented");
    expect(r.body.implements).toEqual(["desktop-app-cdp", "desktop-app-ax"]);
  }, 60_000);
});
