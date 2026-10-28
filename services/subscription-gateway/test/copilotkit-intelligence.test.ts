import { describe, expect, it } from 'vitest';
import { runIntelligence, intelligenceReceipt } from '../adapters/_shared/copilotkit-intelligence.js';
import { OpenBotCopilotkitProvider } from '../adapters/openbot-copilotkit/provider.js';
import { runWithCallScope } from '../src/aai/call-scope.js';
import { fakePhoenix, replyEvents } from './fake-phoenix.js';

const receipt = (over: Record<string, unknown> = {}) => ({ threadId: 't-1', runId: 'canon-run', joinToken: 'jt', realtime: { clientUrl: 'wss://rt.example/socket', topic: 'thread:t-1' }, ...over });
const run = (phoenix: ReturnType<typeof fakePhoenix>, response: Response | (() => Response) = Response.json(receipt()), extra = {}) => {
  const bodies: unknown[] = [];
  return { bodies, result: runIntelligence({ runtimeUrl: 'http://127.0.0.1:4310/api/copilotkit', agentId: 'bot one', threadId: 't-1', runId: 'input-run', messages: [{ id: 'u1', role: 'user', content: 'Hi' }], headers: { authorization: 'Bearer x' }, socket: phoenix.factory, timeoutMs: 200,
    fetch: (async (url: string, init: RequestInit) => { bodies.push({ url, body: JSON.parse(String(init.body)), headers: init.headers, redirect: init.redirect }); return typeof response === 'function' ? response() : response; }) as typeof fetch, ...extra }) };
};

describe('own CopilotKit Intelligence client (wire-compatible with @copilotkit/core 1.75.0 IntelligenceAgent run mode)', () => {
  it('posts RunAgentInput to /agent/:id/run, joins the receipt topic with the canonical run id, and folds ag_ui_event pushes', async () => {
    const phoenix = fakePhoenix({ events: replyEvents('t-1', 'Hello back') });
    const { bodies, result } = run(phoenix);
    const r = await result;
    expect(r).toMatchObject({ finished: true, identityMismatch: false, messages: [{ id: 'answer', role: 'assistant', content: 'Hello back' }], receipt: { runId: 'canon-run' } });
    expect(r.transport).toBeUndefined();
    expect(bodies[0]).toMatchObject({ url: 'http://127.0.0.1:4310/api/copilotkit/agent/bot%20one/run', redirect: 'error', headers: { 'Content-Type': 'application/json', authorization: 'Bearer x' }, body: { threadId: 't-1', runId: 'input-run', tools: [], context: [], forwardedProps: {}, state: {}, messages: [{ id: 'u1', role: 'user', content: 'Hi' }] } });
    expect(phoenix.urls).toEqual(['wss://rt.example/socket/websocket?join_token=jt&vsn=2.0.0']);
    expect(phoenix.sent[0]).toEqual(['1', '1', 'thread:t-1', 'phx_join', { stream_mode: 'run', run_id: 'canon-run' }]);
    expect(phoenix.sent.at(-1)).toMatchObject(['1', expect.any(String), 'thread:t-1', 'phx_leave', {}]);
    expect(phoenix.closed()).toBe(1);
  });
  it('mirrors the SDK receipt rules: join token, client URL and topic are required; thread and run fall back to the request', () => {
    expect(intelligenceReceipt({ realtime: { clientUrl: 'wss://a', topic: 't' } }, { threadId: 't', runId: 'r' })).toBeNull();
    expect(intelligenceReceipt({ joinToken: 'j', realtime: { clientUrl: 'wss://a' } }, { threadId: 't', runId: 'r' })).toBeNull();
    expect(intelligenceReceipt({ joinToken: 'j', realtime: { clientUrl: 'wss://a', topic: 'x' } }, { threadId: 't', runId: 'r' })).toEqual({ threadId: 't', runId: 'r', joinToken: 'j', realtime: { clientUrl: 'wss://a', topic: 'x' } });
  });
  it('reports HTTP failures, a locked thread and a bad receipt without opening a socket', async () => {
    for (const [response, expected] of [[new Response('no', { status: 401 }), { status: 401 }], [new Response('busy', { status: 409 }), { status: 409, transport: 'locked' }], [Response.json({ threadId: 't-1' }), { transport: 'receipt' }]] as const) {
      const phoenix = fakePhoenix();
      expect(await run(phoenix, response).result).toMatchObject(expected);
      expect(phoenix.urls).toHaveLength(0);
    }
  });
  it('refuses events for another run, a refused join, a dropped socket and a silent run, and never retries', async () => {
    const cases = [
      [fakePhoenix({ events: replyEvents('t-1', 'x', { runId: 'other-run' }) }), { identityMismatch: true, finished: false }],
      [fakePhoenix({ joinStatus: 'error' }), { transport: 'join' }],
      [fakePhoenix({ dropAfterJoin: true }), { transport: 'closed' }],
      [fakePhoenix({ events: () => [{ type: 'RUN_STARTED', threadId: 't-1', runId: 'canon-run' }] }), { transport: 'timeout', started: true, finished: false }],
      [fakePhoenix({ events: (_t, runId) => [{ type: 'RUN_ERROR', message: 'boom' }, { type: 'RUN_FINISHED', threadId: 't-1', runId }] }), { error: { message: 'boom' }, finished: false }]
    ] as const;
    for (const [phoenix, expected] of cases) {
      const { bodies, result } = run(phoenix);
      expect(await result).toMatchObject(expected);
      expect(bodies).toHaveLength(1); expect(phoenix.urls).toHaveLength(1); expect(phoenix.closed()).toBe(1);
    }
  });
  it('times out a join the server never answers', async () => {
    const phoenix = fakePhoenix({ joinStatus: 'silent' });
    expect(await run(phoenix, undefined, { joinTimeoutMs: 20 }).result).toMatchObject({ transport: 'join' });
  });
});

describe('OpenBot CopilotKit adapter over the own Intelligence client', () => {
  const thread = '11111111-2222-3333-4444-555555555555';
  const setup = (clientUrl = 'wss://realtime.intelligence.copilotkit.ai/socket') => {
    const calls: { path: string; cookie: string | null }[] = [];
    const fetcher = (async (url: string, init: RequestInit) => {
      const path = new URL(url).pathname; calls.push({ path, cookie: new Headers(init.headers).get('cookie') });
      if (path === '/api/agents') return Response.json({ agents: [{ id: 'bot-1', name: 'Bot' }] });
      if (path === '/api/threads/mint') return Response.json({ threadId: thread });
      if (path === '/api/copilotkit/info') return Response.json({ mode: 'intelligence', agents: { 'bot-1': {} }, intelligence: { wsUrl: 'wss://realtime.intelligence.copilotkit.ai/socket' } });
      return Response.json({ threadId: thread, runId: 'run-9', joinToken: 'jt', realtime: { clientUrl, topic: `thread:${thread}` } });
    }) as typeof fetch;
    const phoenix = fakePhoenix({ events: replyEvents(thread, 'OpenBot reply') });
    return { calls, phoenix, provider: new OpenBotCopilotkitProvider({ fetch: fetcher, socket: phoenix.factory, replyTimeoutMs: 200 }) };
  };
  const scope = { binding: { id: 'acct' }, credential: { apiKey: JSON.stringify({ origin: 'https://bot.example', sessionToken: 'session-token-1' }) } };
  it('sends through the default wss realtime origin with the session cookie and returns the run reply', async () => {
    const { provider, phoenix, calls } = setup();
    const r = await runWithCallScope(scope, async () => { const o = await provider.contextOpen({ agentId: 'openbot-copilotkit:bot-1' }); if (!o.ok) throw Error(o.error.humanMessage); return provider.contextMessage({ contextId: o.value.contextId, correlationId: 'send-1', text: 'Hi' }); });
    expect(r).toMatchObject({ ok: true, value: { reply: 'OpenBot reply' } });
    expect(calls.find(c => c.path === '/api/copilotkit/agent/bot-1/run')?.cookie).toBe('__Secure-better-auth.session_token=session-token-1');
    expect(phoenix.sent[0]).toEqual(['1', '1', `thread:${thread}`, 'phx_join', { stream_mode: 'run', run_id: 'run-9' }]);
  });
  it('refuses an unapproved realtime endpoint in the receipt', async () => {
    const { provider, phoenix } = setup('wss://evil.example/socket');
    const r = await runWithCallScope(scope, async () => { const o = await provider.contextOpen({ agentId: 'openbot-copilotkit:bot-1' }); if (!o.ok) throw Error(o.error.humanMessage); return provider.contextMessage({ contextId: o.value.contextId, correlationId: 'send-1', text: 'Hi' }); });
    expect(r).toMatchObject({ ok: false, error: { code: 'POLICY_DENIED' } });
    expect(phoenix.urls).toHaveLength(0);
  });
});
