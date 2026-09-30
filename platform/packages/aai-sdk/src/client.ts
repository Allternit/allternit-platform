import type {
  AuthType,
  BotExecutionBinding,
  ChannelConversationBinding,
  ConnectionState,
  PackGap,
  PackParity,
  ProviderAccountBinding,
  RemoteThreadBinding,
} from "@allternit/subscription-fabric-contracts";
import { HumanIntentRequiredError, toHttpError } from "./errors";

export type FetchLike = (input: string, init?: { method?: string; headers?: Record<string, string>; body?: string; signal?: AbortSignal }) => Promise<{
  ok: boolean;
  status: number;
  text(): Promise<string>;
}>;

export interface AllternitAgentsOptions {
  baseUrl: string;
  token?: string;
  /** Path prefix for the API (default "/api/v1"). */
  apiPrefix?: string;
  fetch?: FetchLike;
  /** Extra headers on every request. */
  headers?: Record<string, string>;
}

export interface CreateAccountInput {
  vendor: string;
  authType: AuthType;
  externalAccountId?: string;
  displayName?: string;
  workspace?: string;
  secretRef?: string;
  sessionRef?: string;
  scopes?: string[];
  restrictedBotId?: string;
  expiresAt?: string;
}
export interface PutExecutionInput {
  type?: string;
  mode?: string;
  vendor?: string;
  adapterId?: string;
  accountBindingId?: string;
  preferredLane?: string;
  externalAgentId?: string;
  capabilities?: unknown;
  health?: unknown;
}
export interface ChannelBindingInput {
  provider: string;
  accountBindingId?: string;
  externalWorkspaceId?: string;
  externalChannelId?: string;
  externalConversationId: string;
  externalThreadId?: string;
  canonicalUrl?: string;
  bidirectional?: boolean;
  readOnly?: boolean;
  postingIdentityId?: string;
}
export interface GapInput {
  capability: string;
  surface: "transcript" | "composer" | "activity" | "card" | "computer" | "approval";
  severity?: string;
  fallbackUsed?: boolean;
  sampleRef?: string;
}
export interface ThreadEvent {
  id: string;
  sequence: number;
  type: string;
  actor: { type: string; id: string };
  payload: unknown;
  sessionId: string | null;
  occurredAt: string;
}
export interface GatewayApproval {
  id?: string;
  approvalId?: string;
  state: string;
  authority?: string;
  [k: string]: unknown;
}
export interface EventStreamOptions {
  after?: number;
  /** Stop after this many idle polls (default: run until aborted). */
  maxIdlePolls?: number;
  pollMs?: number;
  maxBackoffMs?: number;
  limit?: number;
  signal?: AbortSignal;
  sleep?: (ms: number) => Promise<void>;
}

const enc = encodeURIComponent;
const defaultSleep = (ms: number) => new Promise<void>((r) => setTimeout(r, ms));

export class AllternitAgents {
  private readonly base: string;
  private readonly f: FetchLike;
  constructor(private readonly opts: AllternitAgentsOptions) {
    this.base = opts.baseUrl.replace(/\/+$/, "") + (opts.apiPrefix ?? "/api/v1");
    this.f = opts.fetch ?? ((globalThis as any).fetch as FetchLike);
  }

  async request<T = any>(method: string, path: string, body?: unknown, query?: Record<string, string | number | undefined>, signal?: AbortSignal): Promise<T> {
    const qs = Object.entries(query ?? {}).filter(([, v]) => v !== undefined).map(([k, v]) => `${enc(k)}=${enc(String(v))}`).join("&");
    const headers: Record<string, string> = { accept: "application/json", ...(this.opts.headers ?? {}) };
    if (this.opts.token) headers.authorization = `Bearer ${this.opts.token}`;
    if (body !== undefined) headers["content-type"] = "application/json";
    const res = await this.f(`${this.base}${path}${qs ? `?${qs}` : ""}`, { method, headers, body: body === undefined ? undefined : JSON.stringify(body), signal });
    const text = await res.text();
    let parsed: any = undefined;
    if (text) { try { parsed = JSON.parse(text); } catch { parsed = { error: text }; } }
    if (!res.ok) throw toHttpError(res.status, parsed);
    return parsed as T;
  }

  async *streamEvents(threadId: string, o: EventStreamOptions = {}): AsyncGenerator<ThreadEvent> {
    const sleep = o.sleep ?? defaultSleep;
    const pollMs = o.pollMs ?? 1000;
    const maxBackoff = o.maxBackoffMs ?? 15000;
    let after = o.after ?? 0;
    let idle = 0;
    let failures = 0;
    while (!o.signal?.aborted) {
      let page: ThreadEvent[];
      try {
        const r = await this.request<any>("GET", `/threads/${enc(threadId)}/events`, undefined, { after, limit: o.limit }, o.signal);
        page = Array.isArray(r) ? r : (r.events ?? []);
        failures = 0;
      } catch (e: any) {
        const st = e?.status;
        if (st !== undefined && st < 500 && st !== 429) throw e;
        if (++failures > 8) throw e;
        await sleep(Math.min(maxBackoff, pollMs * 2 ** failures));
        continue;
      }
      if (page.length) {
        idle = 0;
        for (const ev of page) { if (ev.sequence > after) after = ev.sequence; yield ev; }
        continue;
      }
      if (o.maxIdlePolls !== undefined && ++idle >= o.maxIdlePolls) return;
      await sleep(Math.min(maxBackoff, pollMs * 2 ** Math.min(idle, 6)));
    }
  },

  readonly accounts = {
    create: (i: CreateAccountInput) => this.request<{ account: ProviderAccountBinding }>("POST", "/gateway/provider-accounts", snake(i)),
    list: (q?: { vendor?: string; state?: string }) => this.request<{ accounts: ProviderAccountBinding[] }>("GET", "/gateway/provider-accounts", undefined, q),
    get: (id: string) => this.request<{ account: ProviderAccountBinding }>("GET", `/gateway/provider-accounts/${enc(id)}`),
    /** Move the connection state machine (PATCH state, with optional audit reason). */
    setConnectionState: (id: string, state: ConnectionState, reason?: string) =>
      this.request<{ account: ProviderAccountBinding }>("PATCH", `/gateway/provider-accounts/${enc(id)}`, { state, reason }),
    update: (id: string, patch: { displayName?: string; workspace?: string; externalAccountId?: string; expiresAt?: string; verifiedAt?: string; reason?: string }) =>
      this.request<{ account: ProviderAccountBinding }>("PATCH", `/gateway/provider-accounts/${enc(id)}`, snake(patch)),
    setSecret: (id: string, apiKey: string) => this.request("POST", `/gateway/provider-accounts/${enc(id)}/secret`, { api_key: apiKey }),
    clearSecret: (id: string) => this.request("DELETE", `/gateway/provider-accounts/${enc(id)}/secret`),
    discoverAgents: (id: string) =>
      this.request<{ agents: { externalAgentId: string; name: string; description?: string; avatarUrl?: string }[] }>("GET", `/gateway/provider-accounts/${enc(id)}/agents`),
    delete: (id: string, q?: { force?: boolean }) => this.request("DELETE", `/gateway/provider-accounts/${enc(id)}`, undefined, q?.force ? { force: "true" } : undefined),
  };

  readonly bots = {
    bindExecution: (botId: string, i: PutExecutionInput) => this.request<{ binding: BotExecutionBinding }>("PUT", `/gateway/bots/${enc(botId)}/execution-binding`, snake(i)),
    getBinding: (botId: string) => this.request<{ binding: BotExecutionBinding }>("GET", `/gateway/bots/${enc(botId)}/execution-binding`),
    setBindingState: (botId: string, state: string, reason?: string) =>
      this.request<{ binding: BotExecutionBinding }>("PATCH", `/gateway/bots/${enc(botId)}/execution-binding`, { state, reason }),
    listBindings: () => this.request<{ bindings: BotExecutionBinding[] }>("GET", "/gateway/execution-bindings"),
  };

  readonly threads = {
    /**
     * Send a turn to an agent session. Throws ApprovalRequiredError (428, has approvalId),
     * ConflictError (409) or RateLimitedError (429).
     */
    sendTurn: (sessionId: string, text: string, metadata?: Record<string, unknown>) =>
      this.request<any>("POST", `/agent-sessions/${enc(sessionId)}/messages`, { text, metadata }),
    remoteBindings: (threadId: string) => this.request<{ bindings: RemoteThreadBinding[] }>("GET", `/gateway/threads/${enc(threadId)}/remote-bindings`),
    /** Pull remote events into the ledger. */
    sync: (threadId: string) => this.request<{ events: number }>("POST", `/threads/${enc(threadId)}/gateway/sync`),
    /** One page of events, ascending after a cursor (`sequence`). */
    events: (threadId: string, q?: { after?: number; limit?: number }) => this.request<any>("GET", `/threads/${enc(threadId)}/events`, undefined, q),
    /** Async iterator over events with an `after` cursor; polls with exponential backoff when idle. */
    streamEvents: (threadId: string, o?: EventStreamOptions) => this.streamEvents(threadId, o),
  };

  readonly approvals = {
    list: (threadId: string, q?: { state?: string }) => this.request<{ approvals: GatewayApproval[] }>("GET", `/threads/${enc(threadId)}/approvals`, undefined, q),
    /** Answer an approval. Requires `humanIntent: true`, set only by UI code acting on a person's click. */
    respond: (approvalId: string, decision: "approve" | "deny", o: { humanIntent: boolean }) => {
      if (o?.humanIntent !== true) throw new HumanIntentRequiredError();
      return this.request<{ approvalId: string; state: string }>("POST", `/gateway/approvals/${enc(approvalId)}/respond`, { decision, actor: { type: "user" } });
    },
  };

  readonly vendorPacks = {
    recordGap: (vendor: string, g: GapInput) => this.request<{ gap: PackGap }>("POST", `/gateway/vendor-packs/${enc(vendor)}/gaps`, snake(g)),
    gaps: (vendor: string, q?: { status?: string }) => this.request<{ gaps: PackGap[] }>("GET", `/gateway/vendor-packs/${enc(vendor)}/gaps`, undefined, q),
    updateGap: (id: string, patch: { status?: string; severity?: string }) => this.request<{ gap: PackGap }>("PATCH", `/gateway/vendor-pack-gaps/${enc(id)}`, patch),
    parity: (vendor: string) => this.request<{ vendor: string; parity: PackParity; openGaps: number; blockingGaps: number }>("GET", `/gateway/vendor-packs/${enc(vendor)}/parity`),
  };

  readonly channels = {
    bind: (threadId: string, i: ChannelBindingInput) => this.request<{ binding: ChannelConversationBinding }>("POST", `/gateway/threads/${enc(threadId)}/channel-bindings`, snake(i)),
    list: (threadId: string) => this.request<{ bindings: ChannelConversationBinding[] }>("GET", `/gateway/threads/${enc(threadId)}/channel-bindings`),
    update: (id: string, patch: Record<string, unknown>) => this.request<{ binding: ChannelConversationBinding }>("PATCH", `/gateway/channel-bindings/${enc(id)}`, snake(patch)),
  };
}

function snake(o: Record<string, any>): Record<string, any> {
  const out: Record<string, any> = {};
  for (const [k, v] of Object.entries(o)) if (v !== undefined) out[k.replace(/[A-Z]/g, (c) => "_" + c.toLowerCase())] = v;
  return out;
}
