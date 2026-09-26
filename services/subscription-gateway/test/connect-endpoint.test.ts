// P3 activation — POST /v1/accounts/:id/connect: activates the account's lane
// through the worker pool and returns the account with the probe outcome
// reflected in session_health. Errors follow the sibling-route style.
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import request from "supertest";
import { issueToken } from "../src/security/tokens.js";
import { getAccount, upsertAccount } from "../src/store/queries.js";
import type { WorkerPool } from "../src/worker/pool.js";
import { cleanupDir, makeDeps, tmpStateDir, type TestDeps } from "./helpers.js";

let dir: string;
let deps: TestDeps;

function token(scopes: Parameters<typeof issueToken>[3], caller = "admin-1"): string {
  return issueToken(deps.db, caller, "test", scopes).token;
}

function seedAccount(enabled = true): string {
  upsertAccount(deps.db, {
    account_id: "acct-1",
    provider: "fixture-web",
    label: "Fixture",
    plan: null,
    plan_observed_at: null,
    profile_ref: "profiles/acct-1",
    session_health: "auth_required",
    enabled,
  });
  return "acct-1";
}

// Structural pool fake: activate flips the account's health, as the real
// pool's probe→health persist does. Cast — the route only calls activate().
function fakePool(behavior: "ready" | "auth" | "throw"): {
  pool: WorkerPool;
  activateCalls: { provider: string; account_id: string }[];
} {
  const activateCalls: { provider: string; account_id: string }[] = [];
  const pool = {
    activate: async (lane: { provider: string; account_id: string }) => {
      activateCalls.push(lane);
      if (behavior === "throw") throw new Error("launch exploded");
      const account = getAccount(deps.db, lane.account_id);
      if (account) {
        upsertAccount(deps.db, {
          ...account,
          session_health: behavior === "ready" ? "ready" : "auth_required",
        });
      }
      return {};
    },
  } as unknown as WorkerPool;
  return { pool, activateCalls };
}

beforeEach(() => {
  dir = tmpStateDir();
});

afterEach(() => {
  deps.cleanup();
  cleanupDir(dir);
});

describe("POST /v1/accounts/:id/connect", () => {
  it("activates the lane and returns the account with the probe outcome", async () => {
    const fp = fakePool("ready");
    deps = makeDeps(dir, { pool: fp.pool });
    const id = seedAccount();
    const res = await request(deps.app)
      .post(`/v1/accounts/${id}/connect`)
      .set("authorization", `Bearer ${token(["accounts:manage"])}`);
    expect(res.status).toBe(200);
    expect(res.body.account_id).toBe(id);
    expect(res.body.session_health).toBe("ready");
    expect(fp.activateCalls).toEqual([{ provider: "fixture-web", account_id: id }]);
  });

  it("reflects an auth wall in the body (window stays open for the human)", async () => {
    const fp = fakePool("auth");
    deps = makeDeps(dir, { pool: fp.pool });
    const id = seedAccount();
    const res = await request(deps.app)
      .post(`/v1/accounts/${id}/connect`)
      .set("authorization", `Bearer ${token(["accounts:manage"])}`);
    expect(res.status).toBe(200);
    expect(res.body.session_health).toBe("auth_required");
  });

  it("404 on an unknown account", async () => {
    deps = makeDeps(dir, { pool: fakePool("ready").pool });
    const res = await request(deps.app)
      .post("/v1/accounts/ghost/connect")
      .set("authorization", `Bearer ${token(["accounts:manage"])}`);
    expect(res.status).toBe(404);
    expect(res.body.error).toBe("account_not_found");
  });

  it("403 without the accounts:manage scope (matches sibling routes)", async () => {
    deps = makeDeps(dir, { pool: fakePool("ready").pool });
    const id = seedAccount();
    const res = await request(deps.app)
      .post(`/v1/accounts/${id}/connect`)
      .set("authorization", `Bearer ${token(["tasks:read"], "bot-1")}`);
    expect(res.status).toBe(403);
    expect(res.body.error).toBe("forbidden_scope");
  });

  it("409 on a disabled account", async () => {
    const fp = fakePool("ready");
    deps = makeDeps(dir, { pool: fp.pool });
    const id = seedAccount(false);
    const res = await request(deps.app)
      .post(`/v1/accounts/${id}/connect`)
      .set("authorization", `Bearer ${token(["accounts:manage"])}`);
    expect(res.status).toBe(409);
    expect(res.body.error).toBe("account_disabled");
    expect(fp.activateCalls).toHaveLength(0);
  });

  it("502 when activation itself fails", async () => {
    deps = makeDeps(dir, { pool: fakePool("throw").pool });
    const id = seedAccount();
    const res = await request(deps.app)
      .post(`/v1/accounts/${id}/connect`)
      .set("authorization", `Bearer ${token(["accounts:manage"])}`);
    expect(res.status).toBe(502);
    expect(res.body.error).toBe("activation_failed");
    expect(res.body.detail).toContain("launch exploded");
  });

  it("503 when no worker pool is wired", async () => {
    deps = makeDeps(dir);
    const id = seedAccount();
    const res = await request(deps.app)
      .post(`/v1/accounts/${id}/connect`)
      .set("authorization", `Bearer ${token(["accounts:manage"])}`);
    expect(res.status).toBe(503);
    expect(res.body.error).toBe("worker_unavailable");
  });
});
