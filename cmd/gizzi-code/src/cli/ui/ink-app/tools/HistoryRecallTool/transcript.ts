/**
 * The terminal's compacted history (Primitives Wave B, lane L6).
 *
 * Ink compacts in memory: after a compaction the live message list starts
 * at a compact_boundary, and the older messages are gone from it. The
 * session's JSONL transcript (sessionStorage.getTranscriptPath) is
 * append-only and still holds them, with each compact_boundary written
 * between the stretch it cut and the summary that replaced it. (The resume
 * loader, loadTranscriptFile, drops everything before the last boundary on
 * purpose, so this reads the raw file instead.)
 *
 * Windows. Every stretch before a boundary is a `compaction` window with id
 * `<sessionId>:<index>` (oldest = 0); the stretch after the latest boundary
 * is already in context and is not offered. The terminal keeps no handoff
 * lineage, so there are no `handoff` windows here.
 */
import { createReadStream, existsSync } from 'fs'
import { createInterface } from 'readline'
import { getSessionId } from '../../bootstrap/state.js'
import {
  flushSessionStorage,
  getTranscriptPath,
} from '../../utils/sessionStorage.js'
import type { HistoryWindow } from '@/runtime/primitives/wave-b-types'

const SUMMARY_CHARS = 400
const TOOL_INPUT_CHARS = 300
const TOOL_OUTPUT_CHARS = 1_500
/** One stored message never keeps more than this; recall reads at most 8,000 anyway. */
const MESSAGE_CHARS = 20_000

export type RecallMessage = {
  id: string
  role: 'user' | 'assistant'
  time: number
  text: string
}

export type RecallSegment = {
  window: HistoryWindow
  messages: RecallMessage[]
}

export type Transcript = {
  /** False when this session writes no transcript (session persistence off, or nothing written yet). */
  available: boolean
  /** Compacted stretches, newest first. */
  segments: RecallSegment[]
  /** How many times this session was compacted. */
  compactions: number
}

function clip(text: string, max: number): string {
  const flat = text.trim()
  return flat.length <= max ? flat : `${flat.slice(0, max - 1)}…`
}

function blockText(block): string {
  if (typeof block === 'string') return block
  if (!block || typeof block !== 'object') return ''
  switch (block.type) {
    case 'text':
      return block.text ?? ''
    case 'tool_use': {
      let input = ''
      try {
        input = clip(JSON.stringify(block.input ?? {}), TOOL_INPUT_CHARS)
      } catch {}
      return `[tool ${block.name}] ${input}`.trim()
    }
    case 'tool_result': {
      const body = Array.isArray(block.content) ? block.content.map(blockText).join('\n') : blockText(block.content)
      return `[tool result${block.is_error ? ', error' : ''}]\n${clip(body, TOOL_OUTPUT_CHARS)}`
    }
    case 'image':
      return '[image]'
    case 'document':
      return '[document]'
    default:
      return '' // thinking, redacted_thinking and the like are not recalled
  }
}

/** One transcript message as plain text: its words, its tool calls and results (shortened). */
function renderContent(content): string {
  const parts = Array.isArray(content) ? content.map(blockText) : [blockText(content)]
  return clip(parts.filter(p => p.trim()).join('\n\n'), MESSAGE_CHARS)
}

function makeWindow(sessionId: string, index: number, messages: RecallMessage[], summary?: string): RecallSegment {
  const times = messages.map(m => m.time).filter(t => t > 0)
  return {
    messages,
    window: {
      id: `${sessionId}:${index}`,
      kind: 'compaction',
      session_id: sessionId,
      from: times.length ? Math.min(...times) : 0,
      to: times.length ? Math.max(...times) : 0,
      messages: messages.length,
      ...(summary ? { summary } : {}),
    },
  }
}

/** Reads this session's transcript, streaming it line by line. Never throws. */
export async function readTranscript(): Promise<Transcript> {
  const sessionId = getSessionId()
  await flushSessionStorage().catch(() => {})
  const path = getTranscriptPath()
  if (!existsSync(path)) return { available: false, segments: [], compactions: 0 }

  const done: RecallSegment[] = []
  let current: RecallMessage[] = []
  // The summary written right after a boundary belongs to the window before it.
  let awaitingSummary: RecallSegment | undefined
  let compactions = 0

  try {
    const lines = createInterface({ input: createReadStream(path, { encoding: 'utf8' }), crlfDelay: Infinity })
    for await (const line of lines) {
      // Cheap skip for the big non-message entries (snapshots, metadata).
      if (!line.includes('"type":"user"') && !line.includes('"type":"assistant"') && !line.includes('compact_boundary'))
        continue
      let entry
      try {
        entry = JSON.parse(line)
      } catch {
        continue
      }
      if (entry.isSidechain) continue
      if (entry.type === 'system' && entry.subtype === 'compact_boundary') {
        compactions++
        const segment = makeWindow(sessionId, done.length, current)
        done.push(segment)
        awaitingSummary = segment
        current = []
        continue
      }
      if (entry.type !== 'user' && entry.type !== 'assistant') continue
      const text = renderContent(entry.message?.content)
      if (entry.isCompactSummary) {
        if (awaitingSummary && text) awaitingSummary.window.summary = clip(text, SUMMARY_CHARS)
        awaitingSummary = undefined
        continue
      }
      if (entry.isMeta || !text) continue
      awaitingSummary = undefined
      current.push({
        id: String(entry.uuid ?? ''),
        role: entry.type,
        time: entry.timestamp ? Date.parse(entry.timestamp) || 0 : 0,
        text,
      })
    }
  } catch {
    return { available: true, segments: done.filter(s => s.messages.length > 0).reverse(), compactions }
  }
  return { available: true, segments: done.filter(s => s.messages.length > 0).reverse(), compactions }
}
