// §A8 — supervisor: crash recovery reconciles sent_unconfirmed BEFORE the
// queue resumes; per-attempt stall watchdog (§A9: retryable only when
// not_sent); never-resubmit proven by counting submissions.
import { join } from "node:path";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { Task } from "@allternit/subscription-fabric-contracts";
import { EventLog } from "../src/events/log.js";
import { createScheduler, type Scheduler } from "../src/queue/scheduler.js";
import { WorkerSupervisor, type WorkerKey } from "../src/worker/supervisor.js";
import { runAttempt } from "../src/worker/worker.js";
import { openDatabase, type Db } from "../src/store/db.js";
import { getTask, insertAttempt, insertTask, updateAttempt } from "../src/store/queries.js";
import {
  cleanupDir,
  dummyCtx,
  dummyResolver,
  fakeLease,
  sampleTask,
  scriptedAdapter,
  tmpStateDir,
} from "./helpers.js";

const KEY: WorkerKey = { provider: "fixture-web", account_id: "acct-fw-1" };

// Manual timer registry — deterministic stall/backoff testing.
function fakeTimers() {
  let now = 0;
  const timers: Array<{ at: number; fn: () => void }> = [];
  return {
    setTimeoutFn: (fn: () => void, ms: number) => {
      const t = { at: now + ms, fn };
      timers.push(t);
      return t;
    },
    clearTimeoutFn: (h: unknown) => {
      const i = timers.indexOf(h as { at: number; fn: () => void });
      if (i >= 0) timers.splice(i, 1);
    },
    advance(ms: number) {
      now += ms;
      for (;;) {
        const due = timers.filter((t) => t.at <= now).sort((a, b) => a.at - b.at)[0];
        if (!due) break;
        timers.splice(timers.indexOf(due), 1);
        due.fn();
      }
    },
    pending: () => timers.length,
  };
}

describe("supervisor watchdog", () => {
  let dir: string;
  let db: Db;
  let log: EventLog;
  let scheduler: Scheduler;
  let timers: ReturnType<typeof fakeTimers>;
  let sup: WorkerSupervisor;

  beforeEach(() => {
    dir = tmpStateDir();
    db = openDatabase(":memory:");
    log = new EventLog(db);
    scheduler = createScheduler();
    timers = fakeTimers();
    sup = new WorkerSupervisor({
      db,
      scheduler,
      adapters: () => undefined,
      log,
      setTimeoutFn: timers.setTimeoutFn,
      clearTimeoutFn: timers.clearTimeoutFn,
    });
  });
  afterEach(() => {
    sup.shutdown();
    db.close();
    cleanupDir(dir);
  });

  function seedRunningTask(submissionState: "not_sent" | "sent_unconfirmed"): Task {
    const task = sampleTask({
      task_id: `task-${Math.random().toString(36).slice(2)}`,
      status: "running",
      routing: {
        mode: "auto",
        provider: "fixture-web" as Task["routing"]["provider"],
        account_id: "acct-fw-1",
        allow_fallback: true,
        allow_metered: false,
        allow_thread_migration: false,
      },
    });
    insertTask(db, task);
    insertAttempt(db, task.task_id, {
      attempt_no: 1,
      adapter_id: "fixture-web",
      adapter_version: "0.1.0",
      account_id: "acct-fw-1",
      pool_key: "fixture-web:acct-fw-1:fixture-pool",
      submission_state: submissionState,
      prompt_fingerprint: "fp",
      provider_thread_id: null,
      requested_model_class: null,
      observed_model: null,
      started_at: new Date().toISOString(),
      ended_at: null,
      outcome: "failed",
      error: null,
    });
    return task;
  }

  it("no events past stall_timeout_s → task failed stalled; retryable false when sent_unconfirmed", () => {
    const task = seedRunningTask("sent_unconfirmed");
    sup.trackAttempt(task.task_id, 1, "chat.create"); // default 90 s
    timers.advance(89_000);
    expect(getTask(db, task.task_id)?.status).toBe("running");
    timers.advance(1_001);
    const final = getTask(db, task.task_id);
    expect(final?.status).toBe("failed");
    expect(final?.error?.class).toBe("stalled");
    expect(final?.error?.retryable).toBe(false); // §A9 — sent_unconfirmed
    expect(final?.attempts[0].outcome).toBe("failed");
  });

  it("stalled is retryable only when submission_state = not_sent", () => {
    const task = seedRunningTask("not_sent");
    sup.trackAttempt(task.task_id, 1, "chat.create");
    timers.advance(90_001);
    const final = getTask(db, task.task_id);
    expect(final?.status).toBe("failed");
    expect(final?.error?.class).toBe("stalled");
    expect(final?.error?.retryable).toBe(true);
  });

  it("heartbeats re-arm the watchdog (provider quiet but heartbeating is not stuck)", () => {
    const task = seedRunningTask("sent_unconfirmed");
    sup.trackAttempt(task.task_id, 1, "chat.create");
    timers.advance(80_000);
    sup.heartbeat(task.task_id);
    timers.advance(80_000);
    expect(getTask(db, task.task_id)?.status).toBe("running");
    timers.advance(10_001);
    expect(getTask(db, task.task_id)?.status).toBe("failed");
  });

  it("per-capability stall timeout: research.deep defaults to 1200 s", () => {
    const task = seedRunningTask("sent_unconfirmed");
    sup.trackAttempt(task.task_id, 1, "research.deep");
    timers.advance(90_001);
    expect(getTask(db, task.task_id)?.status).toBe("running");
    timers.advance(1_200_000);
    expect(getTask(db, task.task_id)?.status).toBe("failed");
  });
});

describe("supervisor restart recovery (§A8)", () => {
  let dir: string;
  let db: Db;
  let log: EventLog;
  let scheduler: Scheduler;

  beforeEach(() => {
    dir = tmpStateDir();
    db = openDatabase(":memory:");
    log = new EventLog(db);
    scheduler = createScheduler();
  });
  afterEach(() => {
    db.close();
    cleanupDir(dir);
  });

  it("reconcile runs BEFORE the queue resumes; not_found → submission_ambiguous; never resubmitted", async () => {
    // A task that was submitted once and then "crashed" mid-flight.
    const task = sampleTask({
      task_id: "task-crash-1",
      status: "queued",
      routing: {
        mode: "auto",
        provider: "fixture-web" as Task["routing"]["provider"],
        account_id: "acct-fw-1",
        allow_fallback: true,
        allow_metered: false,
        allow_thread_migration: false,
      },
    });
    insertTask(db, task);
    scheduler.enqueue(task);

    const caller = new AbortController();
    const adapter = scriptedAdapter({
      reconcileResult: { outcome: "not_found", detail: "no matching thread" },
      execute: async function* (_task, ctx) {
        adapter.submissionCount += 1;
        await ctx.markSubmitted(null); // sent_unconfirmed persisted
        yield { t: "submitted", provider_thread_id: null, provider_url: null };
        await new Promise((_, reject) => {
          ctx.signal.addEventListener("abort", () => reject(new Error("worker crashed")), {
            once: true,
          });
        });
      },
    });

    // Pickup + first run: submits, then hangs (the crash leaves it sent_unconfirmed).
    expect(scheduler.next(KEY.provider, KEY.account_id)?.task_id).toBe(task.task_id);
    const firstRun = runAttempt(
      { db, log, artifactsDir: join(dir, "artifacts") },
      {
        taskId: task.task_id,
        adapter,
        accountId: "acct-fw-1",
        page: fakeLease(),
        makeResolver: () => dummyResolver(),
        signal: caller.signal,
      }
    );
    await vi.waitFor(() => {
      expect(getTask(db, task.task_id)?.attempts[0]?.submission_state).toBe("sent_unconfirmed");
    });
    expect(adapter.submissionCount).toBe(1);

    // Crash → supervisor restart: reconcile must run before the lane resumes.
    const sup = new WorkerSupervisor({
      db,
      scheduler,
      adapters: () => adapter,
      log,
      makeReconcileCtx: () => dummyCtx(),
      setTimeoutFn: () => 0, // no watchdog timers in this test
      clearTimeoutFn: () => {},
    });
    expect(sup.nextTask(KEY)).toBeNull(); // gate: nothing is served pre-recovery

    const seenDuringReconcile: Array<string | null> = [];
    const origReconcile = adapter.reconcile.bind(adapter);
    adapter.reconcile = async (att, ctx) => {
      seenDuringReconcile.push(sup.status(KEY));
      expect(sup.nextTask(KEY)).toBeNull(); // queue still gated mid-reconcile
      return origReconcile(att, ctx);
    };

    await sup.ensureWorker(KEY);
    expect(seenDuringReconcile).toEqual(["reconciling"]);

    // reconcile ran: attempt resolved, task failed submission_ambiguous
    expect(adapter.reconcileCalls).toBe(1);
    const final = getTask(db, task.task_id);
    expect(final?.status).toBe("failed");
    expect(final?.error?.class).toBe("submission_ambiguous");
    expect(final?.error?.retryable).toBe(false);
    expect(final?.error?.fallback_eligible).toBe(false);
    expect(final?.attempts[0].outcome).toBe("ambiguous");

    // The never-resubmit rule: exactly one submission across crash + recovery.
    expect(adapter.submissionCount).toBe(1);
    expect(adapter.executeCalls).toBe(1);

    // Queue resumed only after reconcile completed; the lane is empty (the
    // failed task was dequeued at pickup and is never re-served).
    expect(sup.isReady(KEY)).toBe(true);
    expect(sup.nextTask(KEY)).toBeNull();

    caller.abort();
    await firstRun; // worker observes the terminal guard and returns
  });

  it("acknowledged reconcile adopts the thread and ends the attempt (never left running, never resubmitted)", async () => {
    const task = sampleTask({ task_id: "task-adopt-1", status: "running" });
    insertTask(db, task);
    insertAttempt(db, task.task_id, {
      attempt_no: 1,
      adapter_id: "fixture-web",
      adapter_version: "0.1.0",
      account_id: "acct-fw-1",
      pool_key: "fixture-web:acct-fw-1:fixture-pool",
      submission_state: "sent_unconfirmed",
      prompt_fingerprint: "fp",
      provider_thread_id: null,
      requested_model_class: null,
      observed_model: null,
      started_at: new Date().toISOString(),
      ended_at: null,
      outcome: "failed",
      error: null,
    });

    const adapter = scriptedAdapter({
      reconcileResult: { outcome: "acknowledged", provider_thread_id: "fw-thread-77" },
      execute: async function* () {},
    });
    const sup = new WorkerSupervisor({
      db,
      scheduler,
      adapters: () => adapter,
      log,
      makeReconcileCtx: () => dummyCtx(),
    });
    await sup.ensureWorker(KEY);
    const final = getTask(db, task.task_id);
    expect(final?.attempts[0].submission_state).toBe("acknowledged");
    expect(final?.attempts[0].provider_thread_id).toBe("fw-thread-77");
    // Nothing resumes a reconciled attempt, so it must not sit in running
    // forever: delivered → failed(stalled), not retryable, thread kept.
    expect(final?.status).toBe("failed");
    expect(final?.attempts[0].ended_at).not.toBeNull();
    expect(final?.attempts[0].error?.class).toBe("stalled");
    expect(final?.attempts[0].error?.retryable).toBe(false);
    expect(final?.attempts[0].error?.detail).toContain("fw-thread-77");
    expect(adapter.executeCalls).toBe(0);
  });

  const orphan = (taskId: string, state: "not_sent" | "acknowledged"): void => {
    insertTask(db, sampleTask({ task_id: taskId, status: "running" }));
    insertAttempt(db, taskId, {
      attempt_no: 1,
      adapter_id: "fixture-web",
      adapter_version: "0.1.0",
      account_id: "acct-fw-1",
      pool_key: "fixture-web:acct-fw-1:fixture-pool",
      submission_state: state,
      prompt_fingerprint: "fp",
      provider_thread_id: state === "acknowledged" ? "fw-thread-9" : null,
      requested_model_class: null,
      observed_model: null,
      started_at: new Date().toISOString(),
      ended_at: null,
      outcome: "failed",
      error: null,
    });
  };
  const restartSupervisor = (): WorkerSupervisor =>
    new WorkerSupervisor({
      db,
      scheduler,
      adapters: () => scriptedAdapter({ execute: async function* () {} }),
      log,
      makeReconcileCtx: () => dummyCtx(),
    });

  it("restart: an in-flight acknowledged attempt (killed mid-reply) fails as stalled, NOT retryable", async () => {
    orphan("task-orphan-ack", "acknowledged");
    await restartSupervisor().ensureWorker(KEY);
    const final = getTask(db, "task-orphan-ack");
    expect(final?.status).toBe("failed");
    expect(final?.error?.class).toBe("stalled");
    expect(final?.error?.retryable).toBe(false);
    expect(final?.error?.detail).toContain("fw-thread-9");
    expect(final?.attempts[0].ended_at).not.toBeNull();
  });

  it("restart: an in-flight not_sent attempt fails as stalled and IS retryable", async () => {
    orphan("task-orphan-ns", "not_sent");
    await restartSupervisor().ensureWorker(KEY);
    const final = getTask(db, "task-orphan-ns");
    expect(final?.status).toBe("failed");
    expect(final?.error?.retryable).toBe(true);
  });

  it("restart sweep leaves an attempt this process is still driving alone", async () => {
    orphan("task-live", "acknowledged");
    const sup = restartSupervisor();
    sup.trackAttempt("task-live", 1, "chat.create");
    await sup.ensureWorker(KEY);
    expect(getTask(db, "task-live")?.status).toBe("running");
    sup.shutdown();
  });

  it("missing adapter/reconcile fn → ambiguous → task needs_user", async () => {
    const task = sampleTask({ task_id: "task-noadapter-1", status: "running" });
    insertTask(db, task);
    insertAttempt(db, task.task_id, {
      attempt_no: 1,
      adapter_id: "fixture-web",
      adapter_version: "0.1.0",
      account_id: "acct-fw-1",
      pool_key: "fixture-web:acct-fw-1:fixture-pool",
      submission_state: "sent_unconfirmed",
      prompt_fingerprint: "fp",
      provider_thread_id: null,
      requested_model_class: null,
      observed_model: null,
      started_at: new Date().toISOString(),
      ended_at: null,
      outcome: "failed",
      error: null,
    });
    const sup = new WorkerSupervisor({
      db,
      scheduler,
      adapters: () => undefined, // adapter missing entirely
      log,
    });
    await sup.ensureWorker(KEY);
    const final = getTask(db, task.task_id);
    expect(final?.status).toBe("needs_user");
    expect(final?.attempts[0].outcome).toBe("ambiguous");
    expect(sup.isReady(KEY)).toBe(true);
  });
});
