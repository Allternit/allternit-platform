import { describe, expect, it } from 'vitest';
import { OpenMausBotProvider } from '../adapters/openmausbot/provider.js';
import { runWithCallScope } from '../src/aai/call-scope.js';

function fixture(options: { status?: number; drift?: boolean; permission?: boolean; lostSend?: boolean } = {}) {
  const requests: Array<{ path: string; body?: Record<string, unknown>; authorization: string | null }> = [];
  let sent = false;
  let answered = false;
  const fetcher = async (input: string | URL | Request, init?: RequestInit) => {
    const path = new URL(String(input)).pathname;
    const body = init?.body ? JSON.parse(String(init.body)) : undefined;
    requests.push({ path, body, authorization: new Headers(init?.headers).get('authorization') });
    if (options.status) return Response.json({}, { status: options.status });
    if (path === '/api/bots') return Response.json({ bots: [{ id: 'bot-1', threadId: 'thread-1', name: 'Maus', busy: false }] });
    if (path.endsWith('/messages/guarded')) {
      sent = true;
      if (options.lostSend) throw new Error('Lost response with secret-token');
      return Response.json({ ok: true, threadId: 'thread-1', message: { id: 'user-1' } }, { status: 202 });
    }
    if (path.endsWith('/respond')) { answered = true; return Response.json({ ok: true }); }
    if (path.endsWith('/messages')) return Response.json({ activeLeafId: options.drift ? undefined : null, messages: sent ? [
      { id: 'user-1', role: 'user', kind: 'text', text: 'Hello', requestPending: false },
      options.permission ? { id: 'ask-1', role: 'bot', kind: 'options', requestMessageId: 'user-1', card: { requestId: 'permission-1', requestType: 'permission', ...(answered ? { answered: 'allow' } : {}) } } : { id: 'answer-1', role: 'bot', kind: 'text', text: 'Real vendor reply', requestMessageId: 'user-1', turnTerminal: true, turnSucceeded: true },
    ] : [] });
    return Response.json({}, { status: 404 });
  };
  return { provider: new OpenMausBotProvider({ token: 'secret-token', fetch: fetcher as typeof fetch, pollMs: 1, replyTimeoutMs: 50 }), requests };
}

async function open(provider: OpenMausBotProvider) {
  const result = await provider.contextOpen({ agentId: 'openmausbot:bot-1' });
  expect(result.ok).toBe(true);
  if (!result.ok) throw new Error('Context open failed');
  return result.value.contextId;
}

describe('OpenMausBot authenticated vendor transport', () => {
  it('requires a paired credential, rejects remote endpoints and never opens a local auth bypass', async () => {
    expect((await new OpenMausBotProvider().list()).ok).toBe(false);
    expect(() => new OpenMausBotProvider({ baseUrl: 'https://untrusted.example' })).toThrow(/loopback/);
    expect(() => new OpenMausBotProvider({ baseUrl: 'http://token@localhost:8799' })).toThrow(/loopback/);
    const { provider, requests } = fixture();
    expect(await provider.list()).toMatchObject({ ok: true, value: [{ agentId: 'openmausbot:bot-1', displayName: 'Maus' }] });
    expect(requests[0].authorization).toBe('Bearer secret-token');
  });

  it('uses the vendor conversation, guarded approval mode and durable send ID, then returns its reply', async () => {
    const { provider, requests } = fixture();
    const contextId = await open(provider);
    const input = { contextId, correlationId: 'send-1', text: 'Hello' };
    const first = await provider.contextMessage(input);
    expect(first).toMatchObject({ ok: true, value: { reply: 'Real vendor reply', messageId: 'user-1' } });
    expect(await provider.contextMessage(input)).toEqual(first);expect(await provider.contextMessage({...input,text:'Changed'})).toMatchObject({ok:false,error:{code:'POLICY_DENIED'}});
    const mutations = requests.filter((request) => request.body);
    expect(mutations).toHaveLength(1);
    expect(mutations[0].path).toBe('/api/bots/bot-1/messages/guarded');
    expect(mutations[0].body).toEqual({ threadId: 'thread-1', sendId: 'send-1', text: 'Hello', expectedActiveLeafId: null, expectedApprovalMode: 'ask' });
    expect(await provider.events({ contextId })).toMatchObject({ ok: true, value: { events: [{ event: { type: 'agent.message.delta' } }, { event: { type: 'agent.message.completed' } }] } });
    expect(await provider.events({ contextId, cursor: '2' })).toMatchObject({ ok: true, value: { events: [], nextCursor: '2' } });
  });

  it('isolates contexts and credentials between account bindings', async () => {
    const { provider, requests } = fixture();
    const contextId = await runWithCallScope({ binding: { id: 'binding-a', accountBindingId: 'account-a' }, credential: { apiKey: 'paired-a' } }, () => open(provider));
    const result = await runWithCallScope({ binding: { id: 'binding-b', accountBindingId: 'account-b' }, credential: { apiKey: 'paired-b' } }, () => provider.contextMessage({ contextId, correlationId: 'send-1', text: 'Wrong account' }));
    expect(result).toMatchObject({ ok: false, error: { code: 'CONTEXT_NOT_FOUND' } });
    expect(requests.every((request) => request.authorization === 'Bearer paired-a')).toBe(true);
  });

  it('retains uncertain send results, never retries mutations and never exposes credentials', async () => {
    const { provider, requests } = fixture({ lostSend: true });
    const contextId = await open(provider);
    const input = { contextId, correlationId: 'send-1', text: 'Hello' };
    const result = await provider.contextMessage(input);
    expect(result).toMatchObject({ ok: false, error: { retryable: false } });
    expect(JSON.stringify(result)).not.toContain('secret-token');
    expect(await provider.contextMessage(input)).toEqual(result);
    expect(requests.filter((request) => request.body)).toHaveLength(1);
  });

  it('refuses schema drift and upstream approvals without using unguarded sends', async () => {
    const drift = fixture({ drift: true });
    expect(await drift.provider.contextOpen({ agentId: 'openmausbot:bot-1' })).toMatchObject({ ok: false });
    expect(drift.requests.some((request) => request.body)).toBe(false);
    const permission = fixture({ permission: true });
    const contextId = await open(permission.provider);
    expect(await permission.provider.contextMessage({ contextId, correlationId: 'send-1', text: 'Hello' })).toMatchObject({ ok: false, error: { code: 'APPROVAL_REQUIRED' } });
    expect(permission.requests.filter((request) => request.body)).toHaveLength(1);
  });

  it('maps revoked auth and detaches without stopping a shared vendor conversation', async () => {
    expect(await fixture({ status: 403 }).provider.list()).toMatchObject({ ok: false, error: { code: 'AUTH_REVOKED' } });
    const { provider, requests } = fixture();
    const contextId = await open(provider);
    expect(await provider.contextClose({ contextId })).toMatchObject({ ok: true });
    expect(requests.some((request) => request.body)).toBe(false);
    expect(await provider.contextMessage({ contextId, correlationId: 'send-1', text: 'Hello' })).toMatchObject({ ok: false, error: { code: 'CONTEXT_NOT_FOUND' } });
  });

  it('forwards only a human decision for an approval owned by this request and context', async () => {
    const { provider, requests } = fixture({ permission: true });
    const contextId = await open(provider);
    await provider.contextMessage({ contextId, correlationId: 'send-1', text: 'Hello' });
    expect(await provider.events({ contextId })).toMatchObject({ ok: true, value: { events: [{ event: { type: 'agent.approval.requested', payload: { approvalId: 'permission-1' } } }] } });
    expect(await provider.approvals({ contextId, op: 'list' })).toMatchObject({ ok: true, value: { approvals: [{ remoteRef: 'permission-1', state: 'pending' }] } });
    expect(await provider.approvals({ contextId, op: 'respond', approvalId: 'permission-1', decision: 'approve', actor: { type: 'system', id: 'automation' } })).toMatchObject({ ok: false, error: { code: 'POLICY_DENIED' } });
    expect(await provider.approvals({ contextId, op: 'respond', approvalId: 'other-request', decision: 'approve', actor: { type: 'human', id: 'user' } })).toMatchObject({ ok: false });
    expect(await provider.approvals({ contextId, op: 'respond', approvalId: 'permission-1', decision: 'approve', actor: { type: 'human', id: 'user' } })).toMatchObject({ ok: true, value: { resolved: { state: 'approved' } } });
    expect(requests.filter((request) => request.path.endsWith('/respond'))).toMatchObject([{ body: { requestId: 'permission-1', behavior: 'allow' } }]);
  });
});
