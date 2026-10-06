/**
 * Dual-era smoke: the same server answers a 2026-07-28 client (server/discover,
 * per-request _meta) and a 2025-era client (initialize), with the same tools in
 * the same order.
 */
import { describe, expect, it } from 'vitest';
import { Client } from '@modelcontextprotocol/client';
import { InMemoryTransport } from '@modelcontextprotocol/server';

import { ComputersApiClient } from '../src/client.js';
import { runComputersMcpServer } from '../src/server.js';
import { COMPUTER_TOOL_SPECS } from '../src/tool-spec.js';

const EXPECTED = COMPUTER_TOOL_SPECS.map((t) => t.name);

describe('computers-mcp dual-era', () => {
  for (const mode of ['legacy', { pin: '2026-07-28' }] as const) {
    it(`serves tools/list to a ${JSON.stringify(mode)} client`, async () => {
      const [clientSide, serverSide] = InMemoryTransport.createLinkedPair();
      const handle = await runComputersMcpServer(new ComputersApiClient({ baseUrl: 'http://unused.test' }), serverSide);
      const client = new Client({ name: 'smoke', version: '1' }, { versionNegotiation: { mode } });
      await client.connect(clientSide);
      expect(client.getProtocolEra()).toBe(mode === 'legacy' ? 'legacy' : 'modern');
      if (mode !== 'legacy') expect(client.getNegotiatedProtocolVersion()).toBe('2026-07-28');
      const { tools } = await client.listTools();
      expect(tools.map((t) => t.name)).toEqual(EXPECTED);
      const again = await client.listTools();
      expect(again.tools).toEqual(tools);
      await client.close();
      await handle.close();
    });
  }
});
