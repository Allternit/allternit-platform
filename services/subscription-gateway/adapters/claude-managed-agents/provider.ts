// Claude Managed Agents AAI provider (lane official, guarantee exact, mode hosted).
// One Managed Agents session per AAI context (context id == session id). Events come from the session event history
// (`sessions.events.list`, order asc) deduped by vendor event id, giving a lossless, replayable cursor log; a dropped
// live stream can never lose events (managed-agents-client-patterns.md #1). Approvals mirror `agent.tool_use` events
// whose `evaluated_permission === "ask"` and are answered ONLY for a human actor via `user.tool_confirmation`.
// The user's API key arrives per call (credential.ts); one SDK client per binding, dropped on auth failure.
import { randomUUID } from "node:crypto";
import {
  APIConnectionError, APIError, AuthenticationError, BadRequestError, InternalServerError, NotFoundError, PermissionDeniedError, RateLimitError,
} from "@anthropic-ai/sdk";
import {
  BaseAaiProvider, fail, ok,
  type AaiResult, type AgentCapabilityManifest, type AgentDetail, type AgentIdentity, type AgentSummary, type AAIError, type AAIErrorCode, type Approval,
  type ApprovalsInput, type ApprovalsResult, type ArtifactInfo, type CancelResult, type CursoredEvent, type EventsInput, type EventsResult,
  type GatewayEvent, type HealthResult, type MessageInput, type MessageResult, type OpenContextInput, type OpenContextResult, type SteerInput,
} from "@allternit/agent-gateway";
import { ADAPTER_ID, DEFAULT_MAX_PARALLEL, VENDOR, buildCapabilities } from "./manifest.js";
import { callScopeCredentialResolver, currentBinding, fingerprint, redact, type BindingRef, type ResolveCredential } from "./credential.js";
import { sdkClientFactory, type ClientFactory, type MaClient, type MaEvent } from "./client.js";
import { isPaused, isTurnEnd, needsApproval, normalize } from "./normalize.js";

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
  pollMs?: number;
  replyTimeoutMs?: number;
  now?: () => number;
  sleep?: (ms: number) => Promise<void>;
}

interface Ctx {
  id: string; agentId: string; bindingId: string; threadId: string;
  closed: boolean; watchOnly: boolean;
  events: CursoredEvent[]; seq: number; seen: Set<string>; raw: MaEvent[];
  corrByEventId: Map<string, string>; pendingCorrs: string[]; currentCorr?: string; gen: number;
  lock: Promise<unknown>; sendChain: Promise<unknown>;
  done: Map<string, Promise<AaiResult<MessageResult>>>;
}
interface StoredApproval { approval: Approval; sessionId: string; bindingId: string; toolUseId: string; actorId?: string }

const ANTHROPIC_KEY_HINT = "Connect your Anthropic API key to use Claude Managed Agents. Allternit does not supply one.";
const DEFAULT_BINDING = "default";

export class ClaudeManagedAgentsProvider extends BaseAaiProvider {
  readonly adapterId = ADAPTER_ID;
  private ctxs = new Map<string, Ctx>();
  private approvalsStore = new Map<string, StoredApproval>();
  private clients = new Map<string, { fp: string; client: MaClient; apiKey: string }>(); // per binding, never shared
  private authed = new Set<string>();
  private o: Required<Pick<ClaudeManagedAgentsOptions, "resolveCredential" | "clientFactory" | "maxParallel" | "archiveOnClose" | "pollMs" | "replyTimeoutMs" | "now" | "sleep">> & ClaudeManagedAgentsOptions;

  constructor(opts: ClaudeManagedAgentsOptions = {}) {
    super();
    this.o = {
      resolveCredential: callScopeCredentialResolver, clientFactory: sdkClientFactory, maxParallel: DEFAULT_MAX_PARALLEL, archiveOnClose: true,
      pollMs: 500, replyTimeoutMs: 120_000, now: Date.now, sleep: (ms) => new Promise((r) => setTimeout(r, ms)), ...opts,
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
    return { id, agentId, bindingId, threadId: threadId ?? id, closed: false, watchOnly, events: [], seq: 0, seen: new Set(), raw: [], corrByEventId: new Map(),
      pendingCorrs: [], gen: 0, lock: Promise.resolve(), sendChain: Promise.resolve(), done: new Map() };
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
        const s = await c.beta.sessions.retrieve(input.adoptContextId);
        if (s.archived_at || (s.agent?.id && s.agent.id !== input.agentId)) return fail("CONTEXT_NOT_FOUND", "That Claude session is archived or belongs to another agent.");
        const ctx = this.newCtx(s.id, input.agentId, bid, input.threadId);
        this.ctxs.set(s.id, ctx);
        this.push(ctx, "agent.context.opened", "allternit", `${s.id}:opened`, { resumed: true, status: s.status }, undefined, s.id);
        return reply(s.id, true);
      }
      if (!this.o.environmentId) return fail("UNKNOWN", "No Managed Agents environment is configured for this adapter (SUBS_GATEWAY_CLAUDE_MA_ENVIRONMENT_ID).");
      const resources = (this.o.memoryStoreIds ?? []).map((id) => ({ type: "memory_store", memory_store_id: id }));
      const s = await c.beta.sessions.create({
        agent: { type: "agent", id: input.agentId }, environment_id: this.o.environmentId,
        ...(input.title ? { title: input.title } : {}), ...(input.threadId ? { metadata: { allternit_thread_id: input.threadId } } : {}),
        ...(resources.length ? { resources } : {}), ...(this.o.vaultIds?.length ? { vault_ids: this.o.vaultIds } : {}),
      });
      const ctx = this.newCtx(s.id, input.agentId, bid, input.threadId);
      this.ctxs.set(s.id, ctx);
      this.push(ctx, "agent.context.opened", "allternit", `${s.id}:opened`, { title: input.title ?? null, resumed: false }, undefined, s.id);
      return reply(s.id, false);
    });
  }

  // ---------- sync (lossless cursor log from event history) ----------
  private locked<T>(c: Ctx, fn: () => Promise<T>): Promise<T> {
    const run = c.lock.then(fn);
    c.lock = run.catch(() => undefined);
    return run;
  }
  private syncCtx(client: MaClient, c: Ctx) { return this.locked(c, () => this.doSync(client, c)); }

  private async doSync(client: MaClient, c: Ctx): Promise<void> {
    for await (const ev of client.beta.sessions.events.list(c.id, { order: "asc" })) {
      if (!ev?.id || c.seen.has(ev.id)) continue; // dedupe by event id (history overlaps live/previous syncs)
      c.seen.add(ev.id);
      c.raw.push(ev);
      if (ev.type === "user.message") {
        c.gen += 1;
        c.currentCorr = c.corrByEventId.get(ev.id) ?? c.pendingCorrs.shift() ?? ev.id;
      }
      if (needsApproval(ev)) this.registerApproval(c, ev);
      if (ev.type === "user.tool_confirmation") this.applyConfirmation(String(ev.tool_use_id), ev.result === "allow" ? "approved" : "denied");
      if (ev.type === "session.status_terminated") this.cancelPending(c.id);
      for (const d of normalize(ev)) {
        const payload = d.type === "agent.approval.resolved" ? { ...d.payload, actorId: this.approvalsStore.get(String(ev.tool_use_id))?.actorId } : d.payload;
        this.push(c, d.type, d.source, `${ev.id}:${d.suffix}`, payload, ev.processed_at ?? undefined);
      }
    }
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

  // ---------- messaging ----------
  async contextMessage(input: MessageInput): Promise<AaiResult<MessageResult>> {
    const l = this.live(input.contextId); if (!l.ok) return l;
    const c = l.value;
    const prior = c.done.get(input.correlationId);
    if (prior) return prior; // idempotent by correlation id: sequential and concurrent replays return the first result
    const run = c.sendChain.then(() => this.sendAndWait(c, input.correlationId, input.text));
    c.sendChain = run.catch(() => undefined);
    c.done.set(input.correlationId, run);
    const r = await run;
    if (!r.ok && r.error.retryable) c.done.delete(input.correlationId); // transient failure: allow a genuine retry
    return r;
  }

  private sendAndWait(c: Ctx, corr: string, text: string): Promise<AaiResult<MessageResult>> {
    return this.withClient("agent.context.message", { contextId: c.id, agentId: c.agentId }, "CONTEXT_NOT_FOUND", async (client) => {
      const userEventId = await this.locked(c, async () => {
        const resp = await client.beta.sessions.events.send(c.id, { events: [{ type: "user.message", content: [{ type: "text", text }] }] });
        const id = resp?.data?.[0]?.id;
        if (id) c.corrByEventId.set(id, corr); else c.pendingCorrs.push(corr);
        return id;
      });
      const deadline = this.o.now() + this.o.replyTimeoutMs;
      for (;;) {
        await this.syncCtx(client, c);
        const from = userEventId ? c.raw.findIndex((e) => e.id === userEventId) : -1;
        if (from >= 0) {
          const after = c.raw.slice(from + 1);
          const endAt = after.findIndex((e) => isTurnEnd(e) || isPaused(e));
          if (endAt >= 0) {
            const reply = after.slice(0, endAt).filter((e) => e.type === "agent.message")
              .map((e) => (Array.isArray(e.content) ? (e.content as Array<{ type?: string; text?: string }>).map((b) => (b.type === "text" ? b.text ?? "" : "")).join("") : "")).join("\n\n");
            return ok({ messageId: userEventId!, correlationId: corr, reply: reply || undefined, guarantee: "exact" as const });
          }
        }
        if (this.o.now() >= deadline) return ok({ messageId: userEventId ?? `ma-msg-${corr}`, correlationId: corr, guarantee: "exact" as const }); // still running: keep reading events()
        await this.o.sleep(this.o.pollMs);
      }
    });
  }

  private async interrupt(client: MaClient, c: Ctx): Promise<boolean> {
    await client.beta.sessions.events.send(c.id, { events: [{ type: "user.interrupt" }] });
    for (let i = 0; i < 6; i++) { // verify via retrieve (client-patterns #3)
      const s = await client.beta.sessions.retrieve(c.id);
      if (s.status === "idle" || s.status === "terminated") { await this.syncCtx(client, c); return true; }
      await this.o.sleep(this.o.pollMs);
    }
    await this.syncCtx(client, c);
    return false;
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
      const corr = `steer-${randomUUID()}`;
      await this.locked(c, async () => {
        const resp = await client.beta.sessions.events.send(c.id, { events: [{ type: "user.message", content: [{ type: "text", text: input.text }] }] });
        const id = resp?.data?.[0]?.id;
        if (id) c.corrByEventId.set(id, corr); else c.pendingCorrs.push(corr);
      });
      return ok({ accepted: true });
    });
  }

  async contextClose(input: { contextId: string }): Promise<AaiResult<{ closed: boolean }>> {
    const l = this.live(input.contextId); if (!l.ok) return l;
    const c = l.value;
    if (!this.o.archiveOnClose) { c.closed = true; this.cancelPending(c.id); return ok({ closed: true }); }
    const r = await this.withClient("agent.context.close", { contextId: c.id }, "CONTEXT_NOT_FOUND", async (client) => {
      try { await client.beta.sessions.archive(c.id); } catch (e) { if (!(e instanceof NotFoundError)) throw e; } // already gone: fine
      return ok({ closed: true });
    });
    if (r.ok) { c.closed = true; this.cancelPending(c.id); } // archive only; nothing on the Allternit side is deleted
    return r;
  }

  async events(input: EventsInput): Promise<AaiResult<EventsResult>> {
    const l = this.live(input.contextId); if (!l.ok) return l;
    const c = l.value;
    return this.withClient("agent.events", { contextId: c.id }, "CONTEXT_NOT_FOUND", async (client) => {
      await this.syncCtx(client, c);
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
        for (const c of targets) await this.syncCtx(client, c);
        const scopeIds = new Set(input.contextId ? targets.map((t) => t.id) : [...this.ctxs.values()].filter((x) => x.bindingId === bid).map((x) => x.id));
        return ok({ approvals: [...this.approvalsStore.values()].filter((s) => scopeIds.has(s.sessionId)).map((s) => s.approval) });
      });
    }
    const s = this.approvalsStore.get(input.approvalId);
    if (!s || s.bindingId !== bid) return fail("UNKNOWN", "No such approval is known. List approvals first.");
    if (s.approval.state !== "pending") return fail("SYNC_CONFLICT", `That approval is already ${s.approval.state}.`);
    return this.withClient("agent.approvals", { contextId: s.sessionId }, "CONTEXT_NOT_FOUND", async (client) => {
      try {
        await client.beta.sessions.events.send(s.sessionId, { events: [{
          type: "user.tool_confirmation", tool_use_id: s.toolUseId, result: input.op === "respond" && input.decision === "approve" ? "allow" : "deny",
          ...(input.decision === "deny" ? { deny_message: "Denied by the user in Allternit." } : {}) }] });
      } catch (e) {
        if (e instanceof BadRequestError) { s.approval = { ...s.approval, state: "expired" }; return fail("SYNC_CONFLICT", "Claude no longer accepts a decision for that tool call (already answered or not awaiting approval)."); }
        throw e;
      }
      s.actorId = input.actor.id;
      s.approval = { ...s.approval, state: input.decision === "approve" ? "approved" : "denied" };
      return ok({ resolved: s.approval });
    });
  }

  // ---------- artifacts (files the agent wrote to /mnt/session/outputs) ----------
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
