// §A1/§A8 — worker executor: happy path against the SDK fixture pages (real
// browser, fixture-web declarative adapter), durable two-write markSubmitted,
// quota/model pool writes, detach handoff, needs_user.
import { join } from "node:path";
import { afterAll, afterEach, beforeAll, beforeEach, describe, expect, it, vi } from "vitest";
import type { Browser } from "playwright";
import {
  DeclarativeChatAdapter,
  createPageLease,
  createResolver,
  type SdkPageLease,
} from "@allternit/subscription-adapter-sdk";
import type { PageLease, Task } from "@allternit/subscription-fabric-contracts";
import { EventLog } from "../src/events/log.js";
import { createWatchScheduler, type WatchScheduler } from "../src/worker/detach.js";
import { runAttempt, type WorkerDeps } from "../src/worker/worker.js";
import { openDatabase, type Db } from "../src/store/db.js";
import {
  getQuotaPool,
  getTask,
  insertTask,
  listEvents,
} from "../src/store/queries.js";
import {
  cleanupDir,
  dummyResolver,
  fakeLease,
  fixtureWebConfig,
  launchBrowser,
  sampleTask,
  scriptedAdapter,
  sdkFixture,
  tmpStateDir,
} from "./helpers.js";

let dir: string;
let db: Db;
let log: EventLog;

function workerDeps(extra: Partial<WorkerDeps> = {}): WorkerDeps {
  return { db, log, artifactsDir: join(dir, "artifacts"), ...extra };
}

function seedTask(overrides: Partial<Task> = {}): Task {
  const task = sampleTask({ task_id: `task-${Math.random().toString(36).slice(2)}`, ...overrides });
  insertTask(db, task);
  return task;
}

const scriptedRun = (
  adapter: ReturnType<typeof scriptedAdapter>,
  task: Task,
  extra: Partial<WorkerDeps> = {}
) =>
  runAttempt(workerDeps(extra), {
    taskId: task.task_id,
    adapter,
    accountId: "acct-fw-1",
    page: fakeLease(),
    makeResolver: () => dummyResolver(),
  });

describe("worker happy path (fixture-web declarative adapter, real browser)", () => {
  let browser: Browser;
  beforeAll(async () => {
    browser = await launchBrowser();
  }, 30000);
  afterAll(async () => {
    await browser.close();
  }, 30000);
  beforeEach(() => {
    dir = tmpStateDir();
    db = openDatabase(":memory:");
    log = new EventLog(db);
  });
  afterEach(() => {
    db.close();
    cleanupDir(dir);
  });

  it("submitted/progress/done land in the ledger in order; task completes; attempt acknowledged", async () => {
    const adapter = new DeclarativeChatAdapter(fixtureWebConfig());
    const page = await browser.newPage();
    await page.setContent(sdkFixture("complete.html"));
    const lease = createPageLease(page);
    const task = seedTask();

    const outcome = await runAttempt(workerDeps(), {
      taskId: task.task_id,
      adapter,
      accountId: "acct-fw-1",
      page: lease,
      makeResolver: (p: PageLease) => createResolver((p as SdkPageLease).page, adapter.pack),
    });
    await page.close();

    expect(outcome).toEqual({ kind: "terminal", status: "completed" });

    const final = getTask(db, task.task_id);
    expect(final?.status).toBe("completed");
    expect(final?.completed_at).not.toBeNull();
    expect(final?.result?.text).toContain("full result");

    expect(final?.attempts).toHaveLength(1);
    expect(final?.attempts[0].submission_state).toBe("acknowledged");
    expect(final?.attempts[0].outcome).toBe("success");

    const kinds = listEvents(db, task.task_id).map((e) => e.kind);
    expect(kinds[0]).toBe("task.status"); // running
    const seqs = listEvents(db, task.task_id).map((e) => e.seq);
    expect(seqs).toEqual([...seqs].sort((a, b) => a - b));
    expect(kinds.indexOf("submitted")).toBeGreaterThan(-1);
    expect(kinds.indexOf("submitted")).toBeLessThan(kinds.indexOf("reply"));
    expect(kinds.indexOf("reply")).toBeLessThan(kinds.indexOf("done"));
    expect(kinds[kinds.length - 1]).toBe("task.status"); // completed
  }, 30000);
});

describe("worker (scripted fake adapter, no browser)", () => {
  beforeEach(() => {
    dir = tmpStateDir();
    db = openDatabase(":memory:");
    log = new EventLog(db);
  });
  afterEach(() => {
    db.close();
    cleanupDir(dir);
  });

  it("markSubmitted is durably persisted before the adapter continues (two-write)", async () => {
    const observed: string[] = [];
    const adapter = scriptedAdapter({
      execute: async function* (task, ctx) {
        adapter.submissionCount += 1;
        await ctx.markSubmitted(null); // sent_unconfirmed BEFORE Send
        observed.push(getTask(db, task.task_id)?.attempts[0].submission_state ?? "missing");
        await ctx.markSubmitted("fw-thread-9"); // acknowledged after provider ack
        observed.push(getTask(db, task.task_id)?.attempts[0].submission_state ?? "missing");
        yield { t: "submitted", provider_thread_id: "fw-thread-9", provider_url: null };
        yield { t: "done", outcome: "success", text: "ok" };
      },
    });
    const task = seedTask();
    const outcome = await scriptedRun(adapter, task);
    expect(outcome).toEqual({ kind: "terminal", status: "completed" });
    expect(observed).toEqual(["sent_unconfirmed", "acknowledged"]);
    const attempt = getTask(db, task.task_id)?.attempts[0];
    expect(attempt?.submission_state).toBe("acknowledged");
    expect(attempt?.provider_thread_id).toBe("fw-thread-9");
    expect(adapter.submissionCount).toBe(1);
  });

  it("quota.signal updates the pool row (limit_banner → degraded)", async () => {
    const adapter = scriptedAdapter({
      execute: async function* (task) {
        yield {
          t: "quota.signal",
          pool_id: "fixture-pool",
          signal: {
            kind: "limit_banner",
            raw_excerpt: "limit reached",
            observed_at: new Date().toISOString(),
            task_id: task.task_id,
          },
        };
        yield { t: "done", outcome: "success" };
      },
    });
    const task = seedTask();
    await scriptedRun(adapter, task);
    const pool = getQuotaPool(db, "fixture-web:acct-fw-1:fixture-pool");
    expect(pool?.state).toBe("degraded");
    expect(pool?.last_signal?.kind).toBe("limit_banner");
  });

  it("model.observed mismatch on a reasoning request marks the pool degraded (§A4)", async () => {
    const adapter = scriptedAdapter({
      execute: async function* () {
        yield { t: "model.observed", model: "standard" };
        yield { t: "done", outcome: "success" };
      },
    });
    const task = seedTask({ options: { model_class: "reasoning" } });
    await scriptedRun(adapter, task);
    const attempt = getTask(db, task.task_id)?.attempts[0];
    expect(attempt?.requested_model_class).toBe("reasoning");
    expect(attempt?.observed_model).toBe("standard");
    const pool = getQuotaPool(db, "fixture-web:acct-fw-1:fixture-pool");
    expect(pool?.state).toBe("degraded");
    expect(pool?.last_signal?.kind).toBe("model_downgraded");
  });

  it("needs_user pauses the task with status_detail (does not fail it)", async () => {
    const adapter = scriptedAdapter({
      execute: async function* () {
        yield { t: "needs_user", reason: "auth", message: "not logged in" };
      },
    });
    const task = seedTask();
    const outcome = await scriptedRun(adapter, task);
    expect(outcome).toEqual({ kind: "terminal", status: "needs_user" });
    const final = getTask(db, task.task_id);
    expect(final?.status).toBe("needs_user");
    expect(final?.status_detail).toBe("not logged in");
    expect(final?.error?.class).toBe("auth_required");
  });

  it("error while sent_unconfirmed → submission_ambiguous, never resubmitted", async () => {
    const adapter = scriptedAdapter({
      execute: async function* (_task, ctx) {
        adapter.submissionCount += 1;
        await ctx.markSubmitted(null);
        yield {
          t: "error",
          error: {
            class: "network_error",
            scope: "task",
            retryable: true,
            fallback_eligible: true,
            cooldown_s: null,
            user_action: null,
            detail: "socket reset after send",
            evidence_ref: null,
          },
        };
      },
    });
    const task = seedTask();
    const outcome = await scriptedRun(adapter, task);
    expect(outcome).toEqual({ kind: "terminal", status: "failed" });
    const final = getTask(db, task.task_id);
    expect(final?.error?.class).toBe("submission_ambiguous");
    expect(final?.error?.retryable).toBe(false);
    expect(final?.error?.fallback_eligible).toBe(false);
    expect(adapter.submissionCount).toBe(1);
  });

  it("throw after acknowledged (browser died mid-reply) → stalled, never resubmitted", async () => {
    const adapter = scriptedAdapter({
      execute: async function* (_task, ctx) {
        adapter.submissionCount += 1;
        await ctx.markSubmitted(null);
        await ctx.markSubmitted("fw-thread-7");
        yield* [];
        throw new Error("locator.count: Target page, context or browser has been closed");
      },
    });
    const task = seedTask();
    const outcome = await scriptedRun(adapter, task);
    expect(outcome).toEqual({ kind: "terminal", status: "failed" });
    const final = getTask(db, task.task_id);
    expect(final?.error?.class).toBe("stalled");
    expect(final?.error?.retryable).toBe(false);
    expect(final?.error?.fallback_eligible).toBe(false);
    expect(adapter.submissionCount).toBe(1);
  });

  it("stream ends without a terminal event after acknowledged → stalled; while sent_unconfirmed → ambiguous", async () => {
    for (const [acks, expected] of [
      [["fw-thread-8"], "stalled"],
      [[], "submission_ambiguous"],
    ] as const) {
      const adapter = scriptedAdapter({
        execute: async function* (_task, ctx) {
          await ctx.markSubmitted(null);
          for (const id of acks) await ctx.markSubmitted(id);
          yield* [];
        },
      });
      const task = seedTask();
      await scriptedRun(adapter, task);
      const final = getTask(db, task.task_id);
      expect(final?.error?.class).toBe(expected);
      expect(final?.error?.retryable).toBe(false);
      expect(final?.error?.fallback_eligible).toBe(false);
    }
  });

  it("detached event → provider_running + watch scheduled; resume stream completes the task", async () => {
    const fired: Array<() => void> = [];
    const watchScheduler: WatchScheduler = createWatchScheduler({
      setTimeoutFn: (fn) => {
        fired.push(fn);
        return fn;
      },
      clearTimeoutFn: () => {},
    });
    const adapter = scriptedAdapter({
      execute: async function* (_task, ctx) {
        await ctx.markSubmitted(null);
        await ctx.markSubmitted("fw-thread-detached");
        yield { t: "submitted", provider_thread_id: "fw-thread-detached", provider_url: null };
        yield {
          t: "detached",
          resume_token: {
            token: "rt-1",
            adapter_id: "fixture-web",
            attempt_no: 1,
            issued_at: new Date().toISOString(),
            poll_after_s: 5,
          },
          poll_after_s: 5,
        };
      },
      resume: async function* () {
        yield { t: "progress", label: "provider working", fraction: 0.5 };
        yield { t: "done", outcome: "success", text: "resumed result" };
      },
    });
    const task = seedTask();
    const outcome = await scriptedRun(adapter, task, { watchScheduler });
    expect(outcome.kind).toBe("detached");
    expect(getTask(db, task.task_id)?.status).toBe("provider_running");
    expect(watchScheduler.pending()).toBe(1);

    expect(fired).toHaveLength(1);
    fired[0]();
    await vi.waitFor(() => {
      expect(getTask(db, task.task_id)?.status).toBe("completed");
    });
    expect(getTask(db, task.task_id)?.result?.text).toBe("resumed result");
    const kinds = listEvents(db, task.task_id).map((e) => e.kind);
    expect(kinds).toContain("detached");
    expect(kinds).toContain("progress");
  });
});
