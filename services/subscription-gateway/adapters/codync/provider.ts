import { randomUUID } from 'node:crypto';
import { BaseAaiProvider, fail, ok, type AaiResult, type AgentSummary, type OpenContextInput, type MessageInput, type EventsInput, type CursoredEvent, type Approval, type ApprovalsInput } from '@allternit/agent-gateway';
import { agentCapabilityManifestSchema } from '@allternit/subscription-fabric-contracts';
import { PairedLocalTransport, isId, isObject, type Json } from '../_shared/paired-local.js';

export const CAPABILITIES = agentCapabilityManifestSchema.parse({
  vendor: 'codync', adapterId: 'codync', lane: 'local', guarantee: 'best_effort',
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
type Result = AaiResult<{ messageId: string; correlationId: string; reply: string; guarantee: 'best_effort' }>;
type Context = { id: string; owner: string; bot: string; turns: Map<number, string>; seen: Map<string, string>; completed: Set<string>; approvals: Map<string, string>; events: CursoredEvent[]; sends: Map<string, Promise<Result>>; closed: boolean };
const validEntry = (value: unknown): value is Json => isObject(value) && isId(value.id) && isId(value.botId) && Number.isSafeInteger(value.turn) && Number.isSafeInteger(value.seq) && Number.isSafeInteger(value.rev) && typeof value.kind === 'string' && isObject(value.data);

/** Native Codync host RPC. Runs beside the user's host; never impersonates its encrypted phone channel. */
export class CodyncProvider extends BaseAaiProvider {
  readonly adapterId = 'codync';
  private readonly transport: PairedLocalTransport;
  private readonly contexts = new Map<string, Context>();
  private readonly running = new Set<string>();
  private readonly pollMs: number;
  private readonly replyTimeoutMs: number;
  constructor(options: { baseUrl?: string; token?: string; fetch?: typeof fetch; pollMs?: number; replyTimeoutMs?: number } = {}) {
    super(); this.transport = new PairedLocalTransport('Codync', { ...options, baseUrl: options.baseUrl ?? 'http://127.0.0.1:19222' });
    this.pollMs = options.pollMs ?? 500; this.replyTimeoutMs = options.replyTimeoutMs ?? 120_000;
  }
  private rpc(method: string, body: Json = {}) { return this.transport.json(`/api/${method}`, body); }
  private async roster(): Promise<AaiResult<Json[]>> {
    const result = await this.transport.response('/events?since=0', undefined, true);
    if (!result.ok) return result;
    if (!result.value.headers.get('content-type')?.includes('text/event-stream') || !result.value.body) { void result.value.body?.cancel(); return fail('ADAPTER_DRIFT', 'Codync did not return its authenticated event stream.'); }
    const reader = result.value.body.getReader(); const decoder = new TextDecoder(); const bots = new Map<string, Json>();
    let buffer = ''; let target: number | undefined; let revision = 0; let bytes = 0;
    try {
      while (true) {
        const chunk = await reader.read();
        if (chunk.done) return fail('ADAPTER_DRIFT', 'Codync closed before its initial snapshot was complete.');
        bytes += chunk.value.byteLength;
        if (bytes > 16 * 1024 * 1024) return fail('ADAPTER_DRIFT', 'Codync initial snapshot exceeds the connector limit.');
        buffer += decoder.decode(chunk.value, { stream: true }); buffer = buffer.replace(/\r\n/g, '\n');
        let split: number;
        while ((split = buffer.indexOf('\n\n')) >= 0) {
          const frame = buffer.slice(0, split); buffer = buffer.slice(split + 2);
          const data = frame.split('\n').filter((line) => line.startsWith('data:')).map((line) => line.slice(5).trimStart()).join('\n');
          if (!data) continue;
          const event: unknown = JSON.parse(data);
          if (!isObject(event)) return fail('ADAPTER_DRIFT', 'Codync returned an invalid event.');
          if (event.type === 'hello') {
            if (!Number.isSafeInteger(event.rev) || Number(event.rev) < 0 || typeof event.hostId !== 'string') return fail('ADAPTER_DRIFT', 'Codync returned an invalid host handshake.');
            target = Number(event.rev);
          } else if (event.type === 'bot') {
            if (!isObject(event.bot) || !isId(event.bot.id) || typeof event.bot.name !== 'string' || !Number.isSafeInteger(event.rev)) return fail('ADAPTER_DRIFT', 'Codync returned an invalid bot snapshot.');
            bots.set(event.bot.id, event.bot); revision = Math.max(revision, Number(event.rev));
          } else if (event.type === 'entry') {
            if (!validEntry(event.entry) || !Number.isSafeInteger(event.rev)) return fail('ADAPTER_DRIFT', 'Codync returned an invalid transcript snapshot.');
            revision = Math.max(revision, Number(event.rev));
          } else if (event.type === 'resync') return fail('ADAPTER_DRIFT', 'Codync requests a fresh snapshot; reconnect.');
          if (target !== undefined && revision >= target) return ok([...bots.values()].filter((bot) => bot.deleted !== true && bot.kind !== 'group'));
        }
      }
    } catch { return fail('VENDOR_UNAVAILABLE', 'Codync initial snapshot could not be completed.'); }
    finally { await reader.cancel().catch(() => undefined); reader.releaseLock(); }
  }
  private summary(bot: Json): AgentSummary { return { agentId: `codync:${bot.id}`, displayName: String(bot.name), vendor: 'codync', state: String(bot.status ?? 'unknown') }; }
  async list() { const bots = await this.roster(); return bots.ok ? ok(bots.value.map((bot) => this.summary(bot))) : bots; }
  async get(agentId: string) {
    const bots = await this.roster(); if (!bots.ok) return bots;
    const bot = bots.value.find((row) => `codync:${row.id}` === agentId);
    return bot ? ok({ ...this.summary(bot), remoteIds: { botId: String(bot.id), permission: String(bot.permission) }, capabilities: CAPABILITIES }) : fail('CONTEXT_NOT_FOUND', 'No Codync agent with this identity is visible to the connected host.');
  }
  async capabilities(agentId: string) { const bot = await this.get(agentId); return bot.ok ? ok(CAPABILITIES) : bot; }
  async identity(agentId: string) { const bot = await this.get(agentId); return bot.ok ? ok({ agentId, displayName: bot.value.displayName, vendor: 'codync' }) : bot; }
  private async history(bot: string): Promise<AaiResult<Json[]>> {
    const result = await this.rpc('history', { botId: bot, limit: 500 }); if (!result.ok) return result;
    if (!Array.isArray(result.value.entries) || !result.value.entries.every((entry) => validEntry(entry) && entry.botId === bot)) return fail('ADAPTER_DRIFT', 'Codync returned an invalid or foreign transcript.');
    return ok((result.value.entries as Json[]).sort((a, b) => Number(a.seq) - Number(b.seq)));
  }
  private live(id: string): AaiResult<Context> {
    const context = this.contexts.get(id);
    return context && !context.closed && context.owner === this.transport.owner() ? ok(context) : fail('CONTEXT_NOT_FOUND', 'This Codync context is not owned by the current account.');
  }
  async contextOpen(input: OpenContextInput) {
    const bot = await this.get(input.agentId); if (!bot.ok) return bot;
    if (bot.value.remoteIds.permission !== 'ask') return fail('POLICY_DENIED', 'Set this Codync bot to Ask for permission before connecting it to Allternit.');
    if (input.adoptContextId) {
      const context = this.live(input.adoptContextId);
      return context.ok && context.value.bot === bot.value.remoteIds.botId ? ok({ contextId: context.value.id, isolation: 'shared' as const, guarantee: 'best_effort' as const, resumed: true }) : fail('CONTEXT_NOT_FOUND', 'Cannot adopt another bot or account conversation.');
    }
    const history = await this.history(bot.value.remoteIds.botId); if (!history.ok) return history;
    const id = randomUUID();
    this.contexts.set(id, { id, owner: this.transport.owner(), bot: bot.value.remoteIds.botId, turns: new Map(), seen: new Map(), completed: new Set(), approvals: new Map(), events: [], sends: new Map(), closed: false });
    return ok({ contextId: id, isolation: 'shared' as const, guarantee: 'best_effort' as const, resumed: false });
  }
  async contextMessage(input: MessageInput) {
    const context = this.live(input.contextId); if (!context.ok) return context;
    if (!isId(input.correlationId) || !input.text.trim()) return fail('LANE_BLOCKED', 'A nonempty message and a stable send ID are required.');
    const prior = context.value.sends.get(input.correlationId); if (prior) return prior;
    const key = `${context.value.owner}:${context.value.bot}`;
    if (this.running.has(key)) return fail('CONTEXT_BUSY', 'This Codync bot already has an Allternit turn in progress.');
    this.running.add(key);
    const send = this.send(context.value, input).finally(() => this.running.delete(key));
    context.value.sends.set(input.correlationId, send); return send;
  }
  private async send(context: Context, input: MessageInput): Promise<Result> {
    const bot = await this.get(`codync:${context.bot}`); if (!bot.ok) return bot;
    if (bot.value.remoteIds.permission !== 'ask') return fail('POLICY_DENIED', 'This Codync bot no longer uses Ask for permission. Restore that setting before sending.');
    const result = await this.rpc('send', { botId: context.bot, text: input.text, clientNonce: input.correlationId }); if (!result.ok) return result;
    const entry = result.value.entry;
    if (!validEntry(entry) || entry.botId !== context.bot || entry.kind !== 'user' || entry.threadId !== null || (entry.data as Json).clientNonce !== input.correlationId) return fail('ADAPTER_DRIFT', 'Codync did not return the matching idempotent send receipt.');
    const turn = Number(entry.turn); context.turns.set(turn, input.correlationId);
    const deadline = Date.now() + this.replyTimeoutMs; let pending = false;
    while (Date.now() < deadline && !context.closed) {
      const history = await this.history(context.bot); if (!history.ok) return history;
      const owned = history.value.filter((row) => row.turn === turn && row.threadId === null);
      pending = owned.some((row) => row.kind === 'permission' && (row.data as Json).status === 'pending');
      const replies = owned.filter((row) => row.kind === 'agent' && typeof (row.data as Json).text === 'string');
      if (replies.some((row) => (row.data as Json).final === true)) return ok({ messageId: String(entry.id), correlationId: input.correlationId, reply: replies.map((row) => (row.data as Json).text).join('\n\n'), guarantee: 'best_effort' });
      await new Promise((resolve) => setTimeout(resolve, this.pollMs));
    }
    return fail(pending ? 'APPROVAL_REQUIRED' : 'VENDOR_UNAVAILABLE', pending ? 'Codync is waiting for your explicit approval.' : 'Codync accepted the message but did not finish before the connection wait ended. Check its conversation.', { retryable: false });
  }
  private approval(context: Context, entry: Json): Approval | null {
    if (entry.kind !== 'permission' || entry.threadId !== null || !context.turns.has(Number(entry.turn))) return null;
    const data = entry.data as Json;
    const selected = Array.isArray(data.options) ? data.options.find((option) => isObject(option) && option.optionId === data.selected) : undefined;
    const state = data.status === 'pending' ? 'pending' : data.status === 'expired' ? 'expired' : data.status === 'cancelled' ? 'cancelled' : data.status === 'answered' && isObject(selected) ? String(selected.kind).startsWith('reject') ? 'denied' : 'approved' : 'expired';
    return { authority: 'vendor', actor: 'codync', action: String(data.title ?? 'Review the vendor request'), threadId: context.id, remoteRef: String(entry.id), state };
  }
  private push(context: Context, entry: Json, type: 'agent.message.delta' | 'agent.message.completed' | 'agent.approval.requested' | 'agent.approval.resolved', payload: Json) {
    const sequence = context.events.length + 1;
    context.events.push({ cursor: String(sequence), event: { type, botId: `codync:${context.bot}`, threadId: context.id, generationId: '1', source: 'vendor', vendor: 'codync', adapter: 'codync', lane: 'local', remoteContextId: context.bot, remoteEventId: `${entry.id}:${entry.rev}:${type}`, correlationId: context.turns.get(Number(entry.turn))!, causationId: String(entry.id), guarantee: 'best_effort', at: new Date().toISOString(), payload } });
  }
  async events(input: EventsInput) {
    const context = this.live(input.contextId); if (!context.ok) return context;
    const after = Number(input.cursor ?? 0);
    if (!Number.isSafeInteger(after) || after < 0 || after > context.value.events.length) return fail('CONTEXT_NOT_FOUND', 'Invalid Codync event cursor.');
    const history = await this.history(context.value.bot); if (!history.ok) return history;
    for (const entry of history.value) {
      if (!context.value.turns.has(Number(entry.turn)) || entry.threadId !== null) continue;
      const approval = this.approval(context.value, entry);
      if (approval?.remoteRef && context.value.approvals.get(approval.remoteRef) !== approval.state) {
        context.value.approvals.set(approval.remoteRef, approval.state);
        if (['pending', 'approved', 'denied'].includes(approval.state)) this.push(context.value, entry, approval.state === 'pending' ? 'agent.approval.requested' : 'agent.approval.resolved', { approvalId: approval.remoteRef, action: approval.action, ...(approval.state === 'pending' ? {} : { decision: approval.state === 'denied' ? 'deny' : 'approve' }) });
      }
      const data = entry.data as Json;
      if (entry.kind !== 'agent' || typeof data.text !== 'string') continue;
      const previous = context.value.seen.get(String(entry.id)) ?? '';
      if (data.text !== previous) { this.push(context.value, entry, 'agent.message.delta', { chunk: data.text.startsWith(previous) ? data.text.slice(previous.length) : data.text, messageId: entry.id }); context.value.seen.set(String(entry.id), data.text); }
      if (data.final === true && !context.value.completed.has(String(entry.id))) { this.push(context.value, entry, 'agent.message.completed', { reply: data.text, messageId: entry.id }); context.value.completed.add(String(entry.id)); }
    }
    const events = context.value.events.slice(after, after + Math.max(1, Math.min(input.limit ?? 100, 500)));
    return ok({ events, nextCursor: events.at(-1)?.cursor ?? String(after) });
  }
  async approvals(input: ApprovalsInput) {
    if (!input.contextId) return fail('CONTEXT_NOT_FOUND', 'Choose a Codync context before reviewing approvals.');
    const context = this.live(input.contextId); if (!context.ok) return context;
    const history = await this.history(context.value.bot); if (!history.ok) return history;
    const approvals = history.value.map((entry) => this.approval(context.value, entry)).filter((approval): approval is Approval => approval !== null);
    if (input.op === 'list') return ok({ approvals });
    if (input.actor?.type !== 'human' || !input.actor.id || !['approve', 'deny'].includes(input.decision)) return fail('POLICY_DENIED', 'Only an explicit human decision may answer a Codync approval.');
    const approval = approvals.find((row) => row.remoteRef === input.approvalId && row.state === 'pending');
    const entry = history.value.find((row) => row.id === input.approvalId);
    if (!approval || !entry) return fail('POLICY_DENIED', 'This approval is not pending in this account and turn.');
    const options = (entry.data as Json).options;
    const option = Array.isArray(options) ? options.find((value) => isObject(value) && value.kind === (input.decision === 'approve' ? 'allow_once' : 'reject_once') && typeof value.optionId === 'string') : undefined;
    if (!isObject(option)) return fail('UNSUPPORTED', 'Codync did not offer the requested one-time decision. Respond in the vendor app.');
    const result = await this.rpc('respondPermission', { entryId: input.approvalId, optionId: option.optionId }); if (!result.ok) return result;
    const deadline = Date.now() + Math.min(this.replyTimeoutMs, 5_000);
    while (Date.now() < deadline) {
      const refreshed = await this.history(context.value.bot); if (!refreshed.ok) return refreshed;
      const current = refreshed.value.find((row) => row.id === input.approvalId);
      if (current && (current.data as Json).status === 'answered' && (current.data as Json).selected === option.optionId) return ok({ resolved: { ...approval, state: input.decision === 'approve' ? 'approved' as const : 'denied' as const } });
      await new Promise((resolve) => setTimeout(resolve, this.pollMs));
    }
    return fail('VENDOR_UNAVAILABLE', 'Codync has not confirmed the selected approval option. Review the request in Codync before retrying.', { retryable: false });
  }
  async contextClose(input: { contextId: string }) { const context = this.live(input.contextId); if (!context.ok) return context; context.value.closed = true; return ok({ closed: true }); }
  async health() {
    const hello = await this.rpc('hello');
    return ok({ status: hello.ok && typeof hello.value.hostId === 'string' ? 'healthy' as const : 'down' as const, lane: 'local', detail: hello.ok ? 'Authenticated Codync host handshake' : hello.error.humanMessage });
  }
}
