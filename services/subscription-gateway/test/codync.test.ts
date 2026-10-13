import { describe, expect, it } from 'vitest';
import { CodyncProvider } from '../adapters/codync/provider.js';
import { runWithCallScope } from '../src/aai/call-scope.js';

function fixture(options: { permission?: string; pending?: boolean; status?: number; incomplete?: boolean; foreign?: boolean } = {}) {
  const requests: Array<{ path: string; body?: Record<string, unknown> }> = [];
  let sent = false; let selected: string | null = null;
  const entry = (id: string, kind: string, seq: number, data: object, turn = 1) => ({ id, botId: options.foreign ? 'foreign-bot' : 'bot-1', kind, seq, rev: seq, turn, threadId: null, data });
  const fetcher = async (input: string | URL | Request, init?: RequestInit) => {
    const path = new URL(String(input)).pathname; const body = init?.body ? JSON.parse(String(init.body)) : undefined;
    requests.push({ path, body });
    expect(new Headers(init?.headers).get('authorization')).toMatch(/^Bearer /);
    expect(init?.redirect).toBe('error');
    if (options.status) return Response.json({}, { status: options.status });
    if (path === '/events') {
      const events = [{ type: 'hello', hostId: 'host-1', rev: options.incomplete ? 2 : 1 }, { type: 'bot', rev: 1, bot: { id: 'bot-1', name: 'Codync agent', kind: 'agent', rev: 1, deleted: false, status: 'idle', permission: options.permission ?? 'ask' } }];
      const text = events.map((event) => `data: ${JSON.stringify(event)}\r\n\r\n`).join('');
      const bytes = new TextEncoder().encode(text);
      return new Response(new ReadableStream({ start(controller) { controller.enqueue(bytes.slice(0, 11)); controller.enqueue(bytes.slice(11)); controller.close(); } }), { headers: { 'content-type': 'text/event-stream' } });
    }
    if (path === '/api/send') { sent = true; return Response.json({ entry: entry('user-1', 'user', 2, { text: body.text, clientNonce: body.clientNonce }) }); }
    if (path === '/api/respondPermission') { selected = String(body.optionId); return Response.json({}); }
    if (path === '/api/history') return Response.json({ entries: sent ? [
      entry('user-1', 'user', 2, { text: 'Hello', clientNonce: 'send-1' }),
      options.pending ? entry('permission-1', 'permission', 3, { title: 'Run command', status: selected ? 'answered' : 'pending', selected, options: [{ optionId: 'one-time', kind: 'allow_once' }, { optionId: 'reject', kind: 'reject_once' }, { optionId: 'persistent', kind: 'allow_always' }] }) : entry('reply-1', 'agent', 3, { text: 'Codync reply', final: true }),
      entry('other-turn', 'agent', 4, { text: 'Unrelated reply', final: true }, 999),
    ] : [] });
    if (path === '/api/hello') return Response.json({ hostId: 'host-1' });
    return Response.json({}, { status: 404 });
  };
  return { requests, provider: new CodyncProvider({ token: 'codync-secret', fetch: fetcher as typeof fetch, pollMs: 1, replyTimeoutMs: 25 }) };
}
async function open(provider: CodyncProvider) {
  const result = await provider.contextOpen({ agentId: 'codync:bot-1' });
  expect(result.ok).toBe(true); if (!result.ok) throw new Error('open failed'); return result.value.contextId;
}

describe('Codync native host connector', () => {
  it('requires authenticated loopback access and waits for a complete revision snapshot', async () => {
    expect(await new CodyncProvider().list()).toMatchObject({ ok: false, error: { code: 'AUTH_REQUIRED' } });
    expect(() => new CodyncProvider({ baseUrl: 'https://relay.example' })).toThrow(/loopback/);
    expect(await fixture().provider.list()).toMatchObject({ ok: true, value: [{ agentId: 'codync:bot-1', displayName: 'Codync agent' }] });
    expect(await fixture({ incomplete: true }).provider.list()).toMatchObject({ ok: false, error: { code: 'ADAPTER_DRIFT' } });
    expect(await fixture({ status: 401 }).provider.list()).toMatchObject({ ok: false, error: { code: 'AUTH_REVOKED' } });
  });
  it('refuses automatic permissions and never silently changes the vendor bot configuration', async () => {
    const { provider, requests } = fixture({ permission: 'auto' });
    expect(await provider.contextOpen({ agentId: 'codync:bot-1' })).toMatchObject({ ok: false, error: { code: 'POLICY_DENIED' } });
    expect(requests.every((request) => !request.body)).toBe(true);
  });
  it('sends once using the native client nonce and returns only the matching turn', async () => {
    const { provider, requests } = fixture(); const contextId = await open(provider);
    const input = { contextId, correlationId: 'send-1', text: 'Hello' };
    const result = await provider.contextMessage(input);
    expect(result).toMatchObject({ ok: true, value: { reply: 'Codync reply' } });
    expect(await provider.contextMessage(input)).toEqual(result);
    expect(requests.filter((request) => request.path === '/api/send')).toEqual([{ path: '/api/send', body: { botId: 'bot-1', text: 'Hello', clientNonce: 'send-1' } }]);
    const events = await provider.events({ contextId });
    expect(JSON.stringify(events)).not.toContain('Unrelated reply');
    expect(events).toMatchObject({ ok: true, value: { events: [{ event: { type: 'agent.message.delta', correlationId: 'send-1' } }, { event: { type: 'agent.message.completed' } }] } });
  });
  it('enforces account ownership and rejects foreign history instead of leaking it', async () => {
    const { provider } = fixture();
    const contextId = await runWithCallScope({ binding: { id: 'a', accountBindingId: 'account-a' } }, () => open(provider));
    expect(await runWithCallScope({ binding: { id: 'b', accountBindingId: 'account-b' } }, () => provider.contextMessage({ contextId, correlationId: 'send-1', text: 'Hello' }))).toMatchObject({ ok: false, error: { code: 'CONTEXT_NOT_FOUND' } });
    const foreign = fixture({ foreign: true }); const other = await open(foreign.provider);
    expect(await foreign.provider.contextMessage({ contextId: other, correlationId: 'send-1', text: 'Hello' })).toMatchObject({ ok: false, error: { code: 'ADAPTER_DRIFT' } });
  });
  it('forwards human one-time approvals, never selects a persistent allow and never accepts automation', async () => {
    const { provider, requests } = fixture({ pending: true }); const contextId = await open(provider);
    expect(await provider.contextMessage({ contextId, correlationId: 'send-1', text: 'Hello' })).toMatchObject({ ok: false, error: { code: 'APPROVAL_REQUIRED' } });
    expect(await provider.events({ contextId })).toMatchObject({ ok: true, value: { events: [{ event: { type: 'agent.approval.requested', payload: { approvalId: 'permission-1' } } }] } });
    expect(await provider.approvals({ contextId, op: 'respond', approvalId: 'permission-1', decision: 'approve', actor: { type: 'system', id: 'bot' } })).toMatchObject({ ok: false, error: { code: 'POLICY_DENIED' } });
    expect(await provider.approvals({ contextId, op: 'respond', approvalId: 'permission-1', decision: 'approve', actor: { type: 'human', id: 'owner' } })).toMatchObject({ ok: true, value: { resolved: { state: 'approved' } } });
    expect(requests.filter((request) => request.path === '/api/respondPermission')).toEqual([{ path: '/api/respondPermission', body: { entryId: 'permission-1', optionId: 'one-time' } }]);
  });
  it('detaches without stopping the shared Codync agent', async () => {
    const { provider, requests } = fixture(); const contextId = await open(provider);
    expect(await provider.contextClose({ contextId })).toMatchObject({ ok: true });
    expect(requests.some((request) => request.path === '/api/stop')).toBe(false);
  });
});
