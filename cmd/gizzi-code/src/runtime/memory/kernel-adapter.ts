/**
 * Memory kernel adapter — read side only.
 *
 * The user's Memory Drive (src/runtime/memory/drive) is the canonical store;
 * the allternit-api memory kernel is an index rebuilt FROM the drive after
 * every push (drive → server index). gizzi therefore no longer mirrors
 * memories into kernel rows (the old upsert/delete/import-marker path was
 * removed with the drive cut-over). What remains is canonical recall: the
 * server's hybrid search over the indexed drive, used by memory_recall to
 * surface paraphrase matches a literal line search misses.
 *
 * Best-effort: signed out, offline or an old API returns no hits.
 * `GIZZI_MEMORY_KERNEL=0` turns it off.
 */
import { platformRequest } from "@/runtime/bots/platform-api"

const TIMEOUT_MS = 5_000

export interface KernelHit {
  id: string
  type: string
  text: string
  score: number
  source: string | null
  external_id: string | null
}

export namespace MemoryKernelAdapter {
  export function enabled(): boolean {
    return process.env.GIZZI_MEMORY_KERNEL !== "0"
  }

  /** Canonical recall over the user's indexed memory. Empty on failure. */
  export async function search(query: string, limit = 10): Promise<KernelHit[]> {
    if (!enabled() || !query.trim()) return []
    try {
      const r = await platformRequest<{ items: KernelHit[] }>(
        "POST",
        "/api/v1/memory/adapters/search",
        { query, limit },
        { timeoutMs: TIMEOUT_MS },
      )
      return r.items ?? []
    } catch {
      return []
    }
  }
}
