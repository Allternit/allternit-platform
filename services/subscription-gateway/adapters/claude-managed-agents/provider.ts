// Claude Managed Agents AAI provider (lane official, guarantee exact, mode hosted).
// One Managed Agents session per AAI context (context id == session id). While a turn is in flight the provider holds
// the session's SSE stream (`sessions.events.stream`), opened BEFORE each send; every (re)connect first backfills history
// with `sessions.events.list` from a created_at cursor, and everything is deduped by vendor event id, so a dropped stream
// never loses or repeats an event (managed-agents-client-patterns.md #1, #2, #7). The result is one ordered, cursored,
// replayable log per context. Approvals mirror `agent.tool_use` events whose `evaluated_permission === "ask"` and are
// answered ONLY for a human actor via `user.tool_confirmation`. Custom tool calls (`agent.custom_tool_use`) are answered
// with context.message({ toolResult }) -> `user.custom_tool_result`; until then the context reports needs_attention.
// The user's API key arrives per call (credential.ts); one SDK client per binding, dropped on auth failure.
import { randomUUID } from "node:crypto";
import {
  APIConnectionError, APIError, AuthenticationError, BadRequestError, InternalServerError, NotFoundError, PermissionDeniedError, RateLimitError,
} from "@anthropic-ai/sdk";
import {
  BaseAaiProvider, fail, ok,
  type AaiResult, type AgentCapabilityManifest, type AgentDetail, type AgentIdentity, type AgentSummary, type AAIError, type AAIErrorCode, type Approval,
  type ApprovalsInput, type ApprovalsResult, type ArtifactInfo, type CancelResult, type CursoredEvent, type EventsInput, type EventsResult,
  type GatewayEvent, type HealthResult, type MessageInput, type MessageResult, type OpenContextInput, type OpenContextResult, type SnapshotResult, type SteerInput,
} from "@allternit/agent-gateway";
import { ADAPTER_ID, DEFAULT_MAX_PARALLEL, VENDOR, buildCapabilities } from "./manifest.js";
import { callScopeCredentialResolver, currentBinding, fingerprint, redact, type BindingRef, type ResolveCredential } from "./credential.js";
import {
  sdkClientFactory, type ClientFactory, type MaClient, type MaEnvironment, type MaEvent, type MaEventListParams, type MaEventStream, type MaSendEvent, type MaSession,
} from "./client.js";
import { isPaused, isTurnEnd, needsApproval, normalize, waitingOn } from "./normalize.js";

export interface ClaudeManagedAgentsOptions {
  /** Injected. Default reads the per-call credential allternit-api attaches to /aai/call. Never reads env globals. */
  resolveCredential?: ResolveCredential;
  clientFactory?: ClientFactory;
  baseURL?: string;
  /** Managed Agents environment id (cloud or self-hosted worker environment) new sessions run in. */
  environmentId?: string;
  maxParallel?: number;
  memoryStoreIds?: string[];
  vaultIds?: string[];
  /** Archive the session on context.close (documented routine cleanup). Never deletes anything on the Allternit side. Default true. */
  archiveOnClose?: boolean;
  /** Delay between status checks while verifying an interrupt. */
  pollMs?: number;
  /** How long context.message waits for the turn to end before returning without a reply (events() keeps streaming). */
  replyTimeoutMs?: number;
  /** First reconnect delay after the event stream drops (doubles per consecutive failure, capped at 10 s). */
  reconnectBaseMs?: number;
  /** Consecutive failed reconnects before the stream is given up (surfaced as agent.health.changed). */
  maxReconnects?: number;
  /** Backfill reads history from (last seen processed_at - overlap); overlap absorbs created_at/processed_at skew. */
  backfillOverlapMs?: number;
  now?: () => number;
  sleep?: (ms: number) => Promise<void>;
}

type WaitResult = { endIdx: number } | { error: unknown };
/** A client-sent event (message, tool confirmation, custom tool result) whose turn we are following. */
interface Waiter { id?: string; anchor?: number; settle: (r: WaitResult) => void; done: Promise<WaitResult> }
interface CustomCall { name: string; input: unknown; sessionThreadId?: string | null; state: "pending" | "answered" | "cancelled" }

interface Ctx {
  id: string; agentId: string; bindingId: string; threadId: string;
  closed: boolean; watchOnly: boolean;
  events: CursoredEvent[]; seq: number; seen: Set<string>; raw: MaEvent[];
  /** newest processed_at seen: the events.list cursor for backfill */
  lastAt?: string;
  /** client-sent events seen queued (processed_at null); they are re-sent once processed */
  queued: Set<string>;
  corrByEventId: Map<string, string>; pendingCorrs: string[]; corrOf: Map<string, string>; toolCorr: Map<string, string>;
  currentCorr?: string; gen: number;
  lock: Promise<unknown>; sendChain: Promise<unknown>;
  done: Map<string, Promise<AaiResult<MessageResult>>>;
  waiters: Waiter[];
  custom: Map<string, CustomCall>;
  /** set while the SSE pump runs; cleared synchronously when it decides to stop */
  pumpToken?: object; stream?: MaEventStream; reconnects: number;
  /** the pump's current connect + backfill step (reads wait for it so they never see a log older than the stream) */
  ready?: Promise<void>;
}
interface StoredApproval { approval: Approval; sessionId: string; bindingId: string; toolUseId: string; actorId?: string }

export interface ManagedSessionInfo {
  contextId: string; agentId?: string; status: string; title: string | null; threadId: string | null;
  createdAt: string | null; archived: boolean; adopted: boolean;
}

const ANTHROPIC_KEY_HINT = "Connect your Anthropic API key to use Claude Managed Agents. Allternit does not supply one.";
const DEFAULT_BINDING = "default";

/** Stream failures worth a reconnect: network drops, 5xx, and anything that is not a typed 4xx API error. */
const isTransient = (e: unknown): boolean =>
  e instanceof APIConnectionError || e instanceof InternalServerError || !(e instanceof APIError) || (typeof e.status === "number" ? e.status >= 500 : true);

export class ClaudeManagedAgentsProvider extends BaseAaiProvider {
  readonly adapterId = ADAPTER_ID;
  private ctxs = new Map<string, Ctx>();
  private approvalsStore = new Map<string, StoredApproval>();
  private clients = new Map<string, { fp: string; client: MaClient; apiKey: string }>(); // per binding, never shared
  private authed = new Set<string>();
  private o: Required<Pick<ClaudeManagedAgentsOptions, "resolveCredential" | "clientFactory" | "maxParallel" | "archiveOnClose" | "pollMs" | "replyTimeoutMs" | "reconnectBaseMs" | "maxReconnects" | "backfillOverlapMs" | "now" | "sleep">> & ClaudeManagedAgentsOptions;

  constructor(opts: ClaudeManagedAgentsOptions = {}) {
    super();
    this.o = {
      resolveCredential: callScopeCredentialResolver, clientFactory: sdkClientFactory, maxParallel: DEFAULT_MAX_PARALLEL, archiveOnClose: true,
      pollMs: 500, replyTimeoutMs: 120_000, reconnectBaseMs: 250, maxReconnects: 5, backfillOverlapMs: 60_000, now: Date.now, sleep: (ms) => new Promise((r) => setTimeout(r, ms)), ...opts,
    };
  }

  private caps(): AgentCapabilityManifest { return buildCapabilities(this.o.maxParallel); }
  private bindingId(): string { return currentBinding()?.id ?? DEFAULT_BINDING; }

  /** Register a session the gateway already bound (restart / restored binding) so its pending approvals and events are visible. */
  watchSession(sessionId: string, agentId: string, threadId?: string, bindingId: string = DEFAULT_BINDING): void {
    if (!this.ctxs.has(sessionId)) this.ctxs.set(sessionId, this.newCtx(sessionId, agentId, bindingId, threadId, true));
  }

  // ---------- credential + client + errors ----------
  private async withClient<T>(op: string, extra: { agentId?: string; contextId?: string }, notFound: AAIErrorCode,
    fn: (client: MaClient, bindingId: string) => Promise<AaiResult<T>>): Promise<AaiResult<T>> {
    const binding: BindingRef | undefined = currentBinding();
    const bid = binding?.id ?? DEFAULT_BINDING;
    let apiKey: string | undefined;
    try { apiKey = (await this.o.resolveCredential(binding, { op, ...extra }))?.apiKey; } catch { apiKey = undefined; }
    if (!apiKey) return fail("AUTH_REQUIRED", ANTHROPIC_KEY_HINT);
    const fp = fingerprint(apiKey);
    let entry = this.clients.get(bid);
    if (!entry || entry.fp !== fp) { entry = { fp, apiKey, client: this.o.clientFactory(apiKey, { baseURL: this.o.baseURL }) }; this.clients.set(bid, entry); }
    try {
      const r = await fn(entry.client, bid);
      if (r.ok) this.authed.add(bid);
      return r;
    } catch (e) {
      const mapped = this.mapError(e, notFound, bid, apiKey);
      if (mapped.retireClient) this.clients.delete(bid);
      return { ok: false, error: mapped.error };
    }
  }

  private mapError(e: unknown, notFound: AAIErrorCode, bid: string, apiKey: string): { error: AAIError; retireClient: boolean } {
    const mk = (code: AAIErrorCode, msg: string, extra: Partial<AAIError> = {}): AAIError => {
      const r = fail(code, redact(msg, apiKey), extra) as { ok: false; error: AAIError };
      return r.error;
    };
    if (e instanceof AuthenticationError || e instanceof PermissionDeniedError) {
      const revoked = this.authed.has(bid);
      this.authed.delete(bid);
      return { retireClient: true, error: mk(revoked ? "AUTH_REVOKED" : "AUTH_REQUIRED",
        revoked ? "Anthropic rejected the API key (revoked, expired or lacking access). Reconnect your Anthropic API key." : ANTHROPIC_KEY_HINT + " Anthropic rejected the key that was provided.",
        { vendorCode: String(e.status) }) };
    }
    if (e instanceof RateLimitError) {
      const h = e.headers as Headers | undefined;
      const ms = Number(h?.get?.("retry-after-ms")), s = Number(h?.get?.("retry-after"));
      const retryAfterMs = Number.isFinite(ms) && ms > 0 ? Math.round(ms) : Number.isFinite(s) && s > 0 ? Math.round(s * 1000) : 30_000;
      return { retireClient: false, error: mk("RATE_LIMITED", "Anthropic rate limit reached for your account. Allternit will wait before sending again.", { retryAfterMs, vendorCode: "429" }) };
    }
    if (e instanceof InternalServerError || e instanceof APIConnectionError) {
      return { retireClient: false, error: mk("VENDOR_UNAVAILABLE", "Anthropic's API is unavailable right now. Try again shortly.", { vendorCode: e instanceof APIError ? String(e.status) : "connection" }) };
    }
    if (e instanceof NotFoundError) return { retireClient: false, error: mk(notFound, notFound === "CONTEXT_NOT_FOUND" ? "No such Claude session (it may have been archived)." : "Not found.", { vendorCode: "404" }) };
    if (e instanceof BadRequestError) return { retireClient: false, error: mk("UNKNOWN", `Anthropic rejected the request: ${String(e.message).slice(0, 200)}`, { vendorCode: "400" }) };
    return { retireClient: false, error: mk("UNKNOWN", `Unexpected Claude adapter error: ${String((e as Error)?.message ?? e).slice(0, 200)}`) };
  }

  // ---------- identity ----------
  async list(): Promise<AaiResult<AgentSummary[]>> {
    return this.withClient("agent.list", {}, "UNKNOWN", async (c) => {
      const out: AgentSummary[] = [];
      for await (const a of c.beta.agents.list()) {
        if (a.archived_at) continue;
        out.push({ agentId: a.id, displayName: a.name ?? a.id, vendor: VENDOR, state: "linked" });
        if (out.length >= 200) break;
      }
      return ok(out);
    });
  }
  async get(agentId: string): Promise<AaiResult<AgentDetail>> {
    return this.withClient("agent.get", { agentId }, "UNKNOWN", async (c) => {
      const a = await c.beta.agents.retrieve(agentId);
      return ok({ agentId: a.id, displayName: a.name ?? a.id, vendor: VENDOR, state: a.archived_at ? "archived" : "linked", remoteIds: { agentId: a.id }, capabilities: this.caps() });
    });
  }
  async capabilities(_agentId: string): Promise<AaiResult<AgentCapabilityManifest>> { return ok(this.caps()); }
  async identity(agentId: string): Promise<AaiResult<AgentIdentity>> {
    return this.withClient("agent.identity", { agentId }, "UNKNOWN", async (c) => {
      const a = await c.beta.agents.retrieve(agentId);
      return ok({ agentId: a.id, displayName: a.name ?? a.id, vendor: VENDOR, lookPack: ADAPTER_ID });
    });
  }

  // ---------- context ----------
  private newCtx(id: string, agentId: string, bindingId: string, threadId: string | undefined, watchOnly = false): Ctx {
    return { id, agentId, bindingId, threadId: threadId ?? id, closed: false, watchOnly, events: [], seq: 0, seen: new Set(), raw: [], queued: new Set(),
      corrByEventId: new Map(), pendingCorrs: [], corrOf: new Map(), toolCorr: new Map(), gen: 0, lock: Promise.resolve(), sendChain: Promise.resolve(),
      done: new Map(), waiters: [], custom: new Map(), reconnects: 0 };
  }
  /** Context must be open AND belong to the calling binding (keys/bindings never see each other's sessions). */
  private live(id: string): AaiResult<Ctx> {
    const c = this.ctxs.get(id);
    return c && !c.closed && c.bindingId === this.bindingId() ? ok(c) : fail("CONTEXT_NOT_FOUND", "No such open Claude session.");
  }
  private openCount(bid: string) { return [...this.ctxs.values()].filter((c) => !c.closed && !c.watchOnly && c.bindingId === bid).length; }

  private push(c: Ctx, type: GatewayEvent["type"], source: GatewayEvent["source"], remoteEventId: string, payload: Record<string, unknown>, at?: string, corr?: string) {
    c.seq += 1;
    const correlationId = corr ?? c.currentCorr ?? `obs-${c.id}`;
    c.events.push({ cursor: String(c.seq), event: {
      type, botId: c.agentId, threadId: c.threadId, generationId: String(c.gen), source, vendor: VENDOR, adapter: ADAPTER_ID, lane: "official",
      remoteEventId, remoteContextId: c.id, causationId: correlationId, correlationId, guarantee: "exact",
      at: at ?? new Date(this.o.now()).toISOString(), payload } });
  }

  async contextOpen(input: OpenContextInput): Promise<AaiResult<OpenContextResult>> {
    const bid = this.bindingId();
    const declared = this.caps();
    const reply = (id: string, resumed: boolean) => ok({ contextId: id, isolation: declared.context.isolation, guarantee: declared.guarantee, resumed });
    if (input.adoptContextId) {
      const ex = this.ctxs.get(input.adoptContextId);
      if (ex && !ex.closed && ex.bindingId === bid && ex.agentId === input.agentId) { ex.watchOnly = false; return reply(ex.id, true); }
    } else if (this.openCount(bid) >= this.o.maxParallel) {
      return fail("CONTEXT_BUSY", `maxParallel=${this.o.maxParallel} Claude sessions are already open.`, { retryable: true });
    }
    return this.withClient("agent.context.open", { agentId: input.agentId }, "CONTEXT_NOT_FOUND", async (c) => {
      if (input.adoptContextId) {
        // Re-adopt by id (e.g. after a gateway restart): verify ownership, rebuild the cursor log from the full history
        // once, and resume streaming if the session is still working.
        const s = await c.beta.sessions.retrieve(input.adoptContextId);
        if (s.archived_at || (s.agent?.id && s.agent.id !== input.agentId)) return fail("CONTEXT_NOT_FOUND", "That Claude session is archived or belongs to another agent.");
        const ctx = this.newCtx(s.id, input.agentId, bid, input.threadId ?? s.metadata?.allternit_thread_id);
        this.ctxs.set(s.id, ctx);
        this.push(ctx, "agent.context.opened", "allternit", `${s.id}:opened`, { resumed: true, status: s.status }, undefined, s.id);
        await this.backfill(c, ctx);
        if (s.status === "running" || s.status === "rescheduling") this.ensurePump(c, ctx);
        return reply(s.id, true);
      }
      if (!this.o.environmentId) return fail("UNKNOWN", "No Managed Agents environment is configured for this adapter (SUBS_GATEWAY_CLAUDE_MA_ENVIRONMENT_ID).");
      const resources = (this.o.memoryStoreIds ?? []).map((id) => ({ type: "memory_store", memory_store_id: id }));
      let s: MaSession;
      try {
        s = await c.beta.sessions.create({
          agent: { type: "agent", id: input.agentId }, environment_id: this.o.environmentId,
          ...(input.title ? { title: input.title } : {}), ...(input.threadId ? { metadata: { allternit_thread_id: input.threadId } } : {}),
          ...(resources.length ? { resources } : {}), ...(this.o.vaultIds?.length ? { vault_ids: this.o.vaultIds } : {}),
        });
      } catch (e) {
        if (e instanceof NotFoundError || e instanceof BadRequestError) { const envErr = await this.checkEnvironment(c); if (envErr) return envErr; }
        throw e;
      }
      const ctx = this.newCtx(s.id, input.agentId, bid, input.threadId);
      this.ctxs.set(s.id, ctx);
      this.push(ctx, "agent.context.opened", "allternit", `${s.id}:opened`, { title: input.title ?? null, resumed: false }, undefined, s.id);
      return reply(s.id, false);
    });
  }

  /** After a failed create: is the configured environment the problem? Lists environments to say so plainly. */
  private async checkEnvironment(c: MaClient): Promise<AaiResult<never> | undefined> {
    const want = this.o.environmentId!;
    const seen: MaEnvironment[] = [];
    try {
      for await (const env of c.beta.environments.list({ limit: 100 })) { seen.push(env); if (seen.length >= 500) break; }
    } catch { return undefined; } // cannot tell: keep the original error
    const hit = seen.find((e) => e.id === want);
    if (hit && !hit.archived_at) return undefined;
    const avail = seen.filter((e) => !e.archived_at).slice(0, 5).map((e) => (e.name ? `${e.id} (${e.name})` : e.id));
    return fail("UNKNOWN", `Managed Agents environment ${want} (SUBS_GATEWAY_CLAUDE_MA_ENVIRONMENT_ID) ${hit ? "is archived" : "was not found in this Anthropic workspace"}. ` +
      (avail.length ? `Available environments: ${avail.join(", ")}.` : "This workspace has no environments; create one in the Claude Console."), { vendorCode: "environment" });
  }

  // ---------- session listing (re-adoption after a restart) ----------
  /** The key's Managed Agents sessions (newest first), marking which ones this gateway currently has open. */
  async listSessions(input: { agentId?: string; limit?: number; includeArchived?: boolean } = {}): Promise<AaiResult<ManagedSessionInfo[]>> {
    const bid = this.bindingId();
    return this.withClient("agent.snapshot", { agentId: input.agentId }, "UNKNOWN", async (c) => {
      const max = Math.min(Math.max(input.limit ?? 50, 1), 200);
      const out: ManagedSessionInfo[] = [];
      for await (const s of c.beta.sessions.list({ ...(input.agentId ? { agent_id: input.agentId } : {}), include_archived: input.includeArchived ?? false, limit: Math.min(max, 100) })) {
        const ctx = this.ctxs.get(s.id);
        out.push({ contextId: s.id, agentId: s.agent?.id, status: s.status, title: s.title ?? null, threadId: s.metadata?.allternit_thread_id ?? null,
          createdAt: s.created_at ?? null, archived: Boolean(s.archived_at), adopted: Boolean(ctx && !ctx.closed && ctx.bindingId === bid) });
        if (out.length >= max) break;
      }
      return ok(out);
    });
  }
  /** agent.snapshot: the agent's live sessions, so a restarted gateway can re-adopt them with context.open({ adoptContextId }). */
  async snapshot(input: { agentId: string }): Promise<AaiResult<SnapshotResult>> {
    const r = await this.listSessions({ agentId: input.agentId });
    return r.ok ? ok({ agentId: input.agentId, fields: { sessions: r.value } }) : r;
  }

  // ---------- event log: one ordered, deduped, cursored log fed by the stream and by cursor backfill ----------
  private locked<T>(c: Ctx, fn: () => Promise<T>): Promise<T> {
    const run = c.lock.then(fn);
    c.lock = run.catch(() => undefined);
    return run;
  }

  /** History from the cursor (newest processed_at minus an overlap), deduped by event id. Never a full re-read once a cursor exists. */
  private backfill(client: MaClient, c: Ctx): Promise<void> {
    return this.locked(c, async () => {
      const params: MaEventListParams = { order: "asc" };
      if (c.lastAt) params["created_at[gte]"] = new Date(Date.parse(c.lastAt) - this.o.backfillOverlapMs).toISOString();
      for await (const ev of client.beta.sessions.events.list(c.id, params)) this.ingest(c, ev, false);
    });
  }

  private noteAt(c: Ctx, at: string) { if (!c.lastAt || Date.parse(at) > Date.parse(c.lastAt)) c.lastAt = at; }

  private beginTurn(c: Ctx, userEventId: string) {
    c.gen += 1;
    c.currentCorr = c.corrOf.get(userEventId) ?? userEventId;
  }

  private ingest(c: Ctx, ev: MaEvent, live: boolean): void {
    if (!ev?.id) return;
    if (c.seen.has(ev.id)) {
      // Client-sent events appear twice: queued (processed_at null), then processed (client-patterns #2).
      if (ev.processed_at && c.queued.delete(ev.id)) {
        this.noteAt(c, ev.processed_at);
        if (ev.type === "user.message") this.beginTurn(c, ev.id);
        this.anchor(c, ev.id, c.raw.length);
      }
      return;
    }
    c.seen.add(ev.id);
    const idx = c.raw.push(ev) - 1;
    const queued = ev.processed_at == null && ev.type.startsWith("user.");
    if (ev.processed_at) this.noteAt(c, ev.processed_at);
    if (queued) c.queued.add(ev.id);
    let corr: string | undefined;
    if (ev.type === "user.message") {
      corr = c.corrByEventId.get(ev.id) ?? c.pendingCorrs.shift() ?? ev.id;
      c.corrOf.set(ev.id, corr);
      if (!queued) this.beginTurn(c, ev.id);
    }
    if (!queued) this.anchor(c, ev.id, idx + 1);
    if (ev.type === "agent.custom_tool_use" && !c.custom.has(ev.id)) {
      c.custom.set(ev.id, { name: String(ev.name ?? ""), input: ev.input, sessionThreadId: (ev.session_thread_id as string | null | undefined) ?? null, state: "pending" });
    }
    if (ev.type === "user.custom_tool_result") {
      const tid = String(ev.custom_tool_use_id);
      const cc = c.custom.get(tid); if (cc) cc.state = "answered";
      const k = c.toolCorr.get(tid); if (k) c.currentCorr = k;
    }
    if (needsApproval(ev)) this.registerApproval(c, ev);
    if (ev.type === "user.tool_confirmation") this.applyConfirmation(String(ev.tool_use_id), ev.result === "allow" ? "approved" : "denied");
    if (ev.type === "session.status_terminated") {
      this.cancelPending(c.id);
      for (const cc of c.custom.values()) if (cc.state === "pending") cc.state = "cancelled";
    }
    for (const d of normalize(ev, { live })) {
      const payload = d.type === "agent.approval.resolved" ? { ...d.payload, actorId: this.approvalsStore.get(String(ev.tool_use_id))?.actorId } : d.payload;
      this.push(c, d.type, d.source, `${ev.id}:${d.suffix}`, payload, ev.processed_at ?? undefined, corr);
    }
    if (isPaused(ev)) this.flagAttention(c, ev);
    if (isTurnEnd(ev) || isPaused(ev)) this.settle(c, idx);
  }

  /**
   * idle(requires_action): say plainly what the agent is waiting for. Custom tool calls must be answered by Allternit
   * (context.message with toolResult); approvals by a person (approvals.respond). Nothing pending at all is the
   * documented self-hosted worker failure, which also needs a person.
   */
  private flagAttention(c: Ctx, ev: MaEvent) {
    const ids = waitingOn(ev);
    const customToolCalls = ids.filter((id) => c.custom.get(id)?.state === "pending")
      .map((id) => ({ toolUseId: id, name: c.custom.get(id)!.name, input: c.custom.get(id)!.input }));
    const approvalIds = ids.filter((id) => this.approvalsStore.get(id)?.approval.state === "pending");
    const reason = customToolCalls.length ? "custom_tool_result_required" : approvalIds.length ? "approval_required" : "requires_action_without_pending_call";
    this.push(c, "agent.task.updated", "vendor", `${ev.id}:attention`, {
      kind: "needs_attention", status: "requires_action", reason, customToolCalls, approvalIds, waitingOn: ids }, ev.processed_at ?? undefined);
  }

  private registerApproval(c: Ctx, ev: MaEvent) {
    if (this.approvalsStore.has(ev.id)) return;
    this.approvalsStore.set(ev.id, { sessionId: c.id, bindingId: c.bindingId, toolUseId: ev.id, approval: {
      authority: "vendor", actor: c.agentId, action: `${String(ev.name)} ${JSON.stringify(ev.input ?? {})}`.slice(0, 500), threadId: c.threadId, remoteRef: ev.id, state: "pending" } });
  }
  private applyConfirmation(toolUseId: string, state: "approved" | "denied") {
    const s = this.approvalsStore.get(toolUseId);
    if (s && s.approval.state === "pending") s.approval = { ...s.approval, state };
  }
  private cancelPending(sessionId: string) {
    for (const s of this.approvalsStore.values()) if (s.sessionId === sessionId && s.approval.state === "pending") s.approval = { ...s.approval, state: "cancelled" };
  }

  // ---------- waiters: follow a sent event to the end (or pause) of its turn ----------
  private addWaiter(c: Ctx): Waiter {
    let settle!: (r: WaitResult) => void;
    const done = new Promise<WaitResult>((r) => { settle = r; });
    const w: Waiter = { settle, done };
    c.waiters.push(w);
    return w;
  }
  private removeWaiter(c: Ctx, w: Waiter) { c.waiters = c.waiters.filter((x) => x !== w); }
  private anchor(c: Ctx, id: string, at: number) {
    const w = c.waiters.find((x) => x.id === id && x.anchor === undefined);
    if (w) w.anchor = at;
  }
  /** A turn ended (or paused) at raw index endIdx: release every waiter whose event was processed before it. */
  private settle(c: Ctx, endIdx: number) {
    for (const w of [...c.waiters]) if (w.anchor !== undefined && w.anchor <= endIdx) { this.removeWaiter(c, w); w.settle({ endIdx }); }
  }
  /** Attach the vendor id from the send response; the stream may already have delivered (and even finished) the turn. */
  private bindWaiter(c: Ctx, w: Waiter, id: string | undefined) {
    w.id = id;
    if (!id) w.anchor = c.raw.length;
    else if (c.seen.has(id) && !c.queued.has(id)) w.anchor = c.raw.findIndex((e) => e.id === id) + 1;
    if (w.anchor === undefined) return;
    const end = c.raw.findIndex((e, i) => i >= w.anchor! && (isTurnEnd(e) || isPaused(e)));
    if (end >= 0) { this.removeWaiter(c, w); w.settle({ endIdx: end }); }
  }
  private async waitFor(w: Waiter, ms: number): Promise<WaitResult | undefined> {
    let t: ReturnType<typeof setTimeout> | undefined;
    const timeout = new Promise<undefined>((r) => { t = setTimeout(() => r(undefined), ms); (t as { unref?: () => void }).unref?.(); });
    try { return await Promise.race([w.done, timeout]); } finally { clearTimeout(t); }
  }

  // ---------- the SSE pump ----------
  /** Nothing to follow: no waiter outstanding and the latest status event is a turn end or a pause. */
  private quiet(c: Ctx): boolean {
    if (c.waiters.length) return false;
    for (let i = c.raw.length - 1; i >= 0; i--) {
      const e = c.raw[i];
      if (e.type.startsWith("session.status_")) return isTurnEnd(e) || isPaused(e);
    }
    return true;
  }
  private pumping(c: Ctx, token: object) { return !c.closed && c.pumpToken === token; }
  private release(c: Ctx, token: object) { if (c.pumpToken === token) c.pumpToken = undefined; }
  private stopPump(c: Ctx) { c.pumpToken = undefined; c.stream?.controller?.abort(); c.stream = undefined; }

  /** Starts the session's event stream if it is not already running. Call BEFORE sending (stream-first, client-patterns #7). */
  private ensurePump(client: MaClient, c: Ctx): void {
    if (c.pumpToken || c.closed) return;
    const token = {};
    c.pumpToken = token;
    void this.runPump(client, c, token);
  }

  /**
   * Open the stream, backfill history from the cursor (lossless reconnect, client-patterns #1), then consume live
   * events until the turn we follow ends. A dropped or ended stream reconnects with backoff and backfills again; the
   * event-id dedupe makes overlap harmless. Never throws: a fatal error is handed to the waiters and surfaced as health.
   */
  private async runPump(client: MaClient, c: Ctx, token: object): Promise<void> {
    let failures = 0;
    try {
      while (this.pumping(c, token)) {
        let stream: MaEventStream | undefined;
        let lastErr: unknown;
        try {
          const connect = (async () => {
            stream = await client.beta.sessions.events.stream(c.id);
            if (!this.pumping(c, token)) { stream.controller?.abort(); return; }
            c.stream = stream;
            await this.backfill(client, c);
          })();
          c.ready = connect.catch(() => undefined);
          await connect;
          if (!stream || !this.pumping(c, token)) return;
          if (this.quiet(c)) { this.release(c, token); return; }
          for await (const ev of stream as MaEventStream) {
            if (!this.pumping(c, token)) return;
            failures = 0;
            this.ingest(c, ev, true);
            if ((isTurnEnd(ev) || isPaused(ev)) && this.quiet(c)) { this.release(c, token); return; }
          }
          lastErr = new APIConnectionError({ message: "Claude event stream ended" });
        } catch (e) {
          if (!this.pumping(c, token)) return;
          if (!isTransient(e)) throw e;
          lastErr = e;
        } finally {
          if (c.stream === stream) c.stream = undefined;
        }
        failures += 1;
        c.reconnects += 1;
        if (failures > this.o.maxReconnects) throw lastErr;
        await this.o.sleep(Math.min(this.o.reconnectBaseMs * 2 ** (failures - 1), 10_000));
      }
    } catch (e) {
      if (c.pumpToken === token) {
        this.release(c, token);
        const errorType = e instanceof APIError && e.status ? `http_${e.status}` : "stream_lost";
        this.push(c, "agent.health.changed", "allternit", `${c.id}:stream:${c.reconnects}:${c.seq}`, { status: "degraded", errorType, reconnects: c.reconnects });
        const waiters = c.waiters; c.waiters = [];
        for (const w of waiters) w.settle({ error: e });
      }
    } finally {
      this.release(c, token);
    }
  }

  // ---------- sending ----------
  /**
   * Send one client event with stream-first ordering and register a waiter for its turn. Correlation: a user.message
   * takes the caller's correlation id whether its echo arrives on the stream before or after the send returns.
   */
  private async dispatch(client: MaClient, c: Ctx, event: MaSendEvent, corr?: string): Promise<{ w: Waiter; id?: string }> {
    const w = this.addWaiter(c);
    const isMsg = event.type === "user.message";
    if (corr && isMsg) c.pendingCorrs.push(corr);
    if (corr && event.type === "user.custom_tool_result") c.toolCorr.set(event.custom_tool_use_id, corr);
    this.ensurePump(client, c);
    let id: string | undefined;
    try {
      const resp = await client.beta.sessions.events.send(c.id, { events: [event] });
      id = resp?.data?.[0]?.id || undefined;
    } catch (e) {
      this.removeWaiter(c, w);
      if (corr && isMsg) { const i = c.pendingCorrs.indexOf(corr); if (i >= 0) c.pendingCorrs.splice(i, 1); }
      if (this.quiet(c)) this.stopPump(c);
      throw e;
    }
    if (corr && isMsg && id && !c.seen.has(id)) {
      const i = c.pendingCorrs.indexOf(corr); if (i >= 0) c.pendingCorrs.splice(i, 1);
      c.corrByEventId.set(id, corr);
    }
    this.bindWaiter(c, w, id);
    return { w, id };
  }

  // ---------- messaging ----------
  async contextMessage(input: MessageInput): Promise<AaiResult<MessageResult>> {
    const l = this.live(input.contextId); if (!l.ok) return l;
    const c = l.value;
    const prior = c.done.get(input.correlationId);
    if (prior) return prior; // idempotent by correlation id: sequential and concurrent replays return the first result
    if (input.toolResult) {
      const cc = c.custom.get(input.toolResult.toolUseId);
      if (!cc) return fail("UNKNOWN", "No such custom tool call is waiting in this Claude session. Read events() first.");
      if (cc.state !== "pending") return fail("SYNC_CONFLICT", `That custom tool call is already ${cc.state}.`);
    }
    const run = c.sendChain.then(() => this.sendAndWait(c, input));
    c.sendChain = run.catch(() => undefined);
    c.done.set(input.correlationId, run);
    const r = await run;
    if (!r.ok && (r.error.retryable || r.error.code === "SYNC_CONFLICT")) c.done.delete(input.correlationId); // allow a genuine retry
    return r;
  }

  private sendAndWait(c: Ctx, input: MessageInput): Promise<AaiResult<MessageResult>> {
    const corr = input.correlationId;
    return this.withClient("agent.context.message", { contextId: c.id, agentId: c.agentId }, "CONTEXT_NOT_FOUND", async (client) => {
      const tr = input.toolResult;
      let sent: { w: Waiter; id?: string };
      if (tr) {
        const cc = c.custom.get(tr.toolUseId)!;
        try {
          sent = await this.dispatch(client, c, { type: "user.custom_tool_result", custom_tool_use_id: tr.toolUseId, content: [{ type: "text", text: input.text }],
            ...(tr.isError ? { is_error: true } : {}), ...(cc.sessionThreadId ? { session_thread_id: cc.sessionThreadId } : {}) }, corr);
        } catch (e) {
          if (e instanceof BadRequestError) return fail("SYNC_CONFLICT", "Claude is not waiting for that custom tool result (already answered or the turn moved on).", { vendorCode: "400" });
          throw e;
        }
        if (cc.state === "pending") cc.state = "answered";
      } else {
        sent = await this.dispatch(client, c, { type: "user.message", content: [{ type: "text", text: input.text }] }, corr);
      }
      const messageId = sent.id ?? `ma-msg-${corr}`;
      const r = await this.waitFor(sent.w, this.o.replyTimeoutMs);
      if (!r) return ok({ messageId, correlationId: corr, guarantee: "exact" as const }); // still running: events() keeps streaming
      if ("error" in r) throw r.error;
      const reply = c.raw.slice(sent.w.anchor ?? 0, r.endIdx).filter((e) => e.type === "agent.message")
        .map((e) => (Array.isArray(e.content) ? (e.content as Array<{ type?: string; text?: string }>).map((b) => (b.type === "text" ? b.text ?? "" : "")).join("") : "")).join("\n\n");
      return ok({ messageId, correlationId: corr, reply: reply || undefined, guarantee: "exact" as const });
    });
  }

  private async interrupt(client: MaClient, c: Ctx): Promise<boolean> {
    await client.beta.sessions.events.send(c.id, { events: [{ type: "user.interrupt" }] });
    let confirmed = false;
    for (let i = 0; i < 6; i++) { // verify via retrieve (client-patterns #3)
      const s = await client.beta.sessions.retrieve(c.id);
      if (s.status === "idle" || s.status === "terminated") { confirmed = true; break; }
      await this.o.sleep(this.o.pollMs);
    }
    if (!c.pumpToken) await this.backfill(client, c); // a running pump already delivers the resulting events
    return confirmed;
  }

  async contextCancel(input: { contextId: string }): Promise<AaiResult<CancelResult>> {
    const l = this.live(input.contextId); if (!l.ok) return l;
    return this.withClient("agent.context.cancel", { contextId: input.contextId }, "CONTEXT_NOT_FOUND", async (client) =>
      ok({ confirmed: await this.interrupt(client, l.value) }));
  }

  /** Steer = interrupt the running turn, then send the new instruction (documented steering pattern). */
  async contextSteer(input: SteerInput): Promise<AaiResult<{ accepted: boolean }>> {
    const l = this.live(input.contextId); if (!l.ok) return l;
    const c = l.value;
    return this.withClient("agent.context.steer", { contextId: c.id }, "CONTEXT_NOT_FOUND", async (client) => {
      await this.interrupt(client, c);
      await this.dispatch(client, c, { type: "user.message", content: [{ type: "text", text: input.text }] }, `steer-${randomUUID()}`);
      return ok({ accepted: true });
    });
  }

  async contextClose(input: { contextId: string }): Promise<AaiResult<{ closed: boolean }>> {
    const l = this.live(input.contextId); if (!l.ok) return l;
    const c = l.value;
    const finish = () => {
      c.closed = true; this.stopPump(c); this.cancelPending(c.id);
      const waiters = c.waiters; c.waiters = [];
      for (const w of waiters) w.settle({ endIdx: c.raw.length });
    };
    if (!this.o.archiveOnClose) { finish(); return ok({ closed: true }); }
    const r = await this.withClient("agent.context.close", { contextId: c.id }, "CONTEXT_NOT_FOUND", async (client) => {
      try { await client.beta.sessions.archive(c.id); } catch (e) { if (!(e instanceof NotFoundError)) throw e; } // already gone: fine
      return ok({ closed: true });
    });
    if (r.ok) finish(); // archive only; nothing on the Allternit side is deleted
    return r;
  }

  async events(input: EventsInput): Promise<AaiResult<EventsResult>> {
    const l = this.live(input.contextId); if (!l.ok) return l;
    const c = l.value;
    return this.withClient("agent.events", { contextId: c.id }, "CONTEXT_NOT_FOUND", async (client) => {
      if (!c.pumpToken) await this.backfill(client, c);
      else await c.ready; // while streaming, the log is current once the pump's connect + backfill is done
      const from = Number(input.cursor ?? 0) || 0;
      const evs = c.events.filter((e) => Number(e.cursor) > from).slice(0, input.limit ?? 500);
      return ok({ events: evs, nextCursor: evs.length ? evs[evs.length - 1].cursor : String(from) });
    });
  }

  // ---------- approvals (vendor authority; human only) ----------
  async approvals(input: ApprovalsInput): Promise<AaiResult<ApprovalsResult>> {
    if (input.op === "respond" && input.actor.type !== "human") {
      return fail("APPROVAL_REQUIRED", "Only a person can answer a Claude tool approval. Allternit never auto-approves.");
    }
    const bid = this.bindingId();
    if (input.op === "list") {
      return this.withClient("agent.approvals", { contextId: input.contextId }, "CONTEXT_NOT_FOUND", async (client) => {
        const targets = input.contextId
          ? [this.ctxs.get(input.contextId)].filter((x): x is Ctx => !!x && x.bindingId === bid)
          : [...this.ctxs.values()].filter((x) => !x.closed && x.bindingId === bid);
        for (const c of targets) { if (!c.pumpToken) await this.backfill(client, c); else await c.ready; }
        const scopeIds = new Set(input.contextId ? targets.map((t) => t.id) : [...this.ctxs.values()].filter((x) => x.bindingId === bid).map((x) => x.id));
        return ok({ approvals: [...this.approvalsStore.values()].filter((s) => scopeIds.has(s.sessionId)).map((s) => s.approval) });
      });
    }
    const s = this.approvalsStore.get(input.approvalId);
    if (!s || s.bindingId !== bid) return fail("UNKNOWN", "No such approval is known. List approvals first.");
    if (s.approval.state !== "pending") return fail("SYNC_CONFLICT", `That approval is already ${s.approval.state}.`);
    return this.withClient("agent.approvals", { contextId: s.sessionId }, "CONTEXT_NOT_FOUND", async (client) => {
      const c = this.ctxs.get(s.sessionId);
      const event: MaSendEvent = {
        type: "user.tool_confirmation", tool_use_id: s.toolUseId, result: input.decision === "approve" ? "allow" : "deny",
        ...(input.decision === "deny" ? { deny_message: "Denied by the user in Allternit." } : {}) };
      const prevActor = s.actorId;
      s.actorId = input.actor.id; // set first: the stream may deliver the confirmation echo before send returns
      try {
        if (c && !c.closed) await this.dispatch(client, c, event); // the agent resumes: follow it on the stream
        else await client.beta.sessions.events.send(s.sessionId, { events: [event] });
      } catch (e) {
        s.actorId = prevActor;
        if (e instanceof BadRequestError) { s.approval = { ...s.approval, state: "expired" }; return fail("SYNC_CONFLICT", "Claude no longer accepts a decision for that tool call (already answered or not awaiting approval)."); }
        throw e;
      }
      s.approval = { ...s.approval, state: input.decision === "approve" ? "approved" : "denied" };
      return ok({ resolved: s.approval });
    });
  }

  // ---------- artifacts (files the agent wrote to /mnt/session/outputs) ----------
  // Read-only: AAI's artifacts op has no upload shape, so uploading files into a session is not offered.
  async artifacts(input: { agentId: string; contextId?: string }): Promise<AaiResult<ArtifactInfo[]>> {
    const bid = this.bindingId();
    const targets = input.contextId ? [this.ctxs.get(input.contextId)].filter((x): x is Ctx => !!x && x.bindingId === bid)
      : [...this.ctxs.values()].filter((x) => !x.closed && x.bindingId === bid && x.agentId === input.agentId);
    if (input.contextId && !targets.length) return fail("CONTEXT_NOT_FOUND", "No such Claude session.");
    return this.withClient("agent.artifacts", { agentId: input.agentId, contextId: input.contextId }, "CONTEXT_NOT_FOUND", async (client) => {
      const out: ArtifactInfo[] = [];
      for (const c of targets) {
        const files = await client.beta.files.list({ scope_id: c.id, betas: ["managed-agents-2026-04-01"] });
        for (const f of files.data) out.push({ artifactId: f.id, name: f.filename });
      }
      return ok(out);
    });
  }

  // ---------- health ----------
  async health(_i: { agentId?: string }): Promise<AaiResult<HealthResult>> {
    const r = await this.withClient("agent.health", {}, "UNKNOWN", async (c) => {
      for await (const _a of c.beta.agents.list({ limit: 1 })) break;
      return ok(true);
    });
    if (r.ok) return ok({ status: "healthy", lane: "official" });
    return ok({ status: r.error.code === "VENDOR_UNAVAILABLE" || r.error.code === "RATE_LIMITED" ? "degraded" : "down", lane: "official", detail: r.error.code });
  }
}
