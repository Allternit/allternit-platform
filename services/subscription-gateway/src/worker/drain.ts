// §A8 — the lane drain: turns scheduler enqueues into runAttempt calls. The
// scheduler is a pure synchronous FIFO with no notification channel, so the
// drain polls on an interval. One attempt per lane at a time (§A5: no
// parallel submits on one account); a failed attempt is logged and the loop
// continues — the worker's own outcome path (incl. P4 requeueAfterFailure via
// WorkerDeps.dispatch) owns retry/re-route semantics, never the drain.
import type { Task } from "@allternit/subscription-fabric-contracts";
import type { Scheduler } from "../queue/scheduler.js";
import type { Db } from "../store/db.js";
import { getTask, listAccounts } from "../store/queries.js";
import { runAttempt, type WorkerDeps } from "./worker.js";
import type { LaneKey, WorkerPool } from "./pool.js";
import { workerKeyId } from "./supervisor.js";

export interface DrainDeps {
  db: Db;
  scheduler: Scheduler;
  pool: WorkerPool;
  // Full WorkerDeps for every attempt (log/artifactsDir/supervisor/dispatch
  // wired by the caller). A factory so each attempt reads fresh closures.
  makeWorkerDeps: () => WorkerDeps;
  // Injectable for tests; real runAttempt by default.
  runAttemptFn?: typeof runAttempt;
  drainIntervalMs?: number; // default 250
  // Cooldown after a drain-driven activate() throws (e.g. profile locked):
  // the lane is not auto-retried until this elapses (no launch spam).
  activationCooldownMs?: number; // default 30000
  setIntervalFn?: (fn: () => void, ms: number) => unknown;
  clearIntervalFn?: (handle: unknown) => void;
  logger?: (line: string) => void;
}

// Lanes are enumerated from enabled account rows — never from queue contents:
// a task pinned at a lane with no account stays queued forever and no browser
// is ever launched for it. The unrouted lane is likewise never served here
// (router output lands tasks on real lanes; unresolved tasks wait for P5+).
export function startDrain(deps: DrainDeps): () => void {
  const intervalMs = deps.drainIntervalMs ?? 250;
  const cooldownMs = deps.activationCooldownMs ?? 30000;
  const setIntervalFn = deps.setIntervalFn ?? ((fn, ms) => setInterval(fn, ms));
  const clearIntervalFn = deps.clearIntervalFn ?? ((h) => clearInterval(h as NodeJS.Timeout));
  const run = deps.runAttemptFn ?? runAttempt;

  const running = new Set<string>(); // lanes with an in-flight attempt
  const activating = new Set<string>(); // lanes with a drain-driven activate in flight
  const activationBlockedUntil = new Map<string, number>(); // post-failure cooldown
  let stopped = false;

  const serve = (lane: LaneKey, task: Task): void => {
    const id = workerKeyId(lane);
    if (running.has(id)) return;

    const runtime = deps.pool.runtimeFor(lane);
    if (!runtime) {
      // §A8 recovery path: no resident runtime (fresh boot, post-crash) →
      // auto-activate (reconcile-first inside the pool), but only when the
      // lane has an account row — which it always does here — and no recent
      // activation failure. Probe-non-ready lanes then skip below until a
      // human re-drives activate() via the connect endpoint.
      if (activating.has(id)) return;
      const blockedUntil = activationBlockedUntil.get(id) ?? 0;
      if (Date.now() < blockedUntil) return;
      activating.add(id);
      deps
        .pool.activate(lane)
        .catch((err) => {
          activationBlockedUntil.set(id, Date.now() + cooldownMs);
          deps.logger?.(
            `subscription-gateway: drain activation for ${id} failed: ${err instanceof Error ? err.message : String(err)}`
          );
        })
        .finally(() => activating.delete(id));
      return;
    }
    if (deps.pool.healthFor(lane) !== "ready") return;

    // peek → next with no await between: single-threaded drain, so the pick
    // is atomic. The status re-check drops tasks cancelled/resolved while
    // they sat in the queue (the scheduler entry is then garbage, not work).
    const picked = deps.scheduler.next(lane.provider, lane.account_id);
    if (!picked || picked.task_id !== task.task_id) return;
    const fresh = getTask(deps.db, picked.task_id);
    if (!fresh || fresh.status !== "queued") return;

    running.add(id);
    run(deps.makeWorkerDeps(), {
      taskId: picked.task_id,
      adapter: runtime.adapter,
      accountId: lane.account_id,
      page: runtime.page,
      makeResolver: runtime.makeResolver,
    })
      .catch((err) => {
        deps.logger?.(
          `subscription-gateway: drain attempt for task ${picked.task_id} threw: ${err instanceof Error ? err.message : String(err)}`
        );
      })
      .finally(() => running.delete(id));
  };

  const tick = (): void => {
    if (stopped) return;
    for (const account of listAccounts(deps.db)) {
      if (!account.enabled) continue;
      const lane: LaneKey = { provider: account.provider, account_id: account.account_id };
      const task = deps.scheduler.peek(lane.provider, lane.account_id);
      if (!task) continue;
      serve(lane, task);
    }
  };

  const handle = setIntervalFn(tick, intervalMs);
  // A forgotten stop() must never hold the process (or a test runner) open.
  (handle as { unref?: () => void }).unref?.();
  return () => {
    stopped = true;
    clearIntervalFn(handle);
  };
}
