import { Storage } from "@/runtime/session/storage/storage"
import type { ReviewVerdict } from "./verdict"

/** Last verdicts per session, newest first, in the runtime's session storage. */
export namespace ReviewStore {
  export const KEEP = 8
  /** Verdicts from a review with no session (CLI on uncommitted changes). */
  export const NO_SESSION = "_cwd"

  function key(sessionID?: string) {
    return ["review_verdict", sessionID ?? NO_SESSION]
  }

  export async function list(sessionID?: string): Promise<ReviewVerdict.Stored[]> {
    return Storage.read<ReviewVerdict.Stored[]>(key(sessionID)).catch(() => [])
  }

  export async function add(record: ReviewVerdict.Stored): Promise<ReviewVerdict.Stored[]> {
    const prior = await list(record.sessionID)
    const next = [record, ...prior.filter((r) => r.id !== record.id)].slice(0, KEEP)
    await Storage.write(key(record.sessionID), next)
    return next
  }

  export async function latest(sessionID?: string): Promise<ReviewVerdict.Stored | undefined> {
    return (await list(sessionID))[0]
  }
}
