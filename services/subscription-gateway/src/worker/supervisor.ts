// §A8 — worker supervisor: one worker per (provider, account_id), crash →
// restart, and the recovery rule: every sent_unconfirmed attempt is reconciled
// BEFORE the queue resumes. Per-attempt stall watchdog: no ledger event or
// heartbeat for stall_timeout_s (per capability; 90 chat / 1200 deep research,
// config-overridable) → fail as stalled (§A9: retryable only when not_sent).
import type { CapabilityId, Task, TaskError } from "@allternit/subscription-fabric-contracts";
import type { EventLog } from "../events/log.js";
import type { Db } from "../store/db.js";
import { getTask, updateAttempt, updateTaskStatus } from "../store/queries.js";
import type { Scheduler } from "../queue/scheduler.js";
import { reconcileAttempts, type AdapterLookup, type ReconcileDeps } from "./reconcile.js";

export interface WorkerKey {
  provider: string;
  account_id: string;
}

export function workerKeyId(key: WorkerKey): string {
  return `${key.provider}${key.account_id}`;
}

export type WorkerStatus = "reconciling" | "ready" | "crashed";

export interface SupervisorDeps {
  db: Db;
  scheduler: Scheduler;
  adapters: AdapterLookup;
  log?: EventLog;
  makeReconcileCtx?: ReconcileDeps["makeCtx"];
  // Per-capability stall timeout override (config). Default: 1200 s for
  // research.deep, 90 s otherwise (§A8).
  stallTimeoutS?: (capability: CapabilityId) => number;
  setTimeoutFn?: (fn: () => void, ms: number) => unknown;
  clearTimeoutFn?: (handle: unknown) => void;
}

interface WatchdogEntry {
  handle: unknown;
  attemptNo: number;
  timeoutS: number;
  onStall?: () => void;
}

export class WorkerSupervisor {
  private readonly workers = new Map<string, WorkerStatus>();
  private readonly watchdogs = new Map<string, WatchdogEntry>();
  private readonly setTimeoutFn: NonNullable<SupervisorDeps["setTimeoutFn"]>;
  private readonly clearTimeoutFn: NonNullable<SupervisorDeps["clearTimeoutFn"]>;

  constructor(private readonly deps: SupervisorDeps) {
    this.setTimeoutFn = deps.setTimeoutFn ?? ((fn, ms) => setTimeout(fn, ms));
    this.clearTimeoutFn = deps.clearTimeoutFn ?? ((h) => clearTimeout(h as NodeJS.Timeout));
  }

  status(key: WorkerKey): WorkerStatus | null {
    return this.workers.get(workerKeyId(key)) ?? null;
  }

  isReady(key: WorkerKey): boolean {
    return this.workers.get(workerKeyId(key)) === "ready";
  }

  // §A8 — (re)start: reconcile sent_unconfirmed BEFORE the queue resumes.
  async ensureWorker(key: WorkerKey): Promise<void> {
    const id = workerKeyId(key);
    if (this.workers.get(id) === "ready" || this.workers.get(id) === "reconciling") return;
    this.workers.set(id, "reconciling");
    await reconcileAttempts(this.deps.db, this.deps.adapters, {
      log: this.deps.log,
      makeCtx: this.deps.makeReconcileCtx,
      filter: key,
      isLive: (taskId) => this.watchdogs.has(taskId),
    });
    this.workers.set(id, "ready");
  }

  // Boot: settle every attempt the previous process left in flight, on every
  // lane, before anything is served. No browser runs yet, so sent_unconfirmed
  // takes the safe ambiguous path and orphans get the §A8/§A9 stall rule —
  // otherwise an orphan waits in running/streaming for the next task on its
  // lane (forever, if none comes).
  async sweepAtBoot(): Promise<void> {
    await reconcileAttempts(this.deps.db, this.deps.adapters, {
      log: this.deps.log,
      makeCtx: this.deps.makeReconcileCtx,
      isLive: (taskId) => this.watchdogs.has(taskId),
    });
  }

  // Crash → restart: the worker goes through the full recovery rule again
  // before its queue is served.
  async crash(key: WorkerKey): Promise<void> {
    this.workers.set(workerKeyId(key), "crashed");
    this.workers.delete(workerKeyId(key));
    await this.ensureWorker(key);
  }

  // Queue gate: a worker's scheduler lane is only served once ready.
  nextTask(key: WorkerKey): Task | null {
    if (!this.isReady(key)) return null;
    return this.deps.scheduler.next(key.provider, key.account_id);
  }

  stallTimeoutFor(capability: CapabilityId): number {
    if (this.deps.stallTimeoutS) return this.deps.stallTimeoutS(capability);
    return capability === "research.deep" ? 1200 : 90;
  }

  trackAttempt(
    taskId: string,
    attemptNo: number,
    capability: CapabilityId,
    opts: { onStall?: () => void } = {}
  ): void {
    const timeoutS = this.stallTimeoutFor(capability);
    const arm = (): unknown =>
      this.setTimeoutFn(() => this.onStall(taskId), timeoutS * 1000);
    this.watchdogs.set(taskId, { handle: arm(), attemptNo, timeoutS, onStall: opts.onStall });
  }

  // Every ledger event or adapter heartbeat re-arms the watchdog (D11:
  // "provider quiet but heartbeating with recent change" is not stuck).
  heartbeat(taskId: string): void {
    const w = this.watchdogs.get(taskId);
    if (!w) return;
    this.clearTimeoutFn(w.handle);
    w.handle = this.setTimeoutFn(() => this.onStall(taskId), w.timeoutS * 1000);
  }

  releaseAttempt(taskId: string): void {
    const w = this.watchdogs.get(taskId);
    if (!w) return;
    this.clearTimeoutFn(w.handle);
    this.watchdogs.delete(taskId);
  }

  private onStall(taskId: string): void {
    const w = this.watchdogs.get(taskId);
    if (!w) return;
    this.watchdogs.delete(taskId);
    const task = getTask(this.deps.db, taskId);
    if (!task) return;
    if (["completed", "partial", "failed", "cancelled", "needs_user"].includes(task.status)) return;
    const attempt = task.attempts.find((a) => a.attempt_no === w.attemptNo);
    // §A9 — stalled is retryable only when the submit never happened.
    const error: TaskError = {
      class: "stalled",
      scope: "task",
      retryable: attempt?.submission_state === "not_sent",
      fallback_eligible: true,
      cooldown_s: null,
      user_action: null,
      detail: `no events for ${w.timeoutS}s (stall_timeout_s)`,
      evidence_ref: null,
    };
    const now = new Date().toISOString();
    updateAttempt(this.deps.db, taskId, w.attemptNo, {
      ended_at: now,
      outcome: "failed",
      error,
    });
    updateTaskStatus(this.deps.db, taskId, "failed", { error, completedAt: now });
    this.deps.log?.append({
      task_id: taskId,
      kind: "task.status",
      payload: { task_id: taskId, status: "failed", thread_id: task.thread_id },
      callers: [task.requester.id],
    });
    w.onStall?.();
  }

  shutdown(): void {
    for (const w of this.watchdogs.values()) this.clearTimeoutFn(w.handle);
    this.watchdogs.clear();
  }
}
