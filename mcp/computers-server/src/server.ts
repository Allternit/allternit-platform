/**
 * MCP server: wires the Computers tool specs to the REST API client.
 *
 * Official MCP SDK v2 (`@modelcontextprotocol/server`): low-level `Server` with
 * `tools/list` + `tools/call` handlers, served over stdio by `serveStdio`, which
 * speaks MCP 2026-07-28 (`server/discover`, per-request `_meta`) and the legacy
 * `initialize` handshake from the same factory. The tool surface is declared
 * separately in `tool-spec.ts` per the `sdk/computer-use/src/mcp-tool-spec.ts`
 * spec-module idiom; tools/list returns it in that fixed order.
 *
 * Approval semantics: risky tools accept an optional `approvalId` argument,
 * threaded verbatim as `?approval_id=`. This server never mints or obtains
 * approvals itself; the API's denial payload is surfaced in the tool result.
 *
 * @module server
 */

import { Server, type CallToolResult, type Tool, type Transport } from '@modelcontextprotocol/server';
import { serveStdio } from '@modelcontextprotocol/server/stdio';

import { ApiError, ComputersApiClient, configFromEnv } from './client.js';
import {
  executePlatformToolCall,
  isProjectKey,
  PLATFORM_TOOL_SPECS,
  PlatformClient,
} from './platform.js';
import { COMPUTER_TOOL_SPECS, type McpToolName } from './tool-spec.js';

const API = '/api/v1';

type ToolArgs = Record<string, unknown>;

interface DispatchContext {
  client: ComputersApiClient;
  args: ToolArgs;
}

function str(args: ToolArgs, key: string): string {
  const value = args[key];
  if (typeof value !== 'string' || value.length === 0) {
    throw new Error(`Missing or invalid '${key}' argument`);
  }
  return value;
}

function optStr(args: ToolArgs, key: string): string | undefined {
  const value = args[key];
  return typeof value === 'string' && value.length > 0 ? value : undefined;
}

function approvalId(args: ToolArgs): string | undefined {
  return optStr(args, 'approvalId');
}

function base64(bytes: Uint8Array): string {
  return Buffer.from(bytes).toString('base64');
}

function fromBase64(content: string): Uint8Array {
  return new Uint8Array(Buffer.from(content, 'base64'));
}

function query(params: Record<string, string | undefined>): string {
  const entries = Object.entries(params).filter((e): e is [string, string] => Boolean(e[1]));
  if (entries.length === 0) return '';
  return `?${entries.map(([k, v]) => `${k}=${encodeURIComponent(v)}`).join('&')}`;
}

/** Body fields forwarded verbatim to JSON routes (drops MCP-only keys). */
function restBody(args: ToolArgs, ...drop: string[]): Record<string, unknown> {
  const skip = new Set(['approvalId', ...drop]);
  return Object.fromEntries(Object.entries(args).filter(([k]) => !skip.has(k)));
}

async function binaryResult(
  ctx: DispatchContext,
  path: string,
  kind: 'screenshot' | 'download',
): Promise<CallToolResult> {
  const { bytes, contentType } = await ctx.client.downloadBytes(path);
  const result = {
    encoding: 'base64' as const,
    content_type: contentType,
    size: bytes.length,
    note:
      kind === 'screenshot'
        ? 'PNG screenshot, base64-encoded.'
        : 'Binary file content, base64-encoded.',
    base64: base64(bytes),
  };
  return { content: [{ type: 'text', text: JSON.stringify(result) }] };
}

const DISPATCH: Record<McpToolName, (ctx: DispatchContext) => Promise<CallToolResult>> = {
  // ── Lifecycle ──────────────────────────────────────────────────────────────
  'computers.create': async ({ client, args }) => {
    const body = restBody(args);
    const result = await client.requestJson('POST', `${API}/computers`, body, approvalId(args));
    return { content: [{ type: 'text', text: JSON.stringify(result) }] };
  },
  'computers.list': async ({ client, args }) => {
    const path = `${API}/computers${query({
      bot_id: optStr(args, 'bot_id'),
      kind: optStr(args, 'kind'),
      group_id: optStr(args, 'group_id'),
      include_roles: optStr(args, 'include_roles'),
    })}`;
    const result = await client.requestJson('GET', path);
    return { content: [{ type: 'text', text: JSON.stringify(result) }] };
  },
  'computers.get': async ({ client, args }) => {
    const result = await client.requestJson('GET', `${API}/computers/${encodeURIComponent(str(args, 'computer_id'))}`);
    return { content: [{ type: 'text', text: JSON.stringify(result) }] };
  },
  'computers.start': async ({ client, args }) => {
    const result = await client.requestJson(
      'POST',
      `${API}/computers/${encodeURIComponent(str(args, 'computer_id'))}/start`,
      undefined,
      approvalId(args),
    );
    return { content: [{ type: 'text', text: JSON.stringify(result) }] };
  },
  'computers.stop': async ({ client, args }) => {
    const result = await client.requestJson(
      'POST',
      `${API}/computers/${encodeURIComponent(str(args, 'computer_id'))}/stop`,
      undefined,
      approvalId(args),
    );
    return { content: [{ type: 'text', text: JSON.stringify(result) }] };
  },
  'computers.restart': async ({ client, args }) => {
    const result = await client.requestJson(
      'POST',
      `${API}/computers/${encodeURIComponent(str(args, 'computer_id'))}/restart`,
      undefined,
      approvalId(args),
    );
    return { content: [{ type: 'text', text: JSON.stringify(result) }] };
  },
  'computers.resize': async ({ client, args }) => {
    const id = encodeURIComponent(str(args, 'computer_id'));
    const body = restBody(args, 'computer_id');
    const result = await client.requestJson('PATCH', `${API}/computers/${id}/resize`, body, approvalId(args));
    return { content: [{ type: 'text', text: JSON.stringify(result) }] };
  },
  'computers.clone': async ({ client, args }) => {
    const id = encodeURIComponent(str(args, 'computer_id'));
    const name = optStr(args, 'name');
    const result = await client.requestJson(
      'POST',
      `${API}/computers/${id}/clone`,
      name === undefined ? {} : { name },
      approvalId(args),
    );
    return { content: [{ type: 'text', text: JSON.stringify(result) }] };
  },
  'computers.delete': async ({ client, args }) => {
    await client.requestJson(
      'POST',
      `${API}/computers/${encodeURIComponent(str(args, 'computer_id'))}/delete`,
      undefined,
      approvalId(args),
    );
    const result = { success: true, note: 'computer deleted (API returned 204)' };
    return { content: [{ type: 'text', text: JSON.stringify(result) }] };
  },

  // ── Control ────────────────────────────────────────────────────────────────
  'computers.screenshot': async (ctx) =>
    binaryResult(ctx, `${API}/computers/${encodeURIComponent(str(ctx.args, 'computer_id'))}/screenshot`, 'screenshot'),
  'computers.mouse': async ({ client, args }) => {
    const id = encodeURIComponent(str(args, 'computer_id'));
    const result = await client.requestJson('POST', `${API}/computers/${id}/mouse`, restBody(args, 'computer_id'), approvalId(args));
    return { content: [{ type: 'text', text: JSON.stringify(result) }] };
  },
  'computers.keyboard': async ({ client, args }) => {
    const id = encodeURIComponent(str(args, 'computer_id'));
    const result = await client.requestJson('POST', `${API}/computers/${id}/keyboard`, restBody(args, 'computer_id'), approvalId(args));
    return { content: [{ type: 'text', text: JSON.stringify(result) }] };
  },
  'computers.shell': async ({ client, args }) => {
    const id = encodeURIComponent(str(args, 'computer_id'));
    const result = await client.requestJson('POST', `${API}/computers/${id}/shell`, restBody(args, 'computer_id'), approvalId(args));
    return { content: [{ type: 'text', text: JSON.stringify(result) }] };
  },
  'computers.files.upload': async ({ client, args }) => {
    const id = encodeURIComponent(str(args, 'computer_id'));
    const path = query({ path: str(args, 'path') });
    const bytes = fromBase64(str(args, 'content'));
    const result = await client.uploadBytes(`${API}/computers/${id}/files/upload${path}`, bytes, approvalId(args));
    return { content: [{ type: 'text', text: JSON.stringify(result ?? { success: true }) }] };
  },
  'computers.files.download': async (ctx) =>
    binaryResult(
      ctx,
      `${API}/computers/${encodeURIComponent(str(ctx.args, 'computer_id'))}/files/download${query({ path: str(ctx.args, 'path') })}`,
      'download',
    ),

  // ── Snapshots ──────────────────────────────────────────────────────────────
  'computers.snapshots.list': async ({ client, args }) => {
    const result = await client.requestJson('GET', `${API}/computers/${encodeURIComponent(str(args, 'computer_id'))}/snapshots`);
    return { content: [{ type: 'text', text: JSON.stringify(result) }] };
  },
  'computers.snapshots.create': async ({ client, args }) => {
    const id = encodeURIComponent(str(args, 'computer_id'));
    const result = await client.requestJson('POST', `${API}/computers/${id}/snapshots`, {
      stateful: args['stateful'] === true,
    });
    return { content: [{ type: 'text', text: JSON.stringify(result) }] };
  },
  'computers.snapshots.restore': async ({ client, args }) => {
    const id = encodeURIComponent(str(args, 'computer_id'));
    const snap = encodeURIComponent(str(args, 'snapshot_id'));
    const result = await client.requestJson('POST', `${API}/computers/${id}/snapshots/${snap}/restore`, {});
    return { content: [{ type: 'text', text: JSON.stringify(result) }] };
  },
  'computers.snapshots.delete': async ({ client, args }) => {
    const id = encodeURIComponent(str(args, 'computer_id'));
    const snap = encodeURIComponent(str(args, 'snapshot_id'));
    const result = await client.requestJson('DELETE', `${API}/computers/${id}/snapshots/${snap}`);
    return { content: [{ type: 'text', text: JSON.stringify(result) }] };
  },

  // ── Desktop templates (Phase 4) ────────────────────────────────────────────
  'templates.list': async ({ client, args }) => {
    const path = `${API}/desktop-templates${query({ os: optStr(args, 'os'), tag: optStr(args, 'tag') })}`;
    const result = await client.requestJson('GET', path);
    return { content: [{ type: 'text', text: JSON.stringify(result) }] };
  },
  'templates.import': async ({ client, args }) => {
    // The route parses the body as YAML (a JSON superset), so one code path
    // covers both serializations.
    const result = await client.requestRaw(
      'POST',
      `${API}/desktop-templates/import`,
      str(args, 'doc'),
      'application/yaml',
    );
    return { content: [{ type: 'text', text: JSON.stringify(result) }] };
  },
  'templates.build': async ({ client, args }) => {
    const id = encodeURIComponent(str(args, 'template_id'));
    const result = await client.requestJson('POST', `${API}/desktop-templates/${id}/build`, undefined, approvalId(args));
    return { content: [{ type: 'text', text: JSON.stringify(result) }] };
  },
};

/**
 * Execute one tool call against the given client. Shared by the MCP request
 * handler and unit tests (which mock `fetch` and assert the recorded HTTP
 * calls — this is the tool-schema ↔ route-parameter mapping contract).
 */
export async function executeToolCall(
  client: ComputersApiClient,
  name: string,
  args: ToolArgs,
): Promise<CallToolResult> {
  const handler = DISPATCH[name as McpToolName];
  if (!handler) {
    return {
      isError: true,
      content: [{ type: 'text', text: `Unknown tool: ${name}` }],
    };
  }
  try {
    return await handler({ client, args });
  } catch (error) {
    // Surface API denials (confirmation_required / approval_denied) and HTTP
    // errors verbatim so the caller sees the server's own message.
    const text =
      error instanceof ApiError
        ? error.body
        : error instanceof Error
          ? error.message
          : String(error);
    return { isError: true, content: [{ type: 'text', text }] };
  }
}

/**
 * Build the client for the configured credential: a Platform API project key
 * (`alt_live_…` / `alt_test_…`) selects {@link PlatformClient}; anything else
 * keeps the first-party lifecycle client.
 */
export function clientFromEnv(env: NodeJS.ProcessEnv = process.env): ComputersApiClient | PlatformClient {
  const config = configFromEnv(env);
  if (isProjectKey(config.token)) {
    return new PlatformClient({
      baseUrl: (env.ALLTERNIT_PLATFORM_URL ?? 'https://api.allternit.com').replace(/\/+$/, ''),
      apiKey: config.token,
    });
  }
  return new ComputersApiClient(config);
}

export function createComputersMcpServer(
  client: ComputersApiClient | PlatformClient = clientFromEnv(),
): Server {
  const server = new Server(
    { name: 'allternit-computers', version: '0.1.0' },
    { capabilities: { tools: {} } },
  );
  const platform = client instanceof PlatformClient;
  const specs = platform ? [...COMPUTER_TOOL_SPECS, ...PLATFORM_TOOL_SPECS] : COMPUTER_TOOL_SPECS;

  server.setRequestHandler('tools/list', async () => ({
    tools: specs.map(({ name, description, inputSchema }) => ({
      name,
      description,
      inputSchema: inputSchema as Tool['inputSchema'],
    })),
  }));

  server.setRequestHandler('tools/call', async (request) => {
    const args = (request.params.arguments as ToolArgs) ?? {};
    return client instanceof PlatformClient
      ? executePlatformToolCall(client, request.params.name, args)
      : executeToolCall(client, request.params.name, args);
  });

  return server;
}

/**
 * Run the stdio server (bin entry `computers-mcp`). Dual-era: a 2026-07-28
 * client opens with `server/discover`, a 2025-era client with `initialize`.
 * `transport` is for tests; the default is this process's stdio.
 */
export async function runComputersMcpServer(
  client?: ComputersApiClient | PlatformClient,
  transport?: Transport,
): Promise<{ close(): Promise<void> }> {
  const make = () => createComputersMcpServer(client ?? clientFromEnv());
  const handle = serveStdio(make, transport ? { transport } : {});
  if (!transport) {
    let exiting = false;
    const shutdown = (): void => {
      if (exiting) return;
      exiting = true;
      process.exit(0);
    };
    process.stdin.on('end', shutdown);
    process.stdin.on('error', shutdown);
  }
  return handle;
}
