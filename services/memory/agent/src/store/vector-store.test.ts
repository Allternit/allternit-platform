/**
 * VectorStore embeds through the shared Allternit embedding server
 * (OpenAI-compatible /v1/embeddings at ALLTERNIT_EMBED_URL). A stub server
 * returns fixed vectors per text and records each request.
 */

import { describe, it, expect, beforeAll, afterAll, beforeEach } from 'vitest';
import * as http from 'http';
import type { AddressInfo } from 'net';
import { VectorStore, DEFAULT_EMBED_MODEL } from './vector-store.js';
import type { Memory } from '../types/memory.types.js';

const VECTORS: Record<string, number[]> = {
  'refund policy 30 days refunds': [1, 0, 0],
  'office closed holidays closures': [0, 1, 0],
  'money back?': [0.9, 0.1, 0],
};

function memory(id: string, content: string, summary: string): Memory {
  const now = new Date().toISOString();
  return {
    id, content, summary, entities: [], topics: [], importance: 'medium' as Memory['importance'],
    status: 'active' as Memory['status'], source: 'test', sourceType: 'text' as Memory['sourceType'],
    createdAt: now, updatedAt: now, metadata: {},
  };
}

describe('VectorStore (shared embedding server)', () => {
  let server: http.Server;
  let url: string;
  let requests: any[];

  beforeAll(async () => {
    server = http.createServer((req, res) => {
      let raw = '';
      req.on('data', (c) => (raw += c));
      req.on('end', () => {
        const body = JSON.parse(raw);
        requests.push({ url: req.url, body });
        const data = (body.input as string[]).map((t, index) => ({ index, embedding: VECTORS[t] ?? [0, 0, 1] }));
        res.writeHead(200, { 'Content-Type': 'application/json' });
        res.end(JSON.stringify({ object: 'list', model: body.model, data }));
      });
    });
    await new Promise<void>((r) => server.listen(0, '127.0.0.1', () => r()));
    url = `http://127.0.0.1:${(server.address() as AddressInfo).port}`;
  });

  afterAll(() => new Promise<void>((r) => server.close(() => r())));
  beforeEach(() => { requests = []; });

  it('defaults to EmbeddingGemma 2 and tags documents and queries', async () => {
    const store = new VectorStore(undefined, url);
    await store.addMemory(memory('a', 'refund policy 30 days', 'refunds'));
    await store.addMemory(memory('b', 'office closed holidays', 'closures'));
    const hits = await store.similaritySearch('money back?', 1);

    expect(hits.map((m) => m.id)).toEqual(['a']);
    expect(requests.every((r) => r.url === '/v1/embeddings' && r.body.model === DEFAULT_EMBED_MODEL)).toBe(true);
    expect(requests.map((r) => r.body.input_type)).toEqual(['document', 'document', 'query']);
    expect(store.getStats()).toEqual({ count: 2, model: 'google/embeddinggemma-2' });
  });

  it('returns [] when the endpoint is off or down', async () => {
    expect(await new VectorStore(undefined, 'off').embed('x')).toEqual([]);
    expect(await new VectorStore(undefined, 'http://127.0.0.1:9').embed('x')).toEqual([]);
  });
});
