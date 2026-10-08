import { describe, it, expect, beforeEach } from 'vitest';
import { AllternitAgent } from '../agents/controller.js';
import { AllternitHarness } from '../harness/index.js';
import { AgentRun } from '../agents/run.js';
import { NativeToolBelt } from '../tools/search.js';
import { ToolRegistry } from '../tools/registry.js';
import { toStrictJsonSchema } from '../tools/schema.js';
import { COMPUTER_USE_TOOL, ComputerUseCapability, classifyComputerAction } from '../capabilities/computer-use.js';
import { mkdtemp, readFile, rm, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';

/** Small single-page PDF fixture generated with pdf-lib for testing pdf_process. */
const PDF_FIXTURE_BASE64 =
  'JVBERi0xLjcKJYGBgYEKCjYgMCBvYmoKPDwKL0ZpbHRlciAvRmxhdGVEZWNvZGUKL0xlbmd0aCAyNjYKPj4Kc3RyZWFtCnicjVJNSwQxDL33V/QsiGkmecmAeNjdGTx4EeYPiKyyoocV8feb7IyCMAtLoaUvIe+jPZbNVKjm+nwtN/f79+/91+H56dqod3Ey7ytLnV5K7g+lnVpbVapGVKePcisNW2xNoMYY0JswKYkI4uywQZ91JkHU3MSyT+/q9FamqzJM5bEcz6nIYXBWeG2+qgK6qHAoGiTYBliwjXFeyMJE5KS9ozZeZ+GZRQUeLjqmebcuOHdG4VADY4wQ6wPPNDyTADAaL0hnf/p+E5r7IpGGuRq6ebg0nZiCpKtt/Y3U19NRMcN4GUu+l3YdzqejWFh26fVsEuks737qyN/AmUhgFNos0P++fwDD+JGPCmVuZHN0cmVhbQplbmRvYmoKCjcgMCBvYmoKPDwKL0ZpbHRlciAvRmxhdGVEZWNvZGUKL1R5cGUgL09ialN0bQovTiA1Ci9GaXJzdCAyNgovTGVuZ3RoIDQwMgo+PgpzdHJlYW0KeJzVU0tr3DAQvutXzLE9FI0lWY+yLGx21y2U0JAEWlp6cGyxuASp2NqS/vvO2JssIS09FzFIM9830rxUAYICY0CD82Cg1gpq8ErDaiXk7a8fEeRVe4iTkB+GfoKvxEG4hm9CbvMxFajEei3O3G1b2vt8EIsTVEx+ZFyNuT92cYRVs28aRIeI1pBYRLWjfUsSSBTphClPZxJnTkI2pxH1hrBmEesWH8Znbn3y39NOXMuc3cI1ftGf3uW39ssd6l/xhLWQl7nftSXCq91bhcqix1AZ7Sr95TWVY4xtyf9vcnP8Q05/zfBZn7m93OQx8gzMXZbXccrHsaO2M6/JhPDhfbz/GcvQtW8cBk9xOh9oxmaXMxacUdar2vqXGNfLYx28/ZNfjcYGhe4l5mqnaq3tkx+lID9/vPseuzk0VvcP5d1N4ZwXA9suYz+0F/mBph1p2UqBC4pnfpNSLvwL5vlPhbJnzZ7+xLMScQGEvDnelVllYyXkRTvFuTTnOCmI1OV+SAeQn4a0SdPwaOAbfwOUKOKMCmVuZHN0cmVhbQplbmRvYmoKCjggMCBvYmoKPDwKL1NpemUgOQovUm9vdCAyIDAgUgovSW5mbyAzIDAgUgovRmlsdGVyIC9GbGF0ZURlY29kZQovVHlwZSAvWFJlZgovTGVuZ3RoIDQxCi9XIFsgMSAyIDIgXQovSW5kZXggWyAwIDkgXQo+PgpzdHJlYW0KeJwVxLERADAIA7E3cJc2+1Fm/xkIViFgJjjg5MKlK3FBels2fF3YAwsKZW5kc3RyZWFtCmVuZG9iagoKc3RhcnR4cmVmCjg1OQolJUVPRg==';

function createMockMcpFetch() {
  return async (_input: RequestInfo | URL, init?: RequestInit) => {
    const body = JSON.parse(String(init?.body ?? '{}'));
    const id = body.id ?? null;
    if (body.method === 'initialize') {
      return Response.json({
        jsonrpc: '2.0',
        id,
        result: {
          protocolVersion: '2025-03-26',
          capabilities: {},
          serverInfo: { name: 'mock', version: '1' },
        },
      });
    }
    if (body.method === 'tools/list') {
      return Response.json({
        jsonrpc: '2.0',
        id,
        result: {
          tools: [
            {
              name: 'echo',
              description: 'Echo input',
              inputSchema: {
                type: 'object',
                properties: { message: { type: 'string' } },
                required: ['message'],
              },
            },
          ],
        },
      });
    }
    if (body.method === 'tools/call') {
      return Response.json({
        jsonrpc: '2.0',
        id,
        result: { content: [{ type: 'text', text: `echo ${body.params.arguments.message}` }] },
      });
    }
    return new Response('not found', { status: 404 });
  };
}

describe('Native Agent Tool Belt', () => {
  let harness: AllternitHarness;
  let agent: AllternitAgent;

  beforeEach(() => {
    harness = new AllternitHarness({ mode: 'local', local: { baseURL: 'http://localhost:11434' } });
    agent = new AllternitAgent(harness, { persistencePath: ':memory:' });
  });

  function createRun(messages: any[] = [], tools?: any[]): AgentRun {
    const run = new AgentRun('test-run', agent, {
      provider: 'anthropic',
      model: 'claude-sonnet-4-6',
      messages,
      ...(tools ? { tools } : {})
    });
    // Fork the global registry like AllternitAgent.run() does
    run.runState.toolRegistry = (agent as any).globalToolRegistry.fork();
    run.runState.toolBelt = new NativeToolBelt(run.runState.toolRegistry);
    return run;
  }

  it('should support deferred tool registration and discovery', async () => {
    agent.registerDeferredTool({
      id: 'browser-tool',
      name: 'browse',
      description: 'Web browsing capability',
      input_schema: { type: 'object', properties: {} },
      tags: ['web', 'automation']
    });

    const run = createRun();

    // Discover via tool_search
    const searchTool = run.runState.toolRegistry.getActiveTools().find(t => t.name === 'tool_search');
    expect(searchTool).toBeDefined();

    const results = await searchTool!.execute!({ query: 'web' }, { callId: '1' });
    expect(results).toHaveLength(1);
    expect(results[0].id).toBe('browser-tool');
  });

  it('should support tool activation mid-run', async () => {
    agent.registerDeferredTool({
      id: 'git-tool',
      name: 'git_commit',
      description: 'Commit changes',
      input_schema: { type: 'object', properties: {} }
    });

    const run = createRun();

    // Initially not in active tools
    expect(run.runState.getActiveToolSchemas().find(t => t.name === 'git_commit')).toBeUndefined();

    // Activate
    const activateTool = run.runState.toolRegistry.getActiveTools().find(t => t.name === 'tool_activate');
    await activateTool!.execute!({ toolId: 'git-tool' }, { callId: '2' });

    // Now it should be active
    expect(run.runState.getActiveToolSchemas().find(t => t.name === 'git_commit')).toBeDefined();
  });

  it('should support session snapshot and rehydration', async () => {
    agent.registerDeferredTool({
      id: 'db-tool',
      name: 'query_db',
      description: 'SQL query',
      input_schema: { type: 'object', properties: {} }
    });

    const run1 = createRun();

    // Activate a tool in run 1
    await run1.runState.toolRegistry.activateTool('db-tool');
    const snapshot = run1.runState.toolRegistry.snapshot();

    // Rehydrate in run 2
    const run2 = createRun();
    run2.hydrate('thinking', [], snapshot);

    expect(run2.runState.getActiveToolSchemas().find(t => t.name === 'query_db')).toBeDefined();
  });

  it('should use active tool schemas from registry for provider injection', async () => {
    const run = createRun([], [{ name: 'existing_tool', description: 'desc', input_schema: { type: 'object', properties: {} } }]);

    const schemas = run.runState.getActiveToolSchemas();
    // Should include tool_search and tool_activate by default
    expect(schemas.map(s => s.name)).toContain('tool_search');
    expect(schemas.map(s => s.name)).toContain('tool_activate');
    expect(schemas.map(s => s.name)).toContain('web_search');
    expect(schemas.map(s => s.name)).toContain('web_fetch');
    expect(schemas.map(s => s.name)).toContain('str_replace_editor');
  });

  it('supports cached, indexed, and live web search modes', async () => {
    const registry = new ToolRegistry();
    const cache = new Map([['cached query', [{ title: 'Cached', url: 'https://cached.test', snippet: 'hit' }]]]);
    const belt = new NativeToolBelt(registry, {
      cache,
      searchIndex: async query => [{ title: 'Index', url: 'https://index.test', snippet: query }],
      liveSearch: async query => [{ title: 'Live', url: 'https://live.test', snippet: query }],
    });
    const search = belt.getRegistry().getTool('web_search')!;

    expect((await search.execute!({ query: 'cached query', mode: 'cached' }, {}))[0].title).toBe('Cached');
    expect((await search.execute!({ query: 'index query', mode: 'indexed' }, {}))[0].title).toBe('Index');
    expect((await search.execute!({ query: 'live query', mode: 'live' }, {}))[0].title).toBe('Live');
    expect((await search.execute!({ query: 'live query', mode: 'cached' }, {}))[0].title).toBe('Live');
  });

  it('fetches URLs and extracts readable HTML content', async () => {
    const registry = new ToolRegistry();
    new NativeToolBelt(registry, {
      fetch: async () => new Response(
        '<html><head><title>Example &amp; Test</title><style>hidden</style></head><body><h1>Hello</h1><script>bad()</script><p>World</p></body></html>',
        { status: 200, headers: { 'content-type': 'text/html' } },
      ),
    });

    const result = await registry.getTool('web_fetch')!.execute!({ url: 'https://example.test/page' }, {});
    expect(result.title).toBe('Example & Test');
    expect(result.text).toContain('Hello World');
    expect(result.text).not.toContain('bad()');
  });

  it.each([
    {
      provider: 'tavily' as const,
      apiKeys: { tavily: 'tavily-key' },
      response: { results: [{ title: 'Tavily', url: 'https://tavily.test', content: 'result' }] },
      assertRequest: (url: string, init?: RequestInit) => {
        expect(url).toBe('https://api.tavily.com/search');
        expect(JSON.parse(String(init?.body))).toMatchObject({ api_key: 'tavily-key', query: 'tools', max_results: 2 });
      },
    },
    {
      provider: 'perplexity' as const,
      apiKeys: { perplexity: 'perplexity-key' },
      response: { search_results: [{ title: 'Perplexity', url: 'https://perplexity.test', snippet: 'result' }] },
      assertRequest: (url: string, init?: RequestInit) => {
        expect(url).toBe('https://api.perplexity.ai/chat/completions');
        expect((init?.headers as Record<string, string>).authorization).toBe('Bearer perplexity-key');
      },
    },
    {
      provider: 'bing' as const,
      apiKeys: { bing: 'bing-key' },
      response: { webPages: { value: [{ name: 'Bing', url: 'https://bing.test', snippet: 'result' }] } },
      assertRequest: (url: string, init?: RequestInit) => {
        expect(url).toContain('api.bing.microsoft.com/v7.0/search');
        expect((init?.headers as Record<string, string>)['Ocp-Apim-Subscription-Key']).toBe('bing-key');
      },
    },
  ])('uses the $provider search adapter with injected fetch', async ({ provider, apiKeys, response, assertRequest }) => {
    const registry = new ToolRegistry();
    new NativeToolBelt(registry, {
      provider,
      apiKeys,
      fetch: async (input, init) => {
        assertRequest(String(input), init);
        return Response.json(response);
      },
    });

    const results = await registry.getTool('web_search')!.execute!({ query: 'tools', limit: 2 }, {});
    expect(results).toHaveLength(1);
    expect(results[0].snippet).toBe('result');
  });

  it('edits workspace text files and undoes the last edit', async () => {
    const workspaceRoot = await mkdtemp(join(tmpdir(), 'allternit-editor-'));
    try {
      await writeFile(join(workspaceRoot, 'example.txt'), 'alpha\nbeta\n', 'utf8');
      const registry = new ToolRegistry();
      new NativeToolBelt(registry, { workspaceRoot });
      const editor = registry.getTool('str_replace_editor')!;

      expect(await editor.execute!({ command: 'view', path: 'example.txt', view_range: [2, 2] }, {})).toBe('2: beta');
      await editor.execute!({ command: 'str_replace', path: 'example.txt', old_str: 'beta', new_str: 'gamma' }, {});
      await editor.execute!({ command: 'insert', path: 'example.txt', insert_line: 1, new_str: 'inserted' }, {});
      expect(await readFile(join(workspaceRoot, 'example.txt'), 'utf8')).toBe('alpha\ninserted\ngamma\n');
      await editor.execute!({ command: 'undo', path: 'example.txt' }, {});
      expect(await readFile(join(workspaceRoot, 'example.txt'), 'utf8')).toBe('alpha\ngamma\n');
      await expect(editor.execute!({ command: 'view', path: '../outside.txt' }, {})).rejects.toThrow('within the active workspace');
    } finally {
      await rm(workspaceRoot, { recursive: true, force: true });
    }
  });

  it('uses computer_20250124 actions and returns screenshot image blocks', async () => {
    const capability = new ComputerUseCapability({
      fetch: async () => Response.json({ screenshot: 'data:image/png;base64,cG5n' }),
      displayWidthPx: 1280,
      displayHeightPx: 720,
    });
    const tool = capability.getTool();

    expect(tool.metadata).toMatchObject({
      allternitToolType: 'computer',
      computerToolVersion: '20250124',
      anthropicType: 'computer_20250124',
      display_width_px: 1280,
      display_height_px: 720,
    });
    const actionEnum = (tool.input_schema.properties as Record<string, { enum?: string[] }>).action.enum!;
    expect(actionEnum).toContain('scroll');
    expect(actionEnum).not.toContain('zoom');
    expect((tool.input_schema.properties as Record<string, unknown>).region).toBeUndefined();
    expect(await tool.execute!({ action: 'screenshot' }, {})).toEqual([{
      type: 'image',
      source: { type: 'base64', media_type: 'image/png', data: 'cG5n' },
    }]);
  });

  it('supports the computer_20251124 action set with zoom and region', async () => {
    const capability = new ComputerUseCapability({
      fetch: async () => Response.json({ status: 'completed', summary: 'zoomed' }),
      toolVersion: '20251124',
      enableZoom: true,
      displayWidthPx: 1920,
      displayHeightPx: 1080,
    });
    const tool = capability.getTool();
    const properties = tool.input_schema.properties as Record<string, { enum?: string[]; minItems?: number; maxItems?: number }>;

    expect(tool.metadata).toMatchObject({
      allternitToolType: 'computer',
      computerToolVersion: '20251124',
      anthropicType: 'computer_20251124',
      enable_zoom: true,
      display_width_px: 1920,
      display_height_px: 1080,
    });
    expect(properties.action.enum).toContain('zoom');
    expect(properties.action.enum).toHaveLength(17);
    expect(properties.region).toMatchObject({ minItems: 4, maxItems: 4 });
    expect(await tool.execute!({ action: 'zoom', region: [0, 0, 960, 540] }, {})).toBe('zoomed');

    // 20250124 tools never advertise zoom, even with enableZoom set
    const legacy = new ComputerUseCapability({ enableZoom: true }).getTool();
    expect(legacy.metadata).toMatchObject({ computerToolVersion: '20250124', anthropicType: 'computer_20250124' });
    expect(legacy.metadata?.enable_zoom).toBeUndefined();
    expect((legacy.input_schema.properties as Record<string, { enum?: string[] }>).action.enum).not.toContain('zoom');
  });

  it('gates risky computer actions on an approver and denies without one', async () => {
    expect(classifyComputerAction({ action: 'screenshot' })).toBe('reversible');
    expect(classifyComputerAction({ action: 'type', text: 'hello' })).toBe('reversible');
    expect(classifyComputerAction({ action: 'type', text: 'hello\n' })).toBe('risky');
    expect(classifyComputerAction({ action: 'key', text: 'Return' })).toBe('risky');
    expect(classifyComputerAction({ action: 'left_click' })).toBe('risky');

    // Shared definition and a capability with no approver: deny risky, allow reversible.
    expect(await COMPUTER_USE_TOOL.preExecute!({ action: 'left_click', coordinate: [1, 2] }, {})).toMatchObject({ proceed: false });
    const bare = new ComputerUseCapability().getTool();
    expect(await bare.preExecute!({ action: 'screenshot' }, {})).toEqual({ proceed: true });
    const denied = await bare.preExecute!({ action: 'double_click', coordinate: [1, 2] }, {});
    expect(denied.proceed).toBe(false);
    expect(denied.reason).toContain('no approver');

    const asked: string[] = [];
    const gated = new ComputerUseCapability({
      approve: async ({ action, actionClass }) => {
        asked.push(`${action}:${actionClass}`);
        return action === 'left_click' ? true : { approved: false, reason: 'not now' };
      },
    }).getTool();
    expect(await gated.preExecute!({ action: 'left_click', coordinate: [1, 2] }, {})).toEqual({ proceed: true });
    expect(await gated.preExecute!({ action: 'key', text: 'enter' }, {})).toMatchObject({ proceed: false, reason: expect.stringContaining('not now') });
    expect(await gated.preExecute!({ action: 'mouse_move', coordinate: [1, 2] }, {})).toEqual({ proceed: true });
    expect(asked).toEqual(['left_click:risky', 'key:risky']);
  });

  it('namespaces tools and validates strict schemas', () => {
    const registry = new ToolRegistry();
    registry.registerTool({
      name: 'lookup',
      description: 'Lookup a record',
      input_schema: { type: 'object', properties: { id: { type: 'string' } }, required: ['id'] },
    }, { namespace: 'crm', strict: true });

    expect(registry.getTool('crm.lookup')).toBeDefined();
    expect(registry.validateInput('crm.lookup', { id: '1' }).valid).toBe(true);
    expect(registry.validateInput('crm.lookup', { id: '1', extra: true }).errors).toContain('$.extra is not allowed');
    expect(toStrictJsonSchema({ type: 'object', properties: { nested: { type: 'object', properties: {} } } }).properties!.nested.additionalProperties).toBe(false);
  });

  it('validates image type tool parameters with base64 and URL sources', () => {
    const registry = new ToolRegistry();
    registry.registerTool({
      name: 'describe_image',
      description: 'Describe an image',
      input_schema: {
        type: 'object',
        properties: {
          image: { type: 'image', description: 'Image content block' },
        },
        required: ['image'],
      },
    });

    const base64Image = { source: { type: 'base64', media_type: 'image/png', data: 'cG5n' } };
    const urlImage = { source: { type: 'url', url: 'https://example.com/image.png' } };

    expect(registry.validateInput('describe_image', { image: base64Image }).valid).toBe(true);
    expect(registry.validateInput('describe_image', { image: urlImage }).valid).toBe(true);
    expect(registry.validateInput('describe_image', { image: 'not-an-image' }).valid).toBe(false);
    expect(registry.validateInput('describe_image', { image: { source: { type: 'base64', data: 'cG5n' } } }).errors).toContain('$.image.source.media_type must be a string');
  });

  it('attaches MCP server tools as namespaced model-facing tools', async () => {
    const registry = new ToolRegistry();
    const belt = new NativeToolBelt(registry);
    const calls: unknown[] = [];
    const names = await belt.attachMcpServer({
      serverId: 'docs',
      listTools: async () => [{
        name: 'read',
        description: 'Read a document',
        inputSchema: { type: 'object', properties: { id: { type: 'string' } }, required: ['id'] },
      }],
      callTool: async (name, args) => { calls.push({ name, args }); return { content: 'document' }; },
    });

    expect(names).toEqual(['docs.read']);
    const tool = registry.getTool('docs.read')!;
    expect(tool.input_schema.additionalProperties).toBe(false);
    expect(await tool.execute!({ id: 'abc' }, {})).toEqual({ content: 'document' });
    expect(calls).toEqual([{ name: 'read', args: { id: 'abc' } }]);
  });

  it('registers new model-facing bash, code_execution, and memory tools', () => {
    const registry = new ToolRegistry();
    new NativeToolBelt(registry);
    expect(registry.getTool('bash')).toBeDefined();
    expect(registry.getTool('code_execution')).toBeDefined();
    expect(registry.getTool('memory')).toBeDefined();
    expect(registry.getTool('bash')!.input_schema.properties).toHaveProperty('command');
    expect(registry.getTool('bash')!.input_schema.properties).toHaveProperty('timeout');
    expect(registry.getTool('bash')!.input_schema.properties).toHaveProperty('restart');
    expect(registry.getTool('code_execution')!.input_schema.properties).toHaveProperty('language');
    expect(registry.getTool('code_execution')!.input_schema.properties).toHaveProperty('code');
    expect(registry.getTool('code_execution')!.input_schema.properties).toHaveProperty('timeout_seconds');
    expect(registry.getTool('code_execution')!.input_schema.properties).toHaveProperty('dependencies');
    expect(registry.getTool('memory')!.input_schema.properties).toHaveProperty('operation');
    expect(registry.getTool('memory')!.input_schema.properties).toHaveProperty('key');
    expect(registry.getTool('memory')!.input_schema.properties).toHaveProperty('value');
  });

  it('executes bash through an injectable runner', async () => {
    const registry = new ToolRegistry();
    const calls: unknown[] = [];
    new NativeToolBelt(registry, {
      runner: {
        run: async (args) => {
          calls.push(args);
          return { stdout: 'ok', stderr: '', exit_code: 0, success: true };
        },
      },
    });
    const result = await registry.getTool('bash')!.execute!({ command: 'echo hi', timeout: 5, restart: true }, {});
    expect(result).toEqual({ stdout: 'ok', stderr: '', exit_code: 0, success: true });
    expect(calls).toEqual([{ command: 'echo hi', timeout: 5, restart: true }]);
  });

  it('executes code through an injectable sandbox runner', async () => {
    const registry = new ToolRegistry();
    const calls: unknown[] = [];
    new NativeToolBelt(registry, {
      runner: {
        execute: async (req) => {
          calls.push(req);
          return { stdout: '42', stderr: '', exit_code: 0, success: true, artifacts: [] };
        },
      },
    });
    const result = await registry.getTool('code_execution')!.execute!(
      { language: 'python', code: 'print(42)', timeout_seconds: 10, dependencies: ['requests'] },
      {},
    );
    expect(result).toEqual({ stdout: '42', stderr: '', exit_code: 0, success: true, artifacts: [] });
    expect(calls).toEqual([
      { language: 'python', code: 'print(42)', timeout_seconds: 10, dependencies: ['requests'] },
    ]);
  });

  it('reads, writes, and deletes session memory through an injectable store', async () => {
    const registry = new ToolRegistry();
    const entries = new Map<string, unknown>();
    new NativeToolBelt(registry, {
      store: {
        read: async (key) => (entries.has(key) ? { key, value: entries.get(key), updated_at: 'now' } : null),
        write: async (key, value) => {
          entries.set(key, value);
          return { key, value, updated_at: 'now' };
        },
        delete: async (key) => entries.delete(key),
      },
    });
    const memory = registry.getTool('memory')!;

    await memory.execute!({ operation: 'write', key: 'mode', value: 'fast' }, {});
    const read = await memory.execute!({ operation: 'read', key: 'mode' }, {});
    expect(read).toEqual({ key: 'mode', value: 'fast', updated_at: 'now' });

    const missing = await memory.execute!({ operation: 'read', key: 'missing' }, {});
    expect(missing.value).toBeNull();

    const deleted = await memory.execute!({ operation: 'delete', key: 'mode' }, {});
    expect(deleted).toEqual({ key: 'mode', deleted: true });
  });

  it('registers the pdf_process document tool', () => {
    const registry = new ToolRegistry();
    new NativeToolBelt(registry);
    const pdf = registry.getTool('pdf_process');
    expect(pdf).toBeDefined();
    expect(pdf!.input_schema.properties).toHaveProperty('source');
    expect(pdf!.input_schema.properties).toHaveProperty('thumbnails');
  });

  it('extracts markdown text from a base64 PDF fixture', async () => {
    const registry = new ToolRegistry();
    new NativeToolBelt(registry);
    const pdf = registry.getTool('pdf_process')!;

    const result = await pdf.execute!({ source: { type: 'base64', data: PDF_FIXTURE_BASE64 } }, {});

    expect(result.pages).toBe(1);
    expect(result.markdown.length).toBeGreaterThan(0);
    expect(result.markdown).toContain('Allternit PDF Skill Fixture');
    expect(result.structure.headings.length).toBeGreaterThan(0);
  });

  it('renders optional page thumbnails from a base64 PDF fixture', async () => {
    const registry = new ToolRegistry();
    new NativeToolBelt(registry);
    const pdf = registry.getTool('pdf_process')!;

    const result = await pdf.execute!(
      { source: { type: 'base64', data: PDF_FIXTURE_BASE64 }, thumbnails: true },
      {},
    );

    expect(result.thumbnails).toBeDefined();
    expect(result.thumbnails!.length).toBe(1);
    expect(result.thumbnails![0]).toMatch(/^data:image\/png;base64,/);
  });

  it('rejects invalid pdf_process input', async () => {
    const registry = new ToolRegistry();
    new NativeToolBelt(registry);
    const pdf = registry.getTool('pdf_process')!;

    await expect(pdf.execute!({ source: { type: 'base64', data: 'not-valid-base64!!!' } }, {})).rejects.toThrow();
    await expect(pdf.execute!({ source: { type: 'url', url: '' } }, {})).rejects.toThrow();
    await expect(pdf.execute!({ source: { type: 'unknown' } }, {})).rejects.toThrow();
  });

  it('executes multiple tool calls concurrently and preserves call order', async () => {
    const run = createRun();
    const active = new Set<string>();
    let maxActive = 0;

    for (let i = 0; i < 3; i++) {
      run.runState.toolRegistry.registerTool({
        name: `tool_${i}`,
        description: `Tool ${i}`,
        input_schema: { type: 'object', properties: {} },
        execute: async () => {
          active.add(`tool_${i}`);
          maxActive = Math.max(maxActive, active.size);
          await new Promise((resolve) => setTimeout(resolve, 30));
          active.delete(`tool_${i}`);
          return `result_${i}`;
        },
      });
    }

    const harness = agent.getHarness();
    let streamCallCount = 0;
    (harness as any).stream = async function* () {
      streamCallCount++;
      if (streamCallCount === 1) {
        yield { type: 'tool_call_complete', id: 'call_0', name: 'tool_0', arguments: {} };
        yield { type: 'tool_call_complete', id: 'call_1', name: 'tool_1', arguments: {} };
        yield { type: 'tool_call_complete', id: 'call_2', name: 'tool_2', arguments: {} };
      }
      yield { type: 'done' };
    };

    const completed = new Promise<void>((resolve) => run.once('completed', resolve));
    await run.execute();
    await completed;

    expect(maxActive).toBeGreaterThan(1);
    const resultMessages = run.messages.filter((m) => m.role === 'user');
    expect(resultMessages).toHaveLength(3);
    const resultContents = resultMessages.map((m) =>
      Array.isArray(m.content) ? (m.content[0] as any).content : m.content
    );
    expect(resultContents[0]).toContain('result_0');
    expect(resultContents[1]).toContain('result_1');
    expect(resultContents[2]).toContain('result_2');
  });

  it('attaches an HTTP MCP server from config and invokes a tool', async () => {
    const registry = new ToolRegistry();
    const belt = new NativeToolBelt(registry, { fetch: createMockMcpFetch() });
    const names = await belt.attachMcpServer({
      serverId: 'remote',
      transport: 'http',
      url: 'http://localhost:9999/mcp',
      fetch: createMockMcpFetch(),
    });

    expect(names).toEqual(['remote.echo']);
    const tool = registry.getTool('remote.echo')!;
    expect(tool.input_schema.additionalProperties).toBe(false);
    const result = await tool.execute!({ message: 'hi' }, {});
    expect(result).toEqual({ content: [{ type: 'text', text: 'echo hi' }] });
  });

  it('loads enabled MCP servers from ~/.allternit/mcp-servers.json', async () => {
    const dir = await mkdtemp(join(tmpdir(), 'allternit-mcp-dir-'));
    try {
      const path = join(dir, 'mcp-servers.json');
      await writeFile(
        path,
        JSON.stringify({
          mcpServers: {
            docs: {
              type: 'remote',
              url: 'http://localhost:9999/mcp',
              enabled: true,
            },
            ignored: {
              type: 'remote',
              url: 'http://localhost:9999/mcp',
              enabled: false,
            },
          },
        }),
        'utf8',
      );

      const registry = new ToolRegistry();
      const belt = new NativeToolBelt(registry, {
        fetch: createMockMcpFetch(),
        mcpDirectoryPath: path,
      });
      await belt.mcpDirectoryLoaded;

      expect(registry.getTool('docs.echo')).toBeDefined();
      expect(registry.getTool('ignored.echo')).toBeUndefined();

      const result = await registry.getTool('docs.echo')!.execute!({ message: 'from-dir' }, {});
      expect(result).toEqual({ content: [{ type: 'text', text: 'echo from-dir' }] });
    } finally {
      await rm(dir, { recursive: true, force: true });
    }
  });
});
