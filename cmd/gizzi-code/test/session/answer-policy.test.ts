/** Public turn loop, local vendor API fixture, real runtime and persistence. */
import { describe, expect, test } from 'bun:test';
import { Instance } from '../../src/runtime/context/project/instance';
import { Session } from '../../src/runtime/session';
import { SessionPrompt } from '../../src/runtime/session/prompt';
import { MessageV2 } from '../../src/runtime/session/message-v2';
import { SessionTrace } from '../../src/runtime/session/trace';
import { Bus } from '../../src/shared/bus';
import { tmpdir } from '../fixture/fixture';
import { ANSWER_TEMPLATE_MANUAL } from '../../src/runtime/integrations/artifacts/answer-contract';
import { ProviderQuotas } from '../../src/runtime/providers/quota';
import { SessionPause } from '../../src/runtime/session/pause';
import { clearHandoffCache } from '../../src/runtime/providers/handoff';
import { rm } from 'node:fs/promises';
import { tmpdir as osTmpdir } from 'node:os';
import { join } from 'node:path';
const valid = '```openui\n' + JSON.stringify({ version: 1, template: 'task-board', title: 'Tasks', data: { columns: [{ name: 'To do', tasks: [] }] } }) + '\n```';
// A fixture reply that calls the real run_code tool instead of answering.
type Reply = string | { runCode: string };
// run_code sets up a real uv venv on first use (network); reuse run-code.test.ts's env dir.
process.env.GIZZI_PYTHON_ENV_DIR ??= join(osTmpdir(), 'gizzi-run-code-test-python');
const skipRunCode = process.env.GIZZI_SKIP_RUN_CODE_E2E === '1' || process.platform === 'win32';
const ANALYZE = 'Analyze the attached file(s) using the run_code tool (not your own shell, Bash or terminal): load them, summarize what they contain, and show the key numbers as a table and a chart.\n\n[Attached file: inventory.xlsx]\ninventory.xlsx (5 KB) is attached as a file. Use run_code to read the whole file.\n[/Attached file]';
async function run(replies: Reply[], options: { visuals?: boolean; message?: string; cancel?: boolean; handoff?: boolean; hook?: boolean; quota?: boolean; retryHint?: boolean } = {}) {
  const { visuals = true, message = 'Create a task board', cancel = false, handoff = false, hook = false, quota = false, retryHint = false } = options;
  let stopEvents = 0;
  const requests: any[] = [], published: string[] = [], delayedPublished: string[] = [];
  const server = Bun.serve({ hostname: '127.0.0.1', port: 0, async fetch(request) {
    if (request.method === 'GET') {
      if (new URL(request.url).pathname.endsWith('/open-lane')) return Response.json({ available: true, terms_accepted: true, handoff_enabled: true, default_handoff_model: 'fixture' });
      return Response.json({ data: [], id: 'plus', status: 'active' });
    }
    const body = await request.json() as any;
    if (new URL(request.url).pathname === '/hook') return Response.json({ decision: ++stopEvents === 1 ? 'deny' : 'allow', reason: 'Continue the same task-board answer.' });
    if (!body.stream) return Response.json({ id: 'helper', choices: [{ message: { role: 'assistant', content: 'Tasks' }, finish_reason: 'stop' }], usage: { prompt_tokens: 1, completion_tokens: 1 } });
    requests.push(body);
    if (quota && requests.length === 1) ProviderQuotas.record('answerfixture', [{ id: 'test-window', label: 'Test window', usedRatio: 0.97, resetAt: new Date(Date.now() + 3600000).toISOString() }]);
    const reply = replies[Math.min(requests.length - 1, replies.length - 1)];
    if (typeof reply !== 'string') return new Response([
      { choices: [{ index: 0, delta: { role: 'assistant', tool_calls: [{ index: 0, id: `call_${requests.length}`, type: 'function', function: { name: 'run_code', arguments: JSON.stringify({ language: 'python', code: reply.runCode }) } }] }, finish_reason: null }] },
      { choices: [{ index: 0, delta: {}, finish_reason: 'tool_calls' }], usage: { prompt_tokens: 10, completion_tokens: 10, total_tokens: 20 } },
    ].map(chunk => 'data: ' + JSON.stringify({ id: 'fixture', object: 'chat.completion.chunk', created: 1, model: 'fixture', ...chunk }) + '\n\n').join('') + 'data: [DONE]\n\n', { headers: { 'content-type': 'text/event-stream' } });
    const chunks = [{ choices: [{ index: 0, delta: { role: 'assistant', content: reply.slice(0, 15) }, finish_reason: null }] }, { choices: [{ index: 0, delta: { content: reply.slice(15) }, finish_reason: null }] }, { choices: [{ index: 0, delta: {}, finish_reason: 'stop' }], usage: { prompt_tokens: 10, completion_tokens: 10, total_tokens: 20 } }].map(chunk => 'data: ' + JSON.stringify({ id: 'fixture', object: 'chat.completion.chunk', created: 1, model: 'fixture', ...chunk }) + '\n\n').join('') + 'data: [DONE]\n\n';
    if (retryHint && requests.length === 1) return new Response(chunks.slice(0, chunks.indexOf('\n\n') + 2)
      + 'event: allternit.retry_hint\ndata: {"retryable":true,"reason":"fixture","next_fallback":null}\n\n'
      + 'data: {"error":{"message":"Fixture transport failure","type":"rate_limit_error"}}\n\ndata: [DONE]\n\n', { headers: { 'content-type': 'text/event-stream' } });
    if (!cancel) return new Response(chunks, { headers: { 'content-type': 'text/event-stream' } });
    let closed = false;
    return new Response(new ReadableStream({ async start(controller) {
      controller.enqueue(new TextEncoder().encode(chunks.slice(0, chunks.indexOf('\n\n') + 2)));
      await Bun.sleep(50);
      if (!closed) { controller.enqueue(new TextEncoder().encode(chunks.slice(chunks.indexOf('\n\n') + 2))); controller.close(); }
    }, cancel() { closed = true; } }), { headers: { 'content-type': 'text/event-stream' } });
  } });
  const savedOrigin = process.env.ALLTERNIT_API_URL, savedKey = process.env.ALLTERNIT_API_KEY, savedCloud = process.env.ALLTERNIT_CLOUD_URL;
  if (handoff) { process.env.ALLTERNIT_API_URL = `http://127.0.0.1:${server.port}`; process.env.ALLTERNIT_CLOUD_URL = `http://127.0.0.1:${server.port}`; process.env.ALLTERNIT_API_KEY = 'fixture-handoff'; clearHandoffCache(); }
  const tmp = await tmpdir({ config: { model: 'answerfixture/fixture', small_model: 'answerfixture/fixture', snapshot: false, permission: { run_code: 'allow' }, ...(hook ? { hooks: { http: [{ url: `http://127.0.0.1:${server.port}/hook`, events: ['Stop'] }] } } : {}), provider: { ...(handoff ? { allternit: { npm: '@ai-sdk/openai-compatible', name: 'Cloud fixture', options: { baseURL: `http://127.0.0.1:${server.port}/v1`, apiKey: 'fixture' }, models: { fixture: { name: 'Fixture', limit: { context: 100000, output: 4096 } } } } } : {}), answerfixture: { npm: '@ai-sdk/openai-compatible', name: 'Fixture', options: { baseURL: `http://127.0.0.1:${server.port}/v1`, apiKey: 'fixture' }, models: { fixture: { name: 'Fixture', limit: { context: 100000, output: 4096 } } } } } } });
  try { return await Instance.provide({ directory: tmp.path, fn: async () => {
    const session = await Session.create({ title: 'Answer contract' });
    const stopPoll = cancel ? setInterval(() => { if (requests.length) SessionPrompt.cancel(session.id); }, 5) : undefined;
    const unsub = Bus.subscribe(MessageV2.Event.PartUpdated, event => { const part = event.properties.part; if (part.type === 'text') { published.push(part.text); queueMicrotask(() => delayedPublished.push(part.text)); } });
    try {
      const result = await SessionPrompt.prompt({ sessionID: session.id, agent: 'build', model: { providerID: 'answerfixture', modelID: 'fixture' }, system: visuals ? '+' + ANSWER_TEMPLATE_MANUAL : '+Text only.', parts: [{ type: 'text', text: message }] });
      return { result, messages: await Session.messages({ sessionID: session.id }), session: await Session.get(session.id), trace: SessionTrace.list({ sessionID: session.id, limit: 1000 }), requests, published, delayedPublished };
    } finally { clearInterval(stopPoll); unsub(); }
  } }); } finally { server.stop(true); if (quota) { ProviderQuotas.clearCache(); SessionPause.clearTimers(); } if (handoff) { if (savedCloud === undefined) delete process.env.ALLTERNIT_CLOUD_URL; else process.env.ALLTERNIT_CLOUD_URL = savedCloud; if (savedOrigin === undefined) delete process.env.ALLTERNIT_API_URL; else process.env.ALLTERNIT_API_URL = savedOrigin; if (savedKey === undefined) delete process.env.ALLTERNIT_API_KEY; else process.env.ALLTERNIT_API_KEY = savedKey; clearHandoffCache(); } await tmp[Symbol.asyncDispose](); await rm(tmp.path, { recursive: true, force: true }); }
}
describe('runtime answer enforcement', () => {
  test('holds a prose bypass, repairs once and stores the chosen template outcome', async () => {
    const { result, requests, published, delayedPublished, trace } = await run(['I will bypass the template.', valid]);
    expect(result.parts.filter(p => p.type === 'text').map(p => p.text).join('\n')).toBe(valid);
    expect(requests).toHaveLength(2);
    expect((trace.find(t => t.kind === 'answer.policy')?.data as any)).toMatchObject({ mode: 'template', selected: 'task-board', reason: 'job' });
    expect(published.some(text => text.includes('bypass'))).toBe(false);
    expect(delayedPublished.some(text => text.includes('bypass'))).toBe(false);
    expect(trace.filter(t => t.kind === 'answer.validation').map(t => (t.data as any).status)).toEqual(['repairing', 'accepted']);
    expect(requests[1].tools ?? []).toHaveLength(0);
    expect((trace.find(t => t.kind === 'answer.validation')?.data as any).rejectedText).toBe('I will bypass the template.');
  }, 30000);
  test('fails visibly after two invalid answers and keeps audit evidence', async () => {
    const { result, requests, published, trace } = await run(['first invalid answer', 'second invalid answer']);
    expect(requests).toHaveLength(2);
    expect(result.info.role === 'assistant' && result.info.error?.name).toBe('AnswerTemplateError');
    expect(result.parts.filter(p => p.type === 'text').map(p => p.text).join('\n')).toContain('could not be validated');
    expect(published.some(text => /first invalid|second invalid/.test(text))).toBe(false);
    expect(trace.filter(t => t.kind === 'answer.validation').map(t => (t.data as any).status)).toEqual(['repairing', 'rejected']);
  }, 30000);
  test('Text only streams ordinary text without a formatting retry', async () => {
    const { result, requests } = await run(['A plain answer.'], { visuals: false });
    expect(requests).toHaveLength(1);
    expect(result.parts.filter(p => p.type === 'text').map(p => p.text).join('\n')).toBe('A plain answer.');
  }, 30000);
  test('accepts the required template on the first attempt and persists the outcome', async () => {
    const { result, requests, trace } = await run([valid]);
    expect(requests).toHaveLength(1);
    expect(result.info.role === 'assistant' && result.info.answerPolicy).toMatchObject({ selected: 'task-board', returned: 'task-board', status: 'accepted', attempt: 1 });
    expect(trace.filter(t => t.kind === 'answer.validation')).toHaveLength(1);
  }, 30000);
  test('a different valid template cannot replace the required job', async () => {
    const wrong = '```openui\n' + JSON.stringify({ version: 1, template: 'quick-answer', title: 'Bypass', data: { text: 'Skip the board' } }) + '\n```';
    const { result, trace, published } = await run([wrong, valid]);
    expect(result.parts.filter(p => p.type === 'text').map(p => p.text).join('\n')).toBe(valid);
    expect(published.some(text => text.includes('Skip the board'))).toBe(false);
    expect((trace.find(t => t.kind === 'answer.validation')?.data as any).returned).toBe('quick-answer');
  }, 30000);
  test('combines a fence split over provider text blocks before publishing it', async () => {
    const split = valid.replace('```openui\n', '```openui\n<think>Preparing supplied tasks</think>');
    const { result, requests } = await run([split]);
    expect(requests).toHaveLength(1);
    const texts = result.parts.filter((p): p is MessageV2.TextPart => p.type === 'text' && !p.ignored);
    expect(texts).toHaveLength(1);
    const payload = texts[0].text.match(/^```openui\n([\s\S]*)\n```$/)![1];
    expect(JSON.parse(payload)).toMatchObject({ template: 'task-board', data: { columns: [{ name: 'To do', tasks: [] }] } });
  }, 30000);
  test('ordinary conversation is not forced into a catalog card', async () => {
    const { result, requests, trace } = await run(['Hello.'], { message: 'Hello' });
    expect(requests).toHaveLength(1);
    expect(result.parts.filter(p => p.type === 'text').map(p => p.text).join('\n')).toBe('Hello.');
    expect(trace.filter(t => t.kind === 'answer.validation')).toHaveLength(0);
    expect((trace.find(t => t.kind === 'answer.policy')?.data as any)).toMatchObject({ mode: 'text', reason: 'conversation' });
  }, 30000);

  test('cancellation never publishes a partial answer or starts a formatting retry', async () => {
    const { result, requests, published, trace } = await run(['private partial output'], { cancel: true });
    expect(requests).toHaveLength(1);
    expect(published.some(text => text.includes('private partial'))).toBe(false);
    expect(trace.filter(t => t.kind === 'answer.validation').map(t => (t.data as any).interrupted)).toEqual([true]);
    expect(result.info.role === 'assistant' && result.info.answerPolicy?.status).toBe('rejected');
  }, 30000);
  test('oversized responses are withheld and bounded before one correction', async () => {
    const { requests, published, trace } = await run(['x'.repeat(70000), valid]);
    expect(requests).toHaveLength(2);
    expect(published.some(text => text.includes('xxxxxxxx'))).toBe(false);
    const record = trace.find(t => t.kind === 'answer.validation')?.data as any;
    expect(record.truncated).toBe(true);
    expect(record.rejectedText.length).toBeLessThanOrEqual(66000);
  }, 30000);

  test('a routed refusal can still use the enabled handoff and must validate on the new provider', async () => {
    const { result, requests, published } = await run(["I'm sorry, but I can't help with that.", valid], { handoff: true });
    expect(requests).toHaveLength(2);
    expect(result.info.role === 'assistant' && result.info.providerID).toBe('allternit');
    expect(result.info.role === 'assistant' && result.info.answerPolicy).toMatchObject({ selected: 'task-board', status: 'accepted' });
    expect(published.some(text => text.includes("I can't help"))).toBe(false);
  }, 30000);

  test('a synthetic Stop-hook continuation cannot drop the original answer policy', async () => {
    const { result, requests, published } = await run([valid, 'hook continuation bypass', valid], { hook: true });
    expect(requests).toHaveLength(3);
    expect(result.info.role === 'assistant' && result.info.answerPolicy?.status).toBe('accepted');
    expect(published.some(text => text.includes('hook continuation bypass'))).toBe(false);
  }, 30000);
  test('reaching a usage limit ends invalid output without spending a formatting request', async () => {
    const { result, requests, session } = await run(['invalid output at the limit', valid], { quota: true });
    expect(requests).toHaveLength(1);
    expect(session.limit?.state).toBe('paused');
    expect(result.info.role === 'assistant' && result.info.answerPolicy?.status).toBe('rejected');
  }, 30000);

  test('a gateway retry discards the incomplete attempt before validating the fresh response', async () => {
    const { result, requests, published } = await run(['stale incomplete output', valid], { retryHint: true });
    expect(requests).toHaveLength(2);
    expect(result.info.role === 'assistant' && result.info.answerPolicy).toMatchObject({ status: 'accepted', attempt: 1 });
    expect(published.some(text => text.includes('stale incomplete'))).toBe(false);
  }, 30000);

  test('a refusal during the one formatting correction cannot start an additional handoff', async () => {
    const { result, requests } = await run(['Invalid first reply', "I'm sorry, but I can't help with that.", valid], { handoff: true });
    expect(requests).toHaveLength(2);
    expect(result.info.role === 'assistant' && result.info.error?.name).toBe('AnswerTemplateError');
  }, 30000);

});

describe.skipIf(skipRunCode)('run_code results and the required template', () => {
  const chart = "open('out/inventory_value.png', 'wb').write(b'\\x89PNG\\r\\n\\x1a\\n')\nprint('total value: 314.75')";
  test('an Analyze turn whose chart came back as a run_code file is not failed for lacking the chart envelope', async () => {
    const answer = 'Total inventory value is 314.75 across three products; the chart is attached.';
    const { result, requests, trace, messages } = await run([{ runCode: chart }, answer], { message: ANALYZE });
    expect((trace.find(t => t.kind === 'answer.policy')?.data as any)).toMatchObject({ mode: 'template', selected: 'chart' });
    const tool = messages.flatMap(m => m.parts).find((p): p is MessageV2.ToolPart => p.type === 'tool' && p.tool === 'run_code');
    expect(tool?.state.status === 'completed' && tool.state.metadata.files.map((f: any) => f.mediaType)).toEqual(['image/png']);
    expect(requests).toHaveLength(2);
    expect(result.info.role === 'assistant' && result.info.error).toBeUndefined();
    expect(result.info.role === 'assistant' && result.info.answerPolicy).toMatchObject({ selected: 'chart', returned: 'run_code', status: 'accepted' });
    expect(result.parts.filter(p => p.type === 'text' && !p.ignored).map(p => (p as MessageV2.TextPart).text).join('\n')).toBe(answer);
  }, 120000);
  test('a run_code call that saved no chart or sheet does not satisfy the chart template', async () => {
    const { result, requests } = await run([{ runCode: "print('total value: 314.75')" }, 'Prose instead of a chart.', 'Still prose.'], { message: ANALYZE });
    expect(requests).toHaveLength(3);
    expect(result.info.role === 'assistant' && result.info.error?.name).toBe('AnswerTemplateError');
  }, 120000);
});
test('a plain chart request without run_code still gets its template enforcement', async () => {
  const { result, requests } = await run(['Revenue rose.', 'Revenue rose a lot.'], { message: 'Make me a chart of monthly revenue: Jan 10, Feb 30, Mar 20' });
  expect(requests).toHaveLength(2);
  expect(result.info.role === 'assistant' && result.info.answerPolicy).toMatchObject({ selected: 'chart', status: 'rejected' });
  expect(result.info.role === 'assistant' && result.info.error?.name).toBe('AnswerTemplateError');
}, 30000);
