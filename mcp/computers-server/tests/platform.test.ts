/**
 * Project-key (Platform API /v1) mode: client selection, toolset image
 * mapping, the 409 approval text, and lifecycle routing.
 */

import { describe, it, expect, afterEach } from 'vitest';

import { ComputersApiClient } from '../src/client.js';
import { executePlatformToolCall, isProjectKey, PlatformClient } from '../src/platform.js';
import { clientFromEnv } from '../src/server.js';
import { jsonResponse, mockFetch } from './mock-fetch.js';

const BASE = 'https://api.test';

describe('platform mode', () => {
  const originalFetch = globalThis.fetch;
  afterEach(() => {
    globalThis.fetch = originalFetch;
  });

  it('detects project keys and picks the /v1 client', () => {
    expect(isProjectKey('alt_live_abc')).toBe(true);
    expect(isProjectKey('alt_test_abc')).toBe(true);
    expect(isProjectKey('sk_clerk')).toBe(false);
    expect(clientFromEnv({ ALLTERNIT_API_KEY: 'alt_test_x' })).toBeInstanceOf(PlatformClient);
    expect(clientFromEnv({ ALLTERNIT_TOKEN: 'clerk-tok' })).toBeInstanceOf(ComputersApiClient);
    const c = clientFromEnv({ ALLTERNIT_TOKEN: 'alt_live_y', ALLTERNIT_PLATFORM_URL: 'https://x/' });
    expect((c as PlatformClient).config.baseUrl).toBe('https://x');
  });

  it('maps toolset results to MCP text + image content', async () => {
    const calls = mockFetch(() =>
      jsonResponse({
        is_error: false,
        content: [
          { type: 'text', text: 'ok' },
          { type: 'image', media_type: 'image/png', data: 'AAAA' },
        ],
        screen: { width: 1280, height: 800 },
      }),
    );
    const client = new PlatformClient({ baseUrl: BASE, apiKey: 'alt_test_k' });
    const result = await executePlatformToolCall(client, 'computer_toolset', {
      computer_id: 'cmp_1',
      toolset: 'computer',
      member: 'screenshot',
      approval_grant: 'apr_1',
    });
    expect(calls[0]?.url).toBe(`${BASE}/v1/computers/cmp_1/toolset`);
    expect(calls[0]?.headers.authorization).toBe('Bearer alt_test_k');
    expect(JSON.parse(calls[0]?.body ?? '{}')).toEqual({
      toolset: 'computer',
      member: 'screenshot',
      approval_grant: 'apr_1',
    });
    expect(result.isError).toBe(false);
    expect(result.content[0]).toEqual({ type: 'text', text: 'ok' });
    expect(result.content[1]).toEqual({ type: 'image', data: 'AAAA', mimeType: 'image/png' });
  });

  it('turns a 409 into approval instructions', async () => {
    mockFetch(() =>
      jsonResponse(
        {
          error: { type: 'conflict_error', code: 'approval_required', message: 'needs approval', param: null },
          approval: { id: 'apr_9', member: 'left_click', toolset: 'computer', risk: 'medium' },
        },
        409,
      ),
    );
    const client = new PlatformClient({ baseUrl: BASE, apiKey: 'alt_test_k' });
    const result = await executePlatformToolCall(client, 'computer_toolset', {
      computer_id: 'cmp_1',
      toolset: 'computer',
      member: 'left_click',
    });
    expect(result.isError).toBe(true);
    const text = (result.content[0] as { text: string }).text;
    expect(text).toContain('apr_9');
    expect(text).toContain('approval_grant');
  });

  it('routes lifecycle tools to /v1 and refuses ones without a /v1 route', async () => {
    const calls = mockFetch(() => jsonResponse({ id: 'cmp_1', object: 'computer' }));
    const client = new PlatformClient({ baseUrl: BASE, apiKey: 'alt_test_k' });
    await executePlatformToolCall(client, 'computers.delete', { computer_id: 'cmp_1' });
    expect(calls[0]).toMatchObject({ method: 'DELETE', url: `${BASE}/v1/computers/cmp_1` });
    const refused = await executePlatformToolCall(client, 'computers.shell', { computer_id: 'cmp_1' });
    expect(refused.isError).toBe(true);
    expect(calls).toHaveLength(1);
  });
});
