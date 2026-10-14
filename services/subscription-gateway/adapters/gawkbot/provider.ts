import { AsyncLocalStorage } from 'node:async_hooks';
import { randomUUID } from 'node:crypto';
import { BaseAaiProvider, fail, ok, type AaiResult, type OpenContextInput, type MessageInput, type EventsInput, type EventsResult } from '@allternit/agent-gateway';
import { agentCapabilityManifestSchema } from '@allternit/subscription-fabric-contracts';
import { isObject, type Json } from '../_shared/paired-local.js';

// gawkbot (najmuzzaman-mohammad/gawkbot) broker HTTP API. Every route used here
// is wrapped by withAuth in internal/team/broker_auth.go and takes
// `Authorization: Bearer <broker token>`. Replies are attributed by message id:
// the bot's answer is the message from its slug that replies to our message, or
// the first message from its slug after ours.
const PREFIX = 'gawkbot:';
const SESSION_KIND = 'local-session';
const HUMAN = 'human';

export const CAPABILITIES = agentCapabilityManifestSchema.parse({
  vendor: 'gawkbot', adapterId: 'gawkbot', lane: 'local', guarantee: 'best_effort',
  context: { supported: true, resume: true, parallel: false, maxParallel: 1, isolation: 'shared' },
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
type Member = { slug: string; name: string; session: boolean };
type Context = { id: string; owner: string; slug: string; channel: string; closed: boolean; busy: boolean; sends: Map<string, Promise<Reply>>; sendTexts: Map<string, string>; replies: { messageId: string; correlationId: string; reply: string }[] };
type Scope = { binding?: { id?: string; accountBindingId?: string }; credential?: { apiKey: string } };

const scope = (): Scope | undefined => (globalThis as unknown as Record<symbol, AsyncLocalStorage<Scope> | undefined>)[Symbol.for('allternit.aai.callScope')]?.getStore();
const isId = (value: unknown): value is string => typeof value === 'string' && /^[\w-]{1,200}$/.test(value);
const isSlug = (value: string) => /^[a-z0-9][a-z0-9-]{0,99}$/.test(value);

/** Loopback only: the bearer token must never leave the Sessions computer. */
function loopbackBase(raw: string | undefined): AaiResult<string> {
  try {
    const url = new URL(raw ?? 'http://127.0.0.1:7890');
    if (!['127.0.0.1', '[::1]', 'localhost'].includes(url.hostname) || !['http:', 'https:'].includes(url.protocol) || url.username || url.password || url.search || url.hash || url.pathname !== '/') {
      return fail('POLICY_DENIED', 'gawkbot endpoint must be a loopback origin such as http://127.0.0.1:7890.');
    }
    return ok(url.origin);
  } catch {
    return fail('POLICY_DENIED', 'gawkbot endpoint is not a valid URL.');
  }
}

export class GawkbotProvider extends BaseAaiProvider {
  readonly adapterId = 'gawkbot';
  private contexts = new Map<string, Context>();
  private pollMs: number;
  private timeoutMs: number;
  private fetchImpl: typeof fetch;

  constructor(private options: { baseUrl?: string; token?: string; fetch?: typeof fetch; pollMs?: number; replyTimeoutMs?: number } = {}) {
    super();
    this.pollMs = options.pollMs ?? 1000;
    this.timeoutMs = options.replyTimeoutMs ?? 120_000;
    this.fetchImpl = options.fetch ?? fetch;
  }

  private token(): AaiResult<string> {
    const token = scope()?.credential?.apiKey ?? this.options.token;
    return token ? ok(token) : fail('AUTH_REQUIRED', 'Supply the gawkbot broker token from your Sessions computer to connect.');
  }

  /** Bearer-authenticated broker call. Returns the parsed JSON object, or a typed failure. */
  private async request(method: 'GET' | 'POST', path: string, body?: Json): Promise<AaiResult<Json>> {
    const base = loopbackBase(this.options.baseUrl);
    if (!base.ok) return base;
    const token = this.token();
    if (!token.ok) return token;
    try {
      const res = await this.fetchImpl(base.value + path, {
        method, redirect: 'error', signal: AbortSignal.timeout(15_000),
        headers: { authorization: `Bearer ${token.value}`, accept: 'application/json', ...(body ? { 'content-type': 'application/json' } : {}) },
        ...(body ? { body: JSON.stringify(body) } : {}),
      });
      if (!res.ok) {
        void res.body?.cancel();
        const retryable = method === 'GET';
        if (res.status === 401) return fail('AUTH_REVOKED', 'gawkbot refused this broker token. Enter the current token from your Sessions computer.', { retryable: false });
        if (res.status === 403) return fail('POLICY_DENIED', 'gawkbot refused this request for this connection.', { retryable: false });
        if (res.status === 404) return fail('CONTEXT_NOT_FOUND', 'gawkbot could not find this member or channel.', { retryable: false });
        if (res.status === 409) return fail('CONTEXT_BUSY', 'gawkbot has a pending request in this channel. Answer it in gawkbot first.', { retryable: false });
        if (res.status === 429) return fail('RATE_LIMITED', 'gawkbot is rate limiting this connection.', { retryable });
        return fail('VENDOR_UNAVAILABLE', `gawkbot returned HTTP ${res.status}.`, { retryable });
      }
      const value: unknown = await res.json();
      return isObject(value) ? ok(value) : fail('ADAPTER_DRIFT', 'gawkbot returned an invalid response.', { retryable: false });
    } catch {
      return fail('VENDOR_UNAVAILABLE', method === 'POST' ? 'gawkbot did not confirm this message. Its outcome may be uncertain; check the channel before sending again.' : 'gawkbot is unreachable on this machine.', { retryable: method === 'GET' });
    }
  }

  /** Office members, read with the owner's token. Session members are flagged so they can be reported per member. */
  private async members(): Promise<AaiResult<Member[]>> {
    const r = await this.request('GET', '/office-members');
    if (!r.ok) return r;
    const rows = r.value.members;
    if (!Array.isArray(rows) || !rows.every((m) => isObject(m) && typeof m.slug === 'string' && isSlug(m.slug))) {
      return fail('ADAPTER_DRIFT', 'gawkbot returned an invalid member roster.', { retryable: false });
    }
    return ok(rows.map((m) => {
      const row = m as Json & { slug: string; name?: unknown; provider?: unknown };
      const kind = isObject(row.provider) ? row.provider.kind : undefined;
      return { slug: row.slug, name: typeof row.name === 'string' && row.name ? row.name : row.slug, session: kind === SESSION_KIND };
    }));
  }

  /** Session members: gawkbot delivers to them only for messages posted with its web UI operator key. This adapter has no such key, so they are reported unavailable. */
  static sessionUnavailable(): AaiResult<never> {
    return fail('POLICY_DENIED', 'This gawkbot member is a terminal session. gawkbot only delivers to it for messages posted through its own web UI, so this connection reports it unavailable.', { retryable: false });
  }

  async list() {
    const roster = await this.members();
    if (!roster.ok) return roster;
    return ok(roster.value.map((m) => ({ agentId: PREFIX + m.slug, displayName: m.name, vendor: 'gawkbot', state: m.session ? 'unavailable' : 'ready' })));
  }

  async get(agentId: string) {
    const slug = agentId.startsWith(PREFIX) ? agentId.slice(PREFIX.length) : '';
    const roster = await this.members();
    if (!roster.ok) return roster;
    const member = roster.value.find((m) => m.slug === slug);
    if (!member) return fail('CONTEXT_NOT_FOUND', 'This gawkbot member is not visible to this connection.');
    return ok({ agentId, displayName: member.name, vendor: 'gawkbot', state: member.session ? 'unavailable' : 'ready', remoteIds: { slug: member.slug }, capabilities: CAPABILITIES });
  }

  async capabilities(agentId: string) { const agent = await this.get(agentId); return agent.ok ? ok(CAPABILITIES) : agent; }
  async identity(agentId: string) { const agent = await this.get(agentId); return agent.ok ? ok({ agentId, displayName: agent.value.displayName, vendor: 'gawkbot' }) : agent; }

  private owner(): string {
    const s = scope();
    return s?.binding?.accountBindingId ?? s?.binding?.id ?? 'local';
  }

  private live(id: string): AaiResult<Context> {
    const c = this.contexts.get(id);
    return c && !c.closed && c.owner === this.owner() ? ok(c) : fail('CONTEXT_NOT_FOUND', 'This gawkbot context is not owned by the current account.');
  }

  async contextOpen(input: OpenContextInput) {
    const agent = await this.get(input.agentId);
    if (!agent.ok) return agent;
    if (agent.value.state === 'unavailable') return GawkbotProvider.sessionUnavailable();
    const slug = agent.value.remoteIds.slug;
    if (input.adoptContextId) {
      const c = this.live(input.adoptContextId);
      return c.ok && c.value.slug === slug ? ok({ contextId: c.value.id, isolation: 'shared' as const, guarantee: 'best_effort' as const, resumed: true }) : fail('CONTEXT_NOT_FOUND', 'Cannot adopt another account or agent conversation.');
    }
    // The 1:1 DM with this member. The broker creates it on first use and returns the same one afterwards.
    const dm = await this.request('POST', '/channels/dm', { members: [HUMAN, slug], type: 'direct' });
    if (!dm.ok) return dm;
    if (!isId(dm.value.slug)) return fail('ADAPTER_DRIFT', 'gawkbot did not confirm the direct message channel.', { retryable: false });
    const id = randomUUID();
    this.contexts.set(id, { id, owner: this.owner(), slug, channel: String(dm.value.slug), closed: false, busy: false, sends: new Map(), sendTexts: new Map(), replies: [] });
    return ok({ contextId: id, isolation: 'shared' as const, guarantee: 'best_effort' as const, resumed: false });
  }

  async contextMessage(input: MessageInput) {
    const c = this.live(input.contextId);
    if (!c.ok) return c;
    if (!isId(input.correlationId) || !input.text.trim()) return fail('LANE_BLOCKED', 'A nonempty message and stable send ID are required.');
    const prior = c.value.sends.get(input.correlationId);
    if (prior) return c.value.sendTexts.get(input.correlationId) === input.text ? prior : fail('POLICY_DENIED', 'This send ID already belongs to a different message.');
    if (c.value.busy) return fail('CONTEXT_BUSY', 'This gawkbot conversation already has a pending Allternit request.');
    c.value.busy = true;
    const promise = this.send(c.value, input).finally(() => { c.value.busy = false; });
    c.value.sendTexts.set(input.correlationId, input.text);
    c.value.sends.set(input.correlationId, promise);
    return promise;
  }

  private async send(c: Context, input: MessageInput): Promise<Reply> {
    const sent = await this.request('POST', '/messages', { from: HUMAN, channel: c.channel, content: input.text, tagged: [c.slug] });
    if (!sent.ok) return sent;
    const messageId = sent.value.id;
    if (!isId(messageId)) return fail('ADAPTER_DRIFT', 'gawkbot did not confirm the message id.', { retryable: false });
    const deadline = Date.now() + this.timeoutMs;
    while (Date.now() < deadline) {
      await new Promise((resolve) => setTimeout(resolve, this.pollMs));
      const listed = await this.request('GET', `/messages?channel=${encodeURIComponent(c.channel)}&my_slug=${HUMAN}&since_id=${encodeURIComponent(messageId)}&limit=100`);
      if (!listed.ok) return listed;
      const rows = listed.value.messages;
      if (!Array.isArray(rows)) return fail('ADAPTER_DRIFT', 'gawkbot returned an invalid message list.', { retryable: false });
      const sentAt = rows.findIndex((m) => isObject(m) && m.id === messageId);
      if (sentAt === -1) continue; // our message is not in the window yet, so nothing can be attributed
      const after = rows.slice(sentAt + 1).filter(isObject);
      const reply = after.find((m) => m.reply_to === messageId && m.from === c.slug) ?? after.find((m) => m.from === c.slug);
      if (reply && typeof reply.content === 'string') {
        const id = String(reply.id ?? messageId);
        c.replies.push({ messageId: id, correlationId: input.correlationId, reply: reply.content });
        return ok({ messageId: id, correlationId: input.correlationId, reply: reply.content, guarantee: 'best_effort' as const });
      }
    }
    return fail('VENDOR_UNAVAILABLE', 'gawkbot accepted the message but no attributable reply arrived in time. Check the channel before sending again.', { retryable: false });
  }

  async events(input: EventsInput): Promise<AaiResult<EventsResult>> {
    const c = this.live(input.contextId);
    if (!c.ok) return c;
    const after = Number(input.cursor ?? 0);
    if (!Number.isSafeInteger(after) || after < 0 || after > c.value.replies.length) return fail('CONTEXT_NOT_FOUND', 'Invalid gawkbot event cursor.');
    const rows = c.value.replies.slice(after, after + Math.max(1, Math.min(input.limit ?? 100, 500)));
    return ok({
      events: rows.map((r, i) => ({
        cursor: String(after + i + 1),
        event: {
          type: 'agent.message.completed' as const, botId: PREFIX + c.value.slug, threadId: c.value.id, generationId: '1', source: 'vendor' as const,
          vendor: 'gawkbot', adapter: 'gawkbot', lane: 'local' as const, remoteContextId: c.value.channel, remoteEventId: r.messageId,
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
    if (c.value.busy) return fail('CONTEXT_BUSY', 'Wait for the outstanding gawkbot request before closing this context.');
    c.value.closed = true; // the DM stays in gawkbot; closing only detaches this connection
    return ok({ closed: true });
  }

  async health() {
    const roster = await this.members();
    if (!roster.ok) return ok({ status: 'down' as const, lane: 'local', detail: roster.error.humanMessage });
    return ok({ status: 'healthy' as const, lane: 'local', detail: 'Authenticated gawkbot broker roster' });
  }
}
