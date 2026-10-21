import { KIND_LABELS, safeAvatar, type AccountBot } from "./account-bots.js";
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
  /** All account-owned bot kinds reported by this subscription worker. */
  accountBots?: boolean;
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
  accountState(provider: string): Promise<{ health: string; remainingPct?: number | null; resetsAt?: string | null; agents?: { id: string; name: string; kind?: string; kindLabel?: string; avatarUrl?: string }[] | null } | null>;
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
  /** Turns queued or running in this conversation (the busy limit counts conversations with any). */
  inFlight?: number;
  id: string; closed: boolean; turns: number; seq: number; events: CursoredEvent[]; projectId: string | null; accountBot?: { id: string; kind: string };
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
  /** Public identity cached by the worker's non-spending account read. */
  private async accountBots(): Promise<AccountBot[]> {
    if (!this.spec.projects && !this.spec.accountBots) return [];
    const s = await this.o.tasks?.accountState(this.spec.provider).catch(() => null);
    const seen = new Set<string>();
    return (s?.agents ?? []).filter((a) => {
      const kind = a.kind ?? "project";
      if (!a.name.trim() || !/^[\w-]+$/.test(a.id) || !Object.hasOwn(KIND_LABELS, kind)) return false;
      if (this.spec.projects && (kind !== "project" || !UUID.test(a.id))) return false;
      const key = `${kind}:${a.id}`;
      if (seen.has(key)) return false;
      seen.add(key);
      return true;
    }).map((a) => ({ ...a, kind: a.kind ?? "project", kindLabel: KIND_LABELS[a.kind ?? "project"], avatarUrl: safeAvatar(a.avatarUrl) }));
  }
  private botOf(agentId: string): { id: string; kind: string } | undefined {
    const prefix = `${this.spec.agentId}:`;
    if (!agentId.startsWith(prefix)) return undefined;
    const m = /^([a-z]+):([\w-]+)$/.exec(agentId.slice(prefix.length));
    return m && Object.hasOwn(KIND_LABELS, m[1]) ? { kind: m[1], id: m[2] } : undefined;
  }
  private async known(agentId: string) {
    if (agentId === this.spec.agentId || this.projectOf(agentId) !== null) return true;
    const bot = this.botOf(agentId);
    return !!bot && (await this.accountBots()).some((a) => a.id === bot.id && a.kind === bot.kind);
  }
  async list(): Promise<AaiResult<AgentSummary[]>> {
    const r = await this.ready();
    const state = r.ok ? "READY" : "blocked";
    const bots = await this.accountBots();
    return ok([
      this.summary(state),
      ...bots.map((a): AgentSummary => ({ agentId: `${this.spec.agentId}:${a.kind}:${a.id}`, displayName: a.name, vendor: this.spec.vendor, state, kind: a.kind, kindLabel: a.kindLabel, ...(a.avatarUrl ? { avatarUrl: a.avatarUrl } : {}) })),
    ]);
  }
  private async nameOf(agentId: string): Promise<string> {
    const bot = this.botOf(agentId);
    return bot ? (await this.accountBots()).find((a) => a.id === bot.id && a.kind === bot.kind)?.name ?? `${this.spec.displayName} ${KIND_LABELS[bot.kind]}` : this.spec.displayName;
  }
  async get(agentId: string): Promise<AaiResult<AgentDetail>> {
    if (!await this.known(agentId)) return fail("CONTEXT_NOT_FOUND", `No such agent ${agentId}`);
    const bot = this.botOf(agentId);
    const entry = bot && (await this.accountBots()).find((a) => a.id === bot.id && a.kind === bot.kind);
    return ok({ agentId, displayName: await this.nameOf(agentId), vendor: this.spec.vendor, state: "READY", remoteIds: bot ? { [bot.kind]: bot.id } : {}, ...(bot ? { kind: bot.kind, kindLabel: KIND_LABELS[bot.kind] } : {}), ...(entry?.avatarUrl ? { avatarUrl: entry.avatarUrl } : {}), capabilities: this.spec.capabilities });
  }
  async capabilities(agentId: string): Promise<AaiResult<AgentCapabilityManifest>> {
    return await this.known(agentId) ? ok(this.spec.capabilities) : fail("CONTEXT_NOT_FOUND", `No such agent ${agentId}`);
  }
  async identity(agentId: string): Promise<AaiResult<AgentIdentity>> {
    return await this.known(agentId) ? ok({ agentId, displayName: await this.nameOf(agentId), vendor: this.spec.vendor, lookPack: this.spec.lookPack }) : fail("CONTEXT_NOT_FOUND", `No such agent ${agentId}`);
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
  private botInContext(id: string): { id: string; kind: string } | undefined {
    const m = /~b([a-z]+):([\w-]+)$/.exec(id);
    return m && Object.hasOwn(KIND_LABELS, m[1]) ? { kind: m[1], id: m[2] } : undefined;
  }
  private live(id: string): AaiResult<Ctx> {
    const c = this.ctxs.get(id);
    if (c) return c.closed ? fail("CONTEXT_NOT_FOUND", "No such conversation") : ok(c);
    if (!id.startsWith(this.ctxPrefix)) return fail("CONTEXT_NOT_FOUND", "No such conversation");
    const revived: Ctx = { id, closed: false, turns: 1, seq: 0, events: [], done: new Map(), lock: Promise.resolve(), projectId: projectInContext(id), accountBot: this.botInContext(id) };
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
    if (!await this.known(i.agentId)) return fail("CONTEXT_NOT_FOUND", `No such agent ${i.agentId}`);
    if (i.adoptContextId) {
      const c = this.live(i.adoptContextId); // also revives a context from before a gateway restart
      if (c.ok) {
        const selected = this.botOf(i.agentId);
        const owner = c.value.accountBot ?? (c.value.projectId ? { kind: "project", id: c.value.projectId } : undefined);
        if (selected?.kind !== owner?.kind || selected?.id !== owner?.id) {
          return fail("CONTEXT_NOT_FOUND", "Conversation belongs to a different agent");
        }
      }
      return c.ok ? ok({ contextId: c.value.id, isolation: "isolated", guarantee: "best_effort", resumed: true }) : c;
    }
    const r = await this.ready();
    if (!r.ok) return r;
    // The context id is also the gateway thread_id: the subscription's thread mapping keeps the claude.ai conversation.
    const projectId = this.projectOf(i.agentId);
    const accountBot = projectId ? undefined : this.botOf(i.agentId);
    const id = `${this.ctxPrefix}${i.threadId ?? ""}${i.threadId ? "-" : ""}${++this.n}-${Math.random().toString(36).slice(2, 8)}${projectId ? `~p${projectId}` : accountBot ? `~b${accountBot.kind}:${accountBot.id}` : ""}`;
    const c: Ctx = { id, closed: false, turns: 0, seq: 0, events: [], done: new Map(), lock: Promise.resolve(), projectId, accountBot };
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
    // The limit is on replies running at once, not on conversations that exist: every thread keeps its
    // conversation open, so counting open ones locked a person out after their Nth thread.
    const running = [...this.ctxs.values()].filter((c) => c !== ctx && (c.inFlight ?? 0) > 0).length;
    if (running >= this.spec.capabilities.context.maxParallel) {
      return Promise.resolve(fail("CONTEXT_BUSY", `${this.spec.displayName} is already answering ${this.spec.capabilities.context.maxParallel} conversations; try again in a moment.`, { retryAfterMs: 5000 }));
    }
    ctx.inFlight = (ctx.inFlight ?? 0) + 1;
    const p = ctx.lock.then(() => this.run(ctx, i)).finally(() => { ctx.inFlight = (ctx.inFlight ?? 1) - 1; });
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
      ...(capability === "chat.create" && (ctx.projectId || ctx.accountBot) ? { options: ctx.projectId ? { project_id: ctx.projectId } : { account_bot: ctx.accountBot } } : {}),
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
