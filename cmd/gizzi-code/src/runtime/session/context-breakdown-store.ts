import { Storage } from "@/runtime/session/storage/storage"
import { ContextBreakdown } from "@/runtime/session/context-breakdown"

/**
 * The latest context breakdown per session, kept in memory and on disk so
 * the panel still shows the full split after gizzi restarts (otherwise
 * only the provider's total survives, all of it under Messages).
 */
export namespace ContextBreakdownStore {
  const key = (sessionID: string) => ["context_breakdown", sessionID]

  export function record(snapshot: ContextBreakdown.Snapshot) {
    ContextBreakdown.record(snapshot)
    Storage.write(key(snapshot.sessionID), snapshot).catch(() => {
      // Best effort: the in-memory copy still serves this process.
    })
  }

  export async function get(sessionID: string): Promise<ContextBreakdown.Snapshot | undefined> {
    const live = ContextBreakdown.get(sessionID)
    if (live) return live
    const saved = await Storage.read<ContextBreakdown.Snapshot>(key(sessionID)).catch(() => undefined)
    const parsed = saved ? ContextBreakdown.Snapshot.safeParse(saved) : undefined
    if (!parsed?.success) return undefined
    ContextBreakdown.record(parsed.data)
    return parsed.data
  }

  export async function remove(sessionID: string) {
    await Storage.remove(key(sessionID)).catch(() => {})
  }
}
