import { randomUUID } from 'node:crypto';
import { AsyncLocalStorage } from 'node:async_hooks';
import { BaseAaiProvider, fail, ok, type AaiResult, type OpenContextInput, type MessageInput, type EventsInput, type EventsResult, type CursoredEvent, type AgentSummary } from '@allternit/agent-gateway';
import { agentCapabilityManifestSchema } from '@allternit/subscription-fabric-contracts';
import { isObject, isId, type Json } from '../_shared/paired-local.js';
import { runIntelligence, type IntelligenceSocketFactory } from '../_shared/copilotkit-intelligence.js';

export const CAPABILITIES = agentCapabilityManifestSchema.parse({
 vendor:'openbot-copilotkit', adapterId:'openbot-copilotkit', lane:'local', guarantee:'best_effort',
 context:{supported:true,resume:true,parallel:false,maxParallel:1,isolation:'isolated'},
 messaging:{send:true,stream:false,steer:false,interrupt:false,cancel:false},
 memory:{read:false,write:false,snapshot:false,opaque:true},tools:{tools:false,mcp:false,plugins:false,connectors:false},
 tasks:{list:false,schedule:false,cancel:false,background:false},approvals:{read:false,respond:false,exact:false},
 computer:{view:false,control:false,takeover:false},artifacts:{read:false,write:false,export:false},
 events:{native:false,polling:true,transcriptDerived:false,replay:true},runtime:{alwaysOn:false,localRequired:false,cloud:true}
});

type Scope = { binding?: { id?: string; accountBindingId?: string }; credential?: { apiKey: string } };
type Account = { origin: string; cookie: string };
type Reply = AaiResult<{ messageId: string; correlationId: string; reply: string; guarantee: 'best_effort' }>;
type Context = { id: string; owner: string; bot: string; thread: string; closed: boolean; busy: boolean; sends: Map<string, Promise<Reply>>; sendTexts: Map<string, string>; events: CursoredEvent[] };

const scopeStore = () => (globalThis as unknown as Record<symbol, AsyncLocalStorage<Scope> | undefined>)[Symbol.for('allternit.aai.callScope')]?.getStore();
const DEFAULT_REALTIME = 'wss://realtime.intelligence.copilotkit.ai';
const TOKEN = /^[A-Za-z0-9._~%+\-/=]{8,4096}$/;

/** https origin, or loopback http origin, with no path, query or credentials. */
export function appOrigin(raw: string): string | null {
 try {
  const url = new URL(raw);
  const loopback = ['127.0.0.1', '[::1]', 'localhost'].includes(url.hostname);
  if ((url.protocol !== 'https:' && !(url.protocol === 'http:' && loopback)) || url.username || url.password || url.search || url.hash || (url.pathname !== '/' && url.pathname !== '')) return null;
  return url.origin;
 } catch { return null; }
}

/**
 * CopilotKit runtime of an OpenBot deployment, driven through the gateway's own Intelligence client
 * (adapters/_shared/copilotkit-intelligence.ts, wire-compatible with @copilotkit/core 1.75.0; same protocol as adapters/opendots-copilotkit). Authentication is the user's own Better Auth session,
 * sent as the session cookie on every request. The bot list is GET /api/agents, threads are minted by
 * POST /api/threads/mint, and sends go through /api/copilotkit/agent/:id/run with the Intelligence run receipt.
 */
export class OpenBotCopilotkitProvider extends BaseAaiProvider {
 readonly adapterId = 'openbot-copilotkit';
 private readonly contexts = new Map<string, Context>();
 private readonly fetchImpl: typeof fetch;
 private readonly socket?: IntelligenceSocketFactory;
 private readonly realtimeOrigin: string;
 private readonly timeoutMs: number;

 constructor(options: { fetch?: typeof fetch; socket?: IntelligenceSocketFactory; realtimeOrigin?: string; replyTimeoutMs?: number } = {}) {
  super();
  this.fetchImpl = options.fetch ?? fetch;
  this.socket = options.socket;
  this.realtimeOrigin = new URL(options.realtimeOrigin ?? DEFAULT_REALTIME).origin;
  this.timeoutMs = options.replyTimeoutMs ?? 120_000;
 }

 /** The user's own session token, from their own sign-in. Never persisted by this adapter. */
 private account(): AaiResult<{ owner: string; account: Account }> {
  const scope = scopeStore(), raw = scope?.credential?.apiKey;
  if (!raw) return fail('AUTH_REQUIRED', 'Provide your OpenBot session as JSON {"origin":"https://your-openbot-host","sessionToken":"<your session token>"}.');
  let parsed: unknown;
  try { parsed = JSON.parse(raw); } catch { return fail('AUTH_REQUIRED', 'OpenBot credential must be JSON with origin and sessionToken.'); }
  if (!isObject(parsed) || typeof parsed.origin !== 'string' || typeof parsed.sessionToken !== 'string') return fail('AUTH_REQUIRED', 'OpenBot credential must include origin and sessionToken.');
  const origin = appOrigin(parsed.origin);
  if (!origin) return fail('AUTH_REQUIRED', 'OpenBot origin must be an https origin (or loopback http).');
  const token = parsed.sessionToken.trim();
  if (!TOKEN.test(token)) return fail('AUTH_REQUIRED', 'OpenBot sessionToken is not a valid session cookie value.');
  // Better Auth prefixes its cookie with __Secure- on HTTPS deployments (auth/organization.ts accepts both).
  const name = origin.startsWith('https:') ? '__Secure-better-auth.session_token' : 'better-auth.session_token';
  const owner = scope?.binding?.accountBindingId ?? scope?.binding?.id;
  if (!owner) return fail('AUTH_REQUIRED', 'A per-account OpenBot credential binding is required.');
  return ok({ owner, account: { origin, cookie: `${name}=${token}` } });
 }

 private async request(account: Account, path: string, init: { method?: 'GET' | 'POST'; body?: Json } = {}): Promise<AaiResult<Response>> {
  const controller = AbortSignal.timeout(15_000);
  try {
   const response = await this.fetchImpl(`${account.origin}${path}`, { method: init.method ?? 'GET', redirect: 'error', signal: controller, headers: { cookie: account.cookie, accept: 'application/json', ...(init.body !== undefined ? { 'content-type': 'application/json' } : {}) }, ...(init.body !== undefined ? { body: JSON.stringify(init.body) } : {}) });
   if (response.ok) return ok(response);
   void response.body?.cancel();
   if (response.status === 401 || response.status === 403) return fail('AUTH_REVOKED', 'OpenBot rejected this session. Sign in again and paste a new session token.');
   if (response.status === 429) return fail('RATE_LIMITED', 'OpenBot is rate limiting this session.');
   return fail('VENDOR_UNAVAILABLE', `OpenBot returned HTTP ${response.status}.`, { retryable: init.method !== 'POST' });
  } catch { return fail('VENDOR_UNAVAILABLE', 'OpenBot is unreachable.', { retryable: init.method !== 'POST' }); }
 }

 private async json(account: Account, path: string, init?: { method?: 'GET' | 'POST'; body?: Json }): Promise<AaiResult<Json>> {
  const response = await this.request(account, path, init);
  if (!response.ok) return response;
  try { const value: unknown = await response.value.json(); return isObject(value) ? ok(value) : fail('ADAPTER_DRIFT', 'OpenBot returned a non-object response.', { retryable: false }); }
  catch { return fail('ADAPTER_DRIFT', 'OpenBot returned invalid JSON.', { retryable: false }); }
 }

 private async bots(): Promise<AaiResult<{ owner: string; account: Account; bots: { id: string; name: string }[] }>> {
  const auth = this.account(); if (!auth.ok) return auth;
  const result = await this.json(auth.value.account, '/api/agents'); if (!result.ok) return result;
  if (!Array.isArray(result.value.agents) || !result.value.agents.every(a => isObject(a) && isId(a.id) && typeof a.name === 'string')) return fail('ADAPTER_DRIFT', 'OpenBot returned an invalid agent roster.', { retryable: false });
  const bots = (result.value.agents as Json[]).filter(isObject).filter(a => a.hidden !== true).map(a => ({ id: a.id as string, name: a.name as string }));
  return ok({ ...auth.value, bots });
 }

 private summary(bot: { id: string; name: string }): AgentSummary { return { agentId: `openbot-copilotkit:${bot.id}`, displayName: bot.name, vendor: 'openbot-copilotkit', state: 'ready' }; }

 async list() { const r = await this.bots(); return r.ok ? ok(r.value.bots.map(b => this.summary(b))) : r; }

 async get(agentId: string) {
  const r = await this.bots(); if (!r.ok) return r;
  const bot = r.value.bots.find(b => `openbot-copilotkit:${b.id}` === agentId);
  return bot ? ok({ ...this.summary(bot), remoteIds: { botId: bot.id }, capabilities: CAPABILITIES }) : fail('CONTEXT_NOT_FOUND', 'This OpenBot agent is not visible to this session.');
 }

 async capabilities(agentId: string) { const r = await this.get(agentId); return r.ok ? ok(CAPABILITIES) : r; }
 async identity(agentId: string) { const r = await this.get(agentId); return r.ok ? ok({ agentId, displayName: r.value.displayName, vendor: 'openbot-copilotkit' }) : r; }

 private live(id: string): AaiResult<Context> {
  const c = this.contexts.get(id), auth = this.account();
  if (!auth.ok) return auth;
  return c && !c.closed && c.owner === auth.value.owner ? ok(c) : fail('CONTEXT_NOT_FOUND', 'This OpenBot context is not owned by the current account.');
 }

 async contextOpen(input: OpenContextInput) {
  const agent = await this.get(input.agentId); if (!agent.ok) return agent;
  const owner = this.account(); if (!owner.ok) return owner;
  if (input.adoptContextId) {
   const c = this.live(input.adoptContextId);
   return c.ok && c.value.bot === agent.value.remoteIds.botId ? ok({ contextId: c.value.id, isolation: 'isolated' as const, guarantee: 'best_effort' as const, resumed: true }) : fail('CONTEXT_NOT_FOUND', 'Cannot adopt another account or bot conversation.');
  }
  // A thread id minted by OpenBot, so this conversation is namespaced to the deployment and the account.
  const minted = await this.json(owner.value.account, '/api/threads/mint', { method: 'POST', body: {} }); if (!minted.ok) return minted;
  if (typeof minted.value.threadId !== 'string' || !/^[0-9a-f-]{36}$/i.test(minted.value.threadId)) return fail('ADAPTER_DRIFT', 'OpenBot did not return a thread id.', { retryable: false });
  const id = randomUUID();
  this.contexts.set(id, { id, owner: owner.value.owner, bot: agent.value.remoteIds.botId, thread: minted.value.threadId, closed: false, busy: false, sends: new Map(), sendTexts: new Map(), events: [] });
  return ok({ contextId: id, isolation: 'isolated' as const, guarantee: 'best_effort' as const, resumed: false });
 }

 async contextMessage(input: MessageInput) {
  const c = this.live(input.contextId); if (!c.ok) return c;
  if (!isId(input.correlationId) || !input.text.trim()) return fail('LANE_BLOCKED', 'A nonempty message and stable send ID are required.');
  const prior = c.value.sends.get(input.correlationId);
  if (prior) return c.value.sendTexts.get(input.correlationId) === input.text ? prior : fail('POLICY_DENIED', 'This send ID already belongs to a different message.');
  if (c.value.busy) return fail('CONTEXT_BUSY', 'This OpenBot conversation has a pending Allternit request.');
  c.value.busy = true; c.value.sendTexts.set(input.correlationId, input.text);
  const promise = this.send(c.value, input).finally(() => { c.value.busy = false; });
  c.value.sends.set(input.correlationId, promise);
  return promise;
 }

 private async send(c: Context, input: MessageInput): Promise<Reply> {
  const auth = this.account(); if (!auth.ok) return auth;
  const account = auth.value.account;
  const info = await this.json(account, '/api/copilotkit/info'); if (!info.ok) return info;
  if (info.value.mode !== 'intelligence' || !isObject(info.value.intelligence) || typeof info.value.intelligence.wsUrl !== 'string' || !isObject(info.value.agents) || !Object.hasOwn(info.value.agents, c.bot)) return fail('ADAPTER_DRIFT', 'OpenBot did not expose the selected agent to the CopilotKit runtime.', { retryable: false });
  try { const ws = new URL(info.value.intelligence.wsUrl); if (ws.protocol !== 'wss:' || ws.origin !== this.realtimeOrigin) return fail('POLICY_DENIED', 'OpenBot returned an unapproved realtime endpoint.'); }
  catch { return fail('ADAPTER_DRIFT', 'OpenBot returned an invalid realtime endpoint.', { retryable: false }); }

  const messageId = randomUUID();
  let failure: AaiResult<never> | undefined;
  // Only this bot's run route on this origin is reachable through the client; everything else is refused.
  const runFetch = (async (url: Parameters<typeof fetch>[0], init?: Parameters<typeof fetch>[1]) => {
   const target = new URL(typeof url === 'string' ? url : url instanceof URL ? url.href : url.url);
   if (target.origin !== account.origin || target.pathname !== `/api/copilotkit/agent/${encodeURIComponent(c.bot)}/run` || init?.method !== 'POST') throw Error('Unapproved Intelligence request');
   const body: unknown = JSON.parse(String(init.body));
   if (!isObject(body) || body.threadId !== c.thread) throw Error('Foreign Intelligence thread');
   const result = await this.request(account, target.pathname, { method: 'POST', body: body as Json });
   if (!result.ok) { failure = result; throw Error('OpenBot refused Intelligence request'); }
   const receipt: unknown = await result.value.clone().json();
   if (!isObject(receipt) || receipt.threadId !== c.thread || !isObject(receipt.realtime) || typeof receipt.realtime.clientUrl !== 'string') { failure = fail('ADAPTER_DRIFT', 'OpenBot returned foreign realtime credentials.'); throw Error('Invalid Intelligence receipt'); }
   const ws = new URL(receipt.realtime.clientUrl);
   if (ws.protocol !== 'wss:' || ws.origin !== this.realtimeOrigin) { failure = fail('POLICY_DENIED', 'OpenBot returned an unapproved realtime endpoint.'); throw Error('Invalid Intelligence endpoint'); }
   return result.value;
  }) as typeof fetch;

  const result = await runIntelligence({ runtimeUrl: `${account.origin}/api/copilotkit`, agentId: c.bot, threadId: c.thread, messages: [{ id: messageId, role: 'user', content: input.text }], headers: { cookie: account.cookie }, fetch: runFetch, socket: this.socket, timeoutMs: this.timeoutMs });
  if (failure) return failure;
  if (result.error) return fail('VENDOR_UNAVAILABLE', 'OpenBot reported a run error. Review the conversation in OpenBot.', { retryable: false });
  if (result.transport || result.status || result.identityMismatch || (!result.finished && !result.interrupted)) return fail('VENDOR_UNAVAILABLE', 'OpenBot has not confirmed this run. Check its conversation before sending again.', { retryable: false });
  const reply = result.messages.filter(m => m.role === 'assistant').at(-1)?.content;
  if (typeof reply !== 'string' || !reply.trim()) return fail('ADAPTER_DRIFT', 'OpenBot did not return a current-run assistant reply.', { retryable: false });
  c.events.push({ cursor: String(c.events.length + 1), event: { type: 'agent.message.completed', botId: `openbot-copilotkit:${c.bot}`, threadId: c.id, generationId: '1', source: 'vendor', vendor: 'openbot-copilotkit', adapter: 'openbot-copilotkit', lane: 'local', remoteContextId: c.thread, remoteEventId: messageId, correlationId: input.correlationId, causationId: input.correlationId, guarantee: 'best_effort', at: new Date().toISOString(), payload: { reply, messageId } } });
  return ok({ messageId, correlationId: input.correlationId, reply, guarantee: 'best_effort' as const });
 }

 async events(input: EventsInput): Promise<AaiResult<EventsResult>> {
  const c = this.live(input.contextId); if (!c.ok) return c;
  const after = Number(input.cursor ?? 0);
  if (!Number.isSafeInteger(after) || after < 0 || after > c.value.events.length) return fail('CONTEXT_NOT_FOUND', 'Invalid OpenBot event cursor.');
  const rows = c.value.events.slice(after, after + Math.max(1, Math.min(input.limit ?? 100, 500)));
  return ok({ events: rows, nextCursor: String(after + rows.length) });
 }

 async contextClose(input: { contextId: string }) {
  const c = this.live(input.contextId); if (!c.ok) return c;
  if (c.value.busy) return fail('CONTEXT_BUSY', 'Wait for the current OpenBot turn before detaching.');
  c.value.closed = true;
  return ok({ closed: true });
 }

 async health() {
  const r = await this.bots();
  return ok({ status: r.ok ? 'healthy' as const : 'down' as const, lane: 'local', detail: r.ok ? 'Authenticated OpenBot session' : r.error.humanMessage });
 }
}
