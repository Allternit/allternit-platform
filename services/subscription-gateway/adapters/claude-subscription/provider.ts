// Claude subscription AAI provider (lane browser, guarantee best_effort, mode linked).
// Each AAI context is one claude.ai conversation: its first message is a gateway `chat.create` task with
// thread_id = the context id, later ones are `chat.continue` on the same thread_id (the gateway's thread mapping pins
// the provider thread and the owning Claude account). The subscription worker does the browser work, so login, usage
// limits, verification walls and pacing behave exactly like Settings → Subscriptions chats.
import {
  BaseAaiProvider, fail, ok,
  type AaiResult, type AgentCapabilityManifest, type AgentDetail, type AgentIdentity, type AgentSummary,
  type CursoredEvent, type EventsInput, type EventsResult, type HealthResult, type MessageInput, type MessageResult,
  type OpenContextInput, type OpenContextResult,
} from "@allternit/agent-gateway";
import { ADAPTER_ID, AGENT_ID, CAPABILITIES, SUBSCRIPTION_PROVIDER, VENDOR } from "./manifest.js";

/** The gateway's own task API, in process (VendorContext.gatewayTasks). */
export interface GatewayTasks {
  submit(body: Record<string, unknown>): Promise<{ status: number; body: Record<string, unknown> }>;
  get(taskId: string): Promise<{ status: number; body: Record<string, unknown> }>;
  /** Live task events (the worker's reply/reasoning deltas); returns unsubscribe. Optional. */
  subscribe?(taskId: string, onEvent: (event: { kind?: string; payload?: unknown }) => void): () => void;
  /** The provider's best subscription login: its session_health and usage left (null = none signed in). */
  accountState(provider: string): Promise<{ health: string; remainingPct?: number | null; resetsAt?: string | null } | null>;
}

export interface ClaudeSubscriptionOptions {
  tasks?: GatewayTasks;
  pollMs?: number;
  replyTimeoutMs?: number;
  sleep?: (ms: number) => Promise<void>;
}

interface Ctx {
  id: string; closed: boolean; turns: number; seq: number; events: CursoredEvent[];
  done: Map<string, Promise<AaiResult<MessageResult>>>; lock: Promise<unknown>;
}

/** Process-wide, strictly increasing, clock-seeded (µs) event cursor: survives restarts, never repeats. */
let lastCursor = 0;
function nextCursor(): number {
  lastCursor = Math.max(lastCursor + 1, Date.now() * 1000);
  return lastCursor;
}

const TERMINAL = new Set(["completed", "partial", "failed", "cancelled", "needs_user"]);
const NOT_READY = "Sign in to Claude in Settings → Subscriptions on your Sessions computer, then try again.";

/** Gateway task failure → AAI error code (the vendor's own words stay in the message). */
function mapTaskError(task: Record<string, unknown>): AaiResult<never> {
  const err = (task.error ?? {}) as { class?: string; detail?: string; user_action?: string | null; cooldown_s?: number | null };
  const msg = [err.detail, err.user_action].filter(Boolean).join(" ") || `Claude didn't finish (${String(task.status)}).`;
  const cls = err.class ?? "";
  if (task.status === "needs_user" || /verification|challenge|captcha/.test(cls)) return fail("LANE_BLOCKED", msg);
  if (/auth|login|signed_out|session/.test(cls)) return fail("AUTH_REQUIRED", msg);
  if (/rate|limit|quota|usage/.test(cls)) return fail("RATE_LIMITED", msg, err.cooldown_s ? { retryAfterMs: err.cooldown_s * 1000 } : undefined);
  return fail("VENDOR_UNAVAILABLE", msg);
}

export class ClaudeSubscriptionProvider extends BaseAaiProvider {
  readonly adapterId = ADAPTER_ID;
  private ctxs = new Map<string, Ctx>();
  private n = 0;
  private o: Required<Omit<ClaudeSubscriptionOptions, "tasks">> & { tasks?: GatewayTasks };

  constructor(opts: ClaudeSubscriptionOptions = {}) {
    super();
    this.o = {
      tasks: opts.tasks,
      pollMs: opts.pollMs ?? 2000,
      replyTimeoutMs: opts.replyTimeoutMs ?? 240_000,
      sleep: opts.sleep ?? ((ms) => new Promise((r) => setTimeout(r, ms))),
    };
  }

  // ---------- identity ----------
  private summary(state: string): AgentSummary { return { agentId: AGENT_ID, displayName: "Claude", vendor: VENDOR, state }; }
  /** The Claude login's state → ok, or the AAI error a turn would hit right now. */
  private async ready(): Promise<AaiResult<true>> {
    if (!this.o.tasks) return fail("VENDOR_UNAVAILABLE", "This gateway can't run Claude subscription turns.");
    let s: Awaited<ReturnType<GatewayTasks["accountState"]>>;
    try { s = await this.o.tasks.accountState(SUBSCRIPTION_PROVIDER); }
    catch (e) { return fail("VENDOR_UNAVAILABLE", `The Claude subscription isn't reachable right now: ${(e as Error).message}`, { retryable: true }); }
    if (!s || s.health === "auth_required") return fail("AUTH_REQUIRED", NOT_READY);
    if (s.health === "challenge_presented") return fail("LANE_BLOCKED", "Claude is asking for verification. Click it in the Claude window on your Sessions computer, then try again.");
    if (s.health === "account_restricted") return fail("POLICY_DENIED", "Claude reports this account as restricted.");
    if (s.health === "ui_drift") return fail("ADAPTER_DRIFT", "claude.ai changed in a way the adapter doesn't handle yet.");
    if (s.health === "provider_down" || s.health === "profile_locked") return fail("VENDOR_UNAVAILABLE", "Claude isn't reachable from the Sessions computer right now.", { retryable: true });
    const at = s.resetsAt ? Date.parse(s.resetsAt) : NaN;
    // A 0% reading whose reset time has passed is stale (the page only reports usage sometimes): let the turn try.
    if (s.remainingPct === 0 && !(Number.isFinite(at) && at <= Date.now())) {
      return fail("RATE_LIMITED", `Your Claude usage limit is reached${s.resetsAt ? ` until ${new Date(s.resetsAt).toLocaleTimeString()}` : ""}.`, { retryAfterMs: Number.isFinite(at) ? Math.max(60_000, at - Date.now()) : 3_600_000 });
    }
    return ok(true);
  }
  async list(): Promise<AaiResult<AgentSummary[]>> {
    const r = await this.ready();
    return ok([this.summary(r.ok ? "READY" : "blocked")]);
  }
  async get(agentId: string): Promise<AaiResult<AgentDetail>> {
    if (agentId !== AGENT_ID) return fail("CONTEXT_NOT_FOUND", `No such agent ${agentId}`);
    return ok({ ...this.summary("READY"), remoteIds: {}, capabilities: CAPABILITIES });
  }
  async capabilities(agentId: string): Promise<AaiResult<AgentCapabilityManifest>> {
    return agentId === AGENT_ID ? ok(CAPABILITIES) : fail("CONTEXT_NOT_FOUND", `No such agent ${agentId}`);
  }
  async identity(agentId: string): Promise<AaiResult<AgentIdentity>> {
    return agentId === AGENT_ID ? ok({ agentId, displayName: "Claude", vendor: VENDOR, lookPack: "claude" }) : fail("CONTEXT_NOT_FOUND", `No such agent ${agentId}`);
  }
  async health(_i: { agentId?: string }): Promise<AaiResult<HealthResult>> {
    const r = await this.ready();
    return ok(r.ok ? { status: "healthy", lane: "ui_bridge", detail: "Claude subscription ready" } : { status: "down", lane: "ui_bridge", detail: r.error.humanMessage });
  }

  // ---------- contexts ----------
  /**
   * A context survives a gateway restart: its id is the gateway thread_id, whose mapping to the claude.ai conversation
   * lives in the gateway DB. An unknown `cs-` id is rebuilt as "already started" (chat.continue; run() falls back to
   * chat.create if the gateway has no mapping for it).
   */
  private live(id: string): AaiResult<Ctx> {
    const c = this.ctxs.get(id);
    if (c) return c.closed ? fail("CONTEXT_NOT_FOUND", "No such conversation") : ok(c);
    if (!id.startsWith("cs-")) return fail("CONTEXT_NOT_FOUND", "No such conversation");
    const revived: Ctx = { id, closed: false, turns: 1, seq: 0, events: [], done: new Map(), lock: Promise.resolve() };
    this.ctxs.set(id, revived);
    return ok(revived);
  }
  private push(c: Ctx, type: "agent.context.opened" | "agent.activity.started" | "agent.activity.completed" | "agent.message.completed", correlationId: string, payload: Record<string, unknown>, source: "allternit" | "vendor" = "vendor") {
    // Cursors only grow, also across a gateway restart: a revived context must continue past the cursor
    // allternit-api already holds (live: after a redeploy every new event sat below it and never synced).
    c.seq = nextCursor();
    c.events.push({ cursor: String(c.seq), event: {
      type, botId: AGENT_ID, threadId: c.id, generationId: "1", source, vendor: VENDOR, adapter: ADAPTER_ID, lane: "ui_bridge",
      remoteContextId: c.id, remoteEventId: `${c.id}#${c.seq}`, causationId: correlationId, correlationId, guarantee: "best_effort", at: new Date().toISOString(), payload } });
  }

  async contextOpen(i: OpenContextInput): Promise<AaiResult<OpenContextResult>> {
    if (i.agentId !== AGENT_ID) return fail("CONTEXT_NOT_FOUND", `No such agent ${i.agentId}`);
    if (i.adoptContextId) {
      const c = this.live(i.adoptContextId); // also revives a context from before a gateway restart
      return c.ok ? ok({ contextId: c.value.id, isolation: "isolated", guarantee: "best_effort", resumed: true }) : c;
    }
    const r = await this.ready();
    if (!r.ok) return r;
    if ([...this.ctxs.values()].filter((c) => !c.closed).length >= CAPABILITIES.context.maxParallel) {
      return fail("CONTEXT_BUSY", `At most ${CAPABILITIES.context.maxParallel} Claude conversations can be open at once.`);
    }
    // The context id is also the gateway thread_id: the subscription's thread mapping keeps the claude.ai conversation.
    const id = `cs-${i.threadId ?? ""}${i.threadId ? "-" : ""}${++this.n}-${Math.random().toString(36).slice(2, 8)}`;
    const c: Ctx = { id, closed: false, turns: 0, seq: 0, events: [], done: new Map(), lock: Promise.resolve() };
    this.ctxs.set(id, c);
    this.push(c, "agent.context.opened", `open:${id}`, { title: i.title ?? null });
    return ok({ contextId: id, isolation: "isolated", guarantee: "best_effort", resumed: false });
  }

  contextMessage(i: MessageInput): Promise<AaiResult<MessageResult>> {
    const c = this.live(i.contextId);
    if (!c.ok) return Promise.resolve(c);
    const ctx = c.value;
    const prior = ctx.done.get(i.correlationId);
    if (prior) return prior;
    const p = ctx.lock.then(() => this.run(ctx, i));
    ctx.done.set(i.correlationId, p);
    ctx.lock = p.catch(() => undefined);
    void p.then((r) => { if (!r.ok) ctx.done.delete(i.correlationId); });
    return p;
  }

  private async run(ctx: Ctx, i: MessageInput): Promise<AaiResult<MessageResult>> {
    const tasks = this.o.tasks;
    if (!tasks) return fail("VENDOR_UNAVAILABLE", "This gateway can't run Claude subscription turns.");
    this.push(ctx, "agent.activity.started", i.correlationId, { text: i.text }, "allternit");
    const send = (capability: "chat.create" | "chat.continue") => tasks.submit({
      capability,
      prompt: i.text,
      thread_id: ctx.id,
      idempotency_key: `${ctx.id}:${i.correlationId}`,
      requester_kind: "bot",
      routing: { provider: SUBSCRIPTION_PROVIDER },
      // The Allternit user sent this turn in a thread (allternit-api only forwards human-initiated turns).
      initiated_by: { kind: "human", user_id: "allternit-thread", action_id: i.correlationId },
    }).catch((e: Error) => ({ status: 0, body: { error: e.message } as Record<string, unknown> }));
    let submitted = await send(ctx.turns === 0 ? "chat.create" : "chat.continue");
    // A revived context whose conversation never started (or was lost) begins again.
    if (submitted.status === 409 && submitted.body.error === "thread_not_mapped") submitted = await send("chat.create");
    if (submitted.status < 200 || submitted.status >= 300 || typeof submitted.body.task_id !== "string") {
      return fail("VENDOR_UNAVAILABLE", `Claude subscription refused the turn: ${String(submitted.body.detail ?? submitted.body.error ?? submitted.status)}`);
    }
    const taskId = submitted.body.task_id as string;
    // Stream Claude's thinking as live activity while the reply runs.
    let thinking = "";
    const unsubscribe = tasks.subscribe?.(taskId, (e) => {
      const ev = (e.payload as { event?: { type?: string; delta?: string } } | undefined)?.event;
      if (e.kind !== "reply" || ev?.type !== "reply.reasoning.delta" || typeof ev.delta !== "string") return;
      thinking += ev.delta;
      this.push(ctx, "agent.activity.started", i.correlationId, { activityId: i.correlationId, kind: "thinking", label: "Thinking", detail: thinking });
    });
    const deadline = Date.now() + this.o.replyTimeoutMs;
    let task = submitted.body;
    try {
      while (!TERMINAL.has(String(task.status))) {
        if (Date.now() > deadline) return fail("VENDOR_UNAVAILABLE", "Claude hasn't answered yet. The reply will still land in claude.ai; try again shortly.", { details: { taskId } });
        await this.o.sleep(this.o.pollMs);
        const r = await tasks.get(taskId).catch(() => null);
        if (r && r.status === 200) task = r.body;
      }
    } finally { unsubscribe?.(); }
    const reply = ((task.result ?? {}) as { text?: string }).text ?? "";
    if ((task.status !== "completed" && task.status !== "partial") || !reply.trim()) return mapTaskError(task);
    ctx.turns += 1;
    // Unique per turn even after a restart revives the context (turn counts restart there).
    const messageId = `${ctx.id}:${i.correlationId}`;
    if (thinking) this.push(ctx, "agent.activity.completed", i.correlationId, { activityId: i.correlationId });
    // The finished thought rides with the reply as a `thinking` content block (the Claude pack shows "Thought process").
    this.push(ctx, "agent.message.completed", i.correlationId, { reply, messageId, ...(thinking ? { content: [{ type: "thinking", text: thinking }] } : {}) });
    return ok({ messageId, correlationId: i.correlationId, reply, guarantee: "best_effort" });
  }

  async contextClose(i: { contextId: string }): Promise<AaiResult<{ closed: boolean }>> {
    const c = this.live(i.contextId);
    if (!c.ok) return c;
    c.value.closed = true;
    return ok({ closed: true });
  }
  async events(i: EventsInput): Promise<AaiResult<EventsResult>> {
    const c = this.live(i.contextId);
    if (!c.ok) return c;
    const after = Number(i.cursor ?? 0);
    let events = c.value.events.filter((e) => Number(e.cursor) > after);
    if (i.limit && i.limit > 0) events = events.slice(0, i.limit);
    return ok({ events, nextCursor: events.length ? events[events.length - 1].cursor : String(after) });
  }
}
