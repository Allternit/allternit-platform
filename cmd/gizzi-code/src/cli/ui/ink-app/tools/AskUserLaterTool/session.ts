/**
 * Which thread this REPL's ask_user_later asks belong to, and how the TUI
 * reaches the runtime's question store (runtime/integrations/question).
 *
 * When the terminal is a live engine session (services/liveSession.ts) the
 * asks go to that engine session, so Desktop, web, the PWA and the phone apps
 * show the card, and an answer given there comes back through the live inbox
 * as a prompt. Otherwise they go to this terminal's own session id and are
 * answered here with /answer.
 */
import { getSessionId } from '../../bootstrap/state.js'
import { getLiveSession } from '../../services/liveSession.js'
import { getCwd } from '../../utils/cwd.js'

/** The session a new ask is filed under. */
export function askSessionID(): string {
  return getLiveSession().sessionID ?? String(getSessionId())
}

/** True when the ask can reach the user's other devices (a live engine session). */
export function askIsLive(): boolean {
  return !!getLiveSession().sessionID
}

/** The sessions this REPL owns: its terminal session and, when live, its engine session. */
export function ownedSessionIDs(): Set<string> {
  const ids = new Set<string>([String(getSessionId())])
  const live = getLiveSession().sessionID
  if (live) ids.add(live)
  return ids
}

/** Runs runtime question code in this process (it needs an instance for its state and bus). */
export async function withRuntime<R>(fn: () => Promise<R>): Promise<R> {
  const { Instance } = await import('@/runtime/context/project/instance')
  return Instance.provide({ directory: getCwd(), fn })
}
