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
 * screen and in the model's context for the next turn. The earlier window's
 * last messages go above the rip, like the conversation above a local
 * compaction: in scrollback to read, out of the model's context (it only
 * sees what follows the last boundary). Desktop reads the same window on
 * demand from its rip ("Show the earlier conversation").
 */
import type { LogOption, SerializedMessage } from '../../types/logs.js'
import type { StoredTextMessage } from '@/runtime/bots/session-db.js'
import { createAssistantMessage, createCompactBoundaryMessage, createSystemMessage, createUserMessage } from '../../utils/messages.js'
import { getUserType } from '../../utils/sessionStorage.js'

export interface HandoffHead {
  /** The newest window of the lineage. */
  sessionId: string
  /** The window the head continued from (the seed's `handoff.from`). */
  from?: string
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
    from: seed?.from,
    checkpoint: seed?.text ?? '',
    generation: typeof seed?.generation === 'number' ? seed.generation + 1 : undefined,
    reason: seed?.reason,
    at: seed?.at ? new Date(seed.at).toISOString() : undefined,
  }
}

const VERSION = typeof MACRO !== 'undefined' ? MACRO.VERSION : 'unknown'

/** Earlier-window messages shown above the rip (newest part of the window). */
export const EARLIER_MESSAGES_SHOWN = 50

export interface EarlierWindow {
  /** Messages in the earlier window with text. */
  total: number
  /** The newest of them, oldest first. */
  messages: StoredTextMessage[]
}

/** The window the head continued from, read from the store; null when unknown or unreadable. */
export async function loadEarlierWindow(head: HandoffHead): Promise<EarlierWindow | null> {
  if (!head.from) return null
  const { sessionTextMessages } = await import('@/runtime/bots/session-db.js')
  const earlier = await sessionTextMessages(head.from, EARLIER_MESSAGES_SHOWN).catch(() => null)
  return earlier && earlier.messages.length > 0 ? earlier : null
}

/**
 * A resume log for the head: the earlier window's last messages (when
 * given), then the rip plus the checkpoint as a compact summary.
 */
export function buildHandoffLog(head: HandoffHead, projectPath: string, earlier?: EarlierWindow | null): LogOption {
  const at = head.at ?? new Date().toISOString()
  const atMs = Date.parse(at)
  const above: object[] = []
  if (earlier && earlier.messages.length > 0) {
    const hidden = earlier.total - earlier.messages.length
    if (hidden > 0) {
      above.push(stamp(createSystemMessage(`${hidden} earlier message${hidden === 1 ? '' : 's'} of the previous window not shown · open the thread in Allternit Desktop to read them`, 'info'), earlier.messages[0]!.at ?? atMs))
    }
    for (const m of earlier.messages) {
      if (m.handoff) {
        // That window's own seed: its rip, back to the window before it.
        const rip = createCompactBoundaryMessage('auto', 0)
        rip.compactMetadata.handoff = { generation: (m.handoff.generation ?? 1) + 1, reason: m.handoff.reason }
        above.push(stamp(rip, m.at ?? atMs))
      } else if (m.role === 'user') {
        above.push(stamp(createUserMessage({ content: m.content }), m.at ?? atMs))
      } else {
        const reply = createAssistantMessage({ content: m.content })
        // The model that wrote it, not the "<synthetic>" placeholder.
        reply.message.model = m.model ?? ''
        above.push(stamp(reply, m.at ?? atMs))
      }
    }
  }
  const boundary = createCompactBoundaryMessage('auto', 0)
  boundary.timestamp = at
  boundary.compactMetadata.handoff = { generation: head.generation, reason: head.reason, ...(above.length > 0 ? { earlierAbove: true } : {}) }
  const summary = createUserMessage({
    content: head.checkpoint || 'This conversation continues from a previous context window. Pick up where it left off.',
    isCompactSummary: true,
    isVisibleInTranscriptOnly: true,
  })
  summary.timestamp = at
  const serialize = (m: object): SerializedMessage =>
    ({ cwd: projectPath, userType: getUserType(), sessionId: head.sessionId, timestamp: at, version: VERSION, ...m }) as SerializedMessage
  const messages = [...above, boundary, summary].map(serialize)
  const now = new Date()
  return {
    date: at,
    messages,
    value: 0,
    created: now,
    modified: now,
    firstPrompt: '',
    messageCount: messages.length,
    isSidechain: false,
    sessionId: head.sessionId,
    projectPath,
  }
}

/** Give a rebuilt message its original time (ms) so it sorts and labels right. */
function stamp<T extends object>(m: T, at: number): T {
  return Number.isFinite(at) ? { ...m, timestamp: new Date(at).toISOString() } : m
}
