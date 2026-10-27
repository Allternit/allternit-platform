import { describe, expect, it } from 'vitest';
import { AgUiProvider } from '../adapters/ag-ui/provider.js';
import { runWithCallScope } from '../src/aai/call-scope.js';

const sse = (events: unknown[]) => new Response(new ReadableStream({ start(c) { const enc = new TextEncoder(); for (const e of events) { const text = `data: ${JSON.stringify(e)}\r\n\r\n`; c.enqueue(enc.encode(text.slice(0, 7))); c.enqueue(enc.encode(text.slice(7))); } c.close(); } }), { headers: { 'content-type': 'text/event-stream; charset=utf-8' } });
function fixture(credential: object, reply: (body: any) => unknown[]) {
 const posts: { url: string; body: any; auth: string | null; accept: string | null }[] = [];
 const fetcher = (async (url: any, init: any) => {
  if (init?.method !== 'POST') return new Response(null, { status: 405 });
  const body = JSON.parse(String(init.body)); const h = new Headers(init.headers);
  posts.push({ url: String(url), body, auth: h.get('authorization'), accept: h.get('accept') });
  return sse(reply(body));
 }) as typeof fetch;
 const provider = new AgUiProvider({ fetch: fetcher, resolve: async () => [{ address: '93.184.216.34', family: 4 }] });
 const call = <T,>(fn: () => Promise<T>) => runWithCallScope({ binding: { id: 'acct-1' }, credential: { apiKey: JSON.stringify(credential) } }, fn);
 return { provider, posts, call };
}

describe('Any AG-UI agent adapter', () => {
 it('runs a turn, assembles streamed text per message and keeps history for the next turn', async () => {
  const { provider, posts, call } = fixture({ url: 'http://127.0.0.1:8000/agent', token: 'tok-1' }, (b) => [
   { type: 'RUN_STARTED', threadId: b.threadId, runId: b.runId },
   { type: 'STEP_STARTED', stepName: 'think' }, { type: 'SOMETHING_NEW', anything: 1 },
   { type: 'TEXT_MESSAGE_START', messageId: 'm1', role: 'assistant' }, { type: 'TEXT_MESSAGE_CONTENT', messageId: 'm1', delta: 'Hel' }, { type: 'TEXT_MESSAGE_CONTENT', messageId: 'm1', delta: 'lo' }, { type: 'TEXT_MESSAGE_END', messageId: 'm1' },
   { type: 'TEXT_MESSAGE_CHUNK', messageId: 'm2', delta: 'Bye' }, { type: 'TEXT_MESSAGE_CHUNK', delta: '!' },
   { type: 'RUN_FINISHED', threadId: b.threadId, runId: b.runId, outcome: { type: 'success' } },
  ]);
  const roster = await call(() => provider.list());
  expect(roster).toMatchObject({ ok: true, value: [{ agentId: 'ag-ui:127.0.0.1:8000', displayName: '127.0.0.1' }] });
  const opened = await call(() => provider.contextOpen({ agentId: 'ag-ui:127.0.0.1:8000' }));
  if (!opened.ok) throw Error(opened.error.humanMessage);
  const contextId = opened.value.contextId;
  expect(await call(() => provider.contextMessage({ contextId, correlationId: 's1', text: 'Hi' }))).toMatchObject({ ok: true, value: { reply: 'Hello\n\nBye!' } });
  expect(posts[0]).toMatchObject({ url: 'http://127.0.0.1:8000/agent', auth: 'Bearer tok-1', accept: 'text/event-stream', body: { tools: [], context: [], forwardedProps: {}, messages: [{ role: 'user', content: 'Hi' }] } });
  await call(() => provider.contextMessage({ contextId, correlationId: 's2', text: 'Again' }));
  expect(posts[1].body.threadId).toBe(posts[0].body.threadId);
  expect(posts[1].body.runId).not.toBe(posts[0].body.runId);
  expect(posts[1].body.messages.map((m: any) => m.content)).toEqual(['Hi', 'Hello', 'Bye!', 'Again']);
  expect(await call(() => provider.events({ contextId }))).toMatchObject({ ok: true, value: { events: [{ event: { payload: { reply: 'Hello\n\nBye!' } } }, {}] } });
 });

 it('refuses a reply whose run events name another thread or run', async () => {
  const { provider, call } = fixture({ url: 'https://agent.example.com/run' }, (b) => [
   { type: 'RUN_STARTED', threadId: b.threadId, runId: 'someone-elses-run' },
   { type: 'TEXT_MESSAGE_CHUNK', messageId: 'm1', delta: 'not yours' },
   { type: 'RUN_FINISHED', threadId: b.threadId, runId: 'someone-elses-run' },
  ]);
  const opened = await call(() => provider.contextOpen({ agentId: 'ag-ui:agent.example.com' }));
  if (!opened.ok) throw Error(opened.error.humanMessage);
  expect(await call(() => provider.contextMessage({ contextId: opened.value.contextId, correlationId: 's1', text: 'Hi' }))).toMatchObject({ ok: false, error: { code: 'ADAPTER_DRIFT' } });
 });

 it('refuses private, metadata and non-https endpoints before any request', async () => {
  let requests = 0;
  const provider = new AgUiProvider({ fetch: (async () => { requests++; return new Response(null, { status: 405 }); }) as typeof fetch, resolve: async (host) => host === 'rebind.example.com' ? [{ address: '8.8.8.8', family: 4 }, { address: '10.0.0.5', family: 4 }] : [{ address: '100.64.1.2', family: 4 }] });
  const list = (url: string) => runWithCallScope({ binding: { id: 'a' }, credential: { apiKey: JSON.stringify({ url }) } }, () => provider.list());
  for (const url of ['https://rebind.example.com/run', 'https://cgnat.example.com/run', 'https://169.254.169.254/latest', 'https://[fd00::1]/run', 'https://[::ffff:192.168.1.1]/run'])
   expect(await list(url)).toMatchObject({ ok: false, error: { code: 'POLICY_DENIED' } });
  for (const url of ['http://agent.example.com/run', 'https://user:pw@agent.example.com/run', 'https://agent.example.com/run#x'])
   expect(await list(url)).toMatchObject({ ok: false, error: { code: 'POLICY_DENIED' } });
  expect(requests).toBe(0);
 });
});
