import { randomUUID } from 'node:crypto';
import { AsyncLocalStorage } from 'node:async_hooks';
import { BaseAaiProvider, fail, ok, type AaiResult, type AgentSummary, type OpenContextInput, type MessageInput, type EventsInput, type CursoredEvent, type ApprovalsInput, type Approval } from '@allternit/agent-gateway';
import { agentCapabilityManifestSchema } from '@allternit/subscription-fabric-contracts';

export const ADAPTER_ID = 'openmausbot';
export const CAPABILITIES = agentCapabilityManifestSchema.parse({
  vendor: ADAPTER_ID, adapterId: ADAPTER_ID, lane: 'local', guarantee: 'best_effort',
  context: { supported: true, resume: true, parallel: false, maxParallel: 1, isolation: 'shared' },
  messaging: { send: true, stream: true, steer: false, interrupt: false, cancel: false },
  memory: { read: false, write: false, snapshot: false, opaque: true },
  tools: { tools: false, mcp: false, plugins: false, connectors: false },
  tasks: { list: false, schedule: false, cancel: false, background: false },
  approvals: { read: true, respond: true, exact: false },
  computer: { view: false, control: false, takeover: false },
  artifacts: { read: false, write: false, export: false },
  events: { native: false, polling: true, transcriptDerived: true, replay: true },
  runtime: { alwaysOn: false, localRequired: true, cloud: false },
});

type Json = Record<string, unknown>;
type Scope = { binding?: { id?: string; accountBindingId?: string }; credential?: { apiKey: string } };
type Context = { id: string; owner: string; bot: string; thread: string; seen: Map<string, string>; completed: Set<string>; requests: Map<string, string>; approvals: Map<string, string>; events: CursoredEvent[]; sends: Map<string, Promise<AaiResult<{ messageId: string; correlationId: string; reply: string; guarantee: 'best_effort' }>>>; closed: boolean };
const object = (value: unknown): value is Json => !!value && typeof value === 'object' && !Array.isArray(value);
const validId = (value: unknown): value is string => typeof value === 'string' && /^[\w-]+$/.test(value);

/** Uses the paired user's bearer, never a browser cookie, model key or local auth bypass. */
export class OpenMausBotProvider extends BaseAaiProvider {
  readonly adapterId = ADAPTER_ID;
  private readonly contexts = new Map<string, Context>();
  private readonly base: string;
  private readonly request: typeof fetch;
  private readonly configuredToken?: string;
  private readonly pollMs: number;
  private readonly replyTimeoutMs: number;
  private readonly running = new Set<string>();
  private readonly sendTexts = new Map<string, string>();
  constructor(options: { baseUrl?: string; token?: string; fetch?: typeof fetch; pollMs?: number; replyTimeoutMs?: number } = {}) {
    super();
    const url = new URL(options.baseUrl ?? 'http://127.0.0.1:8799');
    if (!['127.0.0.1', '[::1]', 'localhost'].includes(url.hostname) || !['http:', 'https:'].includes(url.protocol) || url.username || url.password || url.search || url.hash || url.pathname !== '/') throw new Error('OpenMausBot endpoint must be a loopback origin');
    this.base = url.origin; this.request = options.fetch ?? fetch; this.configuredToken = options.token;
    this.pollMs = options.pollMs ?? 500; this.replyTimeoutMs = options.replyTimeoutMs ?? 120_000;
  }
  private scope(): Scope | undefined {
    return (globalThis as unknown as Record<symbol, AsyncLocalStorage<Scope> | undefined>)[Symbol.for('allternit.aai.callScope')]?.getStore();
  }
  private owner() { return this.scope()?.binding?.accountBindingId ?? this.scope()?.binding?.id ?? 'local'; }
  private async http(path: string, body?: Json): Promise<AaiResult<Json>> {
    const token = this.scope()?.credential?.apiKey ?? this.configuredToken;
    if (!token) return fail('AUTH_REQUIRED', 'Pair Allternit with your OpenMausBot instance and supply its session token. A model API key is not an OpenMausBot session.');
    try {
      const response = await this.request(this.base + path, { method: body ? 'POST' : 'GET', redirect: 'error', signal: AbortSignal.timeout(15_000), headers: { authorization: `Bearer ${token}`, accept: 'application/json', ...(body ? { 'content-type': 'application/json' } : {}) }, ...(body ? { body: JSON.stringify(body) } : {}) });
      if (!response.ok) {
        void response.body?.cancel();
        if (response.status === 401 || response.status === 403) return fail('AUTH_REVOKED', 'OpenMausBot refused this paired session or its permissions. Pair again with the required bot access.');
        if (response.status === 409) return fail('CONTEXT_BUSY', 'OpenMausBot refused the send because the conversation or approval settings changed. Refresh and review before retrying.');
        if (response.status === 429) return fail('RATE_LIMITED', 'OpenMausBot is rate limiting this connection.');
        return fail('VENDOR_UNAVAILABLE', `OpenMausBot returned HTTP ${response.status}. Verify its version and supported routes.`, { retryable: body === undefined });
      }
      const value: unknown = await response.json();
      return object(value) ? ok(value) : fail('VENDOR_UNAVAILABLE', 'OpenMausBot returned an invalid response.', { retryable: false });
    } catch {
      // Mutations can have been accepted before the connection failed. Never retry them automatically.
      return fail('VENDOR_UNAVAILABLE', body ? 'The OpenMausBot send outcome is uncertain. Check its conversation before retrying with the same send ID.' : 'OpenMausBot is unreachable or returned an invalid response.', { retryable: body === undefined });
    }
  }
  private async bots(): Promise<AaiResult<Json[]>> {
    const result = await this.http('/api/bots?messages=0');
    if (!result.ok) return result;
    if (!Array.isArray(result.value.bots) || !result.value.bots.every((bot) => object(bot) && validId(bot.id) && validId(bot.threadId) && typeof bot.name === 'string')) return fail('VENDOR_UNAVAILABLE', 'OpenMausBot returned an invalid bot roster.', { retryable: false });
    return ok(result.value.bots as Json[]);
  }
  private summary(bot: Json): AgentSummary { return { agentId: `openmausbot:${bot.id}`, displayName: String(bot.name), vendor: ADAPTER_ID, state: bot.busy ? 'busy' : 'ready' }; }
  async list() { const bots = await this.bots(); return bots.ok ? ok(bots.value.map((bot) => this.summary(bot))) : bots; }
  async get(agentId: string) {
    const bots = await this.bots(); if (!bots.ok) return bots;
    const bot = bots.value.find((row) => `openmausbot:${row.id}` === agentId);
    return bot ? ok({ ...this.summary(bot), remoteIds: { botId: String(bot.id), threadId: String(bot.threadId) }, capabilities: CAPABILITIES }) : fail('CONTEXT_NOT_FOUND', 'No OpenMausBot agent with this identity is visible to your account.');
  }
  async capabilities(agentId: string) { const bot = await this.get(agentId); return bot.ok ? ok(CAPABILITIES) : bot; }
  async identity(agentId: string) { const bot = await this.get(agentId); return bot.ok ? ok({ agentId, displayName: bot.value.displayName, vendor: ADAPTER_ID }) : bot; }
  private async transcript(thread: string) {
    const result = await this.http(`/api/threads/${encodeURIComponent(thread)}/messages?limit=500`);
    if (!result.ok) return result;
    if (!Array.isArray(result.value.messages) || !result.value.messages.every((message) => object(message) && validId(message.id)) || !(result.value.activeLeafId === null || validId(result.value.activeLeafId))) return fail('VENDOR_UNAVAILABLE', 'OpenMausBot returned an invalid conversation.', { retryable: false });
    return ok({ messages: result.value.messages as Json[], activeLeafId: result.value.activeLeafId });
  }
  private live(id: string): AaiResult<Context> {
    const context = this.contexts.get(id);
    return context && !context.closed && context.owner === this.owner() ? ok(context) : fail('CONTEXT_NOT_FOUND', 'This OpenMausBot context is not owned by the current account.');
  }
  async contextOpen(input: OpenContextInput) {
    const bot = await this.get(input.agentId); if (!bot.ok) return bot;
    if (input.adoptContextId) {
      const context = this.live(input.adoptContextId);
      return context.ok && context.value.bot === bot.value.remoteIds.botId ? ok({ contextId: context.value.id, isolation: 'shared' as const, guarantee: 'best_effort' as const, resumed: true }) : fail('CONTEXT_NOT_FOUND', 'Cannot adopt another bot or account conversation.');
    }
    const thread = bot.value.remoteIds.threadId;
    const transcript = await this.transcript(thread); if (!transcript.ok) return transcript;
    const id = randomUUID();
    this.contexts.set(id, { id, owner: this.owner(), bot: bot.value.remoteIds.botId, thread, seen: new Map(transcript.value.messages.map((message) => [String(message.id), typeof message.text === 'string' ? message.text : ''])), completed: new Set(transcript.value.messages.map((message) => String(message.id))), requests: new Map(), approvals: new Map(), events: [], sends: new Map(), closed: false });
    return ok({ contextId: id, isolation: 'shared' as const, guarantee: 'best_effort' as const, resumed: false });
  }
  async contextMessage(input: MessageInput) {
    const context = this.live(input.contextId); if (!context.ok) return context;
    if (!validId(input.correlationId) || !input.text.trim()) return fail('LANE_BLOCKED', 'A nonempty message and an idempotent send ID containing letters, digits, underscores or hyphens are required.');
    const sendKey = `${context.value.id}:${input.correlationId}`;
    const prior = context.value.sends.get(input.correlationId);
    if (prior) return this.sendTexts.get(sendKey) === input.text ? prior : fail('POLICY_DENIED', 'This send ID already belongs to a different message.');
    const key = `${context.value.owner}:${context.value.bot}`;
    if (this.running.has(key)) return fail('CONTEXT_BUSY', 'This OpenMausBot agent already has an Allternit turn in progress.');
    this.running.add(key);
    const send = this.send(context.value, input).finally(() => this.running.delete(key));
    // Retain failures too: a transport failure might have accepted the mutation upstream.
    this.sendTexts.set(sendKey, input.text); context.value.sends.set(input.correlationId, send);
    return send;
  }
  private async send(context: Context, input: MessageInput) {
    const transcript = await this.transcript(context.thread); if (!transcript.ok) return transcript;
    const receipt = await this.http(`/api/bots/${encodeURIComponent(context.bot)}/messages/guarded`, { threadId: context.thread, sendId: input.correlationId, text: input.text, expectedActiveLeafId: transcript.value.activeLeafId, expectedApprovalMode: 'ask' });
    if (!receipt.ok) return receipt;
    if (receipt.value.ok !== true || receipt.value.threadId !== context.thread || !object(receipt.value.message) || !validId(receipt.value.message.id)) return fail('VENDOR_UNAVAILABLE', 'OpenMausBot did not return a matching accepted-message receipt. Review its conversation.', { retryable: false });
    const messageId = receipt.value.message.id;
    context.requests.set(messageId, input.correlationId);
    const deadline = Date.now() + this.replyTimeoutMs;
    let awaitingApproval = false;
    while (Date.now() < deadline && !context.closed) {
      const history = await this.transcript(context.thread); if (!history.ok) return history;
      const bots = await this.bots(); if (!bots.ok) return bots;
      const bot = bots.value.find((row) => row.id === context.bot);
      if (!bot) return fail('CONTEXT_NOT_FOUND', 'The bot is no longer visible to this paired account.');
      const start = history.value.messages.findIndex((message) => message.id === messageId);
      if (start < 0) return fail('ADAPTER_DRIFT', 'The accepted OpenMausBot message is absent from the conversation. Reconnect before continuing.');
      const later = history.value.messages.slice(start + 1);
      awaitingApproval = later.some((message) => message.requestMessageId === messageId && object(message.card) && message.card.requestType === 'permission' && !message.card.answered && !message.card.dismissed && !message.card.expired);
      if (history.value.messages[start].requestCancelled === true) return fail('POLICY_DENIED', 'The OpenMausBot request was stopped.');
      const replies = later.filter((message) => message.requestMessageId === messageId && message.role === 'bot' && message.kind === 'text' && typeof message.text === 'string');
      if (history.value.messages[start].requestPending === false && replies.some((message) => message.turnTerminal === true && message.turnSucceeded === true)) return ok({ messageId, correlationId: input.correlationId, reply: replies.map((message) => message.text).join('\n\n'), guarantee: 'best_effort' as const });
      await new Promise((resolve) => setTimeout(resolve, this.pollMs));
    }
    return fail(awaitingApproval ? 'APPROVAL_REQUIRED' : 'VENDOR_UNAVAILABLE', context.closed ? 'Allternit detached while OpenMausBot was running. Its work was not stopped.' : awaitingApproval ? 'OpenMausBot is waiting for your approval. Allternit has not approved it.' : 'OpenMausBot accepted the message but has not finished. Check its conversation before sending more work.', { retryable: false });
  }
  async events(input: EventsInput) {
    const context = this.live(input.contextId); if (!context.ok) return context;
    const after = Number(input.cursor ?? 0);
    if (!Number.isSafeInteger(after) || after < 0 || after > context.value.events.length) return fail('CONTEXT_NOT_FOUND', 'Invalid OpenMausBot event cursor.');
    const transcript = await this.transcript(context.value.thread); if (!transcript.ok) return transcript;
    const bots = await this.bots(); if (!bots.ok) return bots;
    const bot = bots.value.find((row) => row.id === context.value.bot);
    if (!bot) return fail('CONTEXT_NOT_FOUND', 'This bot is no longer visible to your account.');
    for (const message of transcript.value.messages) {
      const approval = this.approval(context.value, message);
      if (approval?.remoteRef && context.value.approvals.get(approval.remoteRef) !== approval.state) {
        context.value.approvals.set(approval.remoteRef, approval.state);
        const sequence = context.value.events.length + 1;
        if (approval.state === 'pending' || approval.state === 'approved' || approval.state === 'denied') context.value.events.push({ cursor: String(sequence), event: { type: approval.state === 'pending' ? 'agent.approval.requested' : 'agent.approval.resolved', botId: `openmausbot:${context.value.bot}`, threadId: context.value.id, generationId: '1', source: 'vendor', vendor: ADAPTER_ID, adapter: ADAPTER_ID, lane: 'local', remoteContextId: context.value.thread, remoteEventId: `approval:${approval.remoteRef}:${approval.state}`, correlationId: context.value.requests.get(String(message.requestMessageId))!, causationId: String(message.requestMessageId), guarantee: 'best_effort', at: new Date().toISOString(), payload: { approvalId: approval.remoteRef, action: approval.action, ...(approval.state === 'pending' ? {} : { decision: approval.state === 'denied' ? 'deny' : 'approve' }) } } });
      }
      if (message.role !== 'bot' || message.kind !== 'text' || typeof message.text !== 'string' || typeof message.requestMessageId !== 'string' || !context.value.requests.has(message.requestMessageId)) continue;
      const previous = context.value.seen.get(String(message.id)) ?? '';
      const push = (type: 'agent.message.delta' | 'agent.message.completed', payload: Json) => {
        const sequence = context.value.events.length + 1;
        context.value.events.push({ cursor: String(sequence), event: { type, botId: `openmausbot:${context.value.bot}`, threadId: context.value.id, generationId: '1', source: 'vendor', vendor: ADAPTER_ID, adapter: ADAPTER_ID, lane: 'local', remoteContextId: context.value.thread, remoteEventId: `${message.id}:${sequence}`, correlationId: context.value.requests.get(String(message.requestMessageId))!, causationId: String(message.requestMessageId), guarantee: 'best_effort', at: new Date().toISOString(), payload } });
      };
      if (message.text !== previous) {
        if (!message.text.startsWith(previous)) return fail('ADAPTER_DRIFT', 'The vendor rewrote an already streamed message. Review the native conversation.');
        const chunk = message.text.slice(previous.length);
        context.value.seen.set(String(message.id), message.text);
        push('agent.message.delta', { chunk, messageId: message.id });
      }
      if (message.turnTerminal === true && message.turnSucceeded === true && !context.value.completed.has(String(message.id))) {
        context.value.completed.add(String(message.id));
        push('agent.message.completed', { reply: message.text, messageId: message.id });
      }
    }
    const events = context.value.events.slice(after, after + Math.max(1, Math.min(input.limit ?? 100, 500)));
    return ok({ events, nextCursor: events.at(-1)?.cursor ?? String(after) });
  }
  private approval(context: Context, message: Json): Approval | null {
    if (typeof message.requestMessageId !== 'string' || !context.requests.has(message.requestMessageId) || !object(message.card) || message.card.requestType !== 'permission' || !validId(message.card.requestId)) return null;
    const card = message.card;
    const state = card.expired ? 'expired' : card.dismissed ? 'cancelled' : card.answered === 'deny' ? 'denied' : card.answered ? 'approved' : 'pending';
    return { authority: 'vendor', actor: ADAPTER_ID, action: typeof message.text === 'string' ? message.text : String(card.tool ?? 'Review the vendor request'), threadId: context.id, remoteRef: String(card.requestId), state };
  }
  async approvals(input: ApprovalsInput) {
    if (!input.contextId) return fail('CONTEXT_NOT_FOUND', 'Choose an OpenMausBot conversation before reviewing its approvals.');
    const context = this.live(input.contextId); if (!context.ok) return context;
    const history = await this.transcript(context.value.thread); if (!history.ok) return history;
    const approvals = history.value.messages.map((message) => this.approval(context.value, message)).filter((approval): approval is Approval => approval !== null);
    if (input.op === 'list') return ok({ approvals });
    if (input.actor?.type !== 'human' || !input.actor.id || !['approve', 'deny'].includes(input.decision)) return fail('POLICY_DENIED', 'Only an explicit human decision can answer an OpenMausBot approval.');
    const approval = approvals.find((row) => row.remoteRef === input.approvalId && row.state === 'pending');
    if (!approval) return fail('POLICY_DENIED', 'This approval is not pending in this account and conversation.');
    const result = await this.http(`/api/threads/${encodeURIComponent(context.value.thread)}/respond`, { requestId: input.approvalId, behavior: input.decision === 'approve' ? 'allow' : 'deny' });
    if (!result.ok) return result;
    if (result.value.ok !== true) return fail('VENDOR_UNAVAILABLE', 'OpenMausBot did not confirm this approval response. Review its request before retrying.', { retryable: false });
    return ok({ resolved: { ...approval, state: input.decision === 'approve' ? 'approved' as const : 'denied' as const } });
  }
  async contextClose(input: { contextId: string }) {
    const context = this.live(input.contextId); if (!context.ok) return context;
    context.value.closed = true;
    // Detach only. The vendor conversation is shared and must not be deleted or stopped.
    return ok({ closed: true });
  }
  async health() { const bots = await this.bots(); return ok({ status: bots.ok ? 'healthy' as const : 'down' as const, lane: 'local', detail: bots.ok ? 'Authenticated OpenMausBot bot discovery succeeded' : bots.error.humanMessage }); }
}
