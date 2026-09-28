// §A2/Critical #2 — crash recovery for sent_unconfirmed attempts: every one
// goes through adapter.reconcile(); blind resubmit is forbidden. A missing
// adapter or missing reconcile fn is treated as ambiguous (never resubmitted).
import type {
  ExecutionContext,
  SubscriptionAdapter,
  TaskAttempt,
  TaskError,
} from "@allternit/subscription-fabric-contracts";
import type { EventLog } from "../events/log.js";
import type { Db } from "../store/db.js";
import {
  getTask,
  listAttemptsBySubmissionState,
  updateAttempt,
  updateTaskStatus,
} from "../store/queries.js";

export type AdapterLookup = (adapterId: string) => SubscriptionAdapter | undefined;

export interface ReconcileDeps {
  log?: EventLog;
  // Real workers hand in a watch-page context; tests may omit it when the
  // fake adapter's reconcile ignores ctx.
  makeCtx?: (attempt: TaskAttempt, adapter: SubscriptionAdapter) => ExecutionContext;
  // §A8 — scope the sweep to one worker key (provider, account_id). Attempts
  // whose adapter is missing are included when the account matches (they take
  // the ambiguous path, which needs no adapter).
  filter?: { provider: string; account_id: string };
  // True while this process is still driving the task's attempt (a live
  // supervisor watchdog). Such attempts are not orphans and are left alone.
  isLive?: (taskId: string) => boolean;
}

// §A8/§A9 applied at restart: an attempt still in flight when the process
// died has no worker and no watchdog any more, so the stall rule is applied
// now instead of never. Retryable only when the submit never happened; a
// delivered prompt is never resubmitted (Critical #2).
function interruptedError(state: TaskAttempt["submission_state"], detail: string): TaskError {
  return {
    class: "stalled",
    scope: "task",
    retryable: state === "not_sent",
    fallback_eligible: state === "not_sent",
    cooldown_s: null,
    user_action:
      state === "not_sent"
        ? null
        : "The prompt reached the provider before the gateway restarted; read the reply in the provider thread. It is never resubmitted.",
    detail,
    evidence_ref: null,
  };
}

function ambiguousTaskError(detail: string): TaskError {
  return {
    class: "submission_ambiguous",
    scope: "task",
    retryable: false,
    fallback_eligible: false,
    cooldown_s: null,
    user_action: "Check the provider thread, then retry manually if absent",
    detail,
    evidence_ref: null,
  };
}

export async function reconcileAttempts(
  db: Db,
  adapters: AdapterLookup,
  deps: ReconcileDeps = {}
): Promise<void> {
  const pending = listAttemptsBySubmissionState(db, "sent_unconfirmed");

  for (const { task_id, attempt } of pending) {
    // Already settled (reconciled, stalled or ambiguous) by an earlier sweep.
    if (attempt.ended_at !== null) continue;
    const task = getTask(db, task_id);
    if (!task) continue;
    const adapter = adapters(attempt.adapter_id);
    if (deps.filter) {
      if (attempt.account_id !== deps.filter.account_id) continue;
      if (adapter && adapter.manifest.provider !== deps.filter.provider) continue;
    }
    const now = new Date().toISOString();

    const statusEvent = (status: string, detail?: string): void => {
      deps.log?.append({
        task_id,
        kind: "task.status",
        payload: { task_id, status, thread_id: task.thread_id, detail },
        callers: [task.requester.id],
      });
    };

    // The provider has the prompt: adopt the thread (never resubmit). Nothing
    // resumes observing a reconciled attempt, so it ends here rather than
    // sitting in running forever; the reply lives in the provider thread.
    const adoptDelivered = (threadId: string | null, why: string): void => {
      const error = interruptedError(
        "acknowledged",
        `adopted after restart (${why}); provider thread ${threadId ?? "unknown"}`
      );
      updateAttempt(db, task_id, attempt.attempt_no, {
        submission_state: "acknowledged",
        provider_thread_id: threadId,
        outcome: "failed",
        ended_at: now,
        error,
      });
      updateTaskStatus(db, task_id, "failed", { error, completedAt: now });
      statusEvent("failed", why);
    };

    if (!adapter || typeof adapter.reconcile !== "function" || !deps.makeCtx) {
      // No way to confirm — ambiguous, never resubmit.
      updateAttempt(db, task_id, attempt.attempt_no, { outcome: "ambiguous", ended_at: now });
      updateTaskStatus(db, task_id, "needs_user", {
        statusDetail: "submission outcome could not be reconciled (no reconcile path)",
      });
      statusEvent("needs_user", "reconcile unavailable");
      continue;
    }

    const result = await adapter.reconcile(attempt, deps.makeCtx(attempt, adapter));

    switch (result.outcome) {
      case "acknowledged":
        // The provider has it — adopt and resume; do not resubmit.
        adoptDelivered(result.provider_thread_id ?? attempt.provider_thread_id, "reconcile: acknowledged");
        break;
      case "duplicate":
        // Our earlier submit already landed — adopt the thread, never resubmit.
        adoptDelivered(
          result.provider_thread_id ?? attempt.provider_thread_id,
          "reconcile: duplicate adopted"
        );
        break;
      case "not_found": {
        const error = ambiguousTaskError(
          result.detail ?? "provider has no thread matching this attempt"
        );
        updateAttempt(db, task_id, attempt.attempt_no, {
          outcome: "ambiguous",
          ended_at: now,
          error,
        });
        updateTaskStatus(db, task_id, "failed", { error, completedAt: now });
        statusEvent("failed", "reconcile: not_found");
        break;
      }
      case "ambiguous":
        updateAttempt(db, task_id, attempt.attempt_no, { outcome: "ambiguous", ended_at: now });
        updateTaskStatus(db, task_id, "needs_user", {
          statusDetail: result.detail ?? "submission outcome ambiguous — manual check required",
        });
        statusEvent("needs_user", "reconcile: ambiguous");
        break;
    }
  }

  // Orphans: not_sent / acknowledged attempts still open from before the
  // restart (sent_unconfirmed were handled above via adapter.reconcile).
  for (const state of ["not_sent", "acknowledged"] as const) {
    for (const { task_id, attempt } of listAttemptsBySubmissionState(db, state)) {
      if (attempt.ended_at !== null) continue;
      if (deps.filter && attempt.account_id !== deps.filter.account_id) continue;
      const adapter = adapters(attempt.adapter_id);
      if (deps.filter && adapter && adapter.manifest.provider !== deps.filter.provider) continue;
      if (deps.isLive?.(task_id)) continue;
      const task = getTask(db, task_id);
      // In flight = running/streaming. provider_running (detached) is owned
      // by the watch scheduler, not a live attempt.
      if (!task || (task.status !== "running" && task.status !== "streaming")) continue;
      const now = new Date().toISOString();
      const error = interruptedError(
        state,
        `interrupted by gateway restart (submission_state ${state}${
          attempt.provider_thread_id ? `, provider thread ${attempt.provider_thread_id}` : ""
        })`
      );
      updateAttempt(db, task_id, attempt.attempt_no, { outcome: "failed", ended_at: now, error });
      updateTaskStatus(db, task_id, "failed", { error, completedAt: now });
      deps.log?.append({
        task_id,
        kind: "task.status",
        payload: { task_id, status: "failed", thread_id: task.thread_id, detail: "interrupted by restart" },
        callers: [task.requester.id],
      });
    }
  }
}
