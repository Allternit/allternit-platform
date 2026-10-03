// P3 activation — WorkerPool: idempotent activate (one launch, re-probe on
// demand), probe→health mapping incl. challenge/auth walls, Critical #5
// (challenge surfaces once, never auto-retried), profile-lock mapping, and
// shutdown. The Launcher is faked throughout — no browsers in unit tests.
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import type { ProbeResult, SessionHealth } from "@allternit/subscription-fabric-contracts";
import type { AdapterRegistry } from "../src/adapters/registry.js";
import type { EventLog } from "../src/events/log.js";
import { createScheduler } from "../src/queue/scheduler.js";
import { openDatabase, type Db } from "../src/store/db.js";
import { getAccount, insertTask, upsertAccount } from "../src/store/queries.js";
import {
  healthFromProbe,
  WorkerPool,
  type LaneRuntime,
} from "../src/worker/pool.js";
import { WorkerSupervisor } from "../src/worker/supervisor.js";
import { dummyResolver, fakeLease, fixtureWebConfig, sampleTask, scriptedAdapter } from "./helpers.js";

const MANIFEST = fixtureWebConfig().manifest;
const LANE = { provider: MANIFEST.provider, account_id: "acct-fw-1" };

let db: Db;

function seedAccount(): void {
  upsertAccount(db, {
    account_id: LANE.account_id,
    provider: LANE.provider,
    label: "Fixture",
    plan: null,
    plan_observed_at: null,
    profile_ref: `profiles/${LANE.account_id}`,
    session_health: "auth_required",
    enabled: true,
  });
}

function fakeRegistry(): AdapterRegistry {
  const loaded = { dir: "/nonexistent", manifest: MANIFEST };
  return {
    adapters: [loaded],
    byId: (id: string) => (id === MANIFEST.adapter_id ? loaded : undefined),
    capabilities: () => [],
  };
}

function probeResult(ok: boolean, failedCriticalKey?: string): ProbeResult {
  return {
    ok,
    checks: failedCriticalKey
      ? [{ key: failedCriticalKey, critical: true, ok: false, detail: "failed" }]
      : [],
    observed_at: new Date().toISOString(),
  };
}

interface FakeRuntime extends LaneRuntime {
  probeCalls: number;
  closeCalls: number;
}

function fakeRuntime(probeImpl: () => Promise<ProbeResult>): FakeRuntime {
  const rt: FakeRuntime = {
    adapter: scriptedAdapter({ execute: async function* () {} }),
    page: fakeLease(),
    makeResolver: () => dummyResolver(),
    probeCalls: 0,
    closeCalls: 0,
    async probe() {
      rt.probeCalls += 1;
      return probeImpl();
    },
    async close() {
      rt.closeCalls += 1;
    },
  };
  return rt;
}

interface Harness {
  pool: WorkerPool;
  launchCalls: number;
  logLines: { kind: string; task_id: string }[];
}

function makePool(
  probeImpl: () => Promise<ProbeResult>,
  extra: Partial<Pick<LaneRuntime, "readAccount" | "readPlan">> = {},
  opts: { challengeRecheckMs?: number; challengeRecheckForMs?: number; laneIdleCloseMs?: number; idleSweepMs?: number; now?: () => number } = {}
): Harness {
  const supervisor = new WorkerSupervisor({
    db,
    scheduler: createScheduler(),
    adapters: () => undefined,
  });
  const logLines: { kind: string; task_id: string }[] = [];
  const log = {
    append: (e: { kind: string; task_id: string }) => logLines.push({ kind: e.kind, task_id: e.task_id }),
  } as unknown as EventLog;
  const harness: Harness = {
    launchCalls: 0,
    logLines,
    pool: null as unknown as WorkerPool,
  };
  harness.pool = new WorkerPool({
    db,
    registry: fakeRegistry(),
    supervisor,
    log,
    ...opts,
    launch: async () => {
      harness.launchCalls += 1;
      return Object.assign(fakeRuntime(probeImpl), extra);
    },
  });
  return harness;
}

beforeEach(() => {
  db = openDatabase(":memory:");
  seedAccount();
});

afterEach(() => {
  db.close();
});

describe("healthFromProbe", () => {
  it("maps probe outcomes onto SessionHealth (challenge > auth > drift)", () => {
    expect(healthFromProbe(probeResult(true))).toBe("ready");
    expect(healthFromProbe(probeResult(false, "challenge"))).toBe("challenge_presented");
    expect(healthFromProbe(probeResult(false, "auth.state"))).toBe("auth_required");
    expect(healthFromProbe(probeResult(false, "composer"))).toBe("ui_drift");
  });
});

describe("WorkerPool.activate", () => {
  it("is idempotent: two activates launch once and share the runtime", async () => {
    const h = makePool(async () => probeResult(true));
    const first = await h.pool.activate(LANE);
    const second = await h.pool.activate(LANE);
    expect(h.launchCalls).toBe(1);
    expect(second).toBe(first);
    expect(h.pool.runtimeFor(LANE)).toBe(first);
    expect(h.pool.healthFor(LANE)).toBe("ready");
    expect(getAccount(db, LANE.account_id)?.session_health).toBe("ready");
  });

  it("a ready probe records who is signed in, usage and plan; a failed read changes nothing", async () => {
    const usage = { remaining_pct: 8, resets_at: null, observed_at: "2026-09-29T14:54:00.000Z" };
    const h = makePool(async () => probeResult(true), {
      readAccount: async () => ({ identity: "eoj@example.com", usage }),
      readPlan: async () => "plus",
    });
    await h.pool.activate(LANE);
    expect(getAccount(db, LANE.account_id)).toMatchObject({ identity: "eoj@example.com", usage, plan: "plus" });

    const failing = makePool(async () => probeResult(true), {
      readAccount: async () => {
        throw new Error("page gone");
      },
    });
    await failing.pool.deactivate(LANE);
    await failing.pool.activate(LANE);
    expect(getAccount(db, LANE.account_id)).toMatchObject({ session_health: "ready", identity: "eoj@example.com" });
  });

  it("refreshes rendered bots on ready resident accounts, clears empty lists, and never launches an absent lane", async () => {
    let agents = [{ id: "g-owned", name: "Owned GPT", kind: "gpt", kindLabel: "GPT", avatarUrl: "https://icons.invalid/owned.png" }];
    const h = makePool(async () => probeResult(true), { readAccount: async () => ({ identity: "fixture", usage: null, agents }) });
    await h.pool.refreshAccount(LANE);
    expect(h.launchCalls).toBe(0);
    await h.pool.activate(LANE);
    expect(getAccount(db, LANE.account_id)?.agents).toEqual(agents);
    agents = [];
    await h.pool.refreshAccount(LANE);
    expect(getAccount(db, LANE.account_id)?.agents).toEqual([]);
    expect(h.launchCalls).toBe(1);
    await h.pool.deactivate(LANE);
  });

  it("an auth wall never reads the account", async () => {
    let reads = 0;
    const h = makePool(async () => probeResult(false, "auth.state"), {
      readAccount: async () => {
        reads += 1;
        return { identity: "x", usage: null };
      },
    });
    await h.pool.activate(LANE).catch(() => {});
    expect(reads).toBe(0);
  });

  it("auth wall → auth_required; re-activate re-probes WITHOUT relaunching (Critical #5: no auto-retry)", async () => {
    const h = makePool(async () => probeResult(false, "auth.state"));
    const first = await h.pool.activate(LANE);
    expect(h.pool.healthFor(LANE)).toBe("auth_required");
    expect(getAccount(db, LANE.account_id)?.session_health).toBe("auth_required");
    expect((first as FakeRuntime).probeCalls).toBe(1); // one probe per activate, never a loop
    const second = await h.pool.activate(LANE);
    expect(second).toBe(first); // same window left open for the human
    expect(h.launchCalls).toBe(1); // no relaunch
    expect((first as FakeRuntime).probeCalls).toBe(2); // probe re-driven on demand
  });

  it("challenge → challenge_presented + exactly one account-scoped ledger event per transition", async () => {
    const h = makePool(async () => probeResult(false, "challenge"));
    await h.pool.activate(LANE);
    expect(h.pool.healthFor(LANE)).toBe("challenge_presented");
    expect(getAccount(db, LANE.account_id)?.session_health).toBe("challenge_presented");
    const events = h.logLines.filter(
      (l) => l.kind === "needs_user" && l.task_id === `account:${LANE.account_id}`
    );
    expect(events).toHaveLength(1);
    await h.pool.activate(LANE); // still challenge — no second transition event
    expect(
      h.logLines.filter((l) => l.kind === "needs_user" && l.task_id === `account:${LANE.account_id}`)
    ).toHaveLength(1);
  });

  it("after a verification check the pool looks again by itself; clearing it turns the account ready (probe only)", async () => {
    let result = probeResult(false, "challenge");
    const h = makePool(async () => result, {}, { challengeRecheckMs: 20 });
    const runtime = (await h.pool.activate(LANE)) as FakeRuntime;
    expect(h.pool.healthFor(LANE)).toBe("challenge_presented");
    await new Promise((r) => setTimeout(r, 70));
    expect(runtime.probeCalls).toBeGreaterThan(1); // re-probed without anyone asking
    expect(h.launchCalls).toBe(1); // same browser, nothing relaunched
    result = probeResult(true); // the person ticked the check
    await new Promise((r) => setTimeout(r, 70));
    expect(getAccount(db, LANE.account_id)?.session_health).toBe("ready");
    const probes = runtime.probeCalls;
    await new Promise((r) => setTimeout(r, 70));
    expect(runtime.probeCalls).toBe(probes); // watch stopped once ready
  });

  it("the check watch stops when the lane closes (a login window takes the profile)", async () => {
    const h = makePool(async () => probeResult(false, "challenge"), {}, { challengeRecheckMs: 20 });
    const runtime = (await h.pool.activate(LANE)) as FakeRuntime;
    await h.pool.deactivate(LANE);
    const probes = runtime.probeCalls;
    await new Promise((r) => setTimeout(r, 70));
    expect(runtime.probeCalls).toBe(probes);
    expect(h.launchCalls).toBe(1);
  });

  it("a probe that passes after an auth wall flips the lane to ready", async () => {
    let result = probeResult(false, "auth.state");
    const h = makePool(async () => result);
    await h.pool.activate(LANE);
    expect(h.pool.healthFor(LANE)).toBe("auth_required");
    result = probeResult(true); // the human logged in in the open window
    await h.pool.activate(LANE);
    expect(h.pool.healthFor(LANE)).toBe("ready");
    expect(getAccount(db, LANE.account_id)?.session_health).toBe("ready");
  });

  it("a dead resident browser is relaunched, not re-probed forever as provider_down", async () => {
    const h = makePool(async () => probeResult(true));
    const first = (await h.pool.activate(LANE)) as FakeRuntime;
    let alive = true;
    first.isAlive = () => alive;
    alive = false; // browser killed / window closed under the pool
    expect(h.pool.runtimeFor(LANE)).toBeNull();
    const second = await h.pool.activate(LANE);
    expect(second).not.toBe(first);
    expect(h.launchCalls).toBe(2);
    expect(first.closeCalls).toBe(1);
    expect(h.pool.runtimeFor(LANE)).toBe(second);
    expect(h.pool.healthFor(LANE)).toBe("ready");
  });

  it("launch failure from a profile lock persists profile_locked and propagates", async () => {
    const supervisor = new WorkerSupervisor({
      db,
      scheduler: createScheduler(),
      adapters: () => undefined,
    });
    const pool = new WorkerPool({
      db,
      registry: fakeRegistry(),
      supervisor,
      launch: async () => {
        throw new Error("SingletonLock: user data directory is already in use");
      },
    });
    await expect(pool.activate(LANE)).rejects.toThrow(/SingletonLock/);
    expect(getAccount(db, LANE.account_id)?.session_health).toBe("profile_locked");
  });

  it("refuses lanes with no account row or no registered adapter", async () => {
    const h = makePool(async () => probeResult(true));
    await expect(
      h.pool.activate({ provider: LANE.provider, account_id: "ghost" })
    ).rejects.toThrow(/not found/);
    await expect(
      h.pool.activate({ provider: "no-such-provider", account_id: LANE.account_id })
    ).rejects.toThrow(/no adapter registered/);
  });
});

describe("WorkerPool.shutdown", () => {
  it("closes each resident runtime exactly once", async () => {
    const h = makePool(async () => probeResult(true));
    const runtime = (await h.pool.activate(LANE)) as FakeRuntime;
    await h.pool.shutdown();
    expect(runtime.closeCalls).toBe(1);
    expect(h.pool.runtimeFor(LANE)).toBeNull();
    await h.pool.shutdown(); // idempotent
    expect(runtime.closeCalls).toBe(1);
  });
});

describe("WorkerPool health states", () => {
  it("exposes the health map for the drain's ready gate", async () => {
    const states: [ProbeResult, SessionHealth][] = [
      [probeResult(true), "ready"],
      [probeResult(false, "auth.state"), "auth_required"],
      [probeResult(false, "challenge"), "challenge_presented"],
      [probeResult(false, "composer"), "ui_drift"],
    ];
    for (const [result, expected] of states) {
      const h = makePool(async () => result);
      await h.pool.activate(LANE);
      expect(h.pool.healthFor(LANE)).toBe(expected);
      db.close();
      db = openDatabase(":memory:");
      seedAccount();
    }
  });
});

describe("WorkerPool idle close (Chrome only while needed)", () => {
  it("keeps Chrome resident when no idle limit is set", async () => {
    let t = 0;
    const h = makePool(async () => probeResult(true), {}, { now: () => t });
    await h.pool.activate(LANE);
    t = 10 * 60 * 60_000;
    expect(await h.pool.closeIdleLanes()).toEqual([]);
    expect(h.pool.runtimeFor(LANE)).not.toBeNull();
    await h.pool.shutdown();
  });

  it("closes an idle lane's Chrome and relaunches it for the next task", async () => {
    let t = 0;
    const h = makePool(async () => probeResult(true), {}, { now: () => t, laneIdleCloseMs: 10 * 60_000, idleSweepMs: 3_600_000 });
    const rt = (await h.pool.activate(LANE)) as FakeRuntime;
    t = 9 * 60_000;
    expect(await h.pool.closeIdleLanes()).toEqual([]);
    t = 11 * 60_000;
    expect(await h.pool.closeIdleLanes()).toHaveLength(1);
    expect(rt.closeCalls).toBe(1);
    expect(h.pool.runtimeFor(LANE)).toBeNull();
    await h.pool.activate(LANE);
    expect(h.launchCalls).toBe(2);
    await h.pool.shutdown();
  });

  it("never closes Chrome while the account has a task in flight", async () => {
    let t = 0;
    const h = makePool(async () => probeResult(true), {}, { now: () => t, laneIdleCloseMs: 10 * 60_000, idleSweepMs: 3_600_000 });
    await h.pool.activate(LANE);
    const task = sampleTask({ task_id: "long-turn" });
    insertTask(db, { ...task, status: "provider_running", routing: { ...task.routing, account_id: LANE.account_id } } as never);
    t = 60 * 60_000;
    expect(await h.pool.closeIdleLanes()).toEqual([]);
    expect(h.pool.runtimeFor(LANE)).not.toBeNull();
    await h.pool.shutdown();
  });

  it("touching the lane (a drain tick serving it) resets the idle clock", async () => {
    let t = 0;
    const h = makePool(async () => probeResult(true), {}, { now: () => t, laneIdleCloseMs: 10 * 60_000, idleSweepMs: 3_600_000 });
    await h.pool.activate(LANE);
    t = 8 * 60_000;
    h.pool.runtimeFor(LANE);
    t = 15 * 60_000;
    expect(await h.pool.closeIdleLanes()).toEqual([]);
    await h.pool.shutdown();
  });
});
