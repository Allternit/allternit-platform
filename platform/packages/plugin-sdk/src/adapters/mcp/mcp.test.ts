/** Dual-era smoke: the MCP adapter answers 2026-07-28 and legacy clients with the same tools. */
import { describe, expect, it } from 'vitest';
import { Client } from '@modelcontextprotocol/client';
import { InMemoryTransport } from '@modelcontextprotocol/server';
import { McpAdapter } from './index';

const plugin: any = {
  manifest: {
    id: 'demo-plugin',
    name: 'Demo',
    version: '1.0.0',
    provides: {
      functions: [
        { name: 'echo', description: 'Echo text', parameters: { type: 'object', properties: { text: { type: 'string', description: 'Text' } }, required: ['text'] } },
      ],
    },
    requires: {},
  },
  initialize: async () => {},
  execute: async (_name: string, params: Record<string, unknown>) => ({ success: true, content: String(params.text) }),
};

describe('McpAdapter dual-era', () => {
  for (const mode of ['legacy', { pin: '2026-07-28' }] as const) {
    it(`serves tools, calls and resources to a ${JSON.stringify(mode)} client`, async () => {
      const [clientSide, serverSide] = InMemoryTransport.createLinkedPair();
      const instance: any = await new McpAdapter().initialize(plugin);
      await instance.start(serverSide);
      const client = new Client({ name: 'smoke', version: '1' }, { versionNegotiation: { mode } });
      await client.connect(clientSide);
      expect(client.getProtocolEra()).toBe(mode === 'legacy' ? 'legacy' : 'modern');
      const { tools } = await client.listTools();
      expect(tools.map((t) => t.name)).toEqual(['echo', '_plugin_info']);
      const res: any = await client.callTool({ name: 'echo', arguments: { text: 'hi' } });
      expect(res.content[0].text).toBe('hi');
      const { resources } = await client.listResources();
      expect(resources.map((r) => r.uri)).toEqual(['plugin://demo-plugin/manifest', 'plugin://demo-plugin/readme']);
      await client.close();
      await instance.stop();
    });
  }
});
