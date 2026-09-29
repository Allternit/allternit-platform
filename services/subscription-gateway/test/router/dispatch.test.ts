// P4 wire-in tests: pick-time resolution at POST /v1/tasks, per-hop
// re-resolution after a failed attempt (dispatch.ts), and the extended
// /v1/capabilities entitlement view.
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import request from "supertest";
import type { Task, TaskAttempt } from "@allternit/subscription-fabric-contracts";
import type { AdapterRegistry } from "../../src/adapters/registry.js";
import { createScheduler, type Scheduler } from "../../src/queue/scheduler.js";
import { requeueAfterFailure, resolveForNewTask, type DispatchDeps } from "../../src/router/dispatch.js";
import { FabricRouter } from "../../src/router/resolve.js";
import { issueToken } from "../../src/security/tokens.js";
import { getTask, insertAttempt, insertTask, updateTaskStatus, upsertAccount, upsertQuotaPool } from "../../src/store/queries.js";
import { HUMAN, cleanupDir, makeDeps, sampleTask, tmpStateDir, type TestDeps } from "../helpers.js";
import { makeAccount, makeManifest, makePool, NOW, POOL_ID } from "./fixtures.js";

const manifestA = makeManifest({ adapter_id: "adapter-a", provider: "prov-a" as ReturnType<typeof makeAccount>["provider"] });
const accountA = makeAccount();

function registryOf(...manifests: typeof manifestA[]): AdapterRegistry {
  return {
    adapters: manifests.map((manifest) => ({ dir: "/fixture", manifest })),
    byId: (id) => {
      const manifest = manifests.find((m) => m.adapter_id === id);
      return manifest ? { dir: "/fixture", manifest } : undefined;
    },
    capabilities: () =>
      manifests.flatMap((m) =>
        m.capabilities.map((c) => ({
          capability: c.id as string,
          adapter_id: m.adapter_id,
          adapter_version: m.adapter_version,
          provider: m.provider as string,
          pool_id: c.pool_id,
          plans: c.plans,
          detachable: c.detachable,
          export_formats: c.export_formats,
          status: c.status,
        }))
      ),
  };
}

describe("dispatch — resolveForNewTask / requeueAfterFailure", () => {
  let dir: string;
  let deps: TestDeps;
  let scheduler: Scheduler;
  let dispatch: DispatchDeps;

  beforeEach(() => {
    dir = tmpStateDir();
    scheduler = createScheduler();
    const registry = registryOf(manifestA);
    deps = makeDeps(dir, { scheduler, adapterRegistry: registry, router: new FabricRouter() });
    dispatch = { db: deps.db, registry, router: new FabricRouter(), scheduler };
    upsertAccount(deps.db, accountA);
    upsertQuotaPool(deps.db, makePool({ state: "available" }));
  });

  afterEach(() => {
    deps.cleanup();
    cleanupDir(dir);
  });

  it("resolveForNewTask pins (provider, account) and stamps route_decision", () => {
    const t = sampleTask({ task_id: "route-me" });
    const { task: routed, decision } = resolveForNewTask(dispatch, t);
    expect(decision.primary?.adapter_id).toBe("adapter-a");
    expect(decision.primary?.account_id).toBe("acct-1");
    expect(decision.primary?.pool_key).toBe(`prov-a:acct-1:${POOL_ID}`);
    expect(routed.routing.provider).toBe("prov-a");
    expect(routed.routing.account_id).toBe("acct-1");
    expect(routed.route_decision).toEqual(decision);
  });

  it("requeueAfterFailure moves the task to the next candidate's lane as queued", () => {
    const manifestB = makeManifest({ adapter_id: "adapter-b", provider: "prov-b" as ReturnType<typeof makeAccount>["provider"] });
    const accountB = makeAccount({ account_id: "acct-b", provider: "prov-b" as ReturnType<typeof makeAccount>["provider"] });
    upsertAccount(deps.db, accountB);
    upsertQuotaPool(deps.db, makePool({ pool_key: `prov-b:acct-b:${POOL_ID}`, state: "available" }));
    dispatch = { ...dispatch, registry: registryOf(manifestA, manifestB) };

    const t = sampleTask({ task_id: "flap", status: "queued" });
    insertTask(deps.db, t);
    const failedAttempt: TaskAttempt = {
      attempt_no: 1,
      adapter_id: "adapter-a",
      adapter_version: "1.0.0",
      account_id: "acct-1",
      pool_key: `prov-a:acct-1:${POOL_ID}`,
      submission_state: "not_sent",
      prompt_fingerprint: "fp",
      provider_thread_id: null,
      requested_model_class: null,
      observed_model: null,
      started_at: NOW.toISOString(),
      ended_at: NOW.toISOString(),
      outcome: "failed",
      error: {
        class: "network_error",
        scope: "task",
        retryable: true,
        fallback_eligible: true,
        cooldown_s: null,
        user_action: null,
        detail: "connection reset",
        evidence_ref: null,
      },
    };
    insertAttempt(deps.db, t.task_id, failedAttempt);
    updateTaskStatus(deps.db, t.task_id, "failed");

    const outcome = requeueAfterFailure(dispatch, t.task_id);
    expect(outcome).not.toBe("stop");
    const after = getTask(deps.db, t.task_id) as Task;
    expect(after.status).toBe("queued");
    expect(after.routing.provider).toBe("prov-b");
    expect(after.routing.account_id).toBe("acct-b");
    expect(after.route_decision?.primary?.adapter_id).toBe("adapter-b");
    expect(after.route_decision?.rejected).toContainEqual({
      adapter_id: "adapter-a",
      account_id: "acct-1",
      reason: "health_not_ready",
    });
    expect(scheduler.size("prov-b", "acct-b")).toBe(1);
    expect(scheduler.size()).toBe(1);
  });

  it("requeueAfterFailure returns stop for non-retryable failures (task stays failed)", () => {
    const t = sampleTask({ task_id: "halt", status: "queued" });
    insertTask(deps.db, t);
    insertAttempt(deps.db, t.task_id, {
      attempt_no: 1,
      adapter_id: "adapter-a",
      adapter_version: "1.0.0",
      account_id: "acct-1",
      pool_key: `prov-a:acct-1:${POOL_ID}`,
      submission_state: "sent_unconfirmed",
      prompt_fingerprint: "fp",
      provider_thread_id: null,
      requested_model_class: null,
      observed_model: null,
      started_at: NOW.toISOString(),
      ended_at: NOW.toISOString(),
      outcome: "failed",
      error: {
        class: "submission_ambiguous",
        scope: "task",
        retryable: false,
        fallback_eligible: false,
        cooldown_s: null,
        user_action: null,
        detail: "provider may hold the prompt",
        evidence_ref: null,
      },
    });
    updateTaskStatus(deps.db, t.task_id, "failed");
    expect(requeueAfterFailure(dispatch, t.task_id)).toBe("stop");
    expect(getTask(deps.db, t.task_id)?.status).toBe("failed");
    expect(scheduler.size()).toBe(0);
  });
});

describe("http wiring — POST /v1/tasks resolves at pick time", () => {
  let dir: string;
  let deps: TestDeps;
  let scheduler: Scheduler;

  beforeEach(() => {
    dir = tmpStateDir();
    scheduler = createScheduler();
    deps = makeDeps(dir, {
      scheduler,
      adapterRegistry: registryOf(manifestA),
      router: new FabricRouter(),
    });
    upsertAccount(deps.db, accountA);
    upsertQuotaPool(deps.db, makePool({ state: "available" }));
  });

  afterEach(() => {
    deps.cleanup();
    cleanupDir(dir);
  });

  it("an unrouted auto task lands in the resolved worker lane with a persisted decision", async () => {
    const t = issueToken(deps.db, "caller-1", "test", ["tasks:submit", "tasks:read"]).token;
    const res = await request(deps.app)
      .post("/v1/tasks")
      .set("authorization", `Bearer ${t}`)
      .send({ initiated_by: HUMAN, capability: "chat.create", prompt: "hello" });
    expect(res.status).toBe(201);
    expect(res.body.status).toBe("queued");
    expect(res.body.routing.provider).toBe("prov-a");
    expect(res.body.routing.account_id).toBe("acct-1");
    expect(res.body.route_decision.primary.adapter_id).toBe("adapter-a");
    // Scheduler keyed by the resolved lane, not the unrouted lot.
    expect(scheduler.size("prov-a", "acct-1")).toBe(1);
    expect(scheduler.size("unrouted", "unrouted")).toBe(0);
    // Persisted, not just in the response.
    expect(getTask(deps.db, res.body.task_id)?.route_decision?.primary?.adapter_id).toBe("adapter-a");
  });

  it("a provider-only pin resolves to that provider's account (live: it sat in the provider/unrouted lane)", async () => {
    const t = issueToken(deps.db, "caller-1", "test", ["tasks:submit"]).token;
    const res = await request(deps.app)
      .post("/v1/tasks")
      .set("authorization", `Bearer ${t}`)
      .send({ initiated_by: HUMAN, capability: "chat.create", prompt: "hello", routing: { provider: "prov-a" } });
    expect(res.status).toBe(201);
    expect(res.body.routing).toMatchObject({ provider: "prov-a", account_id: "acct-1", provider_pinned: true });
    expect(scheduler.size("prov-a", "acct-1")).toBe(1);
    expect(scheduler.size("prov-a", "unrouted")).toBe(0);
  });

  it("a provider pin never routes to another provider's account", async () => {
    const t = issueToken(deps.db, "caller-1", "test", ["tasks:submit"]).token;
    const res = await request(deps.app)
      .post("/v1/tasks")
      .set("authorization", `Bearer ${t}`)
      .send({ initiated_by: HUMAN, capability: "chat.create", prompt: "hello", routing: { provider: "prov-other" } });
    expect(res.status).toBe(201);
    expect(res.body.route_decision.primary).toBeNull();
    expect(scheduler.size("prov-a", "acct-1")).toBe(0);
  });

  it("with no eligible route the task stays queued in the unrouted lane, decision recording why", async () => {
    upsertQuotaPool(deps.db, makePool({ state: "exhausted" }));
    const t = issueToken(deps.db, "caller-1", "test", ["tasks:submit"]).token;
    const res = await request(deps.app)
      .post("/v1/tasks")
      .set("authorization", `Bearer ${t}`)
      .send({ initiated_by: HUMAN, capability: "chat.create", prompt: "hello" });
    expect(res.status).toBe(201);
    expect(res.body.route_decision.primary).toBeNull();
    expect(res.body.route_decision.rejected).toContainEqual({
      adapter_id: "adapter-a",
      account_id: "acct-1",
      reason: "pool_exhausted",
    });
    expect(scheduler.size("unrouted", "unrouted")).toBe(1);
  });
});

describe("http wiring — GET /v1/capabilities entitlement view (P4 additive)", () => {
  let dir: string;
  let deps: TestDeps;

  beforeEach(() => {
    dir = tmpStateDir();
    deps = makeDeps(dir, { adapterRegistry: registryOf(manifestA), router: new FabricRouter() });
    upsertAccount(deps.db, accountA);
  });

  afterEach(() => {
    deps.cleanup();
    cleanupDir(dir);
  });

  it("entries keep their P3 shape and gain lane + per-entitlement pool state", async () => {
    upsertQuotaPool(deps.db, makePool({ state: "degraded" }));
    const t = issueToken(deps.db, "caller-1", "test", ["tasks:read"]).token;
    const res = await request(deps.app).get("/v1/capabilities").set("authorization", `Bearer ${t}`);
    expect(res.status).toBe(200);
    expect(res.body).toHaveLength(1);
    const entry = res.body[0];
    // P3 fields untouched.
    expect(entry).toMatchObject({
      capability: "chat.create",
      adapter_id: "adapter-a",
      provider: "prov-a",
      pool_id: POOL_ID,
      status: "stable",
    });
    expect(entry.lane).toBe("subscription");
    expect(entry.entitlements).toEqual([
      {
        account_id: "acct-1",
        pool_key: `prov-a:acct-1:${POOL_ID}`,
        pool_state: "degraded",
        available: true, // degraded still serves interactive work
      },
    ]);
  });

  it("unavailable entitlements carry the reject reason", async () => {
    upsertQuotaPool(deps.db, makePool({ state: "exhausted" }));
    const t = issueToken(deps.db, "caller-1", "test", ["tasks:read"]).token;
    const res = await request(deps.app).get("/v1/capabilities").set("authorization", `Bearer ${t}`);
    expect(res.body[0].entitlements).toEqual([
      {
        account_id: "acct-1",
        pool_key: `prov-a:acct-1:${POOL_ID}`,
        pool_state: "exhausted",
        available: false,
        reason_unavailable: "pool_exhausted",
      },
    ]);
  });
});
