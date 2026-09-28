// P4 Phase 2 — /v1/catalog + /v1/stats/* route tests through the existing
// supertest harness (makeDeps + issueToken), matching sibling-route auth.
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import request from "supertest";
import type { AdapterRegistry } from "../src/adapters/registry.js";
import { issueToken } from "../src/security/tokens.js";
import { insertAttempt, insertTask, upsertAccount } from "../src/store/queries.js";
import { HUMAN, cleanupDir, makeDeps, sampleTask, tmpStateDir, type TestDeps } from "./helpers.js";
import { makeAccount, makeManifest } from "./router/fixtures.js";

const manifestA = makeManifest();

function registryOf(...manifests: typeof manifestA[]): AdapterRegistry {
  return {
    adapters: manifests.map((manifest) => ({ dir: "/fixture", manifest })),
    byId: (id) => {
      const manifest = manifests.find((m) => m.adapter_id === id);
      return manifest ? { dir: "/fixture", manifest } : undefined;
    },
    capabilities: () => [],
  };
}

let dir: string;
let deps: TestDeps;

beforeEach(() => {
  dir = tmpStateDir();
  deps = makeDeps(dir, { adapterRegistry: registryOf(manifestA) });
  upsertAccount(deps.db, makeAccount());
});

afterEach(() => {
  deps.cleanup();
  cleanupDir(dir);
});

describe("GET /v1/catalog", () => {
  it("200 with picker entries for a ready account (any valid token)", async () => {
    const t = issueToken(deps.db, "caller-1", "test", ["tasks:read"]).token;
    const res = await request(deps.app).get("/v1/catalog").set("authorization", `Bearer ${t}`);
    expect(res.status).toBe(200);
    expect(res.body.length).toBe(3); // fast, standard, reasoning (no deep)
    expect(res.body[0]).toMatchObject({
      provider: "prov-a",
      health: "ready",
      supports_effort: false,
      fabric: {
        adapter_id: "adapter-a",
        account_id: "acct-1",
        capability: "chat.create",
      },
    });
    expect(res.body[0].id).toMatch(/^subs\/prov-a:(fast|standard|reasoning)$/);
  });

  it("200 [] when the account health needs a human", async () => {
    upsertAccount(deps.db, makeAccount({ session_health: "auth_required" }));
    const t = issueToken(deps.db, "caller-1", "test", ["tasks:read"]).token;
    const res = await request(deps.app).get("/v1/catalog").set("authorization", `Bearer ${t}`);
    expect(res.status).toBe(200);
    expect(res.body).toEqual([]);
  });

  it("401 without a token", async () => {
    const res = await request(deps.app).get("/v1/catalog");
    expect(res.status).toBe(401);
  });
});

describe("GET /v1/stats/adapters", () => {
  beforeEach(() => {
    insertTask(deps.db, sampleTask({ task_id: "stat-task" }));
    insertAttempt(deps.db, "stat-task", {
      attempt_no: 1,
      adapter_id: "adapter-a",
      adapter_version: "1.0.0",
      account_id: "acct-1",
      pool_key: "prov-a:acct-1:main-pool",
      submission_state: "acknowledged",
      prompt_fingerprint: "fp",
      provider_thread_id: null,
      requested_model_class: null,
      observed_model: null,
      started_at: "2026-09-26T10:00:00.000Z",
      ended_at: "2026-09-26T10:00:02.000Z",
      outcome: "success",
      error: null,
    });
  });

  it("200 with the seeded adapter row", async () => {
    const t = issueToken(deps.db, "caller-1", "test", ["tasks:read"]).token;
    const res = await request(deps.app).get("/v1/stats/adapters").set("authorization", `Bearer ${t}`);
    expect(res.status).toBe(200);
    expect(res.body).toHaveLength(1);
    expect(res.body[0]).toMatchObject({
      adapter_id: "adapter-a",
      adapter_version: "1.0.0",
      attempts: 1,
      completed: 1,
      success_rate: 1,
      latency_ms: { median: 2000, p95: 2000 },
      ui_drift: { consecutive_failures: 0, state: "closed" },
    });
  });

  it("403 without tasks:read", async () => {
    const t = issueToken(deps.db, "caller-1", "test", ["tasks:submit"]).token;
    const res = await request(deps.app).get("/v1/stats/adapters").set("authorization", `Bearer ${t}`);
    expect(res.status).toBe(403);
  });

  it("401 without a token", async () => {
    const res = await request(deps.app).get("/v1/stats/adapters");
    expect(res.status).toBe(401);
  });
});

describe("GET /v1/stats/rejections", () => {
  it("200 with rejections recorded at task submission; limit clamps", async () => {
    // Exhaust the only pool so submissions no-route with recorded rejections.
    const { upsertQuotaPool } = await import("../src/store/queries.js");
    const { makePool } = await import("./router/fixtures.js");
    const { FabricRouter } = await import("../src/router/resolve.js");
    upsertQuotaPool(deps.db, makePool({ state: "exhausted" }));
    const depsWithRouter = makeDeps(dir, { adapterRegistry: registryOf(manifestA), router: new FabricRouter() });
    upsertAccount(depsWithRouter.db, makeAccount());
    upsertQuotaPool(depsWithRouter.db, makePool({ state: "exhausted" }));

    const t = issueToken(depsWithRouter.db, "caller-1", "test", ["tasks:submit", "tasks:read"]).token;
    const post = await request(depsWithRouter.app)
      .post("/v1/tasks")
      .set("authorization", `Bearer ${t}`)
      .send({ initiated_by: HUMAN, capability: "chat.create", prompt: "hello" });
    expect(post.status).toBe(201);

    const res = await request(depsWithRouter.app)
      .get("/v1/stats/rejections?limit=1")
      .set("authorization", `Bearer ${t}`);
    expect(res.status).toBe(200);
    expect(res.body).toHaveLength(1);
    expect(res.body[0]).toMatchObject({
      task_id: post.body.task_id,
      adapter_id: "adapter-a",
      account_id: "acct-1",
      reason: "pool_exhausted",
    });
    depsWithRouter.cleanup();
  });

  it("403 without tasks:read", async () => {
    const t = issueToken(deps.db, "caller-1", "test", ["artifacts:read"]).token;
    const res = await request(deps.app).get("/v1/stats/rejections").set("authorization", `Bearer ${t}`);
    expect(res.status).toBe(403);
  });
});
