/**
 * API memory backend (C4: one memory plane).
 *
 * allternit-api's memory plane (FTS5 + EmbeddingGemma 2 vectors over facts,
 * entities, observations and document chunks) is the single source of truth
 * for semantic recall. This backend:
 * - writes: mirrors addMemory/removeMemory to `/api/v1/memory/adapters/*`
 *   (source `memory-agent`, external id = memory id) through KernelAdapter.
 *   When the SQLite store already mirrors every write (MemoryStore does
 *   whenever ALLTERNIT_API_TOKEN is set) pass `mirrorWrites: false`, so a
 *   memory is sent once.
 * - reads: similaritySearch asks `/api/v1/memory/adapters/search` (the
 *   user's facts/entities/observations) and `/api/v1/memory/adapters/chunks/search`
 *   (their document chunks, e.g. open-notebook passages), so recall here sees
 *   what an Agency run sees.
 * - offline: the local in-memory VectorStore is kept up to date and answers
 *   only when the API is unreachable or errors.
 *
 * Scope: the API derives the user from the bearer token
 * (ALLTERNIT_API_TOKEN); nothing here names a user, so this service can
 * only ever read and write the token owner's memory.
 */
import type { Memory } from '../types/memory.types.js';
import { KernelAdapter, KERNEL_SOURCE } from './kernel-adapter.js';
import type { VectorStore } from './vector-store.js';

export const DEFAULT_API_URL = 'http://127.0.0.1:8013';

interface FactHit {
  id: string;
  type: string;
  text: string;
  score: number;
  source?: string | null;
  external_id?: string | null;
}

interface ChunkHit {
  document_id: string;
  title: string;
  text: string;
  score: number;
  source?: string | null;
  collection?: string | null;
  external_id?: string | null;
}

export interface ApiMemoryBackendOptions {
  baseUrl: string;
  token: string;
  local: VectorStore;
  /** Resolves a memory-agent hit to the full local memory. */
  lookup?: (id: string) => Memory | undefined;
  /** Send writes to the API (false when MemoryStore already mirrors them). */
  mirrorWrites?: boolean;
  /** Also recall document chunks (default true). */
  includeDocuments?: boolean;
  fetchImpl?: typeof fetch;
  timeoutMs?: number;
}

function synthetic(id: string, text: string, source: string, metadata: Record<string, unknown>): Memory {
  const now = new Date().toISOString();
  return {
    id,
    content: text,
    summary: text.length > 200 ? `${text.slice(0, 197)}...` : text,
    entities: [],
    topics: [],
    importance: 'medium',
    status: 'processed',
    source,
    sourceType: 'text',
    createdAt: now,
    updatedAt: now,
    metadata,
  };
}

export class ApiMemoryBackend {
  readonly baseUrl: string;
  private readonly token: string;
  private readonly local: VectorStore;
  private readonly lookup?: (id: string) => Memory | undefined;
  private readonly mirrorWrites: boolean;
  private readonly includeDocuments: boolean;
  private readonly fetchImpl: typeof fetch;
  private readonly timeoutMs: number;
  private readonly adapter: KernelAdapter;
  /** Where the last search was answered from. */
  lastSearchSource: 'api' | 'local' | undefined;

  constructor(opts: ApiMemoryBackendOptions) {
    this.baseUrl = opts.baseUrl.replace(/\/+$/, '');
    this.token = opts.token;
    this.local = opts.local;
    this.lookup = opts.lookup;
    this.mirrorWrites = opts.mirrorWrites ?? true;
    this.includeDocuments = opts.includeDocuments ?? true;
    this.fetchImpl = opts.fetchImpl ?? fetch;
    this.timeoutMs = opts.timeoutMs ?? 10_000;
    this.adapter = new KernelAdapter(this.baseUrl, this.token, this.fetchImpl);
  }

  /**
   * From ALLTERNIT_API_URL + ALLTERNIT_API_TOKEN (the same env KernelAdapter
   * uses); undefined when no token is set or MEMORY_AGENT_KERNEL=0.
   */
  static fromEnv(
    local: VectorStore,
    opts: Partial<Omit<ApiMemoryBackendOptions, 'baseUrl' | 'token' | 'local'>> = {},
  ): ApiMemoryBackend | undefined {
    const token = process.env.ALLTERNIT_API_TOKEN?.trim();
    if (!token || process.env.MEMORY_AGENT_KERNEL === '0') return undefined;
    const baseUrl = process.env.ALLTERNIT_API_URL ?? DEFAULT_API_URL;
    return new ApiMemoryBackend({ ...opts, baseUrl, token, local });
  }

  async addMemory(memory: Memory): Promise<void> {
    await Promise.all([
      this.local.addMemory(memory),
      this.mirrorWrites ? this.adapter.upsert([memory]) : Promise.resolve(true),
    ]);
  }

  async removeMemory(memoryId: string): Promise<void> {
    this.local.removeMemory(memoryId);
    if (this.mirrorWrites) await this.adapter.remove([memoryId]);
  }

  private async post<T>(path: string, body: unknown): Promise<T> {
    const r = await this.fetchImpl(`${this.baseUrl}${path}`, {
      method: 'POST',
      headers: { Authorization: `Bearer ${this.token}`, 'Content-Type': 'application/json' },
      body: JSON.stringify(body),
      signal: AbortSignal.timeout(this.timeoutMs),
    });
    if (!r.ok) throw new Error(`${path} -> HTTP ${r.status}`);
    return (await r.json()) as T;
  }

  private factToMemory(h: FactHit): Memory {
    if (h.source === KERNEL_SOURCE && h.external_id) {
      const own = this.lookup?.(h.external_id);
      if (own) return own;
      return synthetic(h.external_id, h.text, KERNEL_SOURCE, { plane: { type: h.type, id: h.id, score: h.score } });
    }
    return synthetic(`plane:${h.type}:${h.id}`, h.text, h.source ?? 'allternit-api', {
      plane: { type: h.type, id: h.id, score: h.score },
    });
  }

  private chunkToMemory(h: ChunkHit): Memory {
    return synthetic(`plane:document:${h.document_id}`, h.text, h.source ?? 'allternit-api', {
      plane: { type: 'document', id: h.document_id, score: h.score, title: h.title, collection: h.collection, external_id: h.external_id },
    });
  }

  /**
   * Semantic recall from the API memory plane; the local index answers only
   * when the API is unreachable or errors.
   */
  async similaritySearch(query: string, limit: number = 10): Promise<Memory[]> {
    try {
      const facts = this.post<{ items: FactHit[] }>('/api/v1/memory/adapters/search', { query, limit });
      const docs = this.includeDocuments
        ? this.post<{ items: ChunkHit[] }>('/api/v1/memory/adapters/chunks/search', { query, limit }).catch((e) => {
            console.warn(`[memory-agent] document recall failed: ${(e as Error).message}`);
            return { items: [] as ChunkHit[] };
          })
        : Promise.resolve({ items: [] as ChunkHit[] });
      const [f, d] = await Promise.all([facts, docs]);
      const a = (f.items ?? []).map((h) => this.factToMemory(h));
      const b = (d.items ?? []).map((h) => this.chunkToMemory(h));
      // The two lists are ranked separately: interleave them.
      const out: Memory[] = [];
      const seen = new Set<string>();
      for (let i = 0; out.length < limit && (i < a.length || i < b.length); i++) {
        for (const m of [a[i], b[i]]) {
          if (m && !seen.has(m.id) && out.length < limit) {
            seen.add(m.id);
            out.push(m);
          }
        }
      }
      this.lastSearchSource = 'api';
      return out;
    } catch (e) {
      console.warn(`[memory-agent] API recall unavailable (${(e as Error).message}); using the local index`);
      this.lastSearchSource = 'local';
      return this.local.similaritySearch(query, limit);
    }
  }

  getStats(): { count: number; model: string; backend: string; api: string } {
    return { ...this.local.getStats(), backend: 'allternit-api', api: this.baseUrl };
  }

  clear(): void {
    this.local.clear();
  }
}
