/** Dual-era smoke: 2026-07-28 (server/discover + _meta) and legacy initialize clients see the same tools. */
import { mkdtempSync, mkdirSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { describe, expect, it } from 'vitest';
import { Client } from '@modelcontextprotocol/client';
import { InMemoryTransport } from '@modelcontextprotocol/server';
import { serveStdio } from '@modelcontextprotocol/server/stdio';

const root = mkdtempSync(join(tmpdir(), 'docs-mcp-'));
mkdirSync(join(root, 'guides'));
writeFileSync(join(root, 'guides', 'start.md'), '# Getting started\nHello.');
process.env.ALLTERNIT_DOCS_ROOT = root;
const { createDocsServer } = await import('../src/index.js');

describe('docs-mcp dual-era', () => {
  for (const mode of ['legacy', { pin: '2026-07-28' }] as const) {
    it(`lists and calls tools for a ${JSON.stringify(mode)} client`, async () => {
      const [clientSide, serverSide] = InMemoryTransport.createLinkedPair();
      const handle = serveStdio(() => createDocsServer(), { transport: serverSide });
      const client = new Client({ name: 'smoke', version: '1' }, { versionNegotiation: { mode } });
      await client.connect(clientSide);
      expect(client.getProtocolEra()).toBe(mode === 'legacy' ? 'legacy' : 'modern');
      const { tools } = await client.listTools();
      expect(tools.map((t) => t.name)).toEqual(['search_docs', 'read_doc', 'list_docs', 'get_api_reference']);
      const res: any = await client.callTool({ name: 'read_doc', arguments: { path: 'guides/start.md' } });
      expect(res.content[0].text).toContain('Getting started');
      await client.close();
      await handle.close();
    });
  }
});
