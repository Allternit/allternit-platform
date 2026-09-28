/**
 * Following a context handoff into the /bots pane (spec P3.16).
 *
 * A bot's canonical chat may have been handed off to a fresh window by
 * another client (Desktop/API run turns through runtime `prompt()`, which
 * hands off at 70% full or on a smaller-window model switch). The pinned id
 * then points at an older window. The REPL keeps its own transcript store,
 * so the head session has no REPL transcript: without help it would open
 * blank and the bot would start from scratch.
 *
 * resolveHandoffHead finds the newest window and its seed (the checkpoint
 * gizzi wrote as the first user message, text part tagged
 * `metadata.handoff`). buildHandoffLog turns it into a resume log shaped
 * like a compaction: a boundary tagged with the handoff (drawn as the rip)
 * followed by the checkpoint as the compact summary, so it is both on
 * screen and in the model's context for the next turn.
 */
import type { LogOption, SerializedMessage } from '../../types/logs.js'
import { createCompactBoundaryMessage, createUserMessage } from '../../utils/messages.js'
import { getUserType } from '../../utils/sessionStorage.js'

export interface HandoffHead {
  /** The newest window of the lineage. */
  sessionId: string
  /** The seed checkpoint text ('' if the seed couldn't be read). */
  checkpoint: string
  /** Generation that starts at the head (2 = first fresh window). */
  generation?: number
  reason?: string
  at?: string
}

/** null when the session has not been handed off (it is its own head). */
export async function resolveHandoffHead(sessionId: string): Promise<HandoffHead | null> {
  // Direct store reads: work in the TUI without the runtime bootstrap.
  const { sessionHandoffHead, sessionHandoffSeed } = await import('@/runtime/bots/session-db.js')
  const head = await sessionHandoffHead(sessionId)
  if (!head || head === sessionId) return null
  const seed = await sessionHandoffSeed(head).catch(() => null)
  return {
    sessionId: head,
    checkpoint: seed?.text ?? '',
    generation: typeof seed?.generation === 'number' ? seed.generation + 1 : undefined,
    reason: seed?.reason,
    at: seed?.at ? new Date(seed.at).toISOString() : undefined,
  }
}

const VERSION = typeof MACRO !== 'undefined' ? MACRO.VERSION : 'unknown'

/** A resume log for the head: the rip plus the checkpoint as a compact summary. */
export function buildHandoffLog(head: HandoffHead, projectPath: string): LogOption {
  const at = head.at ?? new Date().toISOString()
  const boundary = createCompactBoundaryMessage('auto', 0)
  boundary.timestamp = at
  boundary.compactMetadata.handoff = { generation: head.generation, reason: head.reason }
  const summary = createUserMessage({
    content: head.checkpoint || 'This conversation continues from a previous context window. Pick up where it left off.',
    isCompactSummary: true,
    isVisibleInTranscriptOnly: true,
  })
  const serialize = (m: object): SerializedMessage =>
    ({ ...m, cwd: projectPath, userType: getUserType(), sessionId: head.sessionId, timestamp: at, version: VERSION }) as SerializedMessage
  const now = new Date()
  return {
    date: at,
    messages: [serialize(boundary), serialize(summary)],
    value: 0,
    created: now,
    modified: now,
    firstPrompt: '',
    messageCount: 2,
    isSidechain: false,
    sessionId: head.sessionId,
    projectPath,
  }
}
