/**
 * ApiMemoryBackend against a stub allternit-api: writes go to the adapter
 * routes, recall comes from the API's memory plane, and the local index only
 * answers when the API is down.
 */
import { describe, it, expect, beforeAll, afterAll, beforeEach } from 'vitest';
import * as http from 'http';
import type { AddressInfo } from 'net';
import { ApiMemoryBackend } from './api-memory-backend.js';
import { VectorStore } from './vector-store.js';
import type { Memory } from '../types/memory.types.js';

function memory(id: string, content: string, summary: string): Memory {
  const now = new Date().toISOString();
  return {
    id, content, summary, entities: [], topics: [], importance: 'medium', status: 'raw',
    source: 'test', sourceType: 'text', createdAt: now, updatedAt: now, metadata: {},
  };
}

/** A local index whose recall is fixed, so the fallback is observable. */
class FakeLocal extends VectorStore {
  added: string[] = [];
  removed: string[] = [];
  constructor() { super(undefined, 'off'); }
  async addMemory(m: Memory) { this.added.push(m.id); }
  removeMemory(id: string) { this.removed.push(id); }
  async similaritySearch(): Promise<Memory[]> { return [memory('local-1', 'from the local index', 'local')]; }
}

describe('ApiMemoryBackend', () => {
  let server: http.Server;
  let url: string;
  let requests: { url: string; auth?: string; body: any }[];
  let failSearch = false;

  beforeAll(async () => {
    server = http.createServer((req, res) => {
      let raw = '';
      req.on('data', (c) => (raw += c));
      req.on('end', () => {
        const body = raw ? JSON.parse(raw) : {};
        requests.push({ url: req.url ?? '', auth: req.headers.authorization, body });
        res.setHeader('Content-Type', 'application/json');
        if (failSearch && req.url?.endsWith('/search')) {
          res.writeHead(503);
          return res.end('{}');
        }
        switch (req.url) {
          case '/api/v1/memory/adapters/upsert':
            return res.end(JSON.stringify({ created: body.items.length, updated: 0, unchanged: 0, fact_ids: [] }));
          case '/api/v1/memory/adapters/delete':
            return res.end(JSON.stringify({ retired: body.external_ids.length }));
          case '/api/v1/memory/adapters/search':
            return res.end(JSON.stringify({ items: [
              { id: 'fact_1', type: 'fact', text: 'Prefers tea', score: 0.03, source: 'memory-agent', external_id: 'm1' },
              { id: 'fact_2', type: 'fact', text: 'Works in Lisbon', score: 0.02, source: 'gizzi.brain', external_id: 'x' },
            ] }));
          case '/api/v1/memory/adapters/chunks/search':
            return res.end(JSON.stringify({ items: [
              { document_id: 'mdoc_ext_1', title: 'Trip notes', text: 'Tea houses in Porto', score: 0.03, source: 'open-notebook', collection: 'nb1', external_id: 'c1' },
            ] }));
          default:
            res.writeHead(404);
            return res.end('{}');
        }
      });
    });
    await new Promise<void>((r) => server.listen(0, '127.0.0.1', () => r()));
    url = `http://127.0.0.1:${(server.address() as AddressInfo).port}`;
  });

  afterAll(() => new Promise<void>((r) => server.close(() => r())));
  beforeEach(() => { requests = []; failSearch = false; });

  it('mirrors writes to the adapter routes with the bearer token', async () => {
    const local = new FakeLocal();
    const backend = new ApiMemoryBackend({ baseUrl: url, token: 'at-test', local });
    await backend.addMemory(memory('m1', 'Said they prefer tea over coffee', 'Prefers tea'));
    await backend.removeMemory('m1');
    expect(local.added).toEqual(['m1']);
    expect(local.removed).toEqual(['m1']);
    expect(requests.map((r) => r.url)).toEqual(['/api/v1/memory/adapters/upsert', '/api/v1/memory/adapters/delete']);
    expect(requests.every((r) => r.auth === 'Bearer at-test')).toBe(true);
    expect(requests[0].body).toMatchObject({ source: 'memory-agent', items: [{ external_id: 'm1' }] });
    // Nothing in the payload names a user: the API scopes by the token.
    expect(JSON.stringify(requests.map((r) => r.body))).not.toMatch(/user_id/);
  });

  it('does not resend writes when the store already mirrors them', async () => {
    const local = new FakeLocal();
    const backend = new ApiMemoryBackend({ baseUrl: url, token: 't', local, mirrorWrites: false });
    await backend.addMemory(memory('m2', 'c', 's'));
    await backend.removeMemory('m2');
    expect(requests).toEqual([]);
    expect(local.added).toEqual(['m2']);
  });

  it('recalls facts and document chunks from the API plane', async () => {
    const own = memory('m1', 'Said they prefer tea over coffee', 'Prefers tea');
    const backend = new ApiMemoryBackend({ baseUrl: url, token: 't', local: new FakeLocal(), lookup: (id) => (id === 'm1' ? own : undefined) });
    const hits = await backend.similaritySearch('tea', 5);
    expect(backend.lastSearchSource).toBe('api');
    expect(hits.map((m) => m.id)).toEqual(['m1', 'plane:document:mdoc_ext_1', 'plane:fact:fact_2']);
    expect(hits[0]).toBe(own);
    expect(hits[1]).toMatchObject({ content: 'Tea houses in Porto', source: 'open-notebook' });
    expect(requests.map((r) => r.body)).toEqual([{ query: 'tea', limit: 5 }, { query: 'tea', limit: 5 }]);
  });

  it('falls back to the local index when the API is down or errors', async () => {
    failSearch = true;
    const backend = new ApiMemoryBackend({ baseUrl: url, token: 't', local: new FakeLocal() });
    expect((await backend.similaritySearch('tea')).map((m) => m.id)).toEqual(['local-1']);
    expect(backend.lastSearchSource).toBe('local');
    const offline = new ApiMemoryBackend({ baseUrl: 'http://127.0.0.1:9', token: 't', local: new FakeLocal(), timeoutMs: 2000 });
    expect((await offline.similaritySearch('tea')).map((m) => m.id)).toEqual(['local-1']);
  });

  it('is configured from ALLTERNIT_API_URL + ALLTERNIT_API_TOKEN', () => {
    const saved = { ...process.env };
    try {
      delete process.env.ALLTERNIT_API_TOKEN;
      expect(ApiMemoryBackend.fromEnv(new FakeLocal())).toBeUndefined();
      process.env.ALLTERNIT_API_TOKEN = 'at-x';
      process.env.ALLTERNIT_API_URL = 'http://api.local:8013/';
      expect(ApiMemoryBackend.fromEnv(new FakeLocal())?.baseUrl).toBe('http://api.local:8013');
      process.env.MEMORY_AGENT_KERNEL = '0';
      expect(ApiMemoryBackend.fromEnv(new FakeLocal())).toBeUndefined();
    } finally {
      process.env = saved;
    }
  });
});
