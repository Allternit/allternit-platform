// Loopback provider: exposes an Allternit Bot through AAI by calling the existing allternit-api
// HTTP endpoints (thread_routes.rs + agent_session_routes.rs). Thread <-> Agent Context.
import { agentCapabilityManifestSchema, type GatewayEventType } from "@allternit/subscription-fabric-contracts";
import { BaseAaiProvider } from "./provider.js";
import {
  fail, ok,
  type AaiResult, type AgentCapabilityManifest, type AgentDetail, type AgentIdentity, type AgentSummary,
  type ApprovalsInput, type ApprovalsResult, type CancelResult, type CursoredEvent, type EventsInput, type EventsResult,
  type GatewayEvent, type HealthResult, type MessageInput, type MessageResult, type OpenContextInput, type OpenContextResult,
} from "./types.js";

export interface LoopbackConfig {
  /** e.g. http://127.0.0.1:3010/api/v1 */
  baseUrl: string;
  /** Bots this provider exposes as AAI agents. */
  botIds: string[];
  fetch?: typeof fetch;
  auth?: { token?: string; headers?: () => Record<string, string> | Promise<Record<string, string>> };
  /** Declared concurrent-context cap. Default 4. */
  maxParallel?: number;
  adapterId?: string;
}

interface Ctx {
  threadId: string; sessionId?: string; botId: string; generation: string;
  events: CursoredEvent[]; seen: Set<string>; seq: number; closed: boolean;
}

function mapLedgerType(t: string): GatewayEventType {
  if (t.includes("message")) return "agent.message.completed";
  if (t.includes("tool")) return "agent.tool.called";
  if (t.includes("approval") || t.includes("permission")) return t.includes("resolv") || t.includes("repl") ? "agent.approval.resolved" : "agent.approval.requested";
  if (t.includes("artifact")) return "agent.artifact.created";
  if (t.endsWith(".created") || t.endsWith(".opened")) return "agent.context.opened";
  return "agent.activity.started";
}

export class LoopbackProvider extends BaseAaiProvider {
  readonly adapterId: string;
  private f: typeof fetch;
  private ctxs = new Map<string, Ctx>();
  private opening = 0;
  private idem = new Map<string, Promise<AaiResult<MessageResult>>>();

  constructor(private cfg: LoopbackConfig) {
    super();
    this.adapterId = cfg.adapterId ?? "allternit-loopback";
    this.f = cfg.fetch ?? fetch;
  }

  private manifest(): AgentCapabilityManifest {
    return agentCapabilityManifestSchema.parse({
      vendor: "allternit", adapterId: this.adapterId, lane: "local", guarantee: "exact",
      context: { supported: true, resume: true, parallel: true, maxParallel: this.cfg.maxParallel ?? 4, isolation: "isolated" },
      messaging: { send: true, stream: false, steer: false, interrupt: false, cancel: true },
      memory: { read: false, write: false, snapshot: false, opaque: false },
      tools: { tools: false, mcp: false, plugins: false, connectors: false },
      tasks: { list: false, schedule: false, cancel: false, background: false },
      approvals: { read: false, respond: true, exact: true },
      computer: { view: false, control: false, takeover: false },
      artifacts: { read: false, write: false, export: false },
      events: { native: false, polling: true, transcriptDerived: false, replay: true },
      runtime: { alwaysOn: false, localRequired: true, cloud: false },
    });
  }

  // ---- HTTP ----
  private async http(method: string, path: string, body?: unknown): Promise<AaiResult<unknown>> {
    const headers: Record<string, string> = { accept: "application/json" };
    if (body !== undefined) headers["content-type"] = "application/json";
    if (this.cfg.auth?.token) headers.authorization = `Bearer ${this.cfg.auth.token}`;
    if (this.cfg.auth?.headers) Object.assign(headers, await this.cfg.auth.headers());
    let res: Response;
    try {
      res = await this.f(this.cfg.baseUrl.replace(/\/$/, "") + path, { method, headers, body: body === undefined ? undefined : JSON.stringify(body) });
    } catch (e) {
      return fail("VENDOR_UNAVAILABLE", `allternit-api unreachable: ${(e as Error).message}`);
    }
    const st = res.status;
    if (st >= 200 && st < 300) {
      if (st === 204) return ok(null);
      const text = await res.text();
      if (!text) return ok(null);
      try { return ok(JSON.parse(text)); } catch { return fail("ADAPTER_DRIFT", "allternit-api returned non-JSON", { retryable: false }); }
    }
    const vendorCode = String(st);
    if (st === 401) return fail("AUTH_REQUIRED", "allternit-api rejected credentials", { vendorCode });
    if (st === 403) return fail("POLICY_DENIED", "allternit-api denied the request", { vendorCode });
    if (st === 404) return fail("CONTEXT_NOT_FOUND", `not found: ${path}`, { vendorCode });
    if (st === 409) return fail("CONTEXT_BUSY", "session is busy", { vendorCode, retryable: true });
    if (st === 429) {
      const ra = Number(res.headers.get("retry-after"));
      return fail("RATE_LIMITED", "allternit-api rate limited", { vendorCode, ...(ra > 0 ? { retryAfterMs: ra * 1000 } : {}) });
    }
    if (st >= 500) return fail("VENDOR_UNAVAILABLE", `allternit-api error ${st}`, { vendorCode });
    return fail("UNKNOWN", `unexpected status ${st}`, { vendorCode, retryable: false });
  }

  private emit(c: Ctx, type: GatewayEventType, correlationId: string, key: string, payload?: Record<string, unknown>, remoteEventId?: string, at?: string): void {
    if (c.seen.has(key)) return;
    c.seen.add(key);
    c.seq += 1;
    const event: GatewayEvent = {
      type, botId: c.botId, threadId: c.threadId, generationId: c.generation, source: "allternit",
      vendor: "allternit", adapter: this.adapterId, lane: "local", remoteContextId: c.threadId,
      causationId: correlationId, correlationId, guarantee: "exact", at: at ?? new Date().toISOString(),
      ...(remoteEventId ? { remoteEventId } : {}), ...(payload ? { payload } : {}),
    };
    c.events.push({ cursor: String(c.seq), event });
  }

  // ---- identity ----
  async list(): Promise<AaiResult<AgentSummary[]>> {
    return ok(this.cfg.botIds.map((id) => ({ agentId: id, displayName: id, vendor: "allternit", state: "READY" })));
  }
  async get(agentId: string): Promise<AaiResult<AgentDetail>> {
    if (!this.cfg.botIds.includes(agentId)) return fail("CONTEXT_NOT_FOUND", `unknown bot ${agentId}`);
    return ok({ agentId, displayName: agentId, vendor: "allternit", state: "READY", remoteIds: { botId: agentId }, capabilities: this.manifest() });
  }
  async capabilities(agentId: string): Promise<AaiResult<AgentCapabilityManifest>> {
    if (!this.cfg.botIds.includes(agentId)) return fail("CONTEXT_NOT_FOUND", `unknown bot ${agentId}`);
    return ok(this.manifest());
  }
  async identity(agentId: string): Promise<AaiResult<AgentIdentity>> {
    if (!this.cfg.botIds.includes(agentId)) return fail("CONTEXT_NOT_FOUND", `unknown bot ${agentId}`);
    return ok({ agentId, displayName: agentId, vendor: "allternit", lookPack: null });
  }

  // ---- context ----
  private parseThread(v: unknown, botId: string): AaiResult<Ctx> {
    const t = v as Record<string, unknown> | null;
    if (!t || typeof t.id !== "string") return fail("ADAPTER_DRIFT", "thread response missing id", { retryable: false });
    return ok({
      threadId: t.id, sessionId: typeof t.currentSessionId === "string" ? t.currentSessionId : undefined,
      botId: typeof t.botId === "string" ? t.botId : botId, generation: String(t.generation ?? 1),
      events: [], seen: new Set(), seq: 0, closed: false,
    });
  }

  private openCount(): number {
    let n = this.opening;
    for (const c of this.ctxs.values()) if (!c.closed) n++;
    return n;
  }

  async contextOpen(i: OpenContextInput): Promise<AaiResult<OpenContextResult>> {
    if (!this.cfg.botIds.includes(i.agentId)) return fail("CONTEXT_NOT_FOUND", `unknown bot ${i.agentId}`);
    const max = this.cfg.maxParallel ?? 4;
    if (this.openCount() >= max) return fail("CONTEXT_BUSY", `maxParallel=${max} reached`, { retryable: true });
    this.opening++;
    try {
      const adopt = i.adoptContextId;
      const r = adopt
        ? await this.http("GET", `/threads/${encodeURIComponent(adopt)}`)
        : await this.http("POST", "/threads", {
            botId: i.agentId, title: i.title ?? "AAI context", kind: "task", createdBy: "gateway",
            origin: { type: "gateway", provider: this.adapterId },
          });
      if (!r.ok) return r;
      const c = this.parseThread(r.value, i.agentId);
      if (!c.ok) return c;
      const ctx = c.value;
      const existing = this.ctxs.get(ctx.threadId);
      if (existing && !existing.closed) return ok({ contextId: ctx.threadId, isolation: "isolated", guarantee: "exact", resumed: true });
      this.ctxs.set(ctx.threadId, ctx);
      this.emit(ctx, "agent.context.opened", `open:${ctx.threadId}`, `open:${ctx.threadId}`, { resumed: !!adopt });
      return ok({ contextId: ctx.threadId, isolation: "isolated", guarantee: "exact", resumed: !!adopt });
    } finally { this.opening--; }
  }

  private live(id: string): AaiResult<Ctx> {
    const c = this.ctxs.get(id);
    return c && !c.closed ? ok(c) : fail("CONTEXT_NOT_FOUND", `context ${id} is not open`);
  }

  contextMessage(i: MessageInput): Promise<AaiResult<MessageResult>> {
    const key = `${i.contextId}|${i.correlationId}`;
    const prior = this.idem.get(key);
    if (prior) return prior;
    const run = this.send(i);
    this.idem.set(key, run);
    void run.then((r) => { if (!r.ok && r.error.retryable) this.idem.delete(key); });
    return run;
  }

  private async send(i: MessageInput): Promise<AaiResult<MessageResult>> {
    const c = this.live(i.contextId);
    if (!c.ok) return c;
    const ctx = c.value;
    if (!ctx.sessionId) return fail("ADAPTER_DRIFT", "thread has no current session", { retryable: false });
    this.emit(ctx, "agent.activity.started", i.correlationId, `act:${i.correlationId}`, { text: i.text });
    const r = await this.http("POST", `/agent-sessions/${encodeURIComponent(ctx.sessionId)}/messages`, { text: i.text });
    if (!r.ok) return r;
    const v = r.value as Record<string, unknown> | string | null;
    const reply = typeof v === "string" ? v : v && typeof v === "object"
      ? String(v.reply ?? v.text ?? v.content ?? JSON.stringify(v)) : "";
    this.emit(ctx, "agent.message.completed", i.correlationId, `done:${i.correlationId}`, { reply });
    return ok({ messageId: `msg:${ctx.threadId}:${i.correlationId}`, correlationId: i.correlationId, reply, guarantee: "exact" });
  }

  async contextCancel(i: { contextId: string }): Promise<AaiResult<CancelResult>> {
    const c = this.live(i.contextId);
    if (!c.ok) return c;
    if (!c.value.sessionId) return ok({ confirmed: false });
    const sid = encodeURIComponent(c.value.sessionId);
    const a = await this.http("POST", `/agent-sessions/${sid}/abort`, {});
    if (!a.ok) return a;
    const s = await this.http("GET", `/agent-sessions/${sid}/status`);
    const busy = s.ok ? (s.value as { busy?: boolean } | null)?.busy : undefined;
    return ok({ confirmed: busy === false });
  }

  async contextClose(i: { contextId: string }): Promise<AaiResult<{ closed: boolean }>> {
    const c = this.live(i.contextId);
    if (!c.ok) return c;
    c.value.closed = true; // detach only: the Allternit Thread is not deleted
    return ok({ closed: true });
  }

  // ---- events: ledger polled, buffered with monotonic cursors so replay is deterministic ----
  async events(i: EventsInput): Promise<AaiResult<EventsResult>> {
    const c = this.live(i.contextId);
    if (!c.ok) return c;
    const ctx = c.value;
    const r = await this.http("GET", `/threads/${encodeURIComponent(ctx.threadId)}/events?limit=500`);
    if (!r.ok) return r;
    const list = (r.value as { events?: unknown } | null)?.events;
    if (!Array.isArray(list)) return fail("ADAPTER_DRIFT", "events response missing events[]", { retryable: false });
    const rows = (list as Array<Record<string, unknown>>).slice().sort((a, b) => Number(a.sequence ?? 0) - Number(b.sequence ?? 0));
    for (const e of rows) {
      const id = String(e.id ?? "");
      if (!id || typeof e.type !== "string") continue;
      const payload = (e.payload && typeof e.payload === "object" ? e.payload : {}) as Record<string, unknown>;
      const corr = String(payload.correlationId ?? id);
      if (this.idemOwns(ctx, corr)) continue;
      this.emit(ctx, mapLedgerType(e.type), corr, `ledger:${id}`, { ledgerType: e.type, ...payload }, id, typeof e.occurredAt === "string" ? e.occurredAt : undefined);
    }
    const after = i.cursor === undefined ? 0 : Number(i.cursor);
    if (!Number.isInteger(after) || after < 0) return fail("UNKNOWN", `invalid cursor ${i.cursor}`, { retryable: false });
    let out = ctx.events.filter((e) => Number(e.cursor) > after);
    if (i.limit && i.limit > 0) out = out.slice(0, i.limit);
    return ok({ events: out, nextCursor: out.length ? out[out.length - 1].cursor : String(after) });
  }

  private idemOwns(ctx: Ctx, corr: string): boolean { return ctx.seen.has(`done:${corr}`); }

  // ---- approvals: respond only (no list route exists in allternit-api) ----
  async approvals(i: ApprovalsInput): Promise<AaiResult<ApprovalsResult>> {
    if (i.op === "list") return super.approvals(i);
    if (i.actor.type !== "human") return fail("APPROVAL_REQUIRED", "approvals are never resolved without an explicit human actor", { retryable: false });
    const r = await this.http("POST", `/permissions/${encodeURIComponent(i.approvalId)}/reply`, { reply: i.decision === "approve" ? "once" : "reject" });
    if (!r.ok) return r;
    return ok({ resolved: { authority: "allternit", actor: i.actor.id, action: "permission.reply", threadId: i.contextId ?? "", remoteRef: i.approvalId, state: i.decision === "approve" ? "approved" : "denied" } });
  }

  async health(_i: { agentId?: string }): Promise<AaiResult<HealthResult>> {
    const bot = _i.agentId ?? this.cfg.botIds[0];
    const r = await this.http("GET", `/threads?botId=${encodeURIComponent(bot)}`);
    if (!r.ok) return r;
    return ok({ status: "healthy", lane: "local" });
  }
}
