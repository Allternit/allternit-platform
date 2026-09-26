// P3 activation — the lane drain: enqueued tasks reach runAttempt for ready
// lanes only; terminal failures flow through P4 requeueAfterFailure (dispatch
// wired); a failed attempt never kills the loop; lanes without accounts are
// never launched; missing runtimes auto-activate once (no spam). All fakes —
// no browsers.
import { join } from "node:path";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import type {
  CapabilityRouter,
  SessionHealth,
  Task,
  TaskError,
} from "@allternit/subscription-fabric-contracts";
import type { AdapterRegistry } from "../src/adapters/registry.js";
import { EventLog } from "../src/events/log.js";
import { createScheduler, type Scheduler } from "../src/queue/scheduler.js";
import { openDatabase, type Db } from "../src/store/db.js";
import { getTask, insertTask, upsertAccount } from "../src/store/queries.js";
import { startDrain } from "../src/worker/drain.js";
import type { LaneRuntime, WorkerPool } from "../src/worker/pool.js";
import type { WorkerDeps } from "../src/worker/worker.js";
import {
  cleanupDir,
  dummyResolver,
  fakeLease,
  fixtureWebConfig,
  sampleTask,
  scriptedAdapter,
  tmpStateDir,
  type ScriptedAdapter,
} from "./helpers.js";

const MANIFEST = fixtureWebConfig().manifest;
const LANE = { provider: MANIFEST.provider, account_id: "acct-fw-1" };

let dir: string;
let db: Db;
let log: EventLog;
let scheduler: Scheduler;
let stopDrain: (() => void) | null;

function seedAccount(): void {
  upsertAccount(db, {
    account_id: LANE.account_id,
    provider: LANE.provider,
    label: "Fixture",
    plan: null,
    plan_observed_at: null,
    profile_ref: `profiles/${LANE.account_id}`,
    session_health: "ready",
    enabled: true,
  });
}

function seedQueuedTask(overrides: Partial<Task> = {}): Task {
  const task = sampleTask({
    task_id: `task-${Math.random().toString(36).slice(2)}`,
    routing: {
      mode: "auto",
      allow_fallback: true,
      allow_metered: false,
      allow_thread_migration: false,
      provider: LANE.provider,
      account_id: LANE.account_id,
    },
    ...overrides,
  });
  insertTask(db, task);
  scheduler.enqueue(task);
  return task;
}

function runtimeFor(adapter: ScriptedAdapter): LaneRuntime {
  return {
    adapter,
    page: fakeLease(),
    makeResolver: () => dummyResolver(),
    probe: async () => ({ ok: true, checks: [], observed_at: new Date().toISOString() }),
    close: async () => {},
  };
}

interface FakePool {
  pool: WorkerPool;
  activateCalls: number;
}

// runtimeFor/healthFor mirror the pool's lanes map: health only answers when
// a runtime is resident. activateInstalls scripts a successful activation
// (runtime appears, health flips ready); without it activate throws.
function fakePool(opts: {
  runtime: LaneRuntime | null;
  health: SessionHealth;
  activateInstalls?: LaneRuntime;
}): FakePool {
  let runtime = opts.runtime;
  let health = opts.health;
  const state: FakePool = { activateCalls: 0, pool: null as unknown as WorkerPool };
  state.pool = {
    runtimeFor: () => runtime,
    healthFor: () => (runtime ? health : null),
    activate: async () => {
      state.activateCalls += 1;
      if (opts.activateInstalls) {
        runtime = opts.activateInstalls;
        health = "ready";
      }
      if (!runtime) throw new Error("no runtime scripted");
      return runtime;
    },
  } as unknown as WorkerPool;
  return state;
}

function fakeRegistry(): AdapterRegistry {
  const loaded = { dir: "/nonexistent", manifest: MANIFEST };
  return {
    adapters: [loaded],
    byId: (id: string) => (id === MANIFEST.adapter_id ? loaded : undefined),
    capabilities: () => [],
  };
}

interface SpyRouter {
  router: CapabilityRouter;
  failedCalls: number;
}

function spyRouter(): SpyRouter {
  const spy: SpyRouter = { failedCalls: 0, router: null as unknown as CapabilityRouter };
  spy.router = {
    resolve: () => {
      throw new Error("drain tests never resolve at enqueue");
    },
    onAttemptFailed: () => {
      spy.failedCalls += 1;
      return "stop";
    },
  } as unknown as CapabilityRouter;
  return spy;
}

async function waitFor(cond: () => boolean, timeoutMs = 3000): Promise<void> {
  const start = Date.now();
  while (!cond()) {
    if (Date.now() - start > timeoutMs) throw new Error("waitFor timed out");
    await new Promise((r) => setTimeout(r, 10));
  }
}

function start(pool: WorkerPool, router: CapabilityRouter): void {
  stopDrain = startDrain({
    db,
    scheduler,
    pool,
    makeWorkerDeps: (): WorkerDeps => ({
      db,
      log,
      artifactsDir: join(dir, "artifacts"),
      dispatch: { db, registry: fakeRegistry(), router, scheduler },
    }),
    drainIntervalMs: 5,
  });
}

const DONE = { t: "done", outcome: "success", text: "hello" } as const;
const FAIL = (detail: string): { t: "error"; error: TaskError } => ({
  t: "error",
  error: {
    class: "provider_error",
    scope: "task",
    retryable: true,
    fallback_eligible: true,
    cooldown_s: null,
    user_action: null,
    detail,
    evidence_ref: null,
  },
});

beforeEach(() => {
  dir = tmpStateDir();
  db = openDatabase(":memory:");
  log = new EventLog(db);
  scheduler = createScheduler();
  stopDrain = null;
});

afterEach(() => {
  stopDrain?.();
  db.close();
  cleanupDir(dir);
});

describe("startDrain", () => {
  it("runs an enqueued task on a ready lane to completion", async () => {
    seedAccount();
    const adapter = scriptedAdapter({
      execute: async function* () {
        yield DONE;
      },
    });
    const fp = fakePool({ runtime: runtimeFor(adapter), health: "ready" });
    const spy = spyRouter();
    start(fp.pool, spy.router);
    const task = seedQueuedTask();
    await waitFor(() => getTask(db, task.task_id)?.status === "completed");
    expect(adapter.executeCalls).toBe(1);
    expect(getTask(db, task.task_id)?.result?.text).toBe("hello");
  });

  it("wires P4 dispatch: a terminal failure reaches router.onAttemptFailed", async () => {
    seedAccount();
    const adapter = scriptedAdapter({
      execute: async function* () {
        yield FAIL("boom");
      },
    });
    const fp = fakePool({ runtime: runtimeFor(adapter), health: "ready" });
    const spy = spyRouter();
    start(fp.pool, spy.router);
    const task = seedQueuedTask();
    await waitFor(() => getTask(db, task.task_id)?.status === "failed");
    await waitFor(() => spy.failedCalls === 1);
  });

  it("survives a failed attempt — the next task on the lane still runs", async () => {
    seedAccount();
    const adapter = scriptedAdapter({
      execute: async function* () {
        if (adapter.executeCalls === 1) yield FAIL("first attempt dies");
        else yield DONE;
      },
    });
    const fp = fakePool({ runtime: runtimeFor(adapter), health: "ready" });
    const spy = spyRouter();
    start(fp.pool, spy.router);
    const t1 = seedQueuedTask();
    const t2 = seedQueuedTask();
    await waitFor(() => getTask(db, t1.task_id)?.status === "failed");
    await waitFor(() => getTask(db, t2.task_id)?.status === "completed");
    expect(adapter.executeCalls).toBe(2);
  });

  it("never launches for a lane with no account — the task stays queued", async () => {
    // No account row for this lane.
    const adapter = scriptedAdapter({
      execute: async function* () {
        yield DONE;
      },
    });
    const fp = fakePool({ runtime: runtimeFor(adapter), health: "ready" });
    const spy = spyRouter();
    start(fp.pool, spy.router);
    const task = seedQueuedTask({
      routing: {
        mode: "force",
        allow_fallback: false,
        allow_metered: false,
        allow_thread_migration: false,
        provider: "ghost-provider",
        account_id: "ghost-account",
      },
    });
    await new Promise((r) => setTimeout(r, 100));
    expect(getTask(db, task.task_id)?.status).toBe("queued");
    expect(scheduler.size("ghost-provider", "ghost-account")).toBe(1);
    expect(fp.activateCalls).toBe(0);
    expect(adapter.executeCalls).toBe(0);
  });

  it("auto-activates a lane with an account but no resident runtime (post-restart path)", async () => {
    seedAccount();
    const adapter = scriptedAdapter({
      execute: async function* () {
        yield DONE;
      },
    });
    const fp = fakePool({
      runtime: null,
      health: "auth_required",
      activateInstalls: runtimeFor(adapter),
    });
    const spy = spyRouter();
    const task = seedQueuedTask();
    start(fp.pool, spy.router);
    await waitFor(() => fp.activateCalls >= 1);
    await waitFor(() => getTask(db, task.task_id)?.status === "completed");
    expect(adapter.executeCalls).toBe(1);
  });

  it("does not run attempts on a lane whose health is not ready (auth wall waits for the human)", async () => {
    seedAccount();
    const adapter = scriptedAdapter({
      execute: async function* () {
        yield DONE;
      },
    });
    const fp = fakePool({ runtime: runtimeFor(adapter), health: "auth_required" });
    const spy = spyRouter();
    start(fp.pool, spy.router);
    const task = seedQueuedTask();
    await new Promise((r) => setTimeout(r, 100));
    expect(adapter.executeCalls).toBe(0);
    expect(getTask(db, task.task_id)?.status).toBe("queued");
    expect(scheduler.size(LANE.provider, LANE.account_id)).toBe(1);
  });
});
