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
  /** Sleep `disputes` stage: set when this memory is a side of a dispute. Absent on older servers. */
  dispute?: unknown
}

export interface PastTurn {
  observation_id: string
  session_id: string
  speaker: string
  timestamp: string
  text: string
  score?: number
}

export interface PastChatFilters {
  from?: string
  to?: string
  session?: string
  skip_sessions?: string[]
}

let request: typeof platformRequest = platformRequest

export namespace MemoryKernelAdapter {
  /** Tests swap the network edge; pass nothing to restore. */
  export function setRequestForTests(fn?: typeof platformRequest) {
    request = fn ?? platformRequest
  }

  /** Raw-log search over earlier chats (mode "search": the wide budget). Empty on failure. */
  export async function searchPastChats(query: string, filters: PastChatFilters = {}, limit = 12): Promise<PastTurn[]> {
    if (!enabled() || !query.trim()) return []
    try {
      const r = await request<{ turns?: PastTurn[] }>(
        "POST",
        "/api/v1/memory/v2/raw-search",
        {
          query,
          mode: "search",
          limit,
          ...(filters.from ? { from: filters.from } : {}),
          ...(filters.to ? { to: filters.to } : {}),
          ...(filters.session ? { session_id: filters.session } : {}),
          ...(filters.skip_sessions?.length ? { exclude_sessions: filters.skip_sessions } : {}),
        },
        { timeoutMs: TIMEOUT_MS },
      )
      return r.turns ?? []
    } catch {
      return []
    }
  }

  /** Turns around one observation, same session, in time order. Empty on failure. */
  export async function neighbors(observationId: string, before = 2, after = 2): Promise<PastTurn[]> {
    if (!enabled() || !observationId.trim()) return []
    try {
      const r = await request<{ turns?: PastTurn[] }>(
        "GET",
        `/api/v1/memory/v2/turns/${encodeURIComponent(observationId)}/neighbors?before=${before}&after=${after}`,
        undefined,
        { timeoutMs: TIMEOUT_MS },
      )
      return r.turns ?? []
    } catch {
      return []
    }
  }

  export function enabled(): boolean {
    return process.env.GIZZI_MEMORY_KERNEL !== "0"
  }

  /**
   * Tell Memory Drive → Activity what this agent did with the person's
   * memory (a recall, a save, a removal). Fire and forget: never blocks or
   * fails the tool. Older APIs answer 404 and nothing is shown.
   */
  export function report(
    kind: "recalled" | "saved" | "forgot",
    title: string,
    items: { text: string; source?: string }[],
    sessionId?: string,
  ): void {
    if (!enabled()) return
    platformRequest(
      "POST",
      "/api/v1/memory/drive/activity",
      {
        kind,
        title,
        items: items.slice(0, 20).map((i) => ({ text: i.text, source: i.source ?? "" })),
        source: sessionId ? `gizzi:session/${sessionId}` : undefined,
        session: sessionId,
      },
      { timeoutMs: TIMEOUT_MS },
    ).catch(() => {})
  }

  /** Canonical recall over the user's indexed memory. Empty on failure. */
  export async function search(query: string, limit = 10): Promise<KernelHit[]> {
    if (!enabled() || !query.trim()) return []
    try {
      const r = await request<{ items: KernelHit[] }>(
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
