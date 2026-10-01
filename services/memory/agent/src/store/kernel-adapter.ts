/**
 * Memory kernel adapter (WP-M1d, decision M1).
 *
 * The canonical memory store is the allternit-api memory kernel. This
 * service keeps its own SQLite store (its consolidation/insight pipeline
 * reads it), and mirrors every memory write to the kernel through
 * `/api/v1/memory/adapters/*` (source `memory-agent`, external id = the
 * memory id), so the kernel sees everything this service learns. Existing
 * rows are imported once (`pnpm kernel:import`, idempotent: re-sending an
 * unchanged memory is a no-op on the kernel side).
 *
 * Enabled when ALLTERNIT_API_TOKEN is set (base URL: ALLTERNIT_API_URL,
 * default the local API on :8013). Best-effort: failures are logged, never thrown.
 */
import type { Memory } from '../types/memory.types.js';

export const KERNEL_SOURCE = 'memory-agent';
const CHUNK = 200;

export interface KernelItem {
  external_id: string;
  text: string;
  memory_type: string;
}

export function toKernelItem(m: Pick<Memory, 'id' | 'summary' | 'content'>): KernelItem {
  const text = m.summary && m.summary !== m.content ? `${m.summary}\n\n${m.content}` : m.content;
  return { external_id: m.id, text: text.trim().slice(0, 8000), memory_type: 'fact' };
}

export class KernelAdapter {
  constructor(
    private readonly baseUrl: string,
    private readonly token: string,
    private readonly fetchImpl: typeof fetch = fetch,
  ) {}

  /** From env; undefined when no token is configured. */
  static fromEnv(): KernelAdapter | undefined {
    const token = process.env.ALLTERNIT_API_TOKEN?.trim();
    if (!token || process.env.MEMORY_AGENT_KERNEL === '0') return undefined;
    const base = (process.env.ALLTERNIT_API_URL ?? 'http://127.0.0.1:8013').replace(/\/+$/, '');
    return new KernelAdapter(base, token);
  }

  private async post(path: string, body: unknown): Promise<boolean> {
    try {
      const r = await this.fetchImpl(`${this.baseUrl}${path}`, {
        method: 'POST',
        headers: { Authorization: `Bearer ${this.token}`, 'Content-Type': 'application/json' },
        body: JSON.stringify(body),
        signal: AbortSignal.timeout(10_000),
      });
      if (!r.ok) console.warn(`[memory-agent] kernel ${path} -> HTTP ${r.status}`);
      return r.ok;
    } catch (e) {
      console.warn(`[memory-agent] kernel ${path} failed: ${(e as Error).message}`);
      return false;
    }
  }

  async upsert(memories: Pick<Memory, 'id' | 'summary' | 'content'>[]): Promise<boolean> {
    let ok = true;
    for (let i = 0; i < memories.length; i += CHUNK) {
      const items = memories.slice(i, i + CHUNK).map(toKernelItem);
      ok = (await this.post('/api/v1/memory/adapters/upsert', { source: KERNEL_SOURCE, items })) && ok;
    }
    return ok;
  }

  async remove(ids: string[]): Promise<boolean> {
    if (ids.length === 0) return true;
    return this.post('/api/v1/memory/adapters/delete', { source: KERNEL_SOURCE, external_ids: ids });
  }
}
