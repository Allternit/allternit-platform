// Lane health canary: the free probe re-runs on resident lanes, skips accounts a
// task holds, and a challenge page moves the account to challenge_presented.
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import request from "supertest";
import type { ProbeResult } from "@allternit/subscription-fabric-contracts";
import { getBreaker, recordAdapterUiFailure } from "../src/breakers.js";
import { issueToken } from "../src/security/tokens.js";
import { getAccount, insertTask, latestCanaryRun, upsertAccount } from "../src/store/queries.js";
import { canaryTick } from "../src/worker/canary.js";
import type { WorkerPool } from "../src/worker/pool.js";
import { cleanupDir, makeDeps, sampleTask, tmpStateDir, fixtureWebConfig, type TestDeps } from "./helpers.js";

const MANIFEST = fixtureWebConfig().manifest;
let dir: string;
let deps: TestDeps;

function seed(id = "acct-1"): void {
  upsertAccount(deps.db, {
    account_id: id,
    provider: MANIFEST.provider,
    label: "Fixture",
    plan: null,
    plan_observed_at: null,
    profile_ref: `profiles/${id}`,
    session_health: "ready",
    enabled: true,
  });
}

// A resident-lane pool fake: probeResident persists health the way the real one does.
function fakePool(result: () => ProbeResult["checks"][number]["key"] | "ok") {
  const calls: string[] = [];
  const pool = {
    probeResident: async (lane: { account_id: string }) => {
      calls.push(lane.account_id);
      const outcome = result();
      const health = outcome === "ok" ? "ready" : outcome === "challenge" ? "challenge_presented" : "auth_required";
      const account = getAccount(deps.db, lane.account_id)!;
      upsertAccount(deps.db, { ...account, session_health: health });
      return { health, adapter_id: MANIFEST.adapter_id, adapter_version: MANIFEST.adapter_version };
    },
  } as unknown as Pick<WorkerPool, "probeResident">;
  return { pool, calls };
}

beforeEach(() => {
  dir = tmpStateDir();
  deps = makeDeps(dir);
  seed();
});
afterEach(() => {
  deps.db.close();
  cleanupDir(dir);
});

describe("canaryTick", () => {
  it("a challenge page moves the account to challenge_presented and records a failed run", async () => {
    const { pool } = fakePool(() => "challenge");
    const r = await canaryTick({ db: deps.db, pool });
    expect(r.probed).toBe(1);
    expect(getAccount(deps.db, "acct-1")?.session_health).toBe("challenge_presented");
    expect(latestCanaryRun(deps.db, "acct-1")?.outcome).toBe("fail");
  });

  it("a pass records a run and closes an open drift breaker", async () => {
    for (let i = 0; i < 4; i += 1) recordAdapterUiFailure(deps.db, MANIFEST.adapter_id, MANIFEST.adapter_version, new Date());
    expect(getBreaker(deps.db, MANIFEST.adapter_id, MANIFEST.adapter_version).state).toBe("open");
    const { pool } = fakePool(() => "ok");
    await canaryTick({ db: deps.db, pool });
    expect(latestCanaryRun(deps.db, "acct-1", "pass")).not.toBeNull();
    expect(getBreaker(deps.db, MANIFEST.adapter_id, MANIFEST.adapter_version).state).not.toBe("open");
  });

  it("skips an account a task holds", async () => {
    const task = sampleTask();
    insertTask(deps.db, { ...task, status: "running", routing: { ...(task.routing ?? {}), account_id: "acct-1" } } as never);
    const { pool, calls } = fakePool(() => "ok");
    const r = await canaryTick({ db: deps.db, pool });
    expect(calls).toHaveLength(0);
    expect(r.skipped).toBe(1);
  });
});

describe("GET /v1/lanes", () => {
  it("reports state, last probe and last pass per account", async () => {
    const { pool } = fakePool(() => "ok");
    await canaryTick({ db: deps.db, pool });
    const token = issueToken(deps.db, "admin-1", "test", ["tasks:read"]).token;
    const res = await request(deps.app).get("/v1/lanes").set("authorization", `Bearer ${token}`);
    expect(res.status).toBe(200);
    expect(res.body.lanes[0]).toMatchObject({ account_id: "acct-1", state: "green", lastProbeOutcome: "pass" });
    expect(res.body.lanes[0].lastOkAt).toBeTruthy();
  });
});
