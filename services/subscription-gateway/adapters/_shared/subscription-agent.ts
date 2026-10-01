// Subscription agent AAI provider (lane browser, guarantee best_effort, mode linked): Claude, ChatGPT, Kimi…
// Originally the Claude adapter;
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
/** What makes one subscription agent differ from another (Claude, ChatGPT, Kimi…). */
export interface SubscriptionAgentSpec {
  adapterId: string;
  /** AAI vendor id (anthropic, openai, kimi). */
  vendor: string;
  /** The subscription provider in Settings → Subscriptions (claude, chatgpt, kimi). */
  provider: string;
  /** The plain agent's id and name. */
  agentId: string;
  displayName: string;
  lookPack: string;
  /** Where conversations live, for messages ("claude.ai"). */
  site: string;
  /** Projects become agents ("<agentId>:project:<uuid>") when the provider reports them. */
  projects?: boolean;
  capabilities: AgentCapabilityManifest;
}

/** The gateway's own task API, in process (VendorContext.gatewayTasks). */
export interface GatewayTasks {
  submit(body: Record<string, unknown>): Promise<{ status: number; body: Record<string, unknown> }>;
  get(taskId: string): Promise<{ status: number; body: Record<string, unknown> }>;
  /** Cancel a running task (the worker stops it). Optional. */
  cancel?(taskId: string): Promise<{ status: number; body: Record<string, unknown> }>;
  /** Live task events (the worker's reply/reasoning deltas); returns unsubscribe. Optional. */
  subscribe?(taskId: string, onEvent: (event: { kind?: string; payload?: unknown }) => void): () => void;
  /** The provider's best subscription login: its session_health and usage left (null = none signed in). */
  accountState(provider: string): Promise<{ health: string; remainingPct?: number | null; resetsAt?: string | null; agents?: { id: string; name: string; kind?: string }[] | null } | null>;
}

export interface SubscriptionAgentOptions {
  spec: SubscriptionAgentSpec;
  tasks?: GatewayTasks;
  pollMs?: number;
  replyTimeoutMs?: number;
  sleep?: (ms: number) => Promise<void>;
}

/** Agent ids: "claude" (plain chats) or "claude:project:<uuid>" (chats inside that Claude Project). */

const UUID = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/;

/** The Project rides in the context id ("…~p<uuid>"), so a revived context still starts its chats in it. */
const projectInContext = (contextId: string) => /~p([0-9a-f-]{36})$/.exec(contextId)?.[1] ?? null;

interface Ctx {
  id: string; closed: boolean; turns: number; seq: number; events: CursoredEvent[]; projectId: string | null;
  /** The running turn's gateway task, so Stop can cancel it. */
  runningTask?: string; cancelled?: boolean;
  done: Map<string, Promise<AaiResult<MessageResult>>>; lock: Promise<unknown>;
}

/** Process-wide, strictly increasing, clock-seeded (µs) event cursor: survives restarts, never repeats. */
let lastCursor = 0;
function nextCursor(): number {
  lastCursor = Math.max(lastCursor + 1, Date.now() * 1000);
  return lastCursor;
}

const TERMINAL = new Set(["completed", "partial", "failed", "cancelled", "needs_user"]);


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

export class SubscriptionAgentProvider extends BaseAaiProvider {
  readonly adapterId: string;
  private spec: SubscriptionAgentSpec;
  private ctxs = new Map<string, Ctx>();
  private n = 0;
  private o: Required<Omit<SubscriptionAgentOptions, "tasks" | "spec">> & { tasks?: GatewayTasks };

  constructor(opts: SubscriptionAgentOptions) {
    super();
    this.spec = opts.spec;
    this.adapterId = opts.spec.adapterId;
    this.o = {
      tasks: opts.tasks,
      pollMs: opts.pollMs ?? 2000,
      replyTimeoutMs: opts.replyTimeoutMs ?? 240_000,
      sleep: opts.sleep ?? ((ms) => new Promise((r) => setTimeout(r, ms))),
    };
  }

  /** Claude keeps "cs-" so contexts from before this refactor still revive. */
  private get ctxPrefix() { return this.spec.adapterId === "claude-subscription" ? "cs-" : `${this.spec.agentId}-`; }
  private get projectPrefix() { return `${this.spec.agentId}:project:`; }
  private projectOf(agentId: string): string | null {
    if (!this.spec.projects) return null;
    const id = agentId.startsWith(this.projectPrefix) ? agentId.slice(this.projectPrefix.length) : null;
    return id && UUID.test(id) ? id : null;
  }

  // ---------- identity ----------
  private summary(state: string): AgentSummary { return { agentId: this.spec.agentId, displayName: this.spec.displayName, vendor: this.spec.vendor, state }; }
  /** The Claude login's state → ok, or the AAI error a turn would hit right now. */
  private async ready(): Promise<AaiResult<true>> {
    if (!this.o.tasks) return fail("VENDOR_UNAVAILABLE", `This gateway can't run ${this.spec.displayName} subscription turns.`);
    let s: Awaited<ReturnType<GatewayTasks["accountState"]>>;
    try { s = await this.o.tasks.accountState(this.spec.provider); }
    catch (e) { return fail("VENDOR_UNAVAILABLE", `The ${this.spec.displayName} subscription isn't reachable right now: ${(e as Error).message}`, { retryable: true }); }
    if (!s || s.health === "auth_required") return fail("AUTH_REQUIRED", `Sign in to ${this.spec.displayName} in Settings → Subscriptions on your Sessions computer, then try again.`);
    if (s.health === "challenge_presented") return fail("LANE_BLOCKED", `${this.spec.displayName} is asking for verification. Click it in the ${this.spec.displayName} window on your Sessions computer, then try again.`);
    if (s.health === "account_restricted") return fail("POLICY_DENIED", `${this.spec.displayName} reports this account as restricted.`);
    if (s.health === "ui_drift") return fail("ADAPTER_DRIFT", `${this.spec.site} changed in a way the adapter doesn't handle yet.`);
    if (s.health === "provider_down" || s.health === "profile_locked") return fail("VENDOR_UNAVAILABLE", `${this.spec.displayName} isn't reachable from the Sessions computer right now.`, { retryable: true });
    const at = s.resetsAt ? Date.parse(s.resetsAt) : NaN;
    // A 0% reading whose reset time has passed is stale (the page only reports usage sometimes): let the turn try.
    if (s.remainingPct === 0 && !(Number.isFinite(at) && at <= Date.now())) {
      return fail("RATE_LIMITED", `Your ${this.spec.displayName} usage limit is reached${s.resetsAt ? ` until ${new Date(s.resetsAt).toLocaleTimeString()}` : ""}.`, { retryAfterMs: Number.isFinite(at) ? Math.max(60_000, at - Date.now()) : 3_600_000 });
    }
    return ok(true);
  }
  /** The account's Claude Projects, as the subscription worker last read them. */
  private async projects(): Promise<{ id: string; name: string }[]> {
    if (!this.spec.projects) return [];
    const s = await this.o.tasks?.accountState(this.spec.provider).catch(() => null);
    return (s?.agents ?? []).filter((a) => (a.kind ?? "project") === "project" && UUID.test(a.id));
  }
  private known(agentId: string) { return agentId === this.spec.agentId || this.projectOf(agentId) !== null; }
  async list(): Promise<AaiResult<AgentSummary[]>> {
    const r = await this.ready();
    const state = r.ok ? "READY" : "blocked";
    const projects = await this.projects();
    return ok([
      this.summary(state),
      ...projects.map((p): AgentSummary => ({ agentId: this.projectPrefix + p.id, displayName: p.name, vendor: this.spec.vendor, state })),
    ]);
  }
  private async nameOf(agentId: string): Promise<string> {
    const pid = this.projectOf(agentId);
    return pid ? (await this.projects()).find((p) => p.id === pid)?.name ?? `${this.spec.displayName} Project` : this.spec.displayName;
  }
  async get(agentId: string): Promise<AaiResult<AgentDetail>> {
    if (!this.known(agentId)) return fail("CONTEXT_NOT_FOUND", `No such agent ${agentId}`);
    const pid = this.projectOf(agentId);
    return ok({ agentId, displayName: await this.nameOf(agentId), vendor: this.spec.vendor, state: "READY", remoteIds: pid ? { project: pid } : {}, capabilities: this.spec.capabilities });
  }
  async capabilities(agentId: string): Promise<AaiResult<AgentCapabilityManifest>> {
    return this.known(agentId) ? ok(this.spec.capabilities) : fail("CONTEXT_NOT_FOUND", `No such agent ${agentId}`);
  }
  async identity(agentId: string): Promise<AaiResult<AgentIdentity>> {
    return this.known(agentId) ? ok({ agentId, displayName: await this.nameOf(agentId), vendor: this.spec.vendor, lookPack: this.spec.lookPack }) : fail("CONTEXT_NOT_FOUND", `No such agent ${agentId}`);
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
    if (!id.startsWith(this.ctxPrefix)) return fail("CONTEXT_NOT_FOUND", "No such conversation");
    const revived: Ctx = { id, closed: false, turns: 1, seq: 0, events: [], done: new Map(), lock: Promise.resolve(), projectId: projectInContext(id) };
    this.ctxs.set(id, revived);
    return ok(revived);
  }
  private push(c: Ctx, type: "agent.context.opened" | "agent.activity.started" | "agent.activity.completed" | "agent.message.delta" | "agent.message.completed", correlationId: string, payload: Record<string, unknown>, source: "allternit" | "vendor" = "vendor") {
    // Cursors only grow, also across a gateway restart: a revived context must continue past the cursor
    // allternit-api already holds (live: after a redeploy every new event sat below it and never synced).
    c.seq = nextCursor();
    c.events.push({ cursor: String(c.seq), event: {
      type, botId: this.spec.agentId, threadId: c.id, generationId: "1", source, vendor: this.spec.vendor, adapter: this.spec.adapterId, lane: "ui_bridge",
      remoteContextId: c.id, remoteEventId: `${c.id}#${c.seq}`, causationId: correlationId, correlationId, guarantee: "best_effort", at: new Date().toISOString(), payload } });
  }

  async contextOpen(i: OpenContextInput): Promise<AaiResult<OpenContextResult>> {
    if (!this.known(i.agentId)) return fail("CONTEXT_NOT_FOUND", `No such agent ${i.agentId}`);
    if (i.adoptContextId) {
      const c = this.live(i.adoptContextId); // also revives a context from before a gateway restart
      return c.ok ? ok({ contextId: c.value.id, isolation: "isolated", guarantee: "best_effort", resumed: true }) : c;
    }
    const r = await this.ready();
    if (!r.ok) return r;
    if ([...this.ctxs.values()].filter((c) => !c.closed).length >= this.spec.capabilities.context.maxParallel) {
      return fail("CONTEXT_BUSY", `At most ${this.spec.capabilities.context.maxParallel} ${this.spec.displayName} conversations can be open at once.`);
    }
    // The context id is also the gateway thread_id: the subscription's thread mapping keeps the claude.ai conversation.
    const projectId = this.projectOf(i.agentId);
    const id = `${this.ctxPrefix}${i.threadId ?? ""}${i.threadId ? "-" : ""}${++this.n}-${Math.random().toString(36).slice(2, 8)}${projectId ? `~p${projectId}` : ""}`;
    const c: Ctx = { id, closed: false, turns: 0, seq: 0, events: [], done: new Map(), lock: Promise.resolve(), projectId };
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
    if (!tasks) return fail("VENDOR_UNAVAILABLE", `This gateway can't run ${this.spec.displayName} subscription turns.`);
    this.push(ctx, "agent.activity.started", i.correlationId, { text: i.text }, "allternit");
    const send = (capability: "chat.create" | "chat.continue") => tasks.submit({
      capability,
      prompt: i.text,
      thread_id: ctx.id,
      idempotency_key: `${ctx.id}:${i.correlationId}`,
      requester_kind: "bot",
      routing: { provider: this.spec.provider },
      // A new chat for a Project agent starts inside that Project (claude-web opens the Project page).
      ...(capability === "chat.create" && ctx.projectId ? { options: { project_id: ctx.projectId } } : {}),
      // The Allternit user sent this turn in a thread (allternit-api only forwards human-initiated turns).
      initiated_by: { kind: "human", user_id: "allternit-thread", action_id: i.correlationId },
    }).catch((e: Error) => ({ status: 0, body: { error: e.message } as Record<string, unknown> }));
    let submitted = await send(ctx.turns === 0 ? "chat.create" : "chat.continue");
    // A revived context whose conversation never started (or was lost) begins again.
    if (submitted.status === 409 && submitted.body.error === "thread_not_mapped") submitted = await send("chat.create");
    if (submitted.status < 200 || submitted.status >= 300 || typeof submitted.body.task_id !== "string") {
      return fail("VENDOR_UNAVAILABLE", `${this.spec.displayName} subscription refused the turn: ${String(submitted.body.detail ?? submitted.body.error ?? submitted.status)}`);
    }
    const taskId = submitted.body.task_id as string;
    ctx.runningTask = taskId; ctx.cancelled = false;
    // Unique per turn even after a restart revives the context (turn counts restart there).
    const messageId = `${ctx.id}:${i.correlationId}`;
    // Stream Claude's thinking as live activity, and the reply as it types.
    let thinking = "";
    const unsubscribe = tasks.subscribe?.(taskId, (e) => {
      const ev = (e.payload as { event?: { type?: string; delta?: string } } | undefined)?.event;
      if (e.kind !== "reply" || typeof ev?.delta !== "string") return;
      if (ev.type === "reply.reasoning.delta") {
        thinking += ev.delta;
        this.push(ctx, "agent.activity.started", i.correlationId, { activityId: i.correlationId, kind: "thinking", label: "Thinking", detail: thinking });
      } else if (ev.type === "reply.text.delta") {
        this.push(ctx, "agent.message.delta", i.correlationId, { messageId, chunk: ev.delta });
      }
    });
    const deadline = Date.now() + this.o.replyTimeoutMs;
    let task = submitted.body;
    try {
      while (!TERMINAL.has(String(task.status))) {
        if (Date.now() > deadline) return fail("VENDOR_UNAVAILABLE", `${this.spec.displayName} hasn't answered yet. The reply will still land in ${this.spec.site}; try again shortly.`, { details: { taskId } });
        await this.o.sleep(this.o.pollMs);
        const r = await tasks.get(taskId).catch(() => null);
        if (r && r.status === 200) task = r.body;
      }
    } finally { unsubscribe?.(); ctx.runningTask = undefined; }
    if (ctx.cancelled) return fail("UNKNOWN", "The message was stopped.", { details: { cancelled: true } });
    const reply = ((task.result ?? {}) as { text?: string }).text ?? "";
    if ((task.status !== "completed" && task.status !== "partial") || !reply.trim()) return mapTaskError(task);
    ctx.turns += 1;
    if (thinking) this.push(ctx, "agent.activity.completed", i.correlationId, { activityId: i.correlationId });
    // The finished thought rides with the reply as a `thinking` content block (the Claude pack shows "Thought process").
    this.push(ctx, "agent.message.completed", i.correlationId, { reply, messageId, ...(thinking ? { content: [{ type: "thinking", text: thinking }] } : {}) });
    return ok({ messageId, correlationId: i.correlationId, reply, guarantee: "best_effort" });
  }

  async contextCancel(i: { contextId: string }): Promise<AaiResult<{ confirmed: boolean }>> {
    const c = this.live(i.contextId);
    if (!c.ok) return c;
    const task = c.value.runningTask;
    if (!task || !this.o.tasks?.cancel) return ok({ confirmed: false });
    c.value.cancelled = true;
    const r = await this.o.tasks.cancel(task).catch(() => ({ status: 0, body: {} }));
    return ok({ confirmed: r.status >= 200 && r.status < 300 });
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
