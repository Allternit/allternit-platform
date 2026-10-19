// Model-picker chats (gizzi's `subs-*` models → POST /v1/tasks) on the same
// RouteDirector as vendor bot turns (Eoj, 2026-10-17: "same router for
// both"). At turn start — task submission — the director picks the vendor's
// route. A web route keeps today's path (FabricRouter + the Chrome worker).
// A phone or desktop-app route runs the turn through that route's AAI adapter
// (agent.context.open + agent.context.message, the calls a vendor bot turn
// makes) and writes the task exactly as the web worker does: task.status
// running → `reply` events with reply.text.delta → task.status completed with
// result.text, or failed with a TaskError. /v1/tasks readers see no
// difference. Failover stays at turn start: a running turn is never re-routed.
import type { Task, TaskError } from "@allternit/subscription-fabric-contracts";
import type { EventKind, EventLog } from "../events/log.js";
import type { Db } from "../store/db.js";
import { getActiveThreadMapping, getTask, insertAttempt, listAttempts, updateAttempt, updateTaskStatus } from "../store/queries.js";
import { promptFingerprint } from "../worker/worker.js";

/** Routes whose turns run through an AAI adapter instead of the web worker. */
export const APP_ROUTES = new Set([
  "android-app",
  "iphone-app-ictrl",
  "iphone-app-usb-agent",
  "desktop-app-cdp",
  "desktop-app-ax",
]);

/** What a person reads while the turn starts, per route. */
const ROUTE_LABEL: Record<string, string> = {
  "android-app": "Using the app on your Android phone",
  "iphone-app-ictrl": "Using the app on your iPhone",
  "iphone-app-usb-agent": "Using the app on your iPhone",
  "desktop-app-cdp": "Using the desktop app",
  "desktop-app-ax": "Using the desktop app",
};

type AaiResultLike = { ok: true; value: unknown } | { ok: false; error: { code: string; humanMessage: string; retryable?: boolean; details?: Record<string, unknown> } };

export interface AppTurnHost {
  call(op: string, binding: Record<string, unknown>, input: Record<string, unknown>): Promise<AaiResultLike>;
}

export interface TurnDirector {
  routeForTurn(vendor: string): Promise<
    | { ok: true; route: string; adapterId: string | null; rebound: boolean; reason: string }
    | { ok: false; error: "NO_ROUTE"; reason: string }
  >;
}

export interface AppRoutePick {
  route: string;
  adapterId: string;
  reason: string;
}

export interface AppRouteTasksDeps {
  db: Db;
  log: EventLog;
  host: AppTurnHost;
  director: TurnDirector;
  /** agent.events poll while a reply is on its way (default 750 ms). */
  pollMs?: number;
}

const CHAT_CAPABILITIES = new Set(["chat.create", "chat.continue"]);

function fromAaiError(e: { code: string; humanMessage: string; retryable?: boolean }): TaskError {
  const cls: TaskError["class"] =
    e.code === "AUTH_REQUIRED" || e.code === "AUTH_REVOKED"
      ? "auth_required"
      : e.code === "RATE_LIMITED"
        ? "rate_limited"
        : e.code === "TIMEOUT"
          ? "timeout"
          : e.code === "ADAPTER_DRIFT"
          ? "provider_ui_changed"
          : e.code === "UNSUPPORTED"
            ? "unsupported_capability"
            : e.code === "APPROVAL_REQUIRED"
              ? "user_intervention_required"
              : "provider_error";
  return {
    class: cls,
    scope: "task",
    retryable: e.retryable ?? false,
    // The next turn may pick another route (failover at turn start).
    fallback_eligible: true,
    cooldown_s: null,
    user_action: null,
    detail: e.humanMessage,
    evidence_ref: null,
  };
}

/** Fabric thread → the app conversation it continues in (table app_route_threads, migration 0013). */
class ThreadStore {
  constructor(private readonly db: Db) {}
  has(threadId: string): boolean {
    return this.get(threadId) !== undefined;
  }
  get(threadId: string): { adapterId: string; contextId: string } | undefined {
    const row = this.db.prepare("SELECT adapter_id, context_id FROM app_route_threads WHERE thread_id = ?").get(threadId) as
      | { adapter_id: string; context_id: string }
      | undefined;
    return row ? { adapterId: row.adapter_id, contextId: row.context_id } : undefined;
  }
  set(threadId: string, v: { adapterId: string; contextId: string }): void {
    this.db
      .prepare(
        "INSERT INTO app_route_threads (thread_id, adapter_id, context_id, updated_at) VALUES (?, ?, ?, ?) " +
          "ON CONFLICT(thread_id) DO UPDATE SET adapter_id = excluded.adapter_id, context_id = excluded.context_id, updated_at = excluded.updated_at"
      )
      .run(threadId, v.adapterId, v.contextId, new Date().toISOString());
  }
  delete(threadId: string): void {
    this.db.prepare("DELETE FROM app_route_threads WHERE thread_id = ?").run(threadId);
  }
}

export class AppRouteTasks {
  private readonly threads: ThreadStore;
  /** Running turns: task → what cancel needs. */
  private readonly running = new Map<string, { adapterId: string; contextId: string | null; cancelled: boolean; sent?: boolean }>();
  private readonly inFlight = new Set<Promise<void>>();

  constructor(private readonly deps: AppRouteTasksDeps) {
    this.threads = new ThreadStore(deps.db);
  }

  /**
   * Turn start. Returns the app route to run on, or null for today's web path:
   * not a chat, no provider pin, a thread that already lives in a web
   * account, or the director picked web / found no route (the FabricRouter
   * then decides as before and records its own reasons).
   */
  async pick(task: Task): Promise<AppRoutePick | null> {
    if (!CHAT_CAPABILITIES.has(task.capability)) return null;
    const vendor = task.routing.provider;
    if (!vendor || task.routing.account_id !== undefined) return null;
    // A thread that has a web provider thread stays there: the conversation lives in that account.
    if (task.thread_id && getActiveThreadMapping(this.deps.db, task.thread_id)) return null;
    const r = await this.deps.director.routeForTurn(vendor);
    if (!r.ok || !APP_ROUTES.has(r.route) || !r.adapterId) return null;
    return { route: r.route, adapterId: r.adapterId, reason: r.reason };
  }

  /** A chat.continue whose thread this runner holds (no web mapping exists for it). */
  holdsThread(threadId: string | null): boolean {
    return !!threadId && this.threads.has(threadId);
  }

  isRunning(taskId: string): boolean {
    return this.running.has(taskId);
  }

  /** Stop a running app-route turn (the app's Stop button), then mark the task cancelled. */
  async cancel(taskId: string): Promise<boolean> {
    const r = this.running.get(taskId);
    if (!r) return false;
    r.cancelled = true;
    if (r.contextId) await this.deps.host.call("agent.context.cancel", this.binding(r.adapterId, taskId), { contextId: r.contextId }).catch(() => undefined);
    return true;
  }

  private binding(adapterId: string, key: string): Record<string, unknown> {
    // No `vendor`: the route was chosen at submission; the host must not choose again.
    return { id: `fabric-chat:${key}`, botId: "fabric-chat", type: "vendor", mode: "linked", adapterId, state: "READY" };
  }

  private setStatus(task: Task, status: Task["status"], opts: Parameters<typeof updateTaskStatus>[3] = {}): void {
    updateTaskStatus(this.deps.db, task.task_id, status, opts);
    this.deps.log.append({
      task_id: task.task_id,
      kind: "task.status",
      payload: { task_id: task.task_id, status, thread_id: task.thread_id },
      callers: [task.requester.id],
    });
  }

  private event(task: Task, kind: EventKind, payload: Record<string, unknown>): void {
    this.deps.log.append({ task_id: task.task_id, kind, payload: { ...payload, thread_id: task.thread_id }, callers: [task.requester.id] });
  }

  /** Run the turn in the background; the task row carries the outcome. */
  run(task: Task, pick: AppRoutePick): Promise<void> {
    const entry: { adapterId: string; contextId: string | null; cancelled: boolean; sent?: boolean } = { adapterId: pick.adapterId, contextId: null, cancelled: false };
    this.running.set(task.task_id, entry);
    const attemptNo = this.startAttempt(task, pick);
    const p: Promise<void> = this.turn(task, pick, entry)
      .catch((e: Error) => this.fail(task, { code: "UNKNOWN", humanMessage: `The turn stopped: ${e.message}` }))
      .finally(() => {
        this.endAttempt(task.task_id, attemptNo, entry.sent === true);
        this.running.delete(task.task_id);
        this.inFlight.delete(p);
      });
    this.inFlight.add(p);
    return p;
  }

  /** Resolves when every running turn has written its outcome (gateway shutdown, tests). */
  async settled(): Promise<void> {
    while (this.inFlight.size > 0) await Promise.allSettled([...this.inFlight]);
  }

  // The same task_attempts row the web worker writes, so /v1/stats/adapters
  // and the /v1/routes telemetry see app-route turns. There is no gateway
  // account behind a phone or desktop-app turn (the app holds the sign-in):
  // account_id is "app-route:<route>", pool_key "<vendor>:<route>". The
  // version is "0.0.0", the version the director's lanes report for AAI
  // adapters. submission_state never enters sent_unconfirmed, so restart
  // reconcile (web-only) never picks these rows up.
  private startAttempt(task: Task, pick: AppRoutePick): number {
    const attemptNo = listAttempts(this.deps.db, task.task_id).length + 1;
    insertAttempt(this.deps.db, task.task_id, {
      attempt_no: attemptNo,
      adapter_id: pick.adapterId,
      adapter_version: "0.0.0",
      account_id: `app-route:${pick.route}`,
      pool_key: `${task.routing.provider ?? "unknown"}:${pick.route}`,
      submission_state: "not_sent",
      prompt_fingerprint: promptFingerprint(task),
      provider_thread_id: null,
      requested_model_class: null,
      observed_model: null,
      started_at: new Date().toISOString(),
      ended_at: null,
      outcome: "failed",
      error: null,
    });
    return attemptNo;
  }

  /** Close the attempt from the task row the turn wrote (completed / cancelled / failed). */
  private endAttempt(taskId: string, attemptNo: number, sent: boolean): void {
    try {
      const t = getTask(this.deps.db, taskId);
      const outcome = t?.status === "completed" ? "success" : t?.status === "cancelled" ? "cancelled" : "failed";
      updateAttempt(this.deps.db, taskId, attemptNo, {
        ended_at: new Date().toISOString(),
        outcome,
        // Sent to the app and the call returned (reply or a typed error): acknowledged.
        submission_state: outcome === "success" || sent ? "acknowledged" : "not_sent",
        ...(outcome === "failed" && t?.error ? { error: t.error } : {}),
      });
    } catch {
      // Bookkeeping never breaks a turn.
    }
  }

  private fail(task: Task, e: { code: string; humanMessage: string; retryable?: boolean }): void {
    if (getTask(this.deps.db, task.task_id)?.status === "cancelled") return;
    this.setStatus(task, "failed", { completedAt: new Date().toISOString(), error: fromAaiError(e), statusDetail: e.humanMessage });
  }

  private async turn(task: Task, pick: AppRoutePick, entry: { contextId: string | null; cancelled: boolean; sent?: boolean }): Promise<void> {
    const { host } = this.deps;
    const key = task.thread_id ?? task.task_id;
    const binding = this.binding(pick.adapterId, key);
    this.setStatus(task, "running", { statusDetail: `via ${pick.route}` });
    this.event(task, "progress", { t: "progress", label: ROUTE_LABEL[pick.route] ?? "Using the app" });

    // The thread's app conversation, or a new one (a new chat in the app).
    let held = task.thread_id ? this.threads.get(task.thread_id) : undefined;
    if (held && held.adapterId !== pick.adapterId) held = undefined;
    let contextId = held?.contextId ?? null;
    if (!contextId) {
      const agents = await host.call("agent.list", binding, {});
      if (!agents.ok) return this.fail(task, agents.error);
      const list = agents.value as Array<{ agentId?: string; id?: string }>;
      const agentId = list[0]?.agentId ?? list[0]?.id;
      if (!agentId) return this.fail(task, { code: "VENDOR_UNAVAILABLE", humanMessage: `${pick.adapterId} has no app to talk to.` });
      const opened = await host.call("agent.context.open", binding, { agentId, title: task.prompt.slice(0, 80), threadId: key });
      if (!opened.ok) return this.fail(task, opened.error);
      contextId = (opened.value as { contextId: string }).contextId;
      if (task.thread_id) this.threads.set(task.thread_id, { adapterId: pick.adapterId, contextId });
    }
    entry.contextId = contextId;
    if (entry.cancelled) return;

    // Stream: the app's deltas are snapshots of the reply so far; send only growth that extends what was sent.
    let streamed = "";
    let cursor: string | undefined;
    let done = false;
    const stream = (text: string) => {
      if (!text.startsWith(streamed) || text.length === streamed.length) return;
      if (streamed === "") this.event(task, "reply", { t: "reply", event: { type: "reply.started", replyId: task.task_id } });
      const delta = text.slice(streamed.length);
      streamed = text;
      this.event(task, "reply", { t: "reply", event: { type: "reply.text.delta", replyId: task.task_id, delta } });
    };
    const poll = (async () => {
      while (!done) {
        await new Promise((r) => setTimeout(r, this.deps.pollMs ?? 750));
        if (done) break;
        const r = await host.call("agent.events", binding, { contextId, ...(cursor ? { cursor } : {}) }).catch(() => null);
        if (!r || !r.ok) continue;
        const v = r.value as { events: Array<Record<string, any>>; cursor?: string };
        cursor = v.cursor ?? cursor;
        for (const ev of v.events) {
          const corr = ev.correlation_id ?? ev.correlationId;
          if (ev.type === "agent.message.delta" && corr === task.task_id && typeof ev.payload?.text === "string") stream(ev.payload.text);
        }
      }
    })();

    entry.sent = true;
    const sent = await host.call("agent.context.message", binding, { contextId, text: task.prompt, correlationId: task.task_id });
    done = true;
    await poll;
    if (getTask(this.deps.db, task.task_id)?.status === "cancelled") return;
    if (!sent.ok) {
      // A lost app conversation starts fresh next turn.
      if (task.thread_id && sent.error.code === "CONTEXT_NOT_FOUND") this.threads.delete(task.thread_id);
      return this.fail(task, sent.error);
    }
    const reply = (sent.value as { reply?: string }).reply;
    if (!reply) {
      return this.fail(task, { code: "TIMEOUT", humanMessage: "The app didn't finish its reply in time.", retryable: true });
    }
    stream(reply);
    this.setStatus(task, "completed", { completedAt: new Date().toISOString(), result: { artifact_ids: [], text: reply } });
  }
}
