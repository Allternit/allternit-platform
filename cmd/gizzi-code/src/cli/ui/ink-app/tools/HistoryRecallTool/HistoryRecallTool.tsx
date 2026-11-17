/**
 * history_recall in the Gizzi TUI (Primitives Wave B, lane L6): the runtime
 * tool id, parameters and result shapes, over this session's JSONL
 * transcript (see transcript.ts). Read-only, this session only, and every
 * result carries the data notice. When the session keeps no transcript the
 * result says so (`note`) instead of guessing.
 */
import React from 'react'
import { z } from 'zod/v4'
import { MessageResponse } from '../../components/MessageResponse'
import { Text } from '../../ink'
import { buildTool, type ToolDef } from '../../Tool.js'
import { lazySchema } from '../../utils/lazySchema.js'
import {
  HISTORY_RECALL_ACTIONS,
  HISTORY_RECALL_LIMIT_MAX,
  HISTORY_RECALL_READ_MAX_CHARS,
  type HistoryHit,
  type HistoryRecallResult,
} from '@/runtime/primitives/wave-b-types'
import { DATA_NOTICE, DESCRIPTION, HISTORY_RECALL_TOOL_NAME, NOT_AVAILABLE, PROMPT } from './prompt.js'
import { readTranscript, type RecallMessage, type RecallSegment } from './transcript.js'

const DEFAULT_LIMIT = 10
const SNIPPET_CHARS = 240
const CUT_MARKER = '\n[cut at 8,000 characters; read single messages by item_id for more]'

const inputSchema = lazySchema(() =>
  z.strictObject({
    action: z.enum(HISTORY_RECALL_ACTIONS).describe('windows, search or read'),
    query: z.string().min(1).max(500).optional().describe('search: words to find'),
    window_id: z.string().min(1).max(200).optional().describe('read: which window (from windows or search); search: only this window'),
    item_id: z.string().min(1).max(200).optional().describe('read: one message in the window (from search)'),
    limit: z.number().int().min(1).max(HISTORY_RECALL_LIMIT_MAX).optional().describe(`windows, search: at most ${HISTORY_RECALL_LIMIT_MAX} results`),
  }),
)
type InputSchema = ReturnType<typeof inputSchema>

export type Output = HistoryRecallResult & { note?: string }

function find(all: RecallSegment[], windowId: string): RecallSegment {
  const segment = all.find(s => s.window.id === windowId)
  if (!segment)
    throw new Error(
      `history_recall: window "${windowId}" is not in this conversation. Use action "windows" to list the ones you can read.`,
    )
  return segment
}

function header(m: RecallMessage): string {
  return `--- record ${m.id} · ${m.role} · ${m.time ? new Date(m.time).toISOString() : 'unknown time'} ---`
}

function capped(text: string) {
  if (text.length <= HISTORY_RECALL_READ_MAX_CHARS) return { text, truncated: false }
  return { text: text.slice(0, HISTORY_RECALL_READ_MAX_CHARS - CUT_MARKER.length) + CUT_MARKER, truncated: true }
}

function matcher(query: string) {
  const phrase = query.trim().toLowerCase()
  const terms = phrase.split(/\s+/).filter(Boolean)
  return (text: string): number => {
    const lower = text.toLowerCase()
    const at = lower.indexOf(phrase)
    if (at >= 0) return at
    if (terms.length > 1 && terms.every(t => lower.includes(t))) return lower.indexOf(terms[0])
    return -1
  }
}

function snippet(text: string, at: number): string {
  const start = Math.max(0, at - Math.floor(SNIPPET_CHARS / 3))
  const body = text.slice(start, start + SNIPPET_CHARS).replace(/\s+/g, ' ').trim()
  return `${start > 0 ? '…' : ''}${body}${start + SNIPPET_CHARS < text.length ? '…' : ''}`
}

function title(output: Output): string {
  if (output.note && output.action !== 'read') return 'Not available in this terminal session'
  if (output.action === 'windows') return `${output.windows.length} earlier window${output.windows.length === 1 ? '' : 's'}`
  if (output.action === 'search') return `${output.hits.length} match${output.hits.length === 1 ? '' : 'es'} in earlier history`
  return output.truncated ? 'Read earlier history (shortened)' : 'Read earlier history'
}

export const HistoryRecallTool = buildTool({
  name: HISTORY_RECALL_TOOL_NAME,
  searchHint: 'read compacted earlier conversation history',
  maxResultSizeChars: 100_000,
  userFacingName: () => 'History recall',
  get inputSchema(): InputSchema {
    return inputSchema()
  },
  isReadOnly() {
    return true
  },
  isConcurrencySafe() {
    return true
  },
  toAutoClassifierInput(input) {
    return [input.action, input.query].filter(Boolean).join(' ')
  },
  async validateInput(input) {
    if (input.action === 'search' && !input.query)
      return { result: false, message: 'history_recall search needs a query.', errorCode: 1 }
    if (input.action === 'read' && !input.window_id)
      return {
        result: false,
        message: 'history_recall read needs a window_id. Use action "windows" or "search" to find one.',
        errorCode: 1,
      }
    return { result: true }
  },
  async description() {
    return DESCRIPTION
  },
  async prompt() {
    return PROMPT
  },
  mapToolResultToToolResultBlockParam(output, toolUseID) {
    return { tool_use_id: toolUseID, type: 'tool_result', content: JSON.stringify(output) }
  },
  renderToolUseMessage(input) {
    return [input.action, input.query ?? input.window_id].filter(Boolean).join(' ')
  },
  renderToolResultMessage(output: Output) {
    return (
      <MessageResponse>
        <Text>{title(output)}</Text>
      </MessageResponse>
    )
  },
  async call(input) {
    const limit = Math.min(HISTORY_RECALL_LIMIT_MAX, Math.max(1, input.limit ?? DEFAULT_LIMIT))
    const transcript = await readTranscript()
    const note = transcript.available ? undefined : NOT_AVAILABLE
    const all = transcript.segments

    if (input.action === 'windows') {
      return {
        data: {
          action: 'windows',
          data_notice: DATA_NOTICE,
          windows: all.slice(0, limit).map(s => s.window),
          ...(note ? { note } : {}),
        },
      }
    }

    if (input.action === 'search') {
      const scope = input.window_id ? [find(all, input.window_id)] : all
      const match = matcher(input.query!)
      const hits: HistoryHit[] = []
      for (const segment of scope) {
        for (let i = segment.messages.length - 1; i >= 0 && hits.length < limit; i--) {
          const m = segment.messages[i]
          const at = match(m.text)
          if (at < 0) continue
          hits.push({ window_id: segment.window.id, item_id: m.id, role: m.role, time: m.time, snippet: snippet(m.text, at) })
        }
        if (hits.length >= limit) break
      }
      return { data: { action: 'search', data_notice: DATA_NOTICE, hits, ...(note ? { note } : {}) } }
    }

    if (note) throw new Error(NOT_AVAILABLE)
    const segment = find(all, input.window_id!)
    if (input.item_id) {
      const m = segment.messages.find(x => x.id === input.item_id)
      if (!m) throw new Error(`history_recall: message "${input.item_id}" is not in window "${input.window_id}".`)
      const { text, truncated } = capped(`${header(m)}\n${m.text}`)
      return { data: { action: 'read', data_notice: DATA_NOTICE, window_id: segment.window.id, item_id: m.id, text, truncated } }
    }
    const { text, truncated } = capped(segment.messages.map(m => `${header(m)}\n${m.text}`).join('\n\n'))
    return { data: { action: 'read', data_notice: DATA_NOTICE, window_id: segment.window.id, text, truncated } }
  },
} satisfies ToolDef<InputSchema, Output>)
