import { randomUUID } from 'node:crypto';
import { BaseAaiProvider, fail, ok, type AaiResult, type OpenContextInput, type MessageInput, type EventsInput, type EventsResult } from '@allternit/agent-gateway';
import { agentCapabilityManifestSchema } from '@allternit/subscription-fabric-contracts';
import { PairedLocalTransport, isId, isObject, type Json } from '../_shared/paired-local.js';

export const CAPABILITIES = agentCapabilityManifestSchema.parse({
 vendor:'vellum-assistant', adapterId:'vellum-assistant', lane:'local', guarantee:'best_effort',
 context:{supported:true,resume:true,parallel:false,maxParallel:1,isolation:'isolated'},
 messaging:{send:true,stream:false,steer:false,interrupt:false,cancel:false},
 memory:{read:false,write:false,snapshot:false,opaque:true},tools:{tools:false,mcp:false,plugins:false,connectors:false},
 tasks:{list:false,schedule:false,cancel:false,background:false},approvals:{read:false,respond:false,exact:false},
 computer:{view:false,control:false,takeover:false},artifacts:{read:false,write:false,export:false},
 events:{native:false,polling:true,transcriptDerived:false,replay:true},runtime:{alwaysOn:false,localRequired:true,cloud:false}
});

const AGENT = 'default';
const POLL_MS = 500;
const REPLY_TIMEOUT_MS = 120_000;

type Reply = AaiResult<{messageId:string;correlationId:string;reply:string;guarantee:'best_effort'}>;
type Context = {id:string;owner:string;key:string;remote?:string;closed:boolean;busy:boolean;seq:number;sends:Map<string,Promise<Reply>>;replies:{requestId:string;correlationId:string;messageId:string;reply:string}[]};

const textOf = (content: unknown): string => {
 if (typeof content === 'string') {
  try { const blocks: unknown = JSON.parse(content); if (Array.isArray(blocks)) return blocks.filter(isObject).filter(b => b.type === 'text' && typeof b.text === 'string').map(b => String(b.text)).join('\n\n'); }
  catch { /* plain text row */ }
  return content;
 }
 return Array.isArray(content) ? content.filter(isObject).filter(b => b.type === 'text' && typeof b.text === 'string').map(b => String(b.text)).join('\n\n') : '';
};
const timeOf = (v: unknown): number => typeof v === 'number' ? v : Date.parse(String(v));

/**
 * Vellum Assistant runtime (vellum-ai/vellum-assistant, pinned commit 8e0a4fd7fcfcd7c22fef9d579e733dc5c4cabec9).
 * Loopback runtime only; authenticates with the actor_client_v1 bearer JWT minted by POST /v1/pair on the host.
 */
export class VellumAssistantProvider extends BaseAaiProvider {
 readonly adapterId = 'vellum-assistant';
 private transport: PairedLocalTransport | null = null;
 private configError: string | null = null;
 private contexts = new Map<string, Context>();
 private sendTexts = new Map<string, string>();
 private pollMs: number; private timeoutMs: number;

 constructor(options: {baseUrl?: string; token?: string; fetch?: typeof fetch; pollMs?: number; replyTimeoutMs?: number} = {}) {
  super();
  this.pollMs = options.pollMs ?? POLL_MS; this.timeoutMs = options.replyTimeoutMs ?? REPLY_TIMEOUT_MS;
  // Unset URL is inert: no transport, every call returns AUTH_REQUIRED. Never throw at boot.
  if (!options.baseUrl) { this.configError = 'Set SUBS_GATEWAY_VELLUM_ASSISTANT_URL to the loopback origin of the Vellum assistant runtime.'; return; }
  try { this.transport = new PairedLocalTransport('Vellum Assistant', { ...options, baseUrl: options.baseUrl }); }
  catch (error) { this.configError = error instanceof Error ? error.message : 'Vellum Assistant endpoint is invalid.'; }
 }

 private gate(): AaiResult<PairedLocalTransport> {
  if (!this.transport) return fail('AUTH_REQUIRED', this.configError ?? 'Vellum Assistant is not configured.');
  return ok(this.transport);
 }

 async list() {
  const g = this.gate(); if (!g.ok) return g;
  const result = await g.value.json('/v1/identity');
  if (!result.ok) return result;
  if (typeof result.value.name !== 'string' || !result.value.name.trim()) return fail('ADAPTER_DRIFT', 'Vellum Assistant returned an invalid identity.');
  return ok([{agentId:`vellum-assistant:${AGENT}`, displayName: result.value.name, vendor:'vellum-assistant', state:'unknown' as const}]);
 }

 async get(agentId: string) {
  const roster = await this.list(); if (!roster.ok) return roster;
  const agent = roster.value.find(a => a.agentId === agentId);
  return agent ? ok({...agent, remoteIds:{assistantId: AGENT}, capabilities: CAPABILITIES}) : fail('CONTEXT_NOT_FOUND', 'This Vellum assistant is not visible to the connected credential.');
 }

 async capabilities(agentId: string) { const agent = await this.get(agentId); return agent.ok ? ok(CAPABILITIES) : agent; }
 async identity(agentId: string) { const agent = await this.get(agentId); return agent.ok ? ok({agentId, displayName: agent.value.displayName, vendor:'vellum-assistant'}) : agent; }

 private live(id: string): AaiResult<Context> {
  const owner = this.transport?.owner() ?? 'local';
  const c = this.contexts.get(id);
  return c && !c.closed && c.owner === owner ? ok(c) : fail('CONTEXT_NOT_FOUND', 'This Vellum context is not owned by the current account.');
 }

 async contextOpen(input: OpenContextInput) {
  const agent = await this.get(input.agentId); if (!agent.ok) return agent;
  const owner = this.transport?.owner() ?? 'local';
  if (input.adoptContextId) {
   const c = this.live(input.adoptContextId);
   return c.ok && c.value.key.startsWith('allternit-') ? ok({contextId: c.value.id, isolation:'isolated' as const, guarantee:'best_effort' as const, resumed: true}) : fail('CONTEXT_NOT_FOUND', 'Cannot adopt a Vellum conversation that this account did not open.');
  }
  // The Vellum conversation is minted on first send through a per-context conversationKey, so no vendor call is needed here.
  const id = randomUUID();
  this.contexts.set(id, {id, owner, key:`allternit-${id}`, closed:false, busy:false, seq:0, sends:new Map(), replies:[]});
  return ok({contextId: id, isolation:'isolated' as const, guarantee:'best_effort' as const, resumed: false});
 }

 async contextMessage(input: MessageInput) {
  const c = this.live(input.contextId); if (!c.ok) return c;
  if (!isId(input.correlationId) || !input.text.trim()) return fail('LANE_BLOCKED', 'A nonempty message and stable send ID are required.');
  const sendKey = `${c.value.id}:${input.correlationId}`;
  const prior = c.value.sends.get(input.correlationId);
  if (prior) return this.sendTexts.get(sendKey) === input.text ? prior : fail('POLICY_DENIED', 'This send ID already belongs to a different message.');
  if (c.value.busy) return fail('CONTEXT_BUSY', 'This Vellum conversation has a pending Allternit request.');
  c.value.busy = true;
  const promise = this.send(c.value, input).finally(() => { c.value.busy = false; });
  this.sendTexts.set(sendKey, input.text);
  c.value.sends.set(input.correlationId, promise);
  return promise;
 }

 private async send(c: Context, input: MessageInput): Promise<Reply> {
  const g = this.gate(); if (!g.ok) return g;
  const body: Json = {conversationKey: c.key, content: input.text, sourceChannel:'vellum', interface:'vellum', clientMessageId: input.correlationId, ...(c.remote ? {conversationId: c.remote} : {})};
  const accepted = await g.value.json('/v1/messages', body);
  if (!accepted.ok) return accepted;
  const {requestId, conversationId, messageId} = accepted.value;
  if (accepted.value.accepted !== true || !isId(requestId) || typeof conversationId !== 'string' || !isId(conversationId) || (c.remote && c.remote !== conversationId))
   return fail('ADAPTER_DRIFT', 'Vellum did not confirm the matching accepted request.', {retryable: false});
  c.remote = conversationId;
  const deadline = Date.now() + this.timeoutMs;
  // Wait for the runtime's own completion event for this exact requestId, read from the buffered event tail.
  let done = false;
  while (!done) {
   if (c.closed) return fail('VENDOR_UNAVAILABLE', 'The Vellum conversation was closed before the reply completed.', {retryable: false});
   const tail = await g.value.json(`/v1/events/tail?conversationId=${encodeURIComponent(conversationId)}&fromSeq=${c.seq}`);
   if (!tail.ok) return tail;
   if (tail.value.complete !== true) return fail('VENDOR_UNAVAILABLE', 'Vellum event tail no longer reaches this request. Check the vendor conversation before sending again.', {retryable: false});
   if (!Array.isArray(tail.value.events)) return fail('ADAPTER_DRIFT', 'Vellum returned an invalid event tail.');
   for (const envelope of tail.value.events) {
    if (!isObject(envelope) || !Number.isSafeInteger(envelope.seq) || !isObject(envelope.message)) return fail('ADAPTER_DRIFT', 'Vellum emitted a malformed event envelope.');
    c.seq = Math.max(c.seq, envelope.seq as number);
    const m = envelope.message;
    if (m.requestId !== requestId) continue;
    if (m.type === 'error') return fail('VENDOR_UNAVAILABLE', 'Vellum reported that this request failed. Review its conversation.', {retryable: false});
    if (m.type === 'message_request_complete') done = true;
   }
   if (done) break;
   if (Date.now() >= deadline) return fail('VENDOR_UNAVAILABLE', 'Vellum accepted the request but has not confirmed completion. Check the vendor conversation before sending again.', {retryable: false});
   await new Promise(resolve => setTimeout(resolve, this.pollMs));
  }
  // Attribute the reply: the latest assistant row created at or after this request's own user row.
  const rows = await g.value.json(`/v1/messages?conversationId=${encodeURIComponent(conversationId)}&limit=50`);
  if (!rows.ok) return rows;
  if (!Array.isArray(rows.value.messages)) return fail('ADAPTER_DRIFT', 'Vellum returned an invalid message list.');
  const list = rows.value.messages.filter(isObject);
  const userRow = list.find(r => r.role === 'user' && (r.id === messageId || r.clientMessageId === input.correlationId));
  if (!userRow) return fail('ADAPTER_DRIFT', 'Vellum completed the request without exposing its user message row.', {retryable: false});
  const floor = timeOf(userRow.createdAt);
  const replies = list.filter(r => r.role === 'assistant' && typeof r.id === 'string' && timeOf(r.createdAt) >= floor).sort((a, b) => timeOf(a.createdAt) - timeOf(b.createdAt));
  const last = replies[replies.length - 1];
  const reply = last ? textOf(last.content) : '';
  if (!last || typeof last.id !== 'string' || !reply.trim()) return fail('ADAPTER_DRIFT', 'Vellum completed the matching request without an attributable assistant text message.', {retryable: false});
  c.replies.push({requestId, correlationId: input.correlationId, messageId: last.id, reply});
  return ok({messageId: last.id, correlationId: input.correlationId, reply, guarantee:'best_effort' as const});
 }

 async events(input: EventsInput): Promise<AaiResult<EventsResult>> {
  const c = this.live(input.contextId); if (!c.ok) return c;
  const after = Number(input.cursor ?? 0);
  if (!Number.isSafeInteger(after) || after < 0 || after > c.value.replies.length) return fail('CONTEXT_NOT_FOUND', 'Invalid Vellum event cursor.');
  const rows = c.value.replies.slice(after, after + Math.max(1, Math.min(input.limit ?? 100, 500)));
  return ok({events: rows.map((r, i) => ({cursor: String(after + i + 1), event:{type:'agent.message.completed' as const, botId:`vellum-assistant:${AGENT}`, threadId: c.value.id, generationId:'1', source:'vendor' as const, vendor:'vellum-assistant', adapter:'vellum-assistant', lane:'local' as const, remoteContextId: c.value.remote ?? c.value.key, remoteEventId: r.messageId, correlationId: r.correlationId, causationId: r.requestId, guarantee:'best_effort' as const, at: new Date().toISOString(), payload:{reply: r.reply, messageId: r.messageId}}})), nextCursor: String(after + rows.length)});
 }

 async contextClose(input: {contextId: string}) {
  const c = this.live(input.contextId); if (!c.ok) return c;
  if (c.value.busy) return fail('CONTEXT_BUSY', 'Wait for the outstanding Vellum request before closing its conversation.');
  // No verified vendor close endpoint is used; closure only stops this adapter from sending into the context.
  c.value.closed = true;
  return ok({closed: true});
 }

 async health() {
  const result = await this.list();
  return ok({status: result.ok ? 'healthy' as const : 'down' as const, lane:'local', detail: result.ok ? 'Authenticated Vellum assistant identity' : result.error.humanMessage});
 }
}
