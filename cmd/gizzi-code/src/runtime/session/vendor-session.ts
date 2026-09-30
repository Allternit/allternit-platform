import { Storage } from "@/runtime/session/storage/storage"

/**
 * The vendor CLI session id (claude session_id, ACP sessionId) of a gizzi
 * session, kept per CLI so the next turn resumes the vendor's own memory
 * instead of starting a fresh vendor session.
 */
export namespace VendorSession {
  const key = (sessionID: string, cli: string) => ["vendor_session", sessionID, cli]

  export async function get(sessionID: string, cli: string): Promise<string | undefined> {
    try {
      const rec = await Storage.read<{ id?: string }>(key(sessionID, cli))
      return rec?.id || undefined
    } catch {
      return undefined
    }
  }

  export async function set(sessionID: string, cli: string, id: string): Promise<void> {
    try {
      await Storage.write(key(sessionID, cli), { id, updatedAt: Date.now() })
    } catch {
      // Best effort: losing the id only means the next turn starts fresh.
    }
  }
}
