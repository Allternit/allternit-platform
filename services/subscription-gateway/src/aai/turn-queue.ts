// Per-account FIFO for turns on UI-bridge lanes (desktop apps, subscription browser tabs). Such a lane drives one
// signed-in window/tab per account and runs at most its manifest's `context.maxParallel` turns at once; an extra turn
// waits here in arrival order (bounded wait, bounded queue) instead of failing or racing the running one. Never opens
// another window or tab for the account.
import { fail, ok, type AaiResult } from "@allternit/agent-gateway";

export interface TurnQueueOptions {
  /** Longest a turn waits for the account before giving up (default 10 min). */
  maxWaitMs?: number;
  /** Turns allowed to wait per account; one more answers RATE_LIMITED (default 8). */
  maxQueued?: number;
}
export const DEFAULT_QUEUE_WAIT_MS = 600_000;
export const DEFAULT_QUEUE_MAX = 8;

export interface QueueTicket {
  /** The context the turn belongs to (lets cancel find it); absent for an open. */
  contextId?: string;
  /** "your Grok app", for messages. */
  label: string;
  /** Called with the turn's 1-based place in line whenever it changes, and with 0 when it starts. */
  onPosition?: (position: number) => void;
}

interface Waiter extends QueueTicket { grant: (r: AaiResult<() => void>) => void; timer: NodeJS.Timeout; last?: number }
interface Lane { running: Map<string, number>; capacity: number; waiting: Waiter[] }

export class TurnQueue {
  private lanes = new Map<string, Lane>();
  readonly maxWaitMs: number;
  readonly maxQueued: number;

  constructor(opts: TurnQueueOptions = {}) {
    this.maxWaitMs = opts.maxWaitMs ?? DEFAULT_QUEUE_WAIT_MS;
    this.maxQueued = opts.maxQueued ?? DEFAULT_QUEUE_MAX;
  }

  private lane(account: string, capacity: number): Lane {
    let l = this.lanes.get(account);
    if (!l) { l = { running: new Map(), capacity, waiting: [] }; this.lanes.set(account, l); }
    l.capacity = Math.max(1, capacity);
    return l;
  }
  private size(l: Lane): number { let n = 0; for (const v of l.running.values()) n += v; return n; }

  /** Waits for a running slot; ok carries the release function (call it exactly once when the turn ends). */
  acquire(account: string, capacity: number, t: QueueTicket): Promise<AaiResult<() => void>> {
    const l = this.lane(account, capacity);
    if (l.waiting.length === 0 && this.size(l) < l.capacity) return Promise.resolve(ok(this.start(l, t.contextId)));
    if (l.waiting.length >= this.maxQueued) {
      return Promise.resolve(fail("RATE_LIMITED", `${l.waiting.length} messages are already waiting for ${t.label}. Try again when they have gone through.`, {
        retryable: true, retryAfterMs: 30_000, details: { queued: l.waiting.length },
      }));
    }
    return new Promise((resolve) => {
      const w: Waiter = { ...t, grant: resolve, timer: setTimeout(() => {
        this.drop(l, w);
        resolve(fail("CONTEXT_BUSY", `Waited ${Math.round(this.maxWaitMs / 60_000) || 1} min for ${t.label} and it is still busy with another message. Try again later.`, {
          retryable: true, retryAfterMs: 30_000, details: { queueTimeout: true },
        }));
      }, this.maxWaitMs) };
      w.timer.unref?.();
      l.waiting.push(w);
      this.notify(l);
    });
  }

  /** Drops a context's waiting turns (they answer "cancelled"). Returns how many were dropped. */
  cancel(account: string, contextId: string): number {
    const l = this.lanes.get(account);
    if (!l) return 0;
    const gone = l.waiting.filter((w) => w.contextId === contextId);
    for (const w of gone) {
      this.drop(l, w);
      w.grant(fail("UNKNOWN", "The message was cancelled while it waited its turn.", { retryable: false, details: { cancelled: true, queued: true } }));
    }
    return gone.length;
  }

  /** Whether a turn of this context is running now (not just waiting). */
  isRunning(account: string, contextId: string): boolean { return (this.lanes.get(account)?.running.get(contextId) ?? 0) > 0; }
  waiting(account: string): number { return this.lanes.get(account)?.waiting.length ?? 0; }

  private start(l: Lane, contextId = ""): () => void {
    l.running.set(contextId, (l.running.get(contextId) ?? 0) + 1);
    let done = false;
    return () => {
      if (done) return;
      done = true;
      const n = (l.running.get(contextId) ?? 1) - 1;
      if (n > 0) l.running.set(contextId, n); else l.running.delete(contextId);
      this.pump(l);
    };
  }
  private pump(l: Lane): void {
    while (l.waiting.length && this.size(l) < l.capacity) {
      const w = l.waiting.shift()!;
      clearTimeout(w.timer);
      const release = this.start(l, w.contextId);
      try { w.onPosition?.(0); } catch { /* observer's problem */ }
      w.grant(ok(release));
    }
    this.notify(l);
  }
  private drop(l: Lane, w: Waiter): void {
    clearTimeout(w.timer);
    const k = l.waiting.indexOf(w);
    if (k >= 0) { l.waiting.splice(k, 1); this.notify(l); }
  }
  private notify(l: Lane): void {
    l.waiting.forEach((w, k) => {
      if (w.last === k + 1) return;
      w.last = k + 1;
      try { w.onPosition?.(k + 1); } catch { /* observer's problem */ }
    });
  }
}
