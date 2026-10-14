import { AsyncLocalStorage } from 'node:async_hooks';
import { randomUUID } from 'node:crypto';
import { BaseAaiProvider, fail, ok, type AaiResult, type AgentSummary, type OpenContextInput, type MessageInput, type EventsInput, type EventsResult, type CursoredEvent } from '@allternit/agent-gateway';
import { agentCapabilityManifestSchema } from '@allternit/subscription-fabric-contracts';
import { isObject, type Json } from '../_shared/paired-local.js';
import { DEFAULT_SERVER_URL, authChallenge, b64, decryptWith, encryptWith, parseSecret, sessionCipher, unb64, type SessionCipher } from './crypto.js';

export const CAPABILITIES = agentCapabilityManifestSchema.parse({
 vendor:'happy', adapterId:'happy', lane:'local', guarantee:'best_effort',
 context:{supported:true,resume:false,parallel:false,maxParallel:1,isolation:'shared'},
 messaging:{send:true,stream:false,steer:false,interrupt:false,cancel:false},
 memory:{read:false,write:false,snapshot:false,opaque:true},tools:{tools:false,mcp:false,plugins:false,connectors:false},
 tasks:{list:false,schedule:false,cancel:false,background:false},approvals:{read:false,respond:false,exact:false},
 computer:{view:false,control:false,takeover:false},artifacts:{read:false,write:false,export:false},
 events:{native:false,polling:true,transcriptDerived:true,replay:true},runtime:{alwaysOn:false,localRequired:true,cloud:true}
});

type Credential = { secret: Uint8Array; serverUrl: string };
type Scope = { binding?: { id?: string; accountBindingId?: string }; credential?: { apiKey: string } };
type Session = { id: string; active: boolean; cipher: SessionCipher; name: string };
type Reply = AaiResult<{ messageId: string; correlationId: string; reply: string; guarantee: 'best_effort' }>;
type Context = { id: string; owner: string; sessionId: string; closed: boolean; busy: boolean; sends: Map<string, Promise<Reply>>; sendTexts: Map<string, string>; events: CursoredEvent[] };

const scopeStore = () => (globalThis as unknown as Record<symbol, AsyncLocalStorage<Scope> | undefined>)[Symbol.for('allternit.aai.callScope')]?.getStore();
const idLike = (v: unknown): v is string => typeof v === 'string' && /^[\w-]{1,200}$/.test(v);
const sleep = (ms: number) => new Promise<void>(resolve => setTimeout(resolve, ms));

/** https origin, or a loopback http origin. Anything with a path, query or credentials is refused. */
export function serverOrigin(raw: string): string | null {
 try {
  const url = new URL(raw);
  const loopback = ['127.0.0.1', '[::1]', 'localhost'].includes(url.hostname);
  if ((url.protocol !== 'https:' && !(url.protocol === 'http:' && loopback)) || url.username || url.password || url.search || url.hash || (url.pathname !== '/' && url.pathname !== '')) return null;
  return url.origin;
 } catch { return null; }
}

/** Text of one decrypted Happy message, plus whether it marks the end of a turn. */
export function agentText(content: unknown): { text: string; done: boolean } | null {
 if (!isObject(content) || content.role !== 'agent' || !isObject(content.content)) return null;
 const body = content.content, data = body.data;
 if (!isObject(data)) return null;
 if (body.type === 'output') {
  if (data.type === 'result') return { text: '', done: true };
  if (data.type !== 'assistant' || !isObject(data.message) || !Array.isArray(data.message.content)) return null;
  const text = data.message.content.filter(isObject).filter(b => b.type === 'text' && typeof b.text === 'string').map(b => String(b.text)).join('\n\n');
  return text ? { text, done: false } : null;
 }
 if (body.type === 'acp' || body.type === 'codex') {
  if (data.type === 'task_complete') return { text: '', done: true };
  if (data.type !== 'message' && data.type !== 'agent_message') return null;
  const text = typeof data.message === 'string' ? data.message : typeof data.text === 'string' ? data.text : '';
  return text.trim() ? { text, done: false } : null;
 }
 return null;
}

/**
 * Minimal Socket.IO v4 client over the platform WebSocket. Only what sending a user message needs:
 * engine.io open/ping, socket.io CONNECT with auth, one EVENT. Replies are read back over REST, so no
 * socket.io-client dependency is pulled into the gateway.
 */
type Socket = { send(data: string): void; close(): void; onmessage: ((event: { data: unknown }) => void) | null; onerror: (() => void) | null; onclose: (() => void) | null };
export async function emitSessionMessage(serverUrl: string, token: string, payload: { sid: string; message: string; localId: string }, timeoutMs = 15000): Promise<void> {
 const origin = new URL(serverUrl);
 const wsUrl = `${origin.protocol === 'https:' ? 'wss' : 'ws'}://${origin.host}/v1/updates/?EIO=4&transport=websocket`;
 const WS = (globalThis as unknown as { WebSocket?: new (url: string) => Socket }).WebSocket;
 if (!WS) throw new Error('WebSocket unavailable');
 await new Promise<void>((resolve, reject) => {
  const socket = new WS(wsUrl);
  const timer = setTimeout(() => { socket.close(); reject(new Error('socket timeout')); }, timeoutMs);
  const finish = (error?: Error) => { clearTimeout(timer); try { socket.close(); } catch { /* closed */ } error ? reject(error) : resolve(); };
  socket.onmessage = (event) => {
   const frame = String(event.data);
   if (frame.startsWith('0')) socket.send('40' + JSON.stringify({ token, clientType: 'user-scoped' }));
   else if (frame === '2') socket.send('3');
   else if (frame.startsWith('40')) { socket.send('42' + JSON.stringify(['message', payload])); setTimeout(() => finish(), 300); }
   else if (frame.startsWith('44') || frame === '41') finish(new Error('socket refused'));
  };
  socket.onerror = () => finish(new Error('socket error'));
  socket.onclose = () => { clearTimeout(timer); };
 });
}

/**
 * Happy native session adapter. Each active Happy session (a Claude Code or Codex session on the user's
 * machine) is one agent. Send = encrypted user message on the session's socket; reply = the agent's
 * encrypted messages that follow it, read over the session message REST route and decrypted with the
 * session key. The user supplies their own account secret; this adapter never persists it.
 */
export class HappyProvider extends BaseAaiProvider {
 readonly adapterId = 'happy';
 private readonly contexts = new Map<string, Context>();
 private readonly running = new Set<string>();
 private readonly fetchImpl: typeof fetch;
 private readonly defaultServer: string;
 private readonly timeoutMs: number;
 private readonly settleMs: number;
 private readonly pollMs: number;

 constructor(options: { serverUrl?: string; fetch?: typeof fetch; replyTimeoutMs?: number; settleMs?: number; pollMs?: number } = {}) {
  super();
  this.fetchImpl = options.fetch ?? fetch;
  this.defaultServer = serverOrigin(options.serverUrl ?? DEFAULT_SERVER_URL) ?? DEFAULT_SERVER_URL;
  this.timeoutMs = options.replyTimeoutMs ?? 120_000;
  this.settleMs = options.settleMs ?? 8_000;
  this.pollMs = options.pollMs ?? 1_500;
 }

 private scope() { return scopeStore(); }

 private credential(): AaiResult<{ owner: string; credential: Credential }> {
  const scope = this.scope(), raw = scope?.credential?.apiKey;
  if (!raw) return fail('AUTH_REQUIRED', 'Provide your Happy account secret as JSON {"secret":"<restore secret>"}.');
  let parsed: unknown;
  try { parsed = JSON.parse(raw); } catch { return fail('AUTH_REQUIRED', 'Happy credential must be JSON with your account secret.'); }
  if (!isObject(parsed) || typeof parsed.secret !== 'string') return fail('AUTH_REQUIRED', 'Happy credential must include "secret".');
  const secret = parseSecret(parsed.secret);
  if (!secret) return fail('AUTH_REQUIRED', 'Happy "secret" must be the 32-byte restore secret (base64 or base64url).');
  const server = typeof parsed.serverUrl === 'string' ? serverOrigin(parsed.serverUrl) : this.defaultServer;
  if (!server) return fail('AUTH_REQUIRED', 'Happy serverUrl must be an https origin.');
  const owner = scope?.binding?.accountBindingId ?? scope?.binding?.id;
  if (!owner) return fail('AUTH_REQUIRED', 'A per-account Happy credential binding is required.');
  return ok({ owner, credential: { secret, serverUrl: server } });
 }

 private async authenticate(cred: Credential): Promise<AaiResult<string>> {
  const body = authChallenge(cred.secret);
  try {
   const response = await this.fetchImpl(`${cred.serverUrl}/v1/auth`, { method: 'POST', redirect: 'error', signal: AbortSignal.timeout(15_000), headers: { 'content-type': 'application/json' }, body: JSON.stringify(body) });
   if (response.status === 401 || response.status === 403) { void response.body?.cancel(); return fail('AUTH_REVOKED', 'Happy rejected this account secret.'); }
   if (!response.ok) { void response.body?.cancel(); return fail('VENDOR_UNAVAILABLE', `Happy returned HTTP ${response.status} during sign-in.`); }
   const value: unknown = await response.json();
   if (!isObject(value) || value.success !== true || typeof value.token !== 'string') return fail('ADAPTER_DRIFT', 'Happy sign-in response did not match its contract.', { retryable: false });
   return ok(value.token);
  } catch { return fail('VENDOR_UNAVAILABLE', 'Happy server is unreachable.'); }
 }

 private async getJson(cred: Credential, token: string, path: string): Promise<AaiResult<Json>> {
  try {
   const response = await this.fetchImpl(`${cred.serverUrl}${path}`, { redirect: 'error', signal: AbortSignal.timeout(15_000), headers: { authorization: `Bearer ${token}`, accept: 'application/json' } });
   if (response.status === 401 || response.status === 403) { void response.body?.cancel(); return fail('AUTH_REVOKED', 'Happy rejected the session token.'); }
   if (response.status === 404) { void response.body?.cancel(); return fail('CONTEXT_NOT_FOUND', 'Happy session was not found on this account.'); }
   if (!response.ok) { void response.body?.cancel(); return fail('VENDOR_UNAVAILABLE', `Happy returned HTTP ${response.status}.`); }
   const value: unknown = await response.json();
   return isObject(value) ? ok(value) : fail('ADAPTER_DRIFT', 'Happy returned a non-object response.', { retryable: false });
  } catch { return fail('VENDOR_UNAVAILABLE', 'Happy server is unreachable.'); }
 }

 /** Active sessions on this account, each with its decrypted name and session cipher. */
 private async sessions(cred: Credential): Promise<AaiResult<{ token: string; sessions: Session[] }>> {
  const token = await this.authenticate(cred); if (!token.ok) return token;
  const list = await this.getJson(cred, token.value, '/v2/sessions/active'); if (!list.ok) return list;
  if (!Array.isArray(list.value.sessions)) return fail('ADAPTER_DRIFT', 'Happy session list did not match its contract.', { retryable: false });
  const out: Session[] = [];
  for (const row of list.value.sessions) {
   if (!isObject(row) || !idLike(row.id) || typeof row.metadata !== 'string') continue;
   const cipher = sessionCipher(cred.secret, typeof row.dataEncryptionKey === 'string' ? row.dataEncryptionKey : null);
   if (!cipher) continue;
   const meta = decryptWith(cipher, unb64(row.metadata));
   const summary = isObject(meta) && isObject(meta.summary) && typeof meta.summary.text === 'string' ? meta.summary.text : null;
   const path = isObject(meta) && typeof meta.path === 'string' ? meta.path : 'session';
   const flavor = isObject(meta) && typeof meta.flavor === 'string' ? meta.flavor : 'Happy';
   out.push({ id: row.id, active: row.active === true, cipher, name: `${flavor}: ${summary || path}` });
  }
  return ok({ token: token.value, sessions: out });
 }

 private async roster(): Promise<AaiResult<{ owner: string; cred: Credential; token: string; sessions: Session[] }>> {
  const auth = this.credential(); if (!auth.ok) return auth;
  const result = await this.sessions(auth.value.credential); if (!result.ok) return result;
  return ok({ owner: auth.value.owner, cred: auth.value.credential, token: result.value.token, sessions: result.value.sessions });
 }

 private summary(s: Session): AgentSummary { return { agentId: `happy:${s.id}`, displayName: s.name, vendor: 'happy', state: 'ready' }; }

 async list() { const r = await this.roster(); return r.ok ? ok(r.value.sessions.filter(s => s.active).map(s => this.summary(s))) : r; }

 async get(agentId: string) {
  const r = await this.roster(); if (!r.ok) return r;
  const session = r.value.sessions.find(s => `happy:${s.id}` === agentId);
  return session ? ok({ ...this.summary(session), remoteIds: { sessionId: session.id }, capabilities: CAPABILITIES }) : fail('CONTEXT_NOT_FOUND', 'No active Happy session exists for this account.');
 }

 async capabilities(agentId: string) { const r = await this.get(agentId); return r.ok ? ok(CAPABILITIES) : r; }
 async identity(agentId: string) { const r = await this.get(agentId); return r.ok ? ok({ agentId, displayName: r.value.displayName, vendor: 'happy' }) : r; }

 private live(id: string): AaiResult<Context> {
  const c = this.contexts.get(id), auth = this.credential();
  if (!auth.ok) return auth;
  return c && !c.closed && c.owner === auth.value.owner ? ok(c) : fail('CONTEXT_NOT_FOUND', 'This Happy context is not owned by the current account.');
 }

 async contextOpen(input: OpenContextInput) {
  const agent = await this.get(input.agentId); if (!agent.ok) return agent;
  const sessionId = agent.value.remoteIds.sessionId;
  const owner = this.credential(); if (!owner.ok) return owner;
  if (input.adoptContextId) {
   const c = this.live(input.adoptContextId);
   return c.ok && c.value.sessionId === sessionId ? ok({ contextId: c.value.id, isolation: 'shared' as const, guarantee: 'best_effort' as const, resumed: true }) : fail('CONTEXT_NOT_FOUND', 'Cannot adopt another account or session context.');
  }
  const id = randomUUID();
  this.contexts.set(id, { id, owner: owner.value.owner, sessionId, closed: false, busy: false, sends: new Map(), sendTexts: new Map(), events: [] });
  return ok({ contextId: id, isolation: 'shared' as const, guarantee: 'best_effort' as const, resumed: false });
 }

 async contextMessage(input: MessageInput) {
  const c = this.live(input.contextId); if (!c.ok) return c;
  if (!input.text.trim() || !idLike(input.correlationId)) return fail('LANE_BLOCKED', 'A nonempty message and stable send ID are required.');
  const prior = c.value.sends.get(input.correlationId);
  if (prior) return c.value.sendTexts.get(input.correlationId) === input.text ? prior : fail('POLICY_DENIED', 'This send ID already belongs to a different message.');
  if (c.value.busy || this.running.has(c.value.sessionId)) return fail('CONTEXT_BUSY', 'This Happy session already has an active Allternit request.');
  this.running.add(c.value.sessionId); c.value.busy = true;
  c.value.sendTexts.set(input.correlationId, input.text);
  const promise = this.send(c.value, input).finally(() => { c.value.busy = false; this.running.delete(c.value.sessionId); });
  c.value.sends.set(input.correlationId, promise);
  return promise;
 }

 private async send(context: Context, input: MessageInput): Promise<Reply> {
  const auth = this.credential(); if (!auth.ok) return auth;
  const roster = await this.sessions(auth.value.credential); if (!roster.ok) return roster;
  const session = roster.value.sessions.find(s => s.id === context.sessionId);
  if (!session) return fail('CONTEXT_NOT_FOUND', 'This Happy session is no longer active.', { retryable: false });
  const cred = auth.value.credential, token = roster.value.token;

  // The message is sent under the correlation ID as localId, so the server de-duplicates a retry.
  const payload = encryptWith(session.cipher, { role: 'user', content: { type: 'text', text: input.text }, localKey: input.correlationId, meta: { sentFrom: 'allternit' } });
  try { await emitSessionMessage(cred.serverUrl, token, { sid: session.id, message: b64(payload), localId: input.correlationId }); }
  catch { return fail('VENDOR_UNAVAILABLE', 'Happy did not accept the message. Its outcome is uncertain; check the session before sending again.', { retryable: false }); }

  // Confirm our message landed, then read the agent messages that follow it.
  const deadline = Date.now() + this.timeoutMs;
  let ours: { seq: number } | null = null;
  const texts = new Map<number, string>();
  let done = false, lastSeenAt = 0;
  while (Date.now() < deadline) {
   const page = await this.getJson(cred, token, `/v1/sessions/${encodeURIComponent(session.id)}/messages`);
   if (!page.ok) return page;
   const rows = Array.isArray(page.value.messages) ? page.value.messages.filter(isObject) : [];
   if (!ours) {
    const row = rows.find(r => r.localId === input.correlationId);
    if (row && typeof row.seq === 'number') ours = { seq: row.seq };
   }
   if (ours) {
    for (const row of rows) {
     if (typeof row.seq !== 'number' || row.seq <= ours.seq || texts.has(row.seq) || !isObject(row.content) || row.content.t !== 'encrypted' || typeof row.content.c !== 'string') continue;
     const decoded = decryptWith(session.cipher, unb64(row.content.c));
     const parsed = agentText(decoded);
     if (!parsed) continue;
     if (parsed.done) done = true;
     texts.set(row.seq, parsed.text);
     lastSeenAt = Date.now();
    }
    const nonEmpty = [...texts.values()].filter(Boolean);
    if (done && nonEmpty.length) break;
    if (nonEmpty.length && Date.now() - lastSeenAt >= this.settleMs) break;
   }
   await sleep(this.pollMs);
  }
  if (!ours) return fail('VENDOR_UNAVAILABLE', 'Happy did not confirm the message was stored. Check the session before sending again.', { retryable: false });
  const reply = [...texts.entries()].sort((a, b) => a[0] - b[0]).map(([, t]) => t).filter(Boolean).join('\n\n');
  if (!reply.trim()) return fail('VENDOR_UNAVAILABLE', 'No agent reply arrived for this message before the timeout. The session may not be running.', { retryable: false });
  const messageId = input.correlationId;
  context.events.push({ cursor: String(context.events.length + 1), event: { type: 'agent.message.completed', botId: `happy:${session.id}`, threadId: context.id, generationId: '1', source: 'vendor', vendor: 'happy', adapter: 'happy', lane: 'local', remoteContextId: session.id, remoteEventId: messageId, correlationId: input.correlationId, causationId: input.correlationId, guarantee: 'best_effort', at: new Date().toISOString(), payload: { reply, messageId } } });
  return ok({ messageId, correlationId: input.correlationId, reply, guarantee: 'best_effort' as const });
 }

 async events(input: EventsInput): Promise<AaiResult<EventsResult>> {
  const c = this.live(input.contextId); if (!c.ok) return c;
  const after = Number(input.cursor ?? 0);
  if (!Number.isSafeInteger(after) || after < 0 || after > c.value.events.length) return fail('CONTEXT_NOT_FOUND', 'Invalid Happy event cursor.');
  const rows = c.value.events.slice(after, after + Math.max(1, Math.min(input.limit ?? 100, 500)));
  return ok({ events: rows, nextCursor: String(after + rows.length) });
 }

 async contextClose(input: { contextId: string }) {
  const c = this.live(input.contextId); if (!c.ok) return c;
  if (c.value.busy) return fail('CONTEXT_BUSY', 'Wait for the Happy reply to finish before detaching.');
  c.value.closed = true;
  return ok({ closed: true });
 }

 async health() {
  const auth = this.credential();
  if (!auth.ok) return ok({ status: 'down' as const, lane: 'local', detail: auth.error.humanMessage });
  const r = await this.sessions(auth.value.credential);
  return ok({ status: r.ok ? 'healthy' as const : 'down' as const, lane: 'local', detail: r.ok ? 'Authenticated Happy account' : r.error.humanMessage });
 }
}
