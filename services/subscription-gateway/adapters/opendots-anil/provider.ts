import { AsyncLocalStorage } from 'node:async_hooks';
import { randomUUID } from 'node:crypto';
import { BaseAaiProvider, fail, ok, type AaiResult, type OpenContextInput, type MessageInput, type EventsInput, type EventsResult } from '@allternit/agent-gateway';
import { agentCapabilityManifestSchema } from '@allternit/subscription-fabric-contracts';
import { isObject, type Json } from '../_shared/paired-local.js';

// Open Dots (Anil-matcha/open-dots) local Claude lane. Each send is one
// /api/v1/local/tasks run: Open Dots gives every local run a fresh session, so
// nothing carries over between sends (no resume).
const AGENT_ID = 'opendots-anil:claude-local';
const FINISHED = new Set(['succeeded', 'failed']);

export const CAPABILITIES = agentCapabilityManifestSchema.parse({
  vendor: 'opendots-anil', adapterId: 'opendots-anil', lane: 'local', guarantee: 'best_effort',
  context: { supported: true, resume: false, parallel: false, maxParallel: 1, isolation: 'isolated' },
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
type Context = { id: string; owner: string; closed: boolean; busy: boolean; sends: Map<string, Promise<Reply>>; sendTexts: Map<string, string>; replies: { taskId: string; correlationId: string; reply: string }[] };
type Scope = { binding?: { id?: string; accountBindingId?: string }; credential?: { apiKey: string } };

const scope = (): Scope | undefined => (globalThis as unknown as Record<symbol, AsyncLocalStorage<Scope> | undefined>)[Symbol.for('allternit.aai.callScope')]?.getStore();
const isId = (value: unknown): value is string => typeof value === 'string' && /^[\w-]{1,200}$/.test(value);

/** Loopback only, no redirects, no credential: Open Dots' local routes have no auth of their own. */
function loopbackBase(raw: string | undefined): AaiResult<string> {
  try {
    const url = new URL(raw ?? 'http://127.0.0.1:8000');
    if (!['127.0.0.1', '[::1]', 'localhost'].includes(url.hostname) || !['http:', 'https:'].includes(url.protocol) || url.username || url.password || url.search || url.hash || url.pathname !== '/') {
      return fail('POLICY_DENIED', 'Open Dots endpoint must be a loopback origin such as http://127.0.0.1:8000.');
    }
    return ok(url.origin);
  } catch {
    return fail('POLICY_DENIED', 'Open Dots endpoint is not a valid URL.');
  }
}

export class OpenDotsAnilProvider extends BaseAaiProvider {
  readonly adapterId = 'opendots-anil';
  private contexts = new Map<string, Context>();
  private pollMs: number;
  private timeoutMs: number;
  private fetchImpl: typeof fetch;

  constructor(private options: { baseUrl?: string; userId?: string; fetch?: typeof fetch; pollMs?: number; replyTimeoutMs?: number } = {}) {
    super();
    this.pollMs = options.pollMs ?? 1000;
    this.timeoutMs = options.replyTimeoutMs ?? 330_000;
    this.fetchImpl = options.fetch ?? fetch;
  }

  /** The Open Dots user_id this connection acts as: the sealed account credential, else the deployment setting. */
  private userId(): AaiResult<string> {
    const id = (scope()?.credential?.apiKey ?? this.options.userId ?? '').trim();
    return id ? ok(id) : fail('AUTH_REQUIRED', 'Enter your Open Dots user_id (the id Open Dots uses for you) to connect.');
  }

  private async request(path: string, body?: Json): Promise<AaiResult<Json>> {
    const base = loopbackBase(this.options.baseUrl);
    if (!base.ok) return base;
    try {
      const res = await this.fetchImpl(base.value + path, {
        method: body ? 'POST' : 'GET', redirect: 'error', signal: AbortSignal.timeout(15_000),
        headers: { accept: 'application/json', ...(body ? { 'content-type': 'application/json' } : {}) },
        ...(body ? { body: JSON.stringify(body) } : {}),
      });
      if (!res.ok) {
        void res.body?.cancel();
        if (res.status === 403) return fail('POLICY_DENIED', 'Open Dots refused local access. Set LOCAL_EXECUTION_ENABLED=true on the Open Dots backend and connect from its own machine.', { retryable: false });
        if (res.status === 404) return fail('CONTEXT_NOT_FOUND', 'Open Dots does not have this task.', { retryable: false });
        if (res.status === 409) return fail('VENDOR_UNAVAILABLE', 'Open Dots local Claude is not ready (CLI missing or not signed in).', { retryable: false });
        if (res.status === 429) return fail('RATE_LIMITED', 'Open Dots is rate limiting this connection.', { retryable: !body });
        return fail('VENDOR_UNAVAILABLE', `Open Dots returned HTTP ${res.status}.`, { retryable: !body });
      }
      const value: unknown = await res.json();
      return isObject(value) ? ok(value) : fail('ADAPTER_DRIFT', 'Open Dots returned an invalid response.', { retryable: false });
    } catch {
      return fail('VENDOR_UNAVAILABLE', body ? 'Open Dots did not confirm this request. Check the Open Dots task list before sending again.' : 'Open Dots is unreachable on this machine.', { retryable: !body });
    }
  }

  /** Local status: is local execution enabled, and is the Claude CLI ready for this host. */
  private async status(): Promise<AaiResult<{ enabled: boolean; available: boolean; reason?: string }>> {
    const r = await this.request('/api/v1/local/status');
    if (!r.ok) return r;
    if (typeof r.value.enabled !== 'boolean' || typeof r.value.available !== 'boolean') return fail('ADAPTER_DRIFT', 'Open Dots returned an unknown local status.', { retryable: false });
    return ok({ enabled: r.value.enabled, available: r.value.available, reason: typeof r.value.reason === 'string' ? r.value.reason : undefined });
  }

  async list() {
    const user = this.userId();
    if (!user.ok) return user;
    const st = await this.status();
    if (!st.ok) return st;
    const state = st.value.available ? 'ready' : 'unavailable';
    return ok([{ agentId: AGENT_ID, displayName: 'Open Dots (local Claude)', vendor: 'opendots-anil', state }]);
  }

  async get(agentId: string) {
    if (agentId !== AGENT_ID) return fail('CONTEXT_NOT_FOUND', 'This Open Dots agent is not known to this connection.');
    const roster = await this.list();
    if (!roster.ok) return roster;
    return ok({ ...roster.value[0], remoteIds: {}, capabilities: CAPABILITIES });
  }

  async capabilities(agentId: string) { const agent = await this.get(agentId); return agent.ok ? ok(CAPABILITIES) : agent; }
  async identity(agentId: string) { const agent = await this.get(agentId); return agent.ok ? ok({ agentId, displayName: agent.value.displayName, vendor: 'opendots-anil' }) : agent; }

  private owner(): string {
    const s = scope();
    return s?.binding?.accountBindingId ?? s?.binding?.id ?? 'local';
  }

  private live(id: string): AaiResult<Context> {
    const c = this.contexts.get(id);
    return c && !c.closed && c.owner === this.owner() ? ok(c) : fail('CONTEXT_NOT_FOUND', 'This Open Dots context is not owned by the current account.');
  }

  async contextOpen(input: OpenContextInput) {
    const agent = await this.get(input.agentId);
    if (!agent.ok) return agent;
    if (input.adoptContextId) {
      const c = this.live(input.adoptContextId);
      return c.ok ? ok({ contextId: c.value.id, isolation: 'isolated' as const, guarantee: 'best_effort' as const, resumed: true }) : fail('CONTEXT_NOT_FOUND', 'Cannot adopt another account or agent conversation.');
    }
    const id = randomUUID();
    this.contexts.set(id, { id, owner: this.owner(), closed: false, busy: false, sends: new Map(), sendTexts: new Map(), replies: [] });
    return ok({ contextId: id, isolation: 'isolated' as const, guarantee: 'best_effort' as const, resumed: false });
  }

  async contextMessage(input: MessageInput) {
    const c = this.live(input.contextId);
    if (!c.ok) return c;
    if (!isId(input.correlationId) || !input.text.trim()) return fail('LANE_BLOCKED', 'A nonempty message and stable send ID are required.');
    const prior = c.value.sends.get(input.correlationId);
    if (prior) return c.value.sendTexts.get(input.correlationId) === input.text ? prior : fail('POLICY_DENIED', 'This send ID already belongs to a different message.');
    if (c.value.busy) return fail('CONTEXT_BUSY', 'This Open Dots context already has a pending Allternit request.');
    c.value.busy = true;
    const promise = this.send(c.value, input).finally(() => { c.value.busy = false; });
    c.value.sendTexts.set(input.correlationId, input.text);
    c.value.sends.set(input.correlationId, promise);
    return promise;
  }

  private async send(c: Context, input: MessageInput): Promise<Reply> {
    const user = this.userId();
    if (!user.ok) return user;
    const created = await this.request('/api/v1/local/tasks', { user_id: user.value, prompt_text: input.text });
    if (!created.ok) return created;
    const taskId = created.value.id;
    if (typeof taskId !== 'number' || !Number.isSafeInteger(taskId)) return fail('ADAPTER_DRIFT', 'Open Dots did not confirm a task id.', { retryable: false });
    const deadline = Date.now() + this.timeoutMs;
    while (Date.now() < deadline) {
      const row = await this.request(`/api/v1/local/tasks/${taskId}`);
      if (!row.ok) return row;
      const status = String(row.value.status);
      if (status === 'succeeded' || status === 'failed') {
        if (status === 'failed') return fail('VENDOR_UNAVAILABLE', typeof row.value.error === 'string' && row.value.error ? `Open Dots reported a failed run: ${row.value.error}` : 'Open Dots reported a failed run.', { retryable: false });
        const reply = typeof row.value.result === 'string' ? row.value.result : '';
        const messageId = String(taskId);
        c.replies.push({ taskId: messageId, correlationId: input.correlationId, reply });
        return ok({ messageId, correlationId: input.correlationId, reply, guarantee: 'best_effort' as const });
      }
      if (status === 'waiting_approval') return fail('APPROVAL_REQUIRED', 'Open Dots is waiting for an approval in its own app. Allternit does not answer Open Dots approvals; approve it there.', { retryable: false });
      if (!['running', 'pending', 'starting', 'queued'].includes(status)) return fail('ADAPTER_DRIFT', 'Open Dots returned an unknown task state.', { retryable: false });
      await new Promise((resolve) => setTimeout(resolve, this.pollMs));
    }
    return fail('VENDOR_UNAVAILABLE', `Open Dots task ${taskId} has not finished. Check it in Open Dots before sending again.`, { retryable: false });
  }

  async events(input: EventsInput): Promise<AaiResult<EventsResult>> {
    const c = this.live(input.contextId);
    if (!c.ok) return c;
    const after = Number(input.cursor ?? 0);
    if (!Number.isSafeInteger(after) || after < 0 || after > c.value.replies.length) return fail('CONTEXT_NOT_FOUND', 'Invalid Open Dots event cursor.');
    const rows = c.value.replies.slice(after, after + Math.max(1, Math.min(input.limit ?? 100, 500)));
    return ok({
      events: rows.map((r, i) => ({
        cursor: String(after + i + 1),
        event: {
          type: 'agent.message.completed' as const, botId: AGENT_ID, threadId: c.value.id, generationId: '1', source: 'vendor' as const,
          vendor: 'opendots-anil', adapter: 'opendots-anil', lane: 'local' as const, remoteContextId: c.value.id, remoteEventId: r.taskId,
          correlationId: r.correlationId, causationId: r.taskId, guarantee: 'best_effort' as const, at: new Date().toISOString(),
          payload: { reply: r.reply, messageId: r.taskId },
        },
      })),
      nextCursor: String(after + rows.length),
    });
  }

  async contextClose(input: { contextId: string }) {
    const c = this.live(input.contextId);
    if (!c.ok) return c;
    if (c.value.busy) return fail('CONTEXT_BUSY', 'Wait for the outstanding Open Dots request before closing this context.');
    c.value.closed = true;
    return ok({ closed: true });
  }

  async health() {
    const st = await this.status();
    if (!st.ok) return ok({ status: 'down' as const, lane: 'local', detail: st.error.humanMessage });
    return ok({ status: st.value.available ? 'healthy' as const : 'down' as const, lane: 'local', detail: st.value.available ? 'Open Dots local Claude is ready' : (st.value.reason ?? 'Open Dots local Claude is not ready') });
  }
}
