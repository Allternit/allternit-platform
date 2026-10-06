import { PassThrough } from 'node:stream';
import { describe, expect, it, vi } from 'vitest';
import { ARTEMIS_PINNED_COMMIT, McpStdioClient, runArtemisTask } from './phone-artemis.js';

/** A fake ARTEMIS MCP server on in-memory pipes. */
function fakeServer(handlers: Record<string, (args: Record<string, unknown>) => unknown>, era: 'legacy' | 'modern' = 'legacy') {
  const stdin = new PassThrough();
  const stdout = new PassThrough();
  const calls: Array<{ method: string; params: any }> = [];
  let buf = '';
  stdin.on('data', (chunk: Buffer) => {
    buf += chunk.toString();
    const lines = buf.split('\n');
    buf = lines.pop() ?? '';
    for (const line of lines.filter(Boolean)) {
      const msg = JSON.parse(line);
      calls.push({ method: msg.method, params: msg.params });
      if (msg.id === undefined) continue;
      if (msg.method === 'server/discover') {
        // A 2025-era server (ARTEMIS today) does not know the method; a 2026-07-28 one answers it.
        stdout.write(
          JSON.stringify(
            era === 'modern'
              ? { jsonrpc: '2.0', id: msg.id, result: { supportedVersions: ['2026-07-28'], capabilities: { tools: {} }, resultType: 'complete', _meta: { 'io.modelcontextprotocol/serverInfo': { name: 'artemis', version: '0' } } } }
              : { jsonrpc: '2.0', id: msg.id, error: { code: -32601, message: 'Method not found' } },
          ) + '\n',
        );
      } else if (msg.method === 'initialize') {
        stdout.write(JSON.stringify({ jsonrpc: '2.0', id: msg.id, result: { protocolVersion: '2024-11-05', capabilities: { tools: {} }, serverInfo: { name: 'artemis', version: '0' } } }) + '\n');
      } else if (msg.method === 'tools/call') {
        const handler = handlers[msg.params.name];
        const payload = handler ? handler(msg.params.arguments) : { error: 'no such tool' };
        const result: Record<string, unknown> = { content: [{ type: 'text', text: JSON.stringify(payload) }] };
        if (era === 'modern') Object.assign(result, { resultType: 'complete' });
        stdout.write(JSON.stringify({ jsonrpc: '2.0', id: msg.id, result }) + '\n');
      }
    }
  });
  let exit = () => {};
  const client = new McpStdioClient({ stdin, stdout, kill: () => exit(), onExit: (cb) => (exit = cb) });
  return { client, calls };
}

describe('ARTEMIS MCP client', () => {
  it('initializes once, then calls tools and parses the JSON result', async () => {
    const { client, calls } = fakeServer({ mobile_get_device_state: () => ({ ok: 1 }) });
    expect(await client.callTool('mobile_get_device_state', { device_serial: 's' })).toEqual({ ok: 1 });
    await client.callTool('mobile_get_device_state', {});
    expect(calls.filter((c) => c.method === 'initialize')).toHaveLength(1);
    expect(calls.some((c) => c.method === 'notifications/initialized')).toBe(true);
  });

  it('speaks 2026-07-28 to a server that answers server/discover (no initialize, _meta on every request)', async () => {
    const { client, calls } = fakeServer({ mobile_get_device_state: () => ({ ok: 2 }) }, 'modern');
    expect(await client.callTool('mobile_get_device_state', {})).toEqual({ ok: 2 });
    expect(calls.some((c) => c.method === 'initialize')).toBe(false);
    const call = calls.find((c) => c.method === 'tools/call');
    expect(call?.params._meta['io.modelcontextprotocol/protocolVersion']).toBe('2026-07-28');
  });

  it('rejects calls after the server exits', async () => {
    const { client } = fakeServer({});
    await client.callTool('x', {});
    client.close();
    await expect(client.callTool('x', {})).rejects.toThrow(/not running/);
  });
});

describe('runArtemisTask', () => {
  it('starts the task on the chosen device and polls until it completes', async () => {
    let polls = 0;
    const { client, calls } = fakeServer({
      mobile_run_task: () => ({ trace_id: 'tr-1', status: 'running' }),
      mobile_manage_task: () => ({ status: ++polls < 3 ? 'running' : 'completed', summary: 'done' }),
    });
    const result = await runArtemisTask(client, '192.168.1.20:41234', 'open settings', 'Flash', { pollMs: 1, sleep: async () => {} });
    expect(result).toMatchObject({ traceId: 'tr-1', status: 'completed', summary: 'done' });
    const run = calls.find((c) => c.params?.name === 'mobile_run_task');
    expect(run?.params.arguments).toEqual({ task_desc: 'open settings', model: 'Flash', device_serial: '192.168.1.20:41234' });
    expect(polls).toBe(3);
  });

  it('stops the task and errors on timeout', async () => {
    const { client, calls } = fakeServer({
      mobile_run_task: () => ({ trace_id: 'tr-2' }),
      mobile_manage_task: () => ({ status: 'running' }),
    });
    await expect(runArtemisTask(client, 's', 't', 'Pro', { pollMs: 10, timeoutMs: 20, sleep: async () => {} })).rejects.toThrow(/timed out/);
    expect(calls.some((c) => c.params?.arguments?.action === 'stop')).toBe(true);
  });

  it('fails clearly when ARTEMIS does not return a trace id', async () => {
    const { client } = fakeServer({ mobile_run_task: () => ({ status: 'failed', error: 'no device' }) });
    await expect(runArtemisTask(client, 's', 't', 'Flash', { sleep: async () => {} })).rejects.toThrow(/did not start/);
  });

  it('pins an exact commit', () => {
    expect(ARTEMIS_PINNED_COMMIT).toMatch(/^[0-9a-f]{40}$/);
  });
});
