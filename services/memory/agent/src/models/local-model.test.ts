/**
 * Tests for the LocalModelManager provider switch (MEMORY_LLM_BASE_URL)
 *
 * Uses a stub node:http server implementing both the OpenAI-compatible
 * (/v1/chat/completions) and Ollama (/api/chat) shapes, and records every
 * request so tests can assert which path was taken and with what body.
 */

import { describe, it, expect, beforeAll, afterAll, beforeEach, afterEach } from 'vitest';
import * as http from 'http';
import type { AddressInfo } from 'net';
import { LocalModelManager, MODEL_PRESETS } from './local-model.js';
import { VectorStore } from '../store/vector-store.js';

interface RecordedRequest {
  url: string;
  method: string;
  body: any;
}

describe('LocalModelManager provider switch', () => {
  let server: http.Server;
  let baseUrl: string;
  let requests: RecordedRequest[];
  let failWithStatus: number | null;

  const savedEnv = { ...process.env };

  beforeAll(async () => {
    server = http.createServer((req, res) => {
      let raw = '';
      req.on('data', (chunk) => (raw += chunk));
      req.on('end', () => {
        let body: any = null;
        try {
          body = raw ? JSON.parse(raw) : null;
        } catch {
          body = raw;
        }
        requests.push({ url: req.url || '', method: req.method || '', body });

        if (failWithStatus !== null && req.url === '/v1/chat/completions') {
          res.writeHead(failWithStatus, { 'Content-Type': 'application/json' });
          res.end(JSON.stringify({ error: 'stub failure' }));
          return;
        }

        if (req.url === '/v1/chat/completions') {
          res.writeHead(200, { 'Content-Type': 'application/json' });
          res.end(
            JSON.stringify({
              choices: [{ message: { role: 'assistant', content: 'mlx-response' } }],
            })
          );
        } else if (req.url === '/api/chat') {
          res.writeHead(200, { 'Content-Type': 'application/json' });
          res.end(
            JSON.stringify({
              model: body?.model,
              message: { role: 'assistant', content: 'ollama-response' },
              done: true,
            })
          );
        } else {
          res.writeHead(404, { 'Content-Type': 'application/json' });
          res.end(JSON.stringify({ error: 'not found' }));
        }
      });
    });
    await new Promise<void>((resolve) => server.listen(0, '127.0.0.1', resolve));
    const { port } = server.address() as AddressInfo;
    baseUrl = `http://127.0.0.1:${port}`;
  });

  afterAll(async () => {
    await new Promise<void>((resolve) => server.close(() => resolve()));
    process.env = savedEnv;
  });

  beforeEach(() => {
    requests = [];
    failWithStatus = null;
    delete process.env.MEMORY_LLM_BASE_URL;
    delete process.env.MEMORY_LLM_MODEL;
  });

  afterEach(() => {
    delete process.env.MEMORY_LLM_BASE_URL;
    delete process.env.MEMORY_LLM_MODEL;
  });

  it('routes generation to the OpenAI-compatible endpoint when MEMORY_LLM_BASE_URL is set', async () => {
    process.env.MEMORY_LLM_BASE_URL = `${baseUrl}/v1`;
    process.env.MEMORY_LLM_MODEL = 'mlx-test-model';

    const manager = new LocalModelManager();
    const result = await manager.generate('hello', 'you are helpful');

    expect(result).toBe('mlx-response');

    const chatReqs = requests.filter((r) => r.url === '/v1/chat/completions');
    expect(chatReqs).toHaveLength(1);
    expect(chatReqs[0].method).toBe('POST');
    expect(chatReqs[0].body.model).toBe('mlx-test-model');
    expect(chatReqs[0].body.messages).toEqual([
      { role: 'system', content: 'you are helpful' },
      { role: 'user', content: 'hello' },
    ]);
    // Sampling params come from the preset (ingest default)
    expect(chatReqs[0].body.temperature).toBe(MODEL_PRESETS.ingest.temperature);
    expect(chatReqs[0].body.top_p).toBe(MODEL_PRESETS.ingest.topP);
    expect(chatReqs[0].body.max_tokens).toBe(MODEL_PRESETS.ingest.numPredict);

    // No Ollama generate call
    expect(requests.filter((r) => r.url === '/api/chat')).toHaveLength(0);
  });

  it("defaults the MLX model name to 'qwen3-4b-instruct' when MEMORY_LLM_MODEL is unset", async () => {
    process.env.MEMORY_LLM_BASE_URL = `${baseUrl}/v1`;

    const manager = new LocalModelManager();
    await manager.generate('hello');

    const chatReqs = requests.filter((r) => r.url === '/v1/chat/completions');
    expect(chatReqs).toHaveLength(1);
    expect(chatReqs[0].body.model).toBe('qwen3-4b-instruct');
  });

  it('uses the Ollama generate path with preset model names when the env is unset', async () => {
    const { port } = server.address() as AddressInfo;
    const manager = new LocalModelManager('127.0.0.1', port);
    const result = await manager.generate('hello');

    expect(result).toBe('ollama-response');

    const chatReqs = requests.filter((r) => r.url === '/api/chat');
    expect(chatReqs).toHaveLength(1);
    expect(chatReqs[0].body.model).toBe(MODEL_PRESETS.ingest.name);
    expect(chatReqs[0].body.options.num_predict).toBe(MODEL_PRESETS.ingest.numPredict);

    expect(requests.filter((r) => r.url === '/v1/chat/completions')).toHaveLength(0);
  });

  it('falls back to Ollama on non-2xx from the MLX endpoint', async () => {
    const { port } = server.address() as AddressInfo;
    process.env.MEMORY_LLM_BASE_URL = `http://127.0.0.1:${port}/v1`;
    failWithStatus = 500;

    const manager = new LocalModelManager('127.0.0.1', port);
    const result = await manager.generate('hello');

    expect(result).toBe('ollama-response');

    const mlxReqs = requests.filter((r) => r.url === '/v1/chat/completions');
    expect(mlxReqs).toHaveLength(1);
    expect(mlxReqs[0].body.model).toBe('qwen3-4b-instruct');

    const ollamaReqs = requests.filter((r) => r.url === '/api/chat');
    expect(ollamaReqs).toHaveLength(1);
    expect(ollamaReqs[0].body.model).toBe(MODEL_PRESETS.ingest.name);
  });

  it('falls back to Ollama when the MLX endpoint is unreachable', async () => {
    const { port } = server.address() as AddressInfo;
    // Port 1 is never listening for the MLX path; Ollama points at the stub.
    process.env.MEMORY_LLM_BASE_URL = 'http://127.0.0.1:1/v1';

    const manager = new LocalModelManager('127.0.0.1', port);
    const result = await manager.generate('hello');

    expect(result).toBe('ollama-response');

    const ollamaReqs = requests.filter((r) => r.url === '/api/chat');
    expect(ollamaReqs).toHaveLength(1);
    expect(ollamaReqs[0].body.model).toBe(MODEL_PRESETS.ingest.name);
  });

  it('keeps embeddings on Ollama even when MEMORY_LLM_BASE_URL is set', async () => {
    process.env.MEMORY_LLM_BASE_URL = `${baseUrl}/v1`;

    // VectorStore owns the embeddings path and is hardcoded to Ollama at
    // localhost:11434. If Ollama is running it returns real embeddings; if
    // not, embed() falls back to [] (its documented failure mode). The
    // invariant under test is that embeddings never touch the configured
    // MLX endpoint.
    const vectorStore = new VectorStore();
    const embedding = await vectorStore.embed('test text');

    expect(Array.isArray(embedding)).toBe(true);
    expect(requests).toHaveLength(0);
  });
});
