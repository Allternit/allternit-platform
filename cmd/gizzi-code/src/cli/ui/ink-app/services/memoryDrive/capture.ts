/**
 * Send the person's message from each Gizzi turn to Memory Drive's capture
 * intake, so the nightly Dream can learn from CLI sessions too (Settings →
 * Memory → Sources → Gizzi). Gizzi already saves memories itself, so the
 * server only records the turn. Best-effort and quiet: signed out, offline,
 * an older API (404) or the source switched off all simply do nothing.
 */
import { getSessionId } from '../../bootstrap/state.js'
import type { Message } from '../../types/message.js'
import { getUserMessageText } from '../../utils/messages.js'
import { logForDebugging } from '../../utils/debug.js'
import { platformRequest, platformSignedIn } from '../../../../../runtime/bots/platform-api.js'

const sent = new Set<string>()

/** The text of the last thing the person typed, if it's a real message. */
export function lastUserText(messages: readonly Message[]): { id: string; text: string } | null {
  for (let i = messages.length - 1; i >= 0; i -= 1) {
    const m = messages[i] as Message & { isMeta?: boolean; uuid?: string }
    if (m.type !== 'user' || m.isMeta) continue
    const text = getUserMessageText(m)?.trim()
    // Tool results have no text; slash commands and injected context start with a tag or "/".
    if (!text || text.startsWith('<') || text.startsWith('/')) continue
    return { id: m.uuid ?? text.slice(0, 64), text }
  }
  return null
}

export async function captureUserTurn(messages: readonly Message[]): Promise<void> {
  const turn = lastUserText(messages)
  if (!turn || sent.has(turn.id)) return
  sent.add(turn.id)
  if (sent.size > 500) sent.clear()
  try {
    if (!(await platformSignedIn())) return
    const session = String(getSessionId() ?? '').replace(/[^A-Za-z0-9._-]/g, '-').slice(0, 128) || 'cli'
    await platformRequest('POST', '/api/v1/memory/drive/capture/turn', {
      source: 'gizzi',
      label: `gizzi:session/${session}`,
      text: turn.text.slice(0, 4000),
    }, { timeoutMs: 10_000 })
  } catch (error) {
    logForDebugging(`[memoryDrive] capture skipped: ${error instanceof Error ? error.message : String(error)}`)
  }
}
