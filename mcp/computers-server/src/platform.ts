/**
 * Platform API mode (hosted computer driver, P6).
 *
 * When the credential is a Platform API project key (`alt_live_…` /
 * `alt_test_…`), the server talks to `https://api.allternit.com/v1/computers…`
 * instead of the first-party `/api/v1` lifecycle backend, and adds four
 * contract tools on top of the 22 lifecycle tools:
 *
 * - `computer_toolset`        POST /v1/computers/{id}/toolset
 * - `computer_toolset_schema` GET  /v1/computers/{id}/toolset/schema
 * - `computer_events`         GET  /v1/computers/{id}/events
 * - `computer_approve`        POST /v1/computers/{id}/approvals/{approval_id}
 *
 * Lifecycle tools with a /v1 counterpart (create/list/get/start/stop/delete)
 * are routed there; the rest return a clear "not available with a project
 * key" error. Old (Clerk) credentials never reach this module.
 *
 * @module platform
 */

import type { CallToolResult } from '@modelcontextprotocol/server';

import type { McpToolSpec } from './tool-spec.js';

const PROJECT_KEY = /^alt_(live|test)_/;

export function isProjectKey(token: string | undefined): token is string {
  return typeof token === 'string' && PROJECT_KEY.test(token);
}

export interface PlatformConfig {
  /** Base URL of the Platform API (default https://api.allternit.com). */
  baseUrl: string;
  /** Project key (`alt_live_…` / `alt_test_…`). */
  apiKey: string;
}

type ToolArgs = Record<string, unknown>;

export const PLATFORM_TOOL_NAMES = [
  'computer_toolset',
  'computer_toolset_schema',
  'computer_events',
  'computer_approve',
] as const;
export type PlatformToolName = (typeof PLATFORM_TOOL_NAMES)[number];

const CID = { type: 'string', description: 'ID of the computer (cmp_…).' } as const;
const TOOLSET = {
  type: 'string',
  enum: ['computer', 'browser'],
  description: 'Which toolset to call.',
} as const;

export const PLATFORM_TOOL_SPECS: Array<Omit<McpToolSpec, 'name'> & { name: PlatformToolName }> = [
  {
    name: 'computer_toolset',
    acceptsApproval: true,
    description:
      'Run one toolset member (e.g. screenshot, left_click, type, navigate) on a hosted computer. POST /v1/computers/{id}/toolset. Images come back as image content. If the action needs approval, the result names the approval id: get it approved, then resend the same call with approval_grant set to that id.',
    inputSchema: {
      type: 'object',
      properties: {
        computer_id: CID,
        toolset: TOOLSET,
        member: { type: 'string', description: 'Toolset member name (see computer_toolset_schema).' },
        input: { type: 'object', description: 'Member input, per the member input_schema.' },
        approval_grant: { type: 'string', description: 'Approval id from an earlier approval_required result.' },
        run_id: { type: 'string' },
        turn_id: { type: 'string' },
        call_index: { type: 'integer' },
      },
      required: ['computer_id', 'toolset', 'member'],
    },
  },
  {
    name: 'computer_toolset_schema',
    acceptsApproval: false,
    description:
      'Get the members, input schemas, risk and approval needs for a toolset on a computer. GET /v1/computers/{id}/toolset/schema.',
    inputSchema: {
      type: 'object',
      properties: { computer_id: CID, toolset: TOOLSET },
      required: ['computer_id'],
    },
  },
  {
    name: 'computer_events',
    acceptsApproval: false,
    description:
      'Poll the action event log for a computer. GET /v1/computers/{id}/events?after=&limit=. Pass next_cursor back as after.',
    inputSchema: {
      type: 'object',
      properties: {
        computer_id: CID,
        after: { type: 'string', description: 'Cursor from a previous next_cursor.' },
        limit: { type: 'integer', minimum: 1, maximum: 100 },
      },
      required: ['computer_id'],
    },
  },
  {
    name: 'computer_approve',
    acceptsApproval: false,
    description:
      "Approve a pending action. POST /v1/computers/{id}/approvals/{approval_id}. Works for API keys only when the project's approval_mode is api_key; otherwise the project owner approves in the console.",
    inputSchema: {
      type: 'object',
      properties: {
        computer_id: CID,
        approval_id: { type: 'string', description: 'The approval.id from an approval_required result.' },
      },
      required: ['computer_id', 'approval_id'],
    },
  },
];

export class PlatformApiError extends Error {
  constructor(
    readonly status: number,
    readonly body: string,
  ) {
    super(`Platform API request failed (${status}): ${body}`);
    this.name = 'PlatformApiError';
  }
}

export class PlatformClient {
  constructor(readonly config: PlatformConfig) {}

  async request<T = unknown>(method: string, path: string, body?: unknown): Promise<T> {
    const headers: Record<string, string> = { authorization: `Bearer ${this.config.apiKey}` };
    if (body !== undefined) headers['content-type'] = 'application/json';
    const response = await fetch(`${this.config.baseUrl}/v1${path}`, {
      method,
      headers,
      body: body === undefined ? undefined : JSON.stringify(body),
    });
    const text = await response.text();
    if (!response.ok) throw new PlatformApiError(response.status, text);
    return (text ? JSON.parse(text) : null) as T;
  }
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

function qs(params: Record<string, string | undefined>): string {
  const entries = Object.entries(params).filter((e): e is [string, string] => Boolean(e[1]));
  return entries.length ? `?${entries.map(([k, v]) => `${k}=${encodeURIComponent(v)}`).join('&')}` : '';
}

function json(value: unknown): CallToolResult {
  return { content: [{ type: 'text', text: JSON.stringify(value) }] };
}

interface ToolsetContent {
  type: string;
  text?: string;
  media_type?: string;
  data?: string;
}

interface ToolsetResult {
  is_error?: boolean;
  content?: ToolsetContent[];
  [key: string]: unknown;
}

/** Executor result → MCP content (text stays text, images become image content). */
function toolsetToMcp(result: ToolsetResult): CallToolResult {
  const content: CallToolResult['content'] = [];
  for (const item of result.content ?? []) {
    if (item.type === 'image' && item.data) {
      content.push({ type: 'image', data: item.data, mimeType: item.media_type ?? 'image/png' });
    } else if (item.type === 'text' && typeof item.text === 'string') {
      content.push({ type: 'text', text: item.text });
    }
  }
  const { content: _omit, ...rest } = result;
  content.push({ type: 'text', text: JSON.stringify(rest) });
  return { isError: result.is_error === true, content };
}

/** 409 approval_required → text telling the agent what to do next. */
function approvalText(computerId: string, body: string): string | undefined {
  try {
    const parsed = JSON.parse(body) as {
      error?: { code?: string; message?: string };
      approval?: { id?: string; member?: string; toolset?: string; risk?: string };
    };
    if (parsed.error?.code !== 'approval_required' || !parsed.approval?.id) return undefined;
    const a = parsed.approval;
    return [
      `Approval required for ${a.toolset ?? 'toolset'}.${a.member ?? 'action'} on ${computerId} (risk: ${a.risk ?? 'unknown'}).`,
      `Approval id: ${a.id}`,
      `The project owner must approve it in the Allternit console, or call computer_approve with computer_id=${computerId} and approval_id=${a.id} if the project's approval_mode is api_key.`,
      `Then resend the same computer_toolset call with approval_grant="${a.id}".`,
    ].join('\n');
  } catch {
    return undefined;
  }
}

const LIFECYCLE_V1: Record<string, (c: PlatformClient, a: ToolArgs) => Promise<unknown>> = {
  'computers.create': (c, a) => {
    const body: Record<string, unknown> = {};
    if (optStr(a, 'name')) body.name = a.name;
    if (optStr(a, 'account_id')) body.account_id = a.account_id;
    if (a.metadata && typeof a.metadata === 'object') body.metadata = a.metadata;
    return c.request('POST', '/computers', body);
  },
  'computers.list': (c, a) =>
    c.request('GET', `/computers${qs({ after: optStr(a, 'after'), limit: a.limit === undefined ? undefined : String(a.limit) })}`),
  'computers.get': (c, a) => c.request('GET', `/computers/${encodeURIComponent(str(a, 'computer_id'))}`),
  'computers.start': (c, a) => c.request('POST', `/computers/${encodeURIComponent(str(a, 'computer_id'))}/start`),
  'computers.stop': (c, a) => c.request('POST', `/computers/${encodeURIComponent(str(a, 'computer_id'))}/stop`),
  'computers.delete': (c, a) => c.request('DELETE', `/computers/${encodeURIComponent(str(a, 'computer_id'))}`),
};

/** Execute one tool call in project-key mode. */
export async function executePlatformToolCall(
  client: PlatformClient,
  name: string,
  args: ToolArgs,
): Promise<CallToolResult> {
  try {
    if (name === 'computer_toolset') {
      const id = str(args, 'computer_id');
      const body: Record<string, unknown> = { toolset: str(args, 'toolset'), member: str(args, 'member') };
      for (const key of ['input', 'approval_grant', 'run_id', 'turn_id', 'call_index']) {
        if (args[key] !== undefined) body[key] = args[key];
      }
      try {
        const result = await client.request<ToolsetResult>('POST', `/computers/${encodeURIComponent(id)}/toolset`, body);
        return toolsetToMcp(result ?? {});
      } catch (error) {
        if (error instanceof PlatformApiError && error.status === 409) {
          const text = approvalText(id, error.body);
          if (text) return { isError: true, content: [{ type: 'text', text }] };
        }
        throw error;
      }
    }
    if (name === 'computer_toolset_schema') {
      const id = encodeURIComponent(str(args, 'computer_id'));
      return json(await client.request('GET', `/computers/${id}/toolset/schema${qs({ toolset: optStr(args, 'toolset') ?? 'computer' })}`));
    }
    if (name === 'computer_events') {
      const id = encodeURIComponent(str(args, 'computer_id'));
      const limit = args.limit === undefined ? undefined : String(args.limit);
      return json(await client.request('GET', `/computers/${id}/events${qs({ after: optStr(args, 'after'), limit })}`));
    }
    if (name === 'computer_approve') {
      const id = encodeURIComponent(str(args, 'computer_id'));
      const approval = encodeURIComponent(str(args, 'approval_id'));
      return json(await client.request('POST', `/computers/${id}/approvals/${approval}`));
    }
    const lifecycle = LIFECYCLE_V1[name];
    if (lifecycle) return json(await lifecycle(client, args));
    return {
      isError: true,
      content: [
        {
          type: 'text',
          text: `${name} is not available with a Platform API project key. With a project key use computers.create/list/get/start/stop/delete and the computer_* tools.`,
        },
      ],
    };
  } catch (error) {
    const text =
      error instanceof PlatformApiError ? error.body : error instanceof Error ? error.message : String(error);
    return { isError: true, content: [{ type: 'text', text }] };
  }
}
