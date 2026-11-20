/**
 * Send the person's message from each Gizzi turn to Memory Drive's capture
 * intake, so the nightly Dream can learn from CLI sessions too (Settings →
 * Memory → Sources → Gizzi). Gizzi already saves memories itself, so the
 * server only records the turn. Best-effort and quiet: signed out, offline,
 * an older API (404) or the source switched off all simply do nothing.
 */
import { getSessionId } from '../../bootstrap/state.js'
import type { Message } from '../../types/message.js'
import { getAssistantMessageText, getUserMessageText } from '../../utils/messages.js'
import { logForDebugging } from '../../utils/debug.js'
import { platformRequest, platformSignedIn } from '../../../../../runtime/bots/platform-api.js'

const sent = new Set<string>()
const sentRows = new Set<string>()
const SLEEP_STATUS_TTL_MS = 5 * 60 * 1000
let sleepStatus: { enabled: boolean; at: number } | null = null

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

export type TurnRow = {
  turnId: string
  role: 'assistant' | 'tool'
  text?: string
  toolName?: string
  toolInput?: string
  toolOutput?: string
}

function stringify(v: unknown): string {
  if (typeof v === 'string') return v
  try {
    return JSON.stringify(v) ?? ''
  } catch {
    return String(v)
  }
}

function resultText(content: unknown): string {
  if (typeof content === 'string') return content
  if (Array.isArray(content)) {
    return content
      .map(b => (b && typeof b === 'object' && (b as { type?: string }).type === 'text' ? String((b as { text?: string }).text ?? '') : ''))
      .filter(Boolean)
      .join('\n')
  }
  return content == null ? '' : stringify(content)
}

/**
 * The assistant's text reply and each tool call (with its result) since the
 * person's last real message. Ids come from message uuids and tool_use ids,
 * so a re-run of the same turn sends the same ids.
 */
export function turnRows(messages: readonly Message[]): TurnRow[] {
  let start = -1
  for (let i = messages.length - 1; i >= 0; i -= 1) {
    const m = messages[i] as Message & { isMeta?: boolean }
    if (m.type !== 'user' || m.isMeta) continue
    const text = getUserMessageText(m)?.trim()
    if (!text || text.startsWith('<') || text.startsWith('/')) continue
    start = i
    break
  }
  if (start < 0) return []
  const calls = new Map<string, TurnRow>()
  const rows: TurnRow[] = []
  let lastReply: TurnRow | null = null
  for (let i = start + 1; i < messages.length; i += 1) {
    const m = messages[i] as Message & { uuid?: string; message?: { content?: unknown } }
    const content = m.message?.content
    if (!Array.isArray(content)) continue
    if (m.type === 'assistant') {
      const text = getAssistantMessageText(m)
      if (text) lastReply = { turnId: `${m.uuid ?? i}:a`, role: 'assistant', text }
      for (const b of content as Array<{ type?: string; id?: string; name?: string; input?: unknown }>) {
        if (b.type !== 'tool_use' || !b.id) continue
        const row: TurnRow = { turnId: `${b.id}:t`, role: 'tool', toolName: b.name, toolInput: stringify(b.input ?? {}), toolOutput: '' }
        calls.set(b.id, row)
        rows.push(row)
      }
    } else if (m.type === 'user') {
      for (const b of content as Array<{ type?: string; tool_use_id?: string; content?: unknown }>) {
        if (b.type === 'tool_result' && b.tool_use_id) {
          const row = calls.get(b.tool_use_id)
          if (row) row.toolOutput = resultText(b.content)
        }
      }
    }
  }
  return lastReply ? [lastReply, ...rows] : rows
}

/**
 * Whether the server's Allternit Sleep is on for this person: one status
 * call per process, kept five minutes. Off, an older runtime (404) or any
 * failure means off. (No tool in Gizzi is flagged sensitive today, so tool
 * output is sent as is when Sleep is on.)
 */
async function sleepOn(): Promise<boolean> {
  const now = Date.now()
  if (sleepStatus && now - sleepStatus.at < SLEEP_STATUS_TTL_MS) return sleepStatus.enabled
  let enabled = false
  try {
    const r = await platformRequest<{ enabled?: boolean }>('GET', '/api/v1/memory/sleep/status', undefined, { timeoutMs: 5_000 })
    enabled = r?.enabled === true
  } catch {
    enabled = false
  }
  sleepStatus = { enabled, at: now }
  return enabled
}

export async function captureUserTurn(messages: readonly Message[]): Promise<void> {
  const turn = lastUserText(messages)
  if (!turn || sent.has(turn.id)) return
  sent.add(turn.id)
  if (sent.size > 500) sent.clear()
  try {
    if (!(await platformSignedIn())) return
    const session = String(getSessionId() ?? '').replace(/[^A-Za-z0-9._-]/g, '-').slice(0, 128) || 'cli'
    const label = `gizzi:session/${session}`
    // With Sleep on the server keeps the full text (its own cap); off, it
    // clips to 4000 itself, so the older client cap stays for older runtimes.
    const on = await sleepOn()
    await platformRequest('POST', '/api/v1/memory/drive/capture/turn', {
      source: 'gizzi',
      label,
      text: on ? turn.text.slice(0, 64_000) : turn.text.slice(0, 4000),
      role: 'user',
      turn_id: turn.id,
    }, { timeoutMs: 10_000 })
    if (!on) return
    for (const row of turnRows(messages)) {
      if (sentRows.has(row.turnId)) continue
      sentRows.add(row.turnId)
      if (sentRows.size > 2000) sentRows.clear()
      await platformRequest('POST', '/api/v1/memory/drive/capture/turn', {
        source: 'gizzi',
        label,
        role: row.role,
        turn_id: row.turnId,
        text: row.text?.slice(0, 64_000) ?? '',
        tool_name: row.toolName,
        tool_input: row.toolInput?.slice(0, 8_000),
        tool_output: row.toolOutput?.slice(0, 32_000),
      }, { timeoutMs: 10_000 })
    }
  } catch (error) {
    logForDebugging(`[memoryDrive] capture skipped: ${error instanceof Error ? error.message : String(error)}`)
  }
}
