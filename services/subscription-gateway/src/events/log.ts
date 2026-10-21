// §A1/D11/D12 — append-only event ledger over the events table (mirrors the
// repo's bot_events pattern): monotonic per-task seq, fan-out to caller_outbox
// rows for subscribed callers and to the SSE hub, and terminal-state notify.
import type { AdapterEvent, TaskStatus } from "@allternit/subscription-fabric-contracts";
import type { Db } from "../store/db.js";
import {
  appendEvent,
  getTask,
  outboxEnqueue,
  type StoredEvent,
} from "../store/queries.js";
import type { SseHub } from "./sse.js";
import type { Notifier } from "./notify.js";

// §A1 AdapterEvent tags + D11 heartbeat + lifecycle + internal kinds.
export type EventKind = AdapterEvent["t"] | "progress.heartbeat" | "task.created" | "task.status" | "notify.failed";

export const TERMINAL_STATUSES = ["completed", "partial", "needs_user", "failed"] as const;
export type TerminalStatus = (typeof TERMINAL_STATUSES)[number];

export interface AppendInput {
  task_id: string;
  kind: EventKind;
  payload: unknown;
  callers: string[];
}

export class EventLog {
  private notifier: Notifier | null = null;

  constructor(
    private readonly db: Db,
    private readonly hub?: SseHub
  ) {}

  setNotifier(notifier: Notifier): void {
    this.notifier = notifier;
  }

  append(input: AppendInput): StoredEvent {
    const stored = appendEvent(this.db, {
      taskId: input.task_id,
      callerId: input.callers[0] ?? "system",
      kind: input.kind,
      payload: input.payload,
    });
    if (input.callers.length > 0) {
      outboxEnqueue(this.db, stored.event_id, input.callers);
    }
    const threadId = threadIdOf(input.payload);
    this.hub?.publish(input.task_id, stored, threadId);
    this.maybeNotify(stored, threadId);
    return stored;
  }

  private maybeNotify(stored: StoredEvent, threadId: string | null): void {
    if (!this.notifier || stored.kind !== "task.status") return;
    const status = (stored.payload as { status?: unknown })?.status;
    if (typeof status !== "string") return;
    if (!(TERMINAL_STATUSES as readonly string[]).includes(status)) return;
    const task = getTask(this.db, stored.task_id);
    void this.notifier.notifyTerminal({
      task_id: stored.task_id,
      status: status as TerminalStatus,
      caller_id: task?.requester.id ?? stored.caller_id,
      requester_kind: task?.requester.kind ?? "system",
      thread_id: task?.thread_id ?? threadId,
      detail: task?.status_detail ?? null,
    });
  }
}

function threadIdOf(payload: unknown): string | null {
  if (typeof payload !== "object" || payload === null) return null;
  const t = (payload as { thread_id?: unknown }).thread_id;
  return typeof t === "string" ? t : null;
}

export type { TaskStatus };
