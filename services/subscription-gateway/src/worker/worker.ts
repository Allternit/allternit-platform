// §A1/§A8 — the worker executor: wires the SDK ExecutionContext (real artifact
// sink, manifest pacer, selector resolver, redacting logger, durable two-write
// markSubmitted) and consumes the adapter's AdapterEvent stream into the
// ledger, task transitions, and quota pools. Never resubmits (Critical #2).
import { createHash } from "node:crypto";
import {
  createExecutionContext,
  createPacer,
  createRedactingLogger,
} from "@allternit/subscription-adapter-sdk";
import type {
  AdapterEvent,
  ExecutionContext,
  PageLease,
  RedactingLogger,
  ResumeToken,
  SelectorResolver,
  SubscriptionAdapter,
  Task,
  TaskAttempt,
  TaskError,
} from "@allternit/subscription-fabric-contracts";
import { createArtifactStore } from "../artifacts/store.js";
import { recordAdapterSuccess, recordAdapterUiFailure } from "../breakers.js";
import type { EventLog } from "../events/log.js";
import { applySignalToPool, recordLocalUse, recordPoolSuccess } from "../pools.js";
import { requeueAfterFailure, type DispatchDeps } from "../router/dispatch.js";
import type { Db } from "../store/db.js";
import {
  getTask,
  insertAttempt,
  listArtifactsForTask,
  recordThreadTurn,
  updateAttempt,
  updateTaskStatus,
} from "../store/queries.js";
import type { WatchScheduler } from "./detach.js";
import { appendAdapterEvent, type ActivityTracker } from "./progress.js";
import type { WorkerSupervisor } from "./supervisor.js";

export function promptFingerprint(task: Task): string {
  const normalized = task.prompt.replace(/\s+/g, " ").trim();
  const inputHashes = task.inputs.map((i) =>
    i.type === "file" ? i.sha256 : JSON.stringify(i)
  );
  return createHash("sha256")
    .update(normalized + "\n" + inputHashes.join("\n"))
    .digest("hex");
}

export interface WorkerDeps {
  db: Db;
  log: EventLog;
  artifactsDir: string;
  logger?: RedactingLogger;
  watchScheduler?: WatchScheduler;
  activity?: ActivityTracker;
  // §A8 watchdog: armed per attempt, heartbeated per event, released on
  // terminal; on stall it aborts the stream and fails the task itself.
  supervisor?: WorkerSupervisor;
  // Supervisor heartbeat hook — every adapter event resets the stall watchdog.
  onEvent?: (taskId: string, event: AdapterEvent) => void;
  // P4 — when wired, a terminally failed attempt goes through
  // router.onAttemptFailed: re-routable failures re-queue the task onto the
  // next candidate's lane; "stop"/no-route leaves it failed (dispatch.ts).
  dispatch?: DispatchDeps;
}

export interface RunRequest {
  taskId: string;
  adapter: SubscriptionAdapter;
  accountId: string;
  page: PageLease;
  makeResolver: (page: PageLease) => SelectorResolver;
  signal?: AbortSignal;
}

export type RunOutcome =
  | { kind: "terminal"; status: "completed" | "partial" | "failed" | "needs_user" }
  | { kind: "detached"; resumeToken: ResumeToken };

const NULL_LOGGER: RedactingLogger = {
  debug: () => {},
  info: () => {},
  warn: () => {},
  error: () => {},
};

// §A2/Critical #2 — an error surfacing while the provider may already hold the
// prompt is submission_ambiguous: never retryable, never fallback-eligible.
function submissionAmbiguousError(detail: string): TaskError {
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

// §A8/§A9 — an attempt that dies after the provider acknowledged the prompt is
// stalled, never resent: the reply may still land in the provider thread. Same
// classification the restart sweep gives an orphaned `acknowledged` attempt.
function acknowledgedStallError(detail: string): TaskError {
  return {
    class: "stalled",
    scope: "task",
    retryable: false,
    fallback_eligible: false,
    cooldown_s: null,
    user_action:
      "The prompt reached the provider before the attempt failed; read the reply in the provider thread. It is never resubmitted.",
    detail,
    evidence_ref: null,
  };
}

function needsUserError(reason: "auth" | "challenge" | "confirm_dialog", message: string): TaskError {
  const base = {
    scope: "account" as const,
    retryable: false,
    cooldown_s: null,
    evidence_ref: null,
    detail: message,
  };
  if (reason === "challenge") {
    return { ...base, class: "challenge_presented", fallback_eligible: true, user_action: message };
  }
  if (reason === "confirm_dialog") {
    return { ...base, class: "user_intervention_required", fallback_eligible: false, user_action: message };
  }
  return { ...base, class: "auth_required", fallback_eligible: true, user_action: message };
}

export async function runAttempt(deps: WorkerDeps, req: RunRequest): Promise<RunOutcome> {
  const { db, log } = deps;
  const task = getTask(db, req.taskId);
  if (!task) throw new Error(`task ${req.taskId} not found`);

  const manifest = req.adapter.manifest;
  const capability = manifest.capabilities.find((c) => c.id === task.capability);
  const poolId = capability?.pool_id ?? "unknown";
  const poolKey = `${manifest.provider}:${req.accountId}:${poolId}`;
  const requestedModelClass =
    typeof task.options.model_class === "string" ? task.options.model_class : null;

  const attempt: TaskAttempt = {
    attempt_no: task.attempts.length + 1,
    adapter_id: manifest.adapter_id,
    adapter_version: manifest.adapter_version,
    account_id: req.accountId,
    pool_key: poolKey,
    submission_state: "not_sent",
    prompt_fingerprint: promptFingerprint(task),
    provider_thread_id: null,
    requested_model_class: requestedModelClass,
    observed_model: null,
    started_at: new Date().toISOString(),
    ended_at: null,
    outcome: "failed",
    error: null,
  };
  insertAttempt(db, req.taskId, attempt);

  const controller = new AbortController();
  req.signal?.addEventListener("abort", () => controller.abort(), { once: true });
  deps.supervisor?.trackAttempt(req.taskId, attempt.attempt_no, task.capability, {
    onStall: () => controller.abort(),
  });

  const setStatus = (
    status: Parameters<typeof updateTaskStatus>[2],
    opts: Parameters<typeof updateTaskStatus>[3] = {}
  ): void => {
    updateTaskStatus(db, req.taskId, status, opts);
    log.append({
      task_id: req.taskId,
      kind: "task.status",
      payload: { task_id: req.taskId, status, thread_id: task.thread_id },
      callers: [task.requester.id],
    });
  };
  setStatus("running");

  const sink = createArtifactStore(db, {
    artifactsDir: deps.artifactsDir,
    source: {
      task_id: task.task_id,
      attempt_no: attempt.attempt_no,
      capability: task.capability,
      provider: manifest.provider,
      account_id: req.accountId,
      adapter_id: manifest.adapter_id,
      adapter_version: manifest.adapter_version,
      thread_id: task.thread_id,
      project_id: task.project_id,
      bot_id: task.requester.bot_id ?? null,
      sensitivity: task.constraints.sensitivity,
    },
  });

  const ctx = createExecutionContext({
    page: req.page,
    sink,
    pacer: createPacer(manifest.pacing),
    resolver: req.makeResolver(req.page),
    logger: createRedactingLogger(deps.logger ?? NULL_LOGGER),
    attempt,
    signal: controller.signal,
    // §A1 two-write, durable before the adapter continues: the SDK flips
    // not_sent → sent_unconfirmed → acknowledged across calls.
    onMarkSubmitted: async (providerThreadId, state) => {
      updateAttempt(db, req.taskId, attempt.attempt_no, {
        submission_state: state,
        provider_thread_id: providerThreadId,
      });
    },
  });

  const TERMINAL = ["completed", "partial", "failed", "cancelled", "needs_user"];
  // P4/§A4 — adapter failure classes feed the quota pools and the ui_drift
  // circuit breaker here at the worker boundary; the router itself stays pure.
  const feedFailure = (error: TaskError): void => {
    const now = new Date();
    if (error.class === "quota_exhausted" || error.class === "rate_limited") {
      applySignalToPool(
        db,
        poolKey,
        poolId,
        {
          kind: "hard_error",
          raw_excerpt: error.detail.slice(0, 500),
          observed_at: now.toISOString(),
          task_id: req.taskId,
        },
        now
      );
    } else if (error.class === "model_downgraded") {
      applySignalToPool(
        db,
        poolKey,
        poolId,
        {
          kind: "model_downgraded",
          raw_excerpt: error.detail.slice(0, 500),
          observed_at: now.toISOString(),
          task_id: req.taskId,
        },
        now
      );
    }
    if (error.class === "provider_ui_changed") {
      recordAdapterUiFailure(db, manifest.adapter_id, manifest.adapter_version, now);
    }
  };
  const failTerminal = (error: TaskError): RunOutcome => {
    const current = getTask(db, req.taskId);
    if (current && TERMINAL.includes(current.status)) {
      return { kind: "terminal", status: current.status as "failed" };
    }
    feedFailure(error);
    updateAttempt(db, req.taskId, attempt.attempt_no, {
      ended_at: new Date().toISOString(),
      outcome: "failed",
      error,
    });
    setStatus("failed", { error });
    return { kind: "terminal", status: "failed" };
  };

  // Shared event path for the interactive stream and detached resume streams.
  let providerUrl: string | null = null;
  const handleEvent = (event: AdapterEvent): RunOutcome | null => {
    deps.onEvent?.(req.taskId, event);
    deps.supervisor?.heartbeat(req.taskId);
    appendAdapterEvent(log, task, event, deps.activity);

    switch (event.t) {
      case "submitted":
        providerUrl = event.provider_url;
        return null;
      case "reply":
      case "progress":
      case "progress.heartbeat":
      case "artifact.partial": {
        const current = getTask(db, req.taskId);
        // provider_running stays put: progress over a detached watch is still
        // provider-side execution (Critical #3), not interactive streaming.
        if (current && current.status === "running") {
          setStatus("streaming");
        }
        return null;
      }
      case "quota.signal": {
        const key = `${manifest.provider}:${req.accountId}:${event.pool_id}`;
        applySignalToPool(db, key, event.pool_id, event.signal, new Date());
        return null;
      }
      case "model.observed": {
        updateAttempt(db, req.taskId, attempt.attempt_no, { observed_model: event.model });
        // §A4 — silent downgrade: requested reasoning/deep, observed differs → pool degraded.
        if (
          (requestedModelClass === "reasoning" || requestedModelClass === "deep") &&
          event.model !== requestedModelClass
        ) {
          applySignalToPool(
            db,
            poolKey,
            poolId,
            {
              kind: "model_downgraded",
              raw_excerpt: `requested ${requestedModelClass}, observed ${event.model}`.slice(0, 500),
              observed_at: new Date().toISOString(),
              task_id: req.taskId,
            },
            new Date()
          );
        }
        return null;
      }
      case "needs_user": {
        const error = needsUserError(event.reason, event.message);
        updateAttempt(db, req.taskId, attempt.attempt_no, {
          ended_at: new Date().toISOString(),
          outcome: "failed",
          error,
        });
        setStatus("needs_user", { statusDetail: event.message, error });
        return { kind: "terminal", status: "needs_user" };
      }
      case "detached":
        // Handled by the caller (needs the watch scheduler + adapter resume).
        return null;
      case "done": {
        const artifactIds = listArtifactsForTask(db, req.taskId)
          .filter((a) => a.storage.retrieval_state === "local")
          .map((a) => a.artifact_id);
        updateAttempt(db, req.taskId, attempt.attempt_no, {
          ended_at: new Date().toISOString(),
          outcome: event.outcome,
        });
        // §A4 — a completed real task is the one trustworthy recovery signal:
        // pool → available + cooldown rung reset, local-use counted against
        // the soft budget, and the ui_drift consecutive counter reset.
        const now = new Date();
        recordPoolSuccess(db, poolKey, now);
        recordLocalUse(db, poolKey, now);
        recordAdapterSuccess(db, manifest.adapter_id, manifest.adapter_version, now);
        // §S6 — a threaded chat turn maps (or advances) the fabric thread to
        // the provider thread; the fingerprint is the same sha256 of the
        // extracted last assistant turn that readThread computes, so the next
        // chat.continue can detect divergence.
        const threadId = ctx.attempt.provider_thread_id;
        if (task.thread_id && threadId && task.capability.startsWith("chat.") && event.text !== undefined) {
          recordThreadTurn(db, {
            thread_id: task.thread_id,
            provider: manifest.provider,
            account_id: req.accountId,
            adapter_id: manifest.adapter_id,
            provider_thread_id: threadId,
            provider_url: providerUrl && providerUrl.includes(threadId) ? providerUrl : `${manifest.origins[0] ?? ""}`,
            last_turn_fingerprint: createHash("sha256").update(event.text).digest("hex"),
            model_class: typeof task.options.model_class === "string" ? task.options.model_class : null,
          });
        }
        const status = event.outcome === "success" ? "completed" : "partial";
        setStatus(status, {
          completedAt: new Date().toISOString(),
          result: { artifact_ids: artifactIds, text: event.text },
        });
        return { kind: "terminal", status };
      }
      case "error": {
        const error =
          ctx.attempt.submission_state === "sent_unconfirmed"
            ? submissionAmbiguousError(event.error.detail)
            : event.error;
        return failTerminal(error);
      }
      default:
        return null;
    }
  };

  const consume = async (stream: AsyncIterable<AdapterEvent>): Promise<RunOutcome | null> => {
    for await (const event of stream) {
      if (event.t === "detached") {
        appendAdapterEvent(log, task, event, deps.activity);
        deps.onEvent?.(req.taskId, event);
        deps.supervisor?.heartbeat(req.taskId);
        setStatus("provider_running", { statusDetail: "provider running server-side" });
        scheduleResume(event.resume_token, event.poll_after_s);
        return { kind: "detached", resumeToken: event.resume_token };
      }
      const outcome = handleEvent(event);
      if (outcome) return outcome;
    }
    return null;
  };

  // §A8 — watch-page resume: read-only context; markSubmitted is not wired
  // (watch pages never submit), so a resume stream cannot perform the
  // two-write even if adapter code misbehaves.
  const scheduleResume = (token: ResumeToken, pollAfterS: number): void => {
    const resume = req.adapter.resume;
    if (!deps.watchScheduler || !resume) return;
    const watchCtx: ExecutionContext = {
      ...ctx,
      markSubmitted: async () => {
        throw new Error("watch pages are read-only: markSubmitted is not available");
      },
    };
    deps.watchScheduler.scheduleWatch(token, pollAfterS, async (t) => {
      const outcome = await consume(resume.call(req.adapter, t, watchCtx));
      const terminal = outcome !== null && outcome.kind === "terminal";
      if (terminal) deps.supervisor?.releaseAttempt(req.taskId);
      return terminal;
    });
  };

  // P4 — a terminally failed attempt is the §A2 hop: the router re-checks
  // policy on a fresh snapshot and either re-queues the task onto the next
  // candidate's lane or stops (task stays failed).
  const maybeRequeue = (outcome: RunOutcome): RunOutcome => {
    if (outcome.kind === "terminal" && outcome.status === "failed" && deps.dispatch) {
      requeueAfterFailure(deps.dispatch, req.taskId);
    }
    return outcome;
  };

  try {
    const outcome = await consume(req.adapter.execute(task, ctx));
    const noTerminal = "adapter stream ended without a terminal event";
    const final =
      outcome ??
      failTerminal(
        ctx.attempt.submission_state === "sent_unconfirmed"
          ? submissionAmbiguousError(noTerminal)
          : ctx.attempt.submission_state === "acknowledged"
            ? acknowledgedStallError(noTerminal)
            : {
                class: "provider_error",
                scope: "task",
                retryable: true,
                fallback_eligible: true,
                cooldown_s: null,
                user_action: null,
                detail: noTerminal,
                evidence_ref: null,
              }
      );
    // The watchdog stays armed across detach (D11: watch polls heartbeat it).
    if (final.kind === "terminal") deps.supervisor?.releaseAttempt(req.taskId);
    return maybeRequeue(final);
  } catch (err) {
    deps.supervisor?.releaseAttempt(req.taskId);
    const detail = err instanceof Error ? err.message : String(err);
    if (ctx.attempt.submission_state === "sent_unconfirmed") {
      return maybeRequeue(failTerminal(submissionAmbiguousError(detail)));
    }
    if (ctx.attempt.submission_state === "acknowledged") {
      return maybeRequeue(failTerminal(acknowledgedStallError(detail)));
    }
    return maybeRequeue(
      failTerminal({
        class: "provider_error",
        scope: "task",
        retryable: true,
        fallback_eligible: true,
        cooldown_s: null,
        user_action: null,
        detail,
        evidence_ref: null,
      })
    );
  }
}
