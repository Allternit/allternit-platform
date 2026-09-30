// OpenClaw AAI provider (lane local, guarantee exact, mode hosted).
// Talks to the user's own OpenClaw gateway over its OpenAI-compatible HTTP API:
//   GET  /v1/models              -> agent.list (404 => only the configured agent)
//   POST /v1/chat/completions    -> messages, streamed as SSE when the server supports it (JSON fallback)
// Conversation state: chat completions are stateless per request, so each AAI context keeps its own message history
// client-side and replays it on every turn. Nothing is shared between contexts. Events are our own log of the real
// stream (one cursor per event), so replay is lossless for as long as this process holds the context.
import {
  BaseAaiProvider, fail, ok,
  type AaiResult, type AgentCapabilityManifest, type AgentDetail, type AgentIdentity, type AgentSummary, type AAIError,
  type CancelResult, type CursoredEvent, type EventsInput, type EventsResult, type HealthResult, type MessageInput, type MessageResult,
  type OpenContextInput, type OpenContextResult,
} from "@allternit/agent-gateway";
import { ADAPTER_ID, AGENT_PREFIX, buildCapabilities, DEFAULT_AGENT, DEFAULT_BASE_URL, DEFAULT_MAX_PARALLEL } from "./manifest.js";

export interface OpenClawOptions {
  baseUrl?: string;
  /** Optional gateway bearer token (local auth). Never logged. */
  token?: string;
  /** Agent/model id used when /v1/models is unavailable. Default "openclaw". */
  defaultAgent?: string;
  maxParallel?: number;
  replyTimeoutMs?: number;
  fetch?: typeof fetch;
}

type Msg = { role: "system" | "user" | "assistant"; content: string };
interface Ctx {
  id: string; agentId: string; model: string; closed: boolean;
  history: Msg[]; events: CursoredEvent[]; seq: number; n: number;
  done: Map<string, Promise<AaiResult<MessageResult>>>; lock: Promise<unknown>; abort?: AbortController; cancelled?: boolean;
}

const startHint = (url: string) =>
  `OpenClaw isn't reachable at ${url}. Start it (run "openclaw gateway"), make sure its OpenAI-compatible HTTP endpoint is enabled, then try again.`;
const stripPrefix = (agentId: string) => agentId.slice(AGENT_PREFIX.length);

export class OpenClawProvider extends BaseAaiProvider {
  readonly adapterId = ADAPTER_ID;
  private ctxs = new Map<string, Ctx>();
  private n = 0;
  private o: Required<Omit<OpenClawOptions, "token">> & { token?: string };
  private caps: AgentCapabilityManifest;

  constructor(opts: OpenClawOptions = {}) {
    super();
    this.o = {
      baseUrl: (opts.baseUrl || DEFAULT_BASE_URL).replace(/\/+$/, ""),
      token: opts.token || undefined,
      defaultAgent: opts.defaultAgent || DEFAULT_AGENT,
      maxParallel: opts.maxParallel ?? DEFAULT_MAX_PARALLEL,
      replyTimeoutMs: opts.replyTimeoutMs ?? 120_000,
      fetch: opts.fetch ?? ((...a) => fetch(...a)),
    };
    this.caps = buildCapabilities(this.o.maxParallel);
  }

  // ---------- transport ----------
  private async http(path: string, init: RequestInit & { signal?: AbortSignal } = {}): Promise<AaiResult<Response>> {
    const headers: Record<string, string> = { accept: "application/json", ...(init.headers as Record<string, string> | undefined) };
    if (this.o.token) headers.authorization = `Bearer ${this.o.token}`;
    let res: Response;
    try { res = await this.o.fetch(`${this.o.baseUrl}${path}`, { ...init, headers }); }
    catch (e) {
      if ((e as Error)?.name === "AbortError") return fail("UNKNOWN", "Request aborted", { details: { aborted: true } });
      return fail("VENDOR_UNAVAILABLE", startHint(this.o.baseUrl), { details: { cause: String((e as { cause?: { code?: string } })?.cause?.code ?? (e as Error)?.message) } });
    }
    if (res.ok) return ok(res);
    void res.body?.cancel().catch(() => undefined);
    return { ok: false, error: this.mapStatus(res) };
  }
  private mapStatus(res: Response): AAIError {
    const s = res.status;
    const base = { retryable: false } as const;
    if (s === 401 || s === 403) {
      return { code: this.o.token ? "AUTH_REVOKED" : "AUTH_REQUIRED", ...base, vendorCode: String(s),
        humanMessage: this.o.token ? "OpenClaw rejected the gateway token. Update it and reconnect." : "OpenClaw requires a gateway token. Add it in the OpenClaw connection settings." };
    }
    if (s === 429) {
      const ra = Number(res.headers.get("retry-after"));
      return { code: "RATE_LIMITED", retryable: true, retryAfterMs: Number.isFinite(ra) && ra > 0 ? Math.round(ra * 1000) : 30_000, vendorCode: "429", humanMessage: "OpenClaw (or its model provider) is rate limiting requests. Allternit will wait before retrying." };
    }
    if (s === 404) return { code: "VENDOR_UNAVAILABLE", retryable: true, vendorCode: "404", humanMessage: `${startHint(this.o.baseUrl)} (the gateway answered 404, so the chat endpoint is off or this is not OpenClaw).` };
    if (s >= 500) return { code: "VENDOR_UNAVAILABLE", retryable: true, vendorCode: String(s), humanMessage: `OpenClaw returned an error (${s}). Check its logs, then try again.` };
    return { code: "UNKNOWN", ...base, vendorCode: String(s), humanMessage: `OpenClaw returned an unexpected response (${s}).` };
  }

  // ---------- identity ----------
  private async models(): Promise<AaiResult<string[]>> {
    const r = await this.http("/v1/models");
    if (!r.ok) {
      // 404 on /v1/models only means the listing endpoint is absent; reachability errors still propagate.
      if (r.error.vendorCode === "404") return ok([this.o.defaultAgent]);
      return r;
    }
    try {
      const j = (await r.value.json()) as { data?: Array<{ id?: unknown }> };
      const ids = (j.data ?? []).map((m) => m.id).filter((x): x is string => typeof x === "string" && x !== "");
      return ok(ids.length ? ids : [this.o.defaultAgent]);
    } catch { return ok([this.o.defaultAgent]); }
  }
  private summary(model: string): AgentSummary { return { agentId: AGENT_PREFIX + model, displayName: model === "openclaw" ? "OpenClaw" : `OpenClaw (${model.replace(/^openclaw\//, "")})`, vendor: "openclaw", state: "READY" }; }
  private async resolve(agentId: string): Promise<AaiResult<AgentSummary>> {
    if (!agentId.startsWith(AGENT_PREFIX)) return fail("CONTEXT_NOT_FOUND", `No such agent ${agentId}`);
    const m = await this.models();
    if (!m.ok) return m;
    return m.value.includes(stripPrefix(agentId)) ? ok(this.summary(stripPrefix(agentId))) : fail("CONTEXT_NOT_FOUND", `No such agent ${agentId}`);
  }
  async list(): Promise<AaiResult<AgentSummary[]>> {
    const m = await this.models();
    return m.ok ? ok(m.value.map((x) => this.summary(x))) : m;
  }
  async get(agentId: string): Promise<AaiResult<AgentDetail>> {
    const s = await this.resolve(agentId);
    return s.ok ? ok({ ...s.value, remoteIds: { model: stripPrefix(agentId) }, capabilities: this.caps }) : s;
  }
  async capabilities(agentId: string): Promise<AaiResult<AgentCapabilityManifest>> {
    return agentId.startsWith(AGENT_PREFIX) ? ok(this.caps) : fail("CONTEXT_NOT_FOUND", `No such agent ${agentId}`);
  }
  async identity(agentId: string): Promise<AaiResult<AgentIdentity>> {
    if (!agentId.startsWith(AGENT_PREFIX)) return fail("CONTEXT_NOT_FOUND", `No such agent ${agentId}`);
    const s = this.summary(stripPrefix(agentId));
    return ok({ agentId, displayName: s.displayName, vendor: "openclaw", lookPack: "openclaw" });
  }
  async health(_i: { agentId?: string }): Promise<AaiResult<HealthResult>> {
    const r = await this.http("/v1/models");
    if (r.ok) { void r.value.body?.cancel().catch(() => undefined); return ok({ status: "healthy", lane: "local", detail: this.o.baseUrl }); }
    if (r.error.vendorCode === "404") return ok({ status: "degraded", lane: "local", detail: "gateway reachable, /v1/models not exposed" });
    return ok({ status: "down", lane: "local", detail: r.error.humanMessage });
  }

  // ---------- contexts ----------
  private live(id: string): AaiResult<Ctx> { const c = this.ctxs.get(id); return c && !c.closed ? ok(c) : fail("CONTEXT_NOT_FOUND", "No such conversation"); }
  private push(c: Ctx, type: "agent.context.opened" | "agent.activity.started" | "agent.message.delta" | "agent.message.completed", correlationId: string, payload: Record<string, unknown>, source: "allternit" | "vendor" = "vendor") {
    c.seq += 1;
    c.events.push({ cursor: String(c.seq), event: {
      type, botId: c.agentId, threadId: c.id, generationId: "1", source, vendor: "openclaw", adapter: ADAPTER_ID, lane: "local",
      remoteContextId: c.id, remoteEventId: `${c.id}#${c.seq}`, causationId: correlationId, correlationId, guarantee: "exact", at: new Date().toISOString(), payload } });
  }

  async contextOpen(i: OpenContextInput): Promise<AaiResult<OpenContextResult>> {
    if (i.adoptContextId) {
      const c = this.live(i.adoptContextId);
      return c.ok ? ok({ contextId: c.value.id, isolation: "isolated", guarantee: "exact", resumed: true }) : c;
    }
    const agent = await this.resolve(i.agentId); // also proves OpenClaw is reachable before a context is handed out
    if (!agent.ok) return agent;
    if ([...this.ctxs.values()].filter((c) => !c.closed).length >= this.o.maxParallel) return fail("CONTEXT_BUSY", `At most ${this.o.maxParallel} OpenClaw conversations can be open at once.`);
    const c: Ctx = { id: `oc-${++this.n}-${Math.random().toString(36).slice(2, 8)}`, agentId: i.agentId, model: stripPrefix(i.agentId), closed: false, history: [], events: [], seq: 0, n: 0, done: new Map(), lock: Promise.resolve() };
    this.ctxs.set(c.id, c);
    this.push(c, "agent.context.opened", `open:${c.id}`, { title: i.title ?? null });
    return ok({ contextId: c.id, isolation: "isolated", guarantee: "exact", resumed: false });
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
    void p.then((r) => { if (!r.ok) ctx.done.delete(i.correlationId); }); // failed sends may be retried with the same id
    return p;
  }

  private async run(ctx: Ctx, i: MessageInput): Promise<AaiResult<MessageResult>> {
    if (ctx.closed) return fail("CONTEXT_NOT_FOUND", "No such conversation");
    const abort = new AbortController();
    ctx.abort = abort; ctx.cancelled = false;
    const timer = setTimeout(() => abort.abort(), this.o.replyTimeoutMs);
    const messages: Msg[] = [...ctx.history, { role: "user", content: i.text }];
    this.push(ctx, "agent.activity.started", i.correlationId, { text: i.text }, "allternit");
    try {
      const r = await this.http("/v1/chat/completions", {
        method: "POST", signal: abort.signal, headers: { "content-type": "application/json", accept: "text/event-stream, application/json" },
        body: JSON.stringify({ model: ctx.model, messages, stream: true }),
      });
      if (!r.ok) return ctx.cancelled ? this.cancelled() : r;
      let reply = "";
      try {
        const ct = r.value.headers.get("content-type") ?? "";
        if (ct.includes("text/event-stream") && r.value.body) {
          reply = await this.readSse(r.value.body, (d) => { reply += d; this.push(ctx, "agent.message.delta", i.correlationId, { chunk: d }); });
        } else {
          const j = (await r.value.json()) as { choices?: Array<{ message?: { content?: unknown } }> };
          const t = j.choices?.[0]?.message?.content;
          reply = typeof t === "string" ? t : "";
          if (reply) this.push(ctx, "agent.message.delta", i.correlationId, { chunk: reply });
        }
      } catch (e) {
        if (ctx.cancelled) return this.cancelled();
        return fail("VENDOR_UNAVAILABLE", `OpenClaw closed the connection mid-reply. ${startHint(this.o.baseUrl)}`, { details: { cause: (e as Error)?.message } });
      }
      // History is only extended once the turn fully succeeded, so a failed/cancelled send never corrupts the context.
      ctx.history = [...messages, { role: "assistant", content: reply }];
      const messageId = `${ctx.id}:m${++ctx.n}`;
      this.push(ctx, "agent.message.completed", i.correlationId, { reply, messageId });
      return ok({ messageId, correlationId: i.correlationId, reply, guarantee: "exact" });
    } finally { clearTimeout(timer); if (ctx.abort === abort) ctx.abort = undefined; }
  }
  private cancelled(): AaiResult<never> { return fail("UNKNOWN", "The message was cancelled.", { details: { cancelled: true } }); }

  private async readSse(body: ReadableStream<Uint8Array>, onDelta: (d: string) => void): Promise<string> {
    const reader = body.getReader(), dec = new TextDecoder();
    let buf = "", out = "", finished = false;
    const line = (raw: string) => {
      const l = raw.trim();
      if (!l.startsWith("data:")) return;
      const data = l.slice(5).trim();
      if (data === "[DONE]") { finished = true; return; }
      try {
        const j = JSON.parse(data) as { choices?: Array<{ delta?: { content?: unknown } }> };
        const d = j.choices?.[0]?.delta?.content;
        if (typeof d === "string" && d) { out += d; onDelta(d); }
      } catch { /* keep-alive or non-JSON comment */ }
    };
    while (!finished) {
      const { done, value } = await reader.read();
      if (done) break;
      buf += dec.decode(value, { stream: true });
      let idx: number;
      while ((idx = buf.indexOf("\n")) >= 0) { line(buf.slice(0, idx)); buf = buf.slice(idx + 1); }
    }
    if (buf) line(buf);
    return out;
  }

  async contextCancel(i: { contextId: string }): Promise<AaiResult<CancelResult>> {
    const c = this.live(i.contextId);
    if (!c.ok) return c;
    if (c.value.abort) { c.value.cancelled = true; c.value.abort.abort(); } // aborting the HTTP request stops the stream
    return ok({ confirmed: true });
  }
  async contextClose(i: { contextId: string }): Promise<AaiResult<{ closed: boolean }>> {
    const c = this.live(i.contextId);
    if (!c.ok) return c;
    c.value.closed = true;
    c.value.cancelled = true; c.value.abort?.abort();
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
