import { AsyncLocalStorage } from 'node:async_hooks';
import { randomUUID } from 'node:crypto';
import { BaseAaiProvider, fail, ok, type AaiResult, type OpenContextInput, type MessageInput, type EventsInput, type EventsResult } from '@allternit/agent-gateway';
import { agentCapabilityManifestSchema } from '@allternit/subscription-fabric-contracts';
import { isObject, type Json } from '../_shared/paired-local.js';

// Munder Difflin (HarnessMD/munder-difflin) supported inbound task route: its
// secret-gated webhook server. POST /<endpointId> with x-md-webhook-secret turns
// a message into hive work routed to the god orchestrator (200 with a token, or
// 202 when held for operator approval). GET /<endpointId> with
// x-md-webhook-token reports that one task's status (todo | doing | blocked |
// done, plus awaiting-approval | rejected) and its result. Nothing is typed into
// any terminal. The webhook addresses the floor, not a single agent.
const AGENT_ID = 'munder-difflin:floor';

export const CAPABILITIES = agentCapabilityManifestSchema.parse({
  vendor: 'munder-difflin', adapterId: 'munder-difflin', lane: 'local', guarantee: 'best_effort',
  context: { supported: true, resume: false, parallel: false, maxParallel: 1, isolation: 'shared' },
  messaging: { send: true, stream: false, steer: false, interrupt: false, cancel: false },
  memory: { read: false, write: false, snapshot: false, opaque: true },
  tools: { tools: false, mcp: false, plugins: false, connectors: false },
  tasks: { list: false, schedule: false, cancel: false, background: false },
  approvals: { read: false, respond: false, exact: false },
  computer: { view: false, control: false, takeover: false },
  artifacts: { read: false, write: false, export: false },
  events: { native: false, polling: true, transcriptDerived: false, replay: true },
  runtime: { alwaysOn: false, localRequired: true, cloud: false },
});

type Reply = AaiResult<{ messageId: string; correlationId: string; reply: string; guarantee: 'best_effort' }>;
type Endpoint = { endpointId: string; secret: string };
type Context = { id: string; owner: string; closed: boolean; busy: boolean; sends: Map<string, Promise<Reply>>; sendTexts: Map<string, string>; replies: { messageId: string; correlationId: string; reply: string }[] };
type Scope = { binding?: { id?: string; accountBindingId?: string }; credential?: { apiKey: string } };

const scope = (): Scope | undefined => (globalThis as unknown as Record<symbol, AsyncLocalStorage<Scope> | undefined>)[Symbol.for('allternit.aai.callScope')]?.getStore();
const ID = /^[A-Za-z0-9_-]{1,64}$/;

/** Loopback only: the endpoint secret must never leave the machine that runs Munder Difflin. */
function loopbackBase(raw: string | undefined): AaiResult<string> {
  try {
    const url = new URL(raw ?? 'http://127.0.0.1:3849');
    if (!['127.0.0.1', '[::1]', 'localhost'].includes(url.hostname) || !['http:', 'https:'].includes(url.protocol) || url.username || url.password || url.search || url.hash || url.pathname !== '/') {
      return fail('POLICY_DENIED', 'Munder Difflin webhook endpoint must be a loopback origin such as http://127.0.0.1:3849.');
    }
    return ok(url.origin);
  } catch {
    return fail('POLICY_DENIED', 'Munder Difflin webhook endpoint is not a valid URL.');
  }
}

export class MunderDifflinProvider extends BaseAaiProvider {
  readonly adapterId = 'munder-difflin';
  private contexts = new Map<string, Context>();
  private pollMs: number;
  private timeoutMs: number;
  private fetchImpl: typeof fetch;

  constructor(private options: { baseUrl?: string; endpointId?: string; secret?: string; fetch?: typeof fetch; pollMs?: number; replyTimeoutMs?: number } = {}) {
    super();
    this.pollMs = options.pollMs ?? 2000;
    this.timeoutMs = options.replyTimeoutMs ?? 300_000;
    this.fetchImpl = options.fetch ?? fetch;
  }

  /** The webhook endpoint: the sealed account credential (JSON {"endpointId","secret"}), else deployment settings. */
  private endpoint(): AaiResult<Endpoint> {
    const raw = scope()?.credential?.apiKey;
    let fromCredential: Partial<Endpoint> = {};
    if (raw) {
      try { const parsed: unknown = JSON.parse(raw); if (isObject(parsed)) fromCredential = { endpointId: parsed.endpointId as string, secret: parsed.secret as string }; }
      catch { return fail('AUTH_REQUIRED', 'The Munder Difflin credential must be JSON with endpointId and secret.'); }
    }
    const endpointId = fromCredential.endpointId ?? this.options.endpointId;
    const secret = fromCredential.secret ?? this.options.secret;
    if (!endpointId || !secret) return fail('AUTH_REQUIRED', 'Connect a Munder Difflin webhook endpoint: its id and secret from Triggers settings.');
    if (!ID.test(endpointId)) return fail('AUTH_REQUIRED', 'The Munder Difflin endpoint id is not valid.');
    return ok({ endpointId, secret });
  }

  private async post(endpoint: Endpoint, body: Json): Promise<AaiResult<Json>> {
    const base = loopbackBase(this.options.baseUrl);
    if (!base.ok) return base;
    try {
      const res = await this.fetchImpl(`${base.value}/${endpoint.endpointId}`, {
        method: 'POST', redirect: 'error', signal: AbortSignal.timeout(15_000),
        headers: { 'x-md-webhook-secret': endpoint.secret, 'content-type': 'application/json', accept: 'application/json' },
        body: JSON.stringify(body),
      });
      const value: unknown = await res.json().catch(() => undefined);
      if (res.status === 200 || res.status === 202) return isObject(value) && value.ok === true && typeof value.token === 'string' ? ok(value) : fail('ADAPTER_DRIFT', 'Munder Difflin did not confirm the task.', { retryable: false });
      if (res.status === 401) return fail('AUTH_REVOKED', 'Munder Difflin refused this webhook secret or endpoint.', { retryable: false });
      if (res.status === 429) return fail('RATE_LIMITED', 'Munder Difflin is rate limiting this endpoint.', { retryable: false });
      if (res.status === 400) return fail('LANE_BLOCKED', isObject(value) && typeof value.error === 'string' ? `Munder Difflin rejected the message: ${value.error}` : 'Munder Difflin rejected the message.', { retryable: false });
      return fail('VENDOR_UNAVAILABLE', `Munder Difflin returned HTTP ${res.status}. The task may or may not have been created; check the board before sending again.`, { retryable: false });
    } catch {
      return fail('VENDOR_UNAVAILABLE', 'Munder Difflin did not confirm this message. Its outcome may be uncertain; check the board before sending again.', { retryable: false });
    }
  }

  private async poll(endpoint: Endpoint, token: string): Promise<AaiResult<Json>> {
    const base = loopbackBase(this.options.baseUrl);
    if (!base.ok) return base;
    try {
      const res = await this.fetchImpl(`${base.value}/${endpoint.endpointId}`, {
        method: 'GET', redirect: 'error', signal: AbortSignal.timeout(15_000),
        headers: { 'x-md-webhook-token': token, accept: 'application/json' },
      });
      const value: unknown = await res.json().catch(() => undefined);
      if (!res.ok) return fail(res.status === 404 ? 'CONTEXT_NOT_FOUND' : 'VENDOR_UNAVAILABLE', `Munder Difflin returned HTTP ${res.status} for this task status.`, { retryable: true });
      return isObject(value) && typeof value.status === 'string' ? ok(value) : fail('ADAPTER_DRIFT', 'Munder Difflin returned an invalid task status.', { retryable: false });
    } catch {
      return fail('VENDOR_UNAVAILABLE', 'Munder Difflin is unreachable on this machine.', { retryable: true });
    }
  }

  async list() {
    const endpoint = this.endpoint();
    if (!endpoint.ok) return endpoint;
    return ok([{ agentId: AGENT_ID, displayName: 'Munder Difflin floor', vendor: 'munder-difflin', state: 'ready' }]);
  }

  async get(agentId: string) {
    if (agentId !== AGENT_ID) return fail('CONTEXT_NOT_FOUND', 'This Munder Difflin agent is not known to this connection.');
    const roster = await this.list();
    if (!roster.ok) return roster;
    return ok({ ...roster.value[0], remoteIds: {}, capabilities: CAPABILITIES });
  }

  async capabilities(agentId: string) { const agent = await this.get(agentId); return agent.ok ? ok(CAPABILITIES) : agent; }
  async identity(agentId: string) { const agent = await this.get(agentId); return agent.ok ? ok({ agentId, displayName: agent.value.displayName, vendor: 'munder-difflin' }) : agent; }

  private owner(): string {
    const s = scope();
    return s?.binding?.accountBindingId ?? s?.binding?.id ?? 'local';
  }

  private live(id: string): AaiResult<Context> {
    const c = this.contexts.get(id);
    return c && !c.closed && c.owner === this.owner() ? ok(c) : fail('CONTEXT_NOT_FOUND', 'This Munder Difflin context is not owned by the current account.');
  }

  async contextOpen(input: OpenContextInput) {
    const agent = await this.get(input.agentId);
    if (!agent.ok) return agent;
    if (input.adoptContextId) {
      const c = this.live(input.adoptContextId);
      return c.ok ? ok({ contextId: c.value.id, isolation: 'shared' as const, guarantee: 'best_effort' as const, resumed: true }) : fail('CONTEXT_NOT_FOUND', 'Cannot adopt another account or agent conversation.');
    }
    const id = randomUUID();
    this.contexts.set(id, { id, owner: this.owner(), closed: false, busy: false, sends: new Map(), sendTexts: new Map(), replies: [] });
    return ok({ contextId: id, isolation: 'shared' as const, guarantee: 'best_effort' as const, resumed: false });
  }

  async contextMessage(input: MessageInput) {
    const c = this.live(input.contextId);
    if (!c.ok) return c;
    if (!ID.test(input.correlationId) || !input.text.trim()) return fail('LANE_BLOCKED', 'A nonempty message and stable send ID are required.');
    const prior = c.value.sends.get(input.correlationId);
    if (prior) return c.value.sendTexts.get(input.correlationId) === input.text ? prior : fail('POLICY_DENIED', 'This send ID already belongs to a different message.');
    if (c.value.busy) return fail('CONTEXT_BUSY', 'This Munder Difflin context already has a pending Allternit request.');
    c.value.busy = true;
    const promise = this.send(c.value, input).finally(() => { c.value.busy = false; });
    c.value.sendTexts.set(input.correlationId, input.text);
    c.value.sends.set(input.correlationId, promise);
    return promise;
  }

  private async send(c: Context, input: MessageInput): Promise<Reply> {
    const endpoint = this.endpoint();
    if (!endpoint.ok) return endpoint;
    const text = input.text.trim();
    const accepted = await this.post(endpoint.value, { message: text, title: text.slice(0, 80), from: 'Allternit' });
    if (!accepted.ok) return accepted;
    const token = String(accepted.value.token);
    const messageId = randomUUID();
    const deadline = Date.now() + this.timeoutMs;
    while (Date.now() < deadline) {
      const row = await this.poll(endpoint.value, token);
      if (!row.ok) return row;
      const status = String(row.value.status);
      if (status === 'done') {
        const reply = typeof row.value.result === 'string' ? row.value.result : '';
        c.replies.push({ messageId, correlationId: input.correlationId, reply });
        return ok({ messageId, correlationId: input.correlationId, reply, guarantee: 'best_effort' as const });
      }
      if (status === 'blocked') return fail('VENDOR_UNAVAILABLE', 'Munder Difflin marked this task blocked and needs attention on its board.', { retryable: false });
      if (status === 'rejected') return fail('POLICY_DENIED', 'The operator rejected this message in Munder Difflin.', { retryable: false });
      await new Promise((resolve) => setTimeout(resolve, this.pollMs));
    }
    return fail('VENDOR_UNAVAILABLE', 'Munder Difflin accepted the task but has not finished it. Check its board before sending again.', { retryable: false });
  }

  async events(input: EventsInput): Promise<AaiResult<EventsResult>> {
    const c = this.live(input.contextId);
    if (!c.ok) return c;
    const after = Number(input.cursor ?? 0);
    if (!Number.isSafeInteger(after) || after < 0 || after > c.value.replies.length) return fail('CONTEXT_NOT_FOUND', 'Invalid Munder Difflin event cursor.');
    const rows = c.value.replies.slice(after, after + Math.max(1, Math.min(input.limit ?? 100, 500)));
    return ok({
      events: rows.map((r, i) => ({
        cursor: String(after + i + 1),
        event: {
          type: 'agent.message.completed' as const, botId: AGENT_ID, threadId: c.value.id, generationId: '1', source: 'vendor' as const,
          vendor: 'munder-difflin', adapter: 'munder-difflin', lane: 'local' as const, remoteContextId: c.value.id, remoteEventId: r.messageId,
          correlationId: r.correlationId, causationId: r.messageId, guarantee: 'best_effort' as const, at: new Date().toISOString(),
          payload: { reply: r.reply, messageId: r.messageId },
        },
      })),
      nextCursor: String(after + rows.length),
    });
  }

  async contextClose(input: { contextId: string }) {
    const c = this.live(input.contextId);
    if (!c.ok) return c;
    if (c.value.busy) return fail('CONTEXT_BUSY', 'Wait for the outstanding Munder Difflin task before closing this context.');
    c.value.closed = true;
    return ok({ closed: true });
  }

  /** Any HTTP answer from the endpoint server proves it is listening; a refused connection does not. */
  async health() {
    const base = loopbackBase(this.options.baseUrl);
    if (!base.ok) return ok({ status: 'down' as const, lane: 'local', detail: base.error.humanMessage });
    try {
      const res = await this.fetchImpl(`${base.value}/munder-difflin-health-probe`, { method: 'GET', redirect: 'error', signal: AbortSignal.timeout(5_000) });
      void res.body?.cancel();
      return ok({ status: 'healthy' as const, lane: 'local', detail: 'Munder Difflin webhook server is listening' });
    } catch {
      return ok({ status: 'down' as const, lane: 'local', detail: 'Munder Difflin webhook server is not reachable on this machine' });
    }
  }
}
